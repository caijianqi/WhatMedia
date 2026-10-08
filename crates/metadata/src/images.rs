//! 图片缓存策略：LRU + 按大小 / 时间 / 类型清理。
//!
//! SQL 在 `mosaic-database::repo::image`，正文在磁盘上，这里只管"谁该活着"。
//!
//! 缓存为什么值得单独做一层：海报墙一次要出 200 张图，每张都回源的话
//! 首屏要十几秒，而且会被 TMDB 限流。二次访问命中率必须 ≥ 90%（P2 DoD）。

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::transport::HttpTransport;

/// 清理优先级：**数字越大越先被删**。
///
/// `still` 是剧集每集一张，数量最大、缺了也最不显眼，所以最先牺牲。
/// `poster` 是海报墙的骨架，最后才动。
fn evict_priority(kind: &str) -> u32 {
    match kind {
        "poster" => 0,
        "backdrop" => 1,
        "logo" => 2,
        "still" => 3,
        "profile" => 4,
        _ => 3,
    }
}

#[derive(Debug, Clone)]
pub struct ImageCacheConfig {
    pub dir: PathBuf,
    /// 总字节上限。默认 1GB：1,000 部电影的 poster+backdrop 大约 600MB。
    pub max_bytes: u64,
    pub max_entries: u64,
    /// 多久没访问就清掉。默认 90 天。
    pub max_age: Duration,
}

impl Default for ImageCacheConfig {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("cache/images"),
            max_bytes: 1024 * 1024 * 1024,
            max_entries: 50_000,
            max_age: Duration::days(90),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct EvictReport {
    pub removed: u64,
    pub bytes_freed: u64,
    pub by_age: u64,
    pub by_size: u64,
}

pub struct ImageCache {
    db: mosaic_database::Db,
    cfg: ImageCacheConfig,
}

impl ImageCache {
    pub fn new(db: mosaic_database::Db, cfg: ImageCacheConfig) -> Result<Self> {
        std::fs::create_dir_all(&cfg.dir)
            .with_context(|| format!("创建图片缓存目录失败 {}", cfg.dir.display()))?;
        Ok(Self { db, cfg })
    }

    pub fn dir(&self) -> &Path {
        &self.cfg.dir
    }

    /// 缓存键 = sha256(url + 尺寸变体)。
    ///
    /// 变体参与散列，所以 `poster@w500` 和 `poster@w185` 是两条独立记录 ——
    /// 缩略图和原图不该互相顶掉。
    pub fn key_for(url: &str, variant: &str) -> String {
        let mut h = Sha256::new();
        h.update(url.as_bytes());
        h.update(b"\x1f");
        h.update(variant.as_bytes());
        format!("{:x}", h.finalize())
    }

    fn path_for(&self, key: &str, kind: &str, ext: &str) -> PathBuf {
        self.cfg
            .dir
            .join(kind)
            .join(&key[..2])
            .join(format!("{key}.{ext}"))
    }

    /// 命中则返回磁盘路径，并刷新 LRU。
    ///
    /// 文件被外部删了要当未命中 —— 否则返回一条指向空气的路径，
    /// UI 拿到 404 却不知道该重新下载。
    pub async fn get(&self, key: &str) -> Option<PathBuf> {
        let row = mosaic_database::repo::image::get(&self.db, key)
            .await
            .ok()??;
        let path = PathBuf::from(&row.path);
        if !path.exists() {
            let _ = mosaic_database::repo::image::delete(&self.db, key).await;
            return None;
        }
        let _ = mosaic_database::repo::image::touch(&self.db, key).await;
        Some(path)
    }

    /// 写入一张图。返回磁盘路径。
    ///
    /// 刻意不接 `width`/`height`：不解码图片就不知道真实尺寸，
    /// 而为了填两个可选列去做一次解码不划算。等 P4 生成多尺寸变体时再补。
    pub async fn put(
        &self,
        kind: &str,
        url: &str,
        variant: &str,
        ext: &str,
        bytes: &[u8],
    ) -> Result<PathBuf> {
        let key = Self::key_for(url, variant);
        let path = self.path_for(&key, kind, ext);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)
            .with_context(|| format!("写图片缓存失败 {}", path.display()))?;

