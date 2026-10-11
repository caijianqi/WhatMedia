//! 外挂字幕发现。
//!
//! 命名约定来自社区事实标准（Plex / Jellyfin / Kodi 都这么认）：
//!
//! ```text
//! Movie.mkv
//! Movie.srt                 同名录音
//! Movie.en.srt              带语言
//! Movie.zh-Hans.ass         带地区变体
//! Movie.forced.srt          强制字幕
//! Movie.en.forced.srt       语言 + 标记
//! Subs/Movie.en.srt         子目录
//! ```
//!
//! 匹配规则刻意宽松（允许 `_` 与 `.` 分隔），但**语言与标记只认点分后缀**，
//! 否则 `Movie_2020.srt` 会被误判成语言 `2020`。

use whatmedia_core::{Result, StorageProvider};

use crate::lang::{codec_from_ext, parse_suffix};

/// 一个外挂字幕文件。
#[derive(Debug, Clone)]
pub struct ExternalSubtitle {
    pub path: String,
    pub codec: whatmedia_core::SubtitleCodec,
    pub language: Option<String>,
    pub forced: bool,
    pub default: bool,
    pub sdh: bool,
    /// 用于 UI 展示的名字（不含目录）。
    pub name: String,
}

/// 常见的字幕子目录名。命中其中一个就再扫一层。
const SUB_DIRS: &[&str] = &["Subs", "subs", "Subtitles", "subtitles", "Sub", "sub"];

