//! 数据库行结构体 + 到领域模型的转换。
//!
//! **约定： Row 里 id 与时间一律是 `String`。**
//! 理由见 `db.rs` 模块注释 —— 显式转换让 SQLite/PostgreSQL 差异集中在一处。

use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;
use whatmedia_core::{
    AudioCodec, Container, HdrType, Id, Library, MediaFile, MediaItem, MediaKind, MediaStream,
    MediaVersion, Season, Series, StorageAccount, StreamType, SubtitleCodec, VideoCodec,
};

pub(crate) fn to_id(s: &str) -> Option<Id> {
    Uuid::parse_str(s).ok()
}

pub(crate) fn to_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// 把存储的字符串解析成枚举；未知值一律落回 `Other(...)`，绝不 panic。
///
/// 这条"绝不因未知枚举值崩溃"的规则很重要：媒体文件里的 codec 名永远比我们的
/// 枚举多，扫描 10 万个文件时崩在一个没见过的 codec 上是不可接受的。
pub fn to_video_codec(s: &str) -> VideoCodec {
    match s.to_ascii_lowercase().as_str() {
        "h264" | "avc" => VideoCodec::H264,
        "hevc" | "h265" => VideoCodec::Hevc,
        "av1" => VideoCodec::Av1,
        "vp9" => VideoCodec::Vp9,
        "mpeg2video" | "mpeg2" => VideoCodec::Mpeg2,
        "mpeg4" => VideoCodec::Mpeg4,
        "vc1" => VideoCodec::Vc1,
        other => VideoCodec::Other(other.to_string()),
    }
}

pub fn to_audio_codec(s: &str) -> AudioCodec {
    match s.to_ascii_lowercase().as_str() {
        "aac" => AudioCodec::Aac,
        "ac3" => AudioCodec::Ac3,
        "eac3" => AudioCodec::Eac3,
        "dts" => AudioCodec::Dts,
        "dtshd" | "dts-hd" => AudioCodec::DtsHd,
        "truehd" => AudioCodec::TrueHd,
        "flac" => AudioCodec::Flac,
        "opus" => AudioCodec::Opus,
        "mp3" => AudioCodec::Mp3,
        "vorbis" => AudioCodec::Vorbis,
        "pcm_s16le" | "pcm" => AudioCodec::Pcm,
        other => AudioCodec::Other(other.to_string()),
    }
}

pub fn to_subtitle_codec(s: &str) -> SubtitleCodec {
    match s.to_ascii_lowercase().as_str() {
        "srt" | "subrip" => SubtitleCodec::Srt,
        "ass" => SubtitleCodec::Ass,
        "ssa" => SubtitleCodec::Ssa,
        "webvtt" | "vtt" => SubtitleCodec::Vtt,
        "pgs" | "hdmv_pgs_subtitle" => SubtitleCodec::Pgs,
        "dvb_subtitle" | "dvbsub" => SubtitleCodec::DvbSub,
        "dvd_subtitle" | "dvdsub" => SubtitleCodec::DvdSub,
        other => SubtitleCodec::Other(other.to_string()),
    }
}

pub fn to_container(s: &str) -> Container {
    match s.to_ascii_lowercase().as_str() {
        "mp4" | "mov" => Container::Mp4,
        "matroska" | "mkv" => Container::Mkv,
        "mpegts" | "ts" => Container::Ts,
        "webm" => Container::Webm,
        "m2ts" | "bdav" => Container::M2ts,
        "avi" => Container::Avi,
        "flv" => Container::Flv,
        other => Container::Other(other.to_string()),
    }
}

pub(crate) fn to_kind(s: &str) -> MediaKind {
    match s.to_ascii_lowercase().as_str() {
        "movie" => MediaKind::Movie,
        "series" => MediaKind::Series,
        "season" => MediaKind::Season,
        "episode" => MediaKind::Episode,
        "anime" => MediaKind::Anime,
        "variety" => MediaKind::Variety,
        "documentary" => MediaKind::Documentary,
        "concert" => MediaKind::Concert,
        "music" => MediaKind::Music,
        "music_video" | "musicvideo" => MediaKind::MusicVideo,
        "short_video" | "shortvideo" => MediaKind::ShortVideo,
        "home_video" | "homevideo" => MediaKind::HomeVideo,
        _ => MediaKind::Mixed,
    }
}

pub fn to_hdr(s: &str) -> HdrType {
    match s.to_ascii_lowercase().as_str() {
        "hdr10" => HdrType::Hdr10,
        "hdr10plus" | "hdr10+" => HdrType::Hdr10Plus,
        "hlg" => HdrType::Hlg,
        "dolby_vision" | "dolbyvision" | "dv" => HdrType::DolbyVision,
        _ => HdrType::Sdr,
    }
}