        mosaic_database::repo::image::put(
            &self.db,
            &key,
            kind,
            url,
            None,
            None,
            bytes.len() as i64,
            &path.to_string_lossy(),
        )
        .await?;
        Ok(path)
    }

    /// 取一张图：命中直接返回，未命中下载并缓存。
    ///
    /// 下载失败不抛错 —— 缺图不该让整个刮削任务失败，
    /// 返回 None 由调用方决定要不要放占位图。
    pub async fn fetch(
        &self,
        transport: &dyn HttpTransport,
        kind: &str,
        url: &str,
        variant: &str,
    ) -> Option<PathBuf> {
        let key = Self::key_for(url, variant);
        if let Some(p) = self.get(&key).await {
            return Some(p);
        }

        let res = transport.get(url, None).await.ok()?;
        if res.body.is_empty() {
            return None;
        }
        let ext = extension_of(url).unwrap_or_else(|| guess_ext(&res.body));

        self.put(kind, url, variant, ext, &res.body).await.ok()
    }

    pub async fn stats(&self) -> Result<mosaic_database::repo::image::CacheStats> {
        mosaic_database::repo::image::stats(&self.db).await
    }

    /// 清理。三道闸：过期 → 超量条目 → 超量字节。
    pub async fn evict(&self) -> Result<EvictReport> {
        let mut report = EvictReport::default();
        let all = mosaic_database::repo::image::all_for_eviction(&self.db).await?;
        let cutoff = Utc::now() - self.cfg.max_age;

        let mut expired: Vec<String> = Vec::new();
        let mut live: Vec<mosaic_database::repo::image::ImageCacheRow> = Vec::new();
        for r in all {
            if r.accessed_at < cutoff {
                expired.push(r.key.clone());
            } else {
                live.push(r);
            }
        }
        report.by_age = expired.len() as u64;
        report.bytes_freed += remove_rows(&self.db, &expired).await?;
        report.removed += report.by_age;

        // 清理顺序：优先级（先删 still）→ 最久未访问 → 体积大的优先
        live.sort_by(|a, b| {
            evict_priority(&b.kind)
                .cmp(&evict_priority(&a.kind))
                .then(a.accessed_at.cmp(&b.accessed_at))
                .then(b.size_bytes.cmp(&a.size_bytes))
        });

        let mut keys: Vec<String> = Vec::new();
        let mut bytes: u64 = live.iter().map(|r| r.size_bytes.max(0) as u64).sum();
        let mut entries = live.len() as u64;
        let mut i = 0usize;
        while (bytes > self.cfg.max_bytes || entries > self.cfg.max_entries) && i < live.len() {
            keys.push(live[i].key.clone());
            bytes = bytes.saturating_sub(live[i].size_bytes.max(0) as u64);
            entries -= 1;
            i += 1;
        }

        report.by_size = keys.len() as u64;
        report.bytes_freed += remove_rows(&self.db, &keys).await?;
        report.removed += report.by_size;
        Ok(report)
    }
}

async fn remove_rows(db: &mosaic_database::Db, keys: &[String]) -> Result<u64> {
    if keys.is_empty() {
        return Ok(0);
    }
    let mut freed = 0u64;
    for k in keys {
        if let Ok(Some(row)) = mosaic_database::repo::image::get(db, k).await {
            let p = PathBuf::from(&row.path);
            freed += p.metadata().map(|m| m.len()).unwrap_or(0);
            let _ = std::fs::remove_file(p);
        }
    }
    mosaic_database::repo::image::delete_many(db, keys).await?;
    Ok(freed)
}

