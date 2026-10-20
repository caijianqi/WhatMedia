//! 入库：把"一个被识别出来的文件"写成 item / version / file / stream 四层结构。
//!
//! **关键设计：所有 id 都是确定性 UUID v5，不是随机 UUID。**
//!
//! 理由很实际 —— 扫描会反复跑。如果 id 随机，每次扫描都会造出一批新作品，
//! 用户的元数据、续播点、手动修改全部丢失。用 v5 从自然键（库 + 标题 + 年份 + 季集）
//! 派生 id，重复扫描天然收敛到同一行。

use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;
use whatmedia_core::{
    Container, HdrType, Id, Library, MediaFile, MediaItem, MediaKind, MediaVersion, VideoCodec,
};
use whatmedia_database::{repo, to_video_codec, Db};
use whatmedia_media::{ParsedName, ProbeResult};

/// UUIDv5 命名空间。固定常量，改了就等于重建整个库。
const NS: Uuid = Uuid::from_bytes([
    0x6d, 0x6f, 0x73, 0x61, 0x69, 0x63, 0x2d, 0x6d, 0x65, 0x64, 0x69, 0x61, 0x2d, 0x76, 0x31, 0x00,
]);

fn v5(key: &str) -> Id {
    Uuid::new_v5(&NS, key.as_bytes())
}

#[derive(Debug, Clone)]
pub struct IngestRequest {
    pub library: Library,
    pub storage_provider: String,
    pub path: String,
    pub size_bytes: i64,
    pub fingerprint: String,
    pub parsed: ParsedName,
    pub probe: Option<ProbeResult>,
}

#[derive(Debug, Clone, Copy)]
pub struct IngestOutcome {
    pub item_id: Id,
    pub version_id: Id,
    pub file_id: Id,
}

/// 入库。幂等：同一路径重复调用不会产生重复数据。
pub async fn ingest_file(db: &Db, req: &IngestRequest) -> Result<IngestOutcome> {
    let library_id = req.library.id;
    let is_episode = req.parsed.is_episode();

    let item_id = item_id_for(library_id, &req.parsed);
    let version_id = version_id_for(item_id, &req.parsed);
    let file_id = file_id_for(&req.storage_provider, &req.path);

    let duration = req.probe.as_ref().and_then(|p| p.duration_ms);
    let bitrate = req.probe.as_ref().and_then(|p| p.bitrate);

    let item = MediaItem {
        id: item_id,
        library_id,
        kind: if is_episode {
            MediaKind::Episode
        } else {
            MediaKind::Movie
        },
        title: if !req.parsed.title.trim().is_empty() {
            req.parsed.title.clone()
        } else {
            std::path::Path::new(&req.path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "未知标题".to_string())
        },
        original_title: None,
        sort_title: None,
        year: req.parsed.year,
        overview: None,
        runtime_ms: duration,
        rating: None,
        locked_fields: Vec::new(),
        provider: None,
        provider_id: None,
        added_at: Utc::now(),
    };
    repo::upsert_item(db, &item).await?;

    let video_codec = req
        .parsed
        .video_codec
        .as_deref()
        .map(to_video_codec)
        .or_else(|| best_video_codec(req.probe.as_ref()));

    let version = MediaVersion {
        id: version_id,
        item_id,
        name: version_name(&req.parsed, req.probe.as_ref()),
        source: req.parsed.source.clone(),
        resolution: req.parsed.resolution.clone(),
        video_codec,
        hdr_type: req.parsed.hdr.unwrap_or(HdrType::Sdr),
        is_default: true,
    };
    repo::upsert_version(db, &version).await?;

    let file = MediaFile {
        id: file_id,
        version_id,
        storage_provider: req.storage_provider.clone(),
        path: req.path.clone(),
        container: req
            .probe
            .as_ref()
            .and_then(|p| p.container.clone())
            .map(|c| to_container(&c)),
        size_bytes: req.size_bytes,
        duration_ms: duration,
        bitrate,
        file_fingerprint: req.fingerprint.clone(),
        probe_json: req.probe.as_ref().map(|p| p.raw.clone()),
        is_available: true,
    };
    let stored_file_id = repo::upsert_file(db, &file).await?;

    if let Some(probe) = &req.probe {
        let streams: Vec<_> = probe
            .streams
            .iter()
            .map(|s| s.to_media_stream(stored_file_id))
            .collect();
        repo::replace_streams(db, stored_file_id, &streams).await?;
    }

    if is_episode {
        link_episode(db, item_id, &req.parsed).await?;
    }

    Ok(IngestOutcome {
        item_id,
        version_id,
        file_id: stored_file_id,
    })
}

