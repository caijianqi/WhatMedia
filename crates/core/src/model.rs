use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 全局实体标识。SQLite 存 TEXT，PostgreSQL 存 UUID。
pub type Id = Uuid;

// ─────────────────────────────────────────────────────────────
// 媒体领域
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Movie,
    Series,
    Season,
    Episode,
    Anime,
    Variety,
    Documentary,
    Concert,
    Music,
    MusicVideo,
    ShortVideo,
    HomeVideo,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
    Attachment,
    Data,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
    Mpeg2,
    Mpeg4,
    Vc1,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioCodec {
    Aac,
    Ac3,
    Eac3,
    Dts,
    DtsHd,
    TrueHd,
    Flac,
    Opus,
    Mp3,
    Vorbis,
    Pcm,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleCodec {
    Srt,
    Ass,
    Ssa,
    Vtt,
    /// 蓝光位图字幕。客户端普遍不支持，需 burn-in 或服务端转换。
    Pgs,
    DvbSub,
    DvdSub,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HdrType {
    Sdr,
    Hdr10,
    Hdr10Plus,
    Hlg,
    DolbyVision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mp4,
    Mkv,
    Ts,
    Webm,
    M2ts,
    Iso,
    Avi,
    Flv,
    #[serde(untagged)]
    Other(String),
}

/// 媒体库。存储、扫描策略、元数据策略的聚合配置点。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub id: Id,
    pub name: String,
    pub kind: MediaKind,
    pub storage_provider: String,
    pub root_path: String,
    pub scan_policy: String,
    pub metadata_providers: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// 一部影视作品。**不等于一个文件。**
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaItem {
    pub id: Id,
    pub library_id: Id,
    pub kind: MediaKind,
    pub title: String,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub runtime_ms: Option<i64>,
    pub rating: Option<f64>,
    /// 字段级锁定：记录哪些字段用户手动改过，刮削不得覆盖（§32）。
    pub locked_fields: Vec<String>,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    pub added_at: DateTime<Utc>,
}

/// 作品的一个清晰度 / 来源 / 编码版本（如 2160P REMUX）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaVersion {
    pub id: Id,
    pub item_id: Id,
    pub name: String,
    pub source: Option<String>,
    pub resolution: Option<String>,
    pub video_codec: Option<VideoCodec>,
    pub hdr_type: HdrType,
    pub is_default: bool,
}

/// 一个物理文件。可能位于任意 StorageProvider 上。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFile {
    pub id: Id,
    pub version_id: Id,
    pub storage_provider: String,
    pub path: String,
    pub container: Option<Container>,
    pub size_bytes: i64,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i64>,
    /// 增量扫描指纹 = (mtime_ns, size)。两者缺一不可。
    pub file_fingerprint: String,
    /// ffprobe 原始输出，保留以避免重复探测（ARM 上 ffprobe 很贵）。
    pub probe_json: Option<String>,
    pub is_available: bool,
}

/// 文件内的一条流（视频 / 音频 / 字幕 / 附件）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaStream {
    pub id: Id,
    pub file_id: Id,
    pub stream_type: StreamType,
    pub index: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub bit_depth: Option<i32>,
    pub hdr_type: HdrType,
    pub channels: Option<i32>,
    pub bitrate: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    pub id: Id,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Season {
    pub id: Id,
    pub series_id: Id,
    pub season_number: i32,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: Id,
    pub season_id: Id,
    pub episode_number: i32,
    pub item_id: Id,
    pub title: Option<String>,
}

// ─────────────────────────────────────────────────────────────
// 播放与转码
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackStrategy {
    /// 直接播放原文件，零 CPU。
    DirectPlay,
    /// 换容器不重编码，低 CPU 低延迟。
    Remux,
    /// 重新编码。
    Transcode,
    /// 无可用路径。
    Unsupported,
}

/// 播放器数据源。客户端与服务端通过它解耦物理位置。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaybackSource {
    /// 直出原始文件（HTTP Range）。
    Direct { file_id: Id, url: String },
    /// Remux 后的流。
    Remux { file_id: Id, url: String },
    /// HLS 主播放列表。
    Hls { file_id: Id, url: String },
    /// fMP4 分片流。
    Dash { file_id: Id, url: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscodeState {
    Pending,
    Probing,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackSession {
    pub id: Id,
    pub user_id: Option<Id>,
    pub item_id: Id,
    pub file_id: Id,
    pub device_id: Option<String>,
    pub strategy: PlaybackStrategy,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeJob {
    pub id: Id,
    pub session_id: Id,
    pub args: Vec<String>,
    pub hw_accel: Option<String>,
    pub state: TranscodeState,
    pub progress: f32,
    pub output_dir: Option<String>,
    pub pid: Option<i32>,
}

// ─────────────────────────────────────────────────────────────
// 任务系统（P1 就要有：扫描 / 刮削 / 转码 / AI 四条链路的共同地基）
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    /// 播放抢占：用户正在等，扫描/刮削必须让路。
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Scan,
    Metadata,
    Transcode,
    Subtitle,
    Ai,
    Maintenance,
    Backup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Id,
    pub kind: TaskKind,
    pub priority: TaskPriority,
    pub state: TaskState,
    pub payload: Option<String>,
    pub progress: f32,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// HTTP Range 读取区间。Direct Play 与代理播放都依赖它。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    /// None 表示读到结尾。
    pub end: Option<u64>,
}

impl ByteRange {
    pub fn new(start: u64, end: Option<u64>) -> Self {
        Self { start, end }
    }

    pub fn len(&self, total: u64) -> u64 {
        let end = self.end.unwrap_or(total.saturating_sub(1));
        end.saturating_sub(self.start).saturating_add(1)
    }
}