/// 发现某个媒体文件的所有外挂字幕。
///
/// 目录不可读时返回空列表而不是报错：媒体文件在只读光盘 / 网络盘上很常见，
/// "没有字幕"不该让播放起不来。
pub async fn discover_sidecars(
    storage: &dyn StorageProvider,
    media_path: &str,
) -> Result<Vec<ExternalSubtitle>> {
    let media = std::path::Path::new(media_path);
    let Some(stem) = media.file_stem().and_then(|s| s.to_str()) else {
        return Ok(Vec::new());
    };
    let Some(parent) = media.parent() else {
        return Ok(Vec::new());
    };
    let parent_str = parent.to_string_lossy().to_string();
    if parent_str.is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    collect_from(storage, &parent_str, stem, &mut out).await;

    for sub in SUB_DIRS {
        // 用 Path::join 而不是手拼字符串：Windows 上是 `\`，Linux 上是 `/`。
        // 存储后端的路径语义由它自己解释，这里只负责拼对。
        let dir = parent.join(sub);
        collect_from(storage, &dir.to_string_lossy(), stem, &mut out).await;
    }

    // 稳定排序：同名的按路径排，保证每次返回结果一致（UI 不该抖动）。
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

async fn collect_from(
    storage: &dyn StorageProvider,
    dir: &str,
    media_stem: &str,
    out: &mut Vec<ExternalSubtitle>,
) {
    let entries = match storage.list(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    for e in entries {
        if e.is_dir {
            continue;
        }
        let Some(ext) = std::path::Path::new(&e.name)
            .extension()
            .and_then(|s| s.to_str())
        else {
            continue;
        };
        let Some(codec) = codec_from_ext(ext) else {
            continue;
        };
        let Some(file_stem) = std::path::Path::new(&e.name)
            .file_stem()
            .and_then(|s| s.to_str())
        else {
            continue;
        };

        let (language, flags) = match suffix_tokens(file_stem, media_stem) {
            Some(tokens) => {
                let refs: Vec<&str> = tokens.iter().map(|s| s.as_str()).collect();
                parse_suffix(&refs)
            }
            None => continue,
        };

        out.push(ExternalSubtitle {
            path: e.path,
            codec,
            language,
            forced: flags.forced,
            default: flags.default,
            sdh: flags.sdh,
            name: e.name,
        });
    }
}

/// 字幕文件名是否与媒体同名。返回 Some(点分后缀 tokens) 表示命中。
///
/// `Movie.srt` → Some([])；`Movie.en.forced.srt` → Some(["en","forced"])。
fn suffix_tokens(file_stem: &str, media_stem: &str) -> Option<Vec<String>> {
    if file_stem == media_stem {
        return Some(Vec::new());
    }
    // 只认 `.` 分隔的后缀；`_` 分隔的（`Movie_eng`）也认，但拆词用 `.`。
    let rest = file_stem
        .strip_prefix(media_stem)
        .and_then(|r| r.strip_prefix('.').or_else(|| r.strip_prefix('_')))?;
    if rest.is_empty() {
        return None;
    }
    let tokens: Vec<String> = rest
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    // 全是数字的话多半是 `Movie.2020.srt`（发布年份），不是语言。
    if tokens.iter().all(|t| t.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    Some(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_tokens_matching() {
        assert_eq!(suffix_tokens("Movie", "Movie"), Some(vec![]));
        assert_eq!(
            suffix_tokens("Movie.en.forced", "Movie"),
            Some(vec!["en".to_string(), "forced".to_string()])
        );
        assert_eq!(
            suffix_tokens("Movie_eng", "Movie"),
            Some(vec!["eng".to_string()])
        );
        // 前缀不同 → 不命中
        assert_eq!(suffix_tokens("Other.en", "Movie"), None);
        // 纯数字后缀（发布年份）不算语言标记
        assert_eq!(suffix_tokens("Movie.2020", "Movie"), None);
    }

    // ── 一个内存版 StorageProvider，专供发现逻辑测试 ──────────────

    struct MemStorage {
        entries: Vec<whatmedia_core::StorageEntry>,
    }

    fn norm(p: &str) -> String {
        p.replace('\\', "/")
    }

    #[async_trait::async_trait]
    impl StorageProvider for MemStorage {
        fn id(&self) -> &str {
            "mem"
        }
        async fn list(&self, dir: &str) -> Result<Vec<whatmedia_core::StorageEntry>> {
            let want = norm(dir);
            Ok(self
                .entries
                .iter()
                .filter(|e| {
                    std::path::Path::new(&e.path)
                        .parent()
                        .map(|p| norm(&p.to_string_lossy()))
                        == Some(want.clone())
                })
                .cloned()
                .collect())
        }
        async fn stat(&self, path: &str) -> Result<whatmedia_core::StorageEntry> {
            self.entries
                .iter()
                .find(|e| e.path == path)
                .cloned()
                .ok_or_else(|| whatmedia_core::WhatMediaError::NotFound {
                    entity: "file",
                    id: path.to_string(),
                })
        }
        async fn exists(&self, path: &str) -> Result<bool> {
            Ok(self.entries.iter().any(|e| e.path == path))
        }
        async fn range_read(&self, _p: &str, _r: whatmedia_core::ByteRange) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }
        async fn read(&self, path: &str) -> Result<Vec<u8>> {
            Ok(self
                .entries
                .iter()
                .find(|e| e.path == path)
                .map(|_| Vec::new())
                .unwrap_or_default())
        }
        async fn write(&self, _p: &str, _d: &[u8]) -> Result<()> {
            Ok(())
        }
        async fn rename(&self, _f: &str, _t: &str) -> Result<()> {
            Ok(())
        }
        async fn delete(&self, _p: &str) -> Result<()> {
            Ok(())
        }
    }

    fn entry(path: &str, is_dir: bool) -> whatmedia_core::StorageEntry {
        whatmedia_core::StorageEntry {
            path: path.to_string(),
            name: std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            is_dir,
            size: Some(10),
            modified_ns: None,
        }
    }

    #[tokio::test]
    async fn discovers_sidecars_next_to_media() {
        let st = MemStorage {
            entries: vec![
                entry("/m/Movie.mkv", false),
                entry("/m/Movie.srt", false),
                entry("/m/Movie.zh-Hans.forced.ass", false),
                entry("/m/Other.srt", false),
                entry("/m/cover.jpg", false),
                entry("/m/Subs", true),
            ],
        };
        let found = discover_sidecars(&st, "/m/Movie.mkv").await.unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "Movie.srt");
        assert_eq!(found[0].language, None);
        assert_eq!(found[1].language.as_deref(), Some("zh-Hans"));
        assert!(found[1].forced);
    }

    #[tokio::test]
    async fn discovers_sidecars_in_subs_dir() {
        let st = MemStorage {
            entries: vec![
                entry("/m/Movie.mkv", false),
                entry("/m/Subs/Movie.en.srt", false),
                entry("/m/Subtitles/Movie.ja.srt", false),
            ],
        };
        let found = discover_sidecars(&st, "/m/Movie.mkv").await.unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|f| f.language.is_some()));
    }

    #[tokio::test]
    async fn missing_dirs_are_tolerated() {
        let st = MemStorage { entries: vec![] };
        let found = discover_sidecars(&st, "/m/Movie.mkv").await.unwrap();
        assert!(found.is_empty());
    }
}