fn extension_of(url: &str) -> Option<&'static str> {
    let lower = url.to_lowercase();
    let path = lower.split('?').next().unwrap_or(&lower);
    match path.rsplit('.').next()? {
        "jpg" | "jpeg" => Some("jpg"),
        "png" => Some("png"),
        "webp" => Some("webp"),
        "gif" => Some("gif"),
        "svg" => Some("svg"),
        _ => None,
    }
}

/// 从魔数判断格式。URL 里没有扩展名时（TMDB 的图片 URL 常常没有）只能靠这个。
fn guess_ext(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, ..] => "jpg",
        [b'R', b'I', b'F', b'F', ..] => "webp",
        [b'G', b'I', b'F', ..] => "gif",
        _ => "jpg",
    }
}

/// 缺图占位：一张纯色 SVG，中间写标题首字。
///
/// 用 SVG 而不是 PNG，是为了**零依赖生成** —— 不用引入 image 库，
/// 也不用为了画一张灰底图去调 FFmpeg。
pub fn placeholder_svg(title: &str, width: u32, height: u32) -> String {
    let hue = (fnv1a(title) % 360) as u32;
    let ch: String = title
        .chars()
        .find(|c| !c.is_whitespace())
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".into());
    let escaped = ch
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
  <rect width="{w}" height="{h}" fill="hsl({hue} 22% 26%)"/>
  <text x="50%" y="50%" dominant-baseline="central" text-anchor="middle"
        font-family="system-ui, sans-serif" font-size="{fs}"
        fill="hsl({hue} 30% 78%)" opacity="0.9">{escaped}</text>
</svg>
"#,
        w = width,
        h = height,
        hue = hue,
        fs = height / 3,
    )
}

fn fnv1a(s: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// 缓存目录里的绝对路径是否仍在库内 —— 与 storage 层同样的逃逸检查。
pub fn is_inside(dir: &Path, p: &Path) -> bool {
    p.canonicalize()
        .map(|c| c.starts_with(dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf())))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evict_priority_puts_poster_last() {
        assert!(evict_priority("still") > evict_priority("backdrop"));
        assert!(evict_priority("backdrop") > evict_priority("poster"));
        assert_eq!(evict_priority("poster"), 0);
    }

    #[test]
    fn key_is_stable_and_variant_sensitive() {
        let a = ImageCache::key_for("https://x/p.jpg", "w500");
        let b = ImageCache::key_for("https://x/p.jpg", "w500");
        let c = ImageCache::key_for("https://x/p.jpg", "w185");
        assert_eq!(a, b);
        assert_ne!(a, c, "不同尺寸必须是不同缓存条目");
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn extension_detection() {
        assert_eq!(extension_of("https://i.tmdb/x/p.jpg?w=500"), Some("jpg"));
        assert_eq!(extension_of("https://i.tmdb/x/p.PNG"), Some("png"));
        assert_eq!(extension_of("https://i.tmdb/x/abc"), None);
        assert_eq!(guess_ext(&[0x89, b'P', b'N', b'G', 0]), "png");
        assert_eq!(guess_ext(&[0xFF, 0xD8, 0xFF]), "jpg");
    }

    #[test]
    fn placeholder_is_valid_svg_and_escapes() {
        let svg = placeholder_svg("<&>", 300, 450);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("width=\"300\""));
        // 只渲染首字，但首字本身可能是 XML 元字符，必须转义
        assert!(svg.contains("&lt;"), "必须转义：{svg}");
        assert!(!svg.contains("<&>"), "原始字符不该出现在 SVG 里：{svg}");

        // 同一标题颜色必须稳定，否则每次刷新都在变色
        assert_eq!(
            placeholder_svg("Frieren", 100, 100),
            placeholder_svg("Frieren", 100, 100)
        );
        assert_ne!(
            placeholder_svg("Frieren", 100, 100),
            placeholder_svg("Matrix", 100, 100)
        );
    }

    #[test]
    fn empty_title_placeholder_does_not_panic() {
        let svg = placeholder_svg("   ", 100, 100);
        assert!(svg.contains('?'));
    }
}