pub(crate) fn from_kind(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Movie => "movie",
        MediaKind::Series => "series",
        MediaKind::Season => "season",
        MediaKind::Episode => "episode",
        MediaKind::Anime => "anime",
        MediaKind::Variety => "variety",
        MediaKind::Documentary => "documentary",
        MediaKind::Concert => "concert",
        MediaKind::Music => "music",
        MediaKind::MusicVideo => "music_video",
        MediaKind::ShortVideo => "short_video",
        MediaKind::HomeVideo => "home_video",
        MediaKind::Mixed => "mixed",
    }
}

pub(crate) fn codec_name(c: &VideoCodec) -> String {
    match c {
        VideoCodec::H264 => "h264",
        VideoCodec::Hevc => "hevc",
        VideoCodec::Av1 => "av1",
        VideoCodec::Vp9 => "vp9",
        VideoCodec::Mpeg2 => "mpeg2",
        VideoCodec::Mpeg4 => "mpeg4",
        VideoCodec::Vc1 => "vc1",
        VideoCodec::Other(s) => s.as_str(),
    }
    .to_string()
}

pub(crate) fn container_name(c: &Container) -> String {
    match c {
        Container::Mp4 => "mp4",
        Container::Mkv => "mkv",
        Container::Ts => "ts",
        Container::Webm => "webm",
        Container::M2ts => "m2ts",
        Container::Iso => "iso",
        Container::Avi => "avi",
        Container::Flv => "flv",
        Container::Other(s) => s.as_str(),
    }
    .to_string()
}

pub(crate) fn from_hdr(h: HdrType) -> &'static str {
    match h {
        HdrType::Sdr => "sdr",
        HdrType::Hdr10 => "hdr10",
        HdrType::Hdr10Plus => "hdr10plus",
        HdrType::Hlg => "hlg",
        HdrType::DolbyVision => "dolby_vision",
    }
}

pub(crate) fn from_stream_type(t: StreamType) -> &'static str {
    match t {
        StreamType::Video => "video",
        StreamType::Audio => "audio",
        StreamType::Subtitle => "subtitle",
        StreamType::Attachment => "attachment",
        StreamType::Data => "data",
    }
}

pub(crate) fn to_stream_type(s: &str) -> StreamType {
    match s.to_ascii_lowercase().as_str() {
        "video" => StreamType::Video,
        "audio" => StreamType::Audio,
        "subtitle" => StreamType::Subtitle,
        "attachment" => StreamType::Attachment,
        _ => StreamType::Data,
    }
}

// ─────────────────────────────────────────────────────────────
// 行结构体
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, FromRow)]
pub struct LibraryRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub storage_provider: String,
    pub root_path: String,
    pub scan_policy: String,
    pub metadata_providers: String,
    pub created_at: String,
}

