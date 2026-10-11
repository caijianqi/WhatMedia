use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 全局实体标识。SQLite 存 TEXT，PostgreSQL 存 UUID。
pub type Id = Uuid;

// ─────────────────────────────────────────────────────────────
// 媒体领域
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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

/// 未知流一律归为 `Data`，不要因为没有 Default 就让解析失败。
impl Default for StreamType {
    fn default() -> Self {
        StreamType::Data
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum StreamType {
    Video,
    Audio,
    Subtitle,
    Attachment,
    Data,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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

/// 默认 SDR —— 绝大多数片源都不是 HDR，缺省不该是"未知"。
impl Default for HdrType {
    fn default() -> Self {
        HdrType::Sdr
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum HdrType {
    Sdr,
    Hdr10,
    Hdr10Plus,
    Hlg,
    DolbyVision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Library {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaItem {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaVersion {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaFile {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MediaStream {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Series {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Season {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub series_id: Id,
    pub season_number: i32,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Episode {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub season_id: Id,
    pub episode_number: i32,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    pub title: Option<String>,
}

// ─────────────────────────────────────────────────────────────
// 播放与转码
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaybackSource {
    /// 直出原始文件（HTTP Range）。
    Direct {
        #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
        file_id: Id,
        url: String,
    },
    /// Remux 后的流。
    Remux {
        #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
        file_id: Id,
        url: String,
    },
    /// HLS 主播放列表。
    Hls {
        #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
        file_id: Id,
        url: String,
    },
    /// fMP4 分片流。
    Dash {
        #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
        file_id: Id,
        url: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PlaybackSession {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub user_id: Option<Id>,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub file_id: Id,
    pub device_id: Option<String>,
    /// 客户端能力指纹（如 `web-chrome` / `tauri-mpv`）。
    /// 决策结果依赖它，回放诊断时也靠它区分"设备不行"和"片源不行"。
    pub client_id: Option<String>,
    pub strategy: PlaybackStrategy,
    /// 为什么选了这条路。用户问"为什么我的 4K 片在转码"时要能答得出来。
    pub strategy_reason: Option<String>,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 关闭时间。NULL 表示会话仍在进行（或异常退出，靠回收器收尾）。
    pub stopped_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TranscodeJob {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub session_id: Id,
    /// 被转码的文件。NULL 表示这是一条历史遗留记录（0004 之前插入的）。
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub file_id: Option<Id>,
    pub args: Vec<String>,
    pub hw_accel: Option<String>,
    pub state: TranscodeState,
    pub progress: f32,
    /// 输出形态：`remux_ts` / `remux_fmp4` / `hls`。
    pub output_kind: Option<String>,
    pub output_dir: Option<String>,
    pub pid: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

// ─────────────────────────────────────────────────────────────
// 任务系统（P1 就要有：扫描 / 刮削 / 转码 / AI 四条链路的共同地基）
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Normal,
    High,
    /// 播放抢占：用户正在等，扫描/刮削必须让路。
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Task {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