/// 把剧集挂到 series / season / episode 结构上。
async fn link_episode(db: &Db, item_id: Id, parsed: &ParsedName) -> Result<()> {
    let series_title = if !parsed.title.trim().is_empty() {
        &parsed.title
    } else {
        "未知剧集"
    };
    let series = repo::tv::upsert_series(db, series_title, parsed.year).await?;
    let season = repo::tv::upsert_season(db, series.id, parsed.season.unwrap_or(1)).await?;
    repo::tv::upsert_episode(
        db,
        season.id,
        parsed.episode.unwrap_or(0),
        item_id,
        parsed.episode_title.as_deref(),
    )
    .await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────
// 确定性 id
// ─────────────────────────────────────────────────────────────

/// 作品 id：库 + 标题（小写）+ 年份。剧集再加上季集。
pub fn item_id_for(library_id: Id, parsed: &ParsedName) -> Id {
    let title = parsed.title.to_lowercase();
    let year = parsed.year.map(|y| y.to_string()).unwrap_or_default();
    if parsed.is_episode() {
        let s = parsed.season.unwrap_or(1);
        let e = parsed.episode.unwrap_or(0);
        v5(&format!("item|{library_id}|{title}|{year}|s{s}|e{e}"))
    } else {
        v5(&format!("item|{library_id}|{title}|{year}"))
    }
}

/// 版本 id：同一作品的每个"清晰度 + 来源 + 编码 + HDR"组合算一个版本。
pub fn version_id_for(item_id: Id, parsed: &ParsedName) -> Id {
    let res = parsed.resolution.clone().unwrap_or_default();
    let src = parsed.source.clone().unwrap_or_default();
    let codec = parsed.video_codec.clone().unwrap_or_default();
    v5(&format!("version|{item_id}|{res}|{src}|{codec}"))
}

/// 文件 id：存储在哪个 provider 的哪个路径，是唯一确定的。
pub fn file_id_for(storage_provider: &str, path: &str) -> Id {
    v5(&format!("file|{storage_provider}|{path}"))
}

// ─────────────────────────────────────────────────────────────

fn version_name(parsed: &ParsedName, probe: Option<&ProbeResult>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = &parsed.resolution {
        parts.push(r.to_uppercase());
    } else if let Some(h) = probe.and_then(main_video_height) {
        parts.push(format!("{h}p"));
    }
    if let Some(s) = &parsed.source {
        parts.push(s.clone());
    }
    if let Some(c) = &parsed.video_codec {
        parts.push(c.clone());
    }
    if parts.is_empty() {
        "默认版本".into()
    } else {
        parts.join(" ")
    }
}

fn main_video_height(probe: &ProbeResult) -> Option<i32> {
    probe
        .streams
        .iter()
        .find(|s| s.width.is_some())
        .and_then(|s| s.height)
}

/// 文件名没写编码时，用 ffprobe 的第一条视频轨补齐。
fn best_video_codec(probe: Option<&ProbeResult>) -> Option<VideoCodec> {
    probe?
        .streams
        .iter()
        .find(|s| s.codec.is_some() && s.width.is_some())
        .and_then(|s| s.codec.as_deref())
        .map(to_video_codec)
}

fn to_container(s: &str) -> Container {
    match s.to_lowercase().as_str() {
        "matroska" | "webm" => Container::Mkv,
        "mov" | "mp4" | "m4v" => Container::Mp4,
        "mpegts" => Container::Ts,
        "avi" => Container::Avi,
        "flv" => Container::Flv,
        other => Container::Other(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(title: &str, year: Option<i32>) -> ParsedName {
        ParsedName {
            title: title.into(),
            year,
            ..Default::default()
        }
    }

    #[test]
    fn ids_are_deterministic() {
        let lib = Uuid::new_v4();
        let a = item_id_for(lib, &parsed("The Matrix", Some(1999)));
        let b = item_id_for(lib, &parsed("The Matrix", Some(1999)));
        assert_eq!(a, b);
    }

    #[test]
    fn different_year_is_different_item() {
        let lib = Uuid::new_v4();
        assert_ne!(
            item_id_for(lib, &parsed("The Matrix", Some(1999))),
            item_id_for(lib, &parsed("The Matrix", Some(2003)))
        );
    }

    #[test]
    fn episodes_of_same_title_are_distinct() {
        let lib = Uuid::new_v4();
        let mut e1 = parsed("Breaking Bad", None);
        e1.season = Some(1);
        e1.episode = Some(1);
        let mut e2 = e1.clone();
        e2.episode = Some(2);
        assert_ne!(item_id_for(lib, &e1), item_id_for(lib, &e2));
    }

    #[test]
    fn file_id_depends_only_on_provider_and_path() {
        assert_eq!(
            file_id_for("local", "/media/a.mkv"),
            file_id_for("local", "/media/a.mkv")
        );
        assert_ne!(
            file_id_for("local", "/media/a.mkv"),
            file_id_for("smb", "/media/a.mkv")
        );
    }
}