impl LibraryRow {
    pub fn into_domain(self) -> Option<Library> {
        Some(Library {
            id: to_id(&self.id)?,
            name: self.name,
            kind: to_kind(&self.kind),
            storage_provider: self.storage_provider,
            root_path: self.root_path,
            scan_policy: self.scan_policy,
            metadata_providers: serde_json::from_str(&self.metadata_providers).unwrap_or_default(),
            created_at: to_time(&self.created_at)?,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct ItemRow {
    pub id: String,
    pub library_id: String,
    pub kind: String,
    pub title: String,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub runtime_ms: Option<i64>,
    pub rating: Option<f64>,
    pub locked_fields: String,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    pub added_at: String,
}

impl ItemRow {
    pub fn into_domain(self) -> Option<MediaItem> {
        Some(MediaItem {
            id: to_id(&self.id)?,
            library_id: to_id(&self.library_id)?,
            kind: to_kind(&self.kind),
            title: self.title,
            original_title: self.original_title,
            sort_title: self.sort_title,
            year: self.year,
            overview: self.overview,
            runtime_ms: self.runtime_ms,
            rating: self.rating,
            locked_fields: serde_json::from_str(&self.locked_fields).unwrap_or_default(),
            provider: self.provider,
            provider_id: self.provider_id,
            added_at: to_time(&self.added_at)?,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct VersionRow {
    pub id: String,
    pub item_id: String,
    pub name: String,
    pub source: Option<String>,
    pub resolution: Option<String>,
    pub video_codec: Option<String>,
    pub hdr_type: String,
    pub is_default: i32,
}

impl VersionRow {
    pub fn into_domain(self) -> Option<MediaVersion> {
        Some(MediaVersion {
            id: to_id(&self.id)?,
            item_id: to_id(&self.item_id)?,
            name: self.name,
            source: self.source,
            resolution: self.resolution,
            video_codec: self.video_codec.as_deref().map(to_video_codec),
            hdr_type: to_hdr(&self.hdr_type),
            is_default: self.is_default != 0,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct FileRow {
    pub id: String,
    pub version_id: String,
    pub storage_provider: String,
    pub path: String,
    pub container: Option<String>,
    pub size_bytes: i64,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i64>,
    pub file_fingerprint: String,
    pub probe_json: Option<String>,
    pub is_available: i32,
}

impl FileRow {
    pub fn into_domain(self) -> Option<MediaFile> {
        Some(MediaFile {
            id: to_id(&self.id)?,
            version_id: to_id(&self.version_id)?,
            storage_provider: self.storage_provider,
            path: self.path,
            container: self.container.as_deref().map(to_container),
            size_bytes: self.size_bytes,
            duration_ms: self.duration_ms,
            bitrate: self.bitrate,
            file_fingerprint: self.file_fingerprint,
            probe_json: self.probe_json,
            is_available: self.is_available != 0,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct StreamRow {
    pub id: String,
    pub file_id: String,
    pub stream_type: String,
    pub idx: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: i32,
    pub is_forced: i32,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub bit_depth: Option<i32>,
    pub hdr_type: String,
    pub channels: Option<i32>,
    pub bitrate: Option<i64>,
}

impl StreamRow {
    pub fn into_domain(self) -> Option<MediaStream> {
        Some(MediaStream {
            id: to_id(&self.id)?,
            file_id: to_id(&self.file_id)?,
            stream_type: to_stream_type(&self.stream_type),
            index: self.idx,
            codec: self.codec,
            language: self.language,
            title: self.title,
            is_default: self.is_default != 0,
            is_forced: self.is_forced != 0,
            width: self.width,
            height: self.height,
            bit_depth: self.bit_depth,
            hdr_type: to_hdr(&self.hdr_type),
            channels: self.channels,
            bitrate: self.bitrate,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct SeriesRow {
    pub id: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
}

impl SeriesRow {
    pub fn into_domain(self) -> Option<Series> {
        Some(Series {
            id: to_id(&self.id)?,
            title: self.title,
            original_title: self.original_title,
            year: self.year,
            overview: self.overview,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct SeasonRow {
    pub id: String,
    pub series_id: String,
    pub season_number: i32,
    pub title: Option<String>,
}

impl SeasonRow {
    pub fn into_domain(self) -> Option<Season> {
        Some(Season {
            id: to_id(&self.id)?,
            series_id: to_id(&self.series_id)?,
            season_number: self.season_number,
            title: self.title,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct StorageAccountRow {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub endpoint: String,
    pub auth_type: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub root_path: String,
    pub direct_stream: i64,
    pub extra_headers: String,
    pub created_at: String,
    pub updated_at: String,
}

impl StorageAccountRow {
    pub fn into_domain(self) -> Option<StorageAccount> {
        let extra_headers =
            serde_json::from_str(&self.extra_headers).unwrap_or_else(|_| serde_json::json!({}));
        Some(StorageAccount {
            id: to_id(&self.id)?,
            name: self.name,
            driver: self.driver,
            endpoint: self.endpoint,
            auth_type: self.auth_type,
            username: self.username,
            password: self.password,
            root_path: self.root_path,
            direct_stream: self.direct_stream != 0,
            extra_headers,
            created_at: to_time(&self.created_at)?,
            updated_at: to_time(&self.updated_at)?,
        })
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct VodSourceRow {
    pub id: String,
    pub name: String,
    pub api_url: String,
    pub is_active: i64,
    pub weight: i64,
    pub timeout_ms: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl VodSourceRow {
    pub fn into_config(self) -> whatmedia_aggregator::VodSourceConfig {
        whatmedia_aggregator::VodSourceConfig {
            id: self.id,
            name: self.name,
            api_url: self.api_url,
            is_active: self.is_active != 0,
            weight: self.weight as u32,
            timeout_ms: self.timeout_ms as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_codec_never_panics() {
        // 媒体文件里的 codec 名永远比枚举多。未知值必须落回 Other，不能 panic。
        assert_eq!(
            to_video_codec("mpeg1video"),
            VideoCodec::Other("mpeg1video".into())
        );
        assert_eq!(to_video_codec("h264"), VideoCodec::H264);
        assert_eq!(to_video_codec("HEVC"), VideoCodec::Hevc);
        assert_eq!(to_audio_codec("truehd"), AudioCodec::TrueHd);
        assert_eq!(to_subtitle_codec("hdmv_pgs_subtitle"), SubtitleCodec::Pgs);
        assert_eq!(to_hdr("hdr10+"), HdrType::Hdr10Plus);
    }
}
