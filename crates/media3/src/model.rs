//! 与 Kotlin 侧共享的数据契约。
//!
//! ## 为什么契约放在 Rust 里，而不是 Kotlin 里
//!
//! 播放器有三层：TypeScript（UI）→ Rust（Tauri 桥）→ Kotlin（Media3）。
//! 契约放哪一层，就有两层要跟着它改。放 Rust 是因为：
//!
//! 1. Rust 侧能跑 `cargo test`，Kotlin 侧在当前环境没有 Android SDK，**跑不了任何测试**。
//!    把语义放进一个没法验证的地方，等于放弃验证。
//! 2. TypeScript 侧的类型是从 `client/src/player/types.ts` 手写的，
//!    一致性靠 `client/src/player/media3-bridge.test.ts` 里的一组"契约快照"测试钉住
//!    —— 它不是自动派生的，但至少是**会被 CI 拦下来的**。
//!
//! Kotlin 侧用 `kotlinx.serialization` 手写下同名字段。**改字段名必须三处一起改**，
//! 且 Kotlin 是唯一没有编译期检查的那一侧（JSON 字段名不匹配在 Kotlin 上是运行时缺字段，
//! 不会报错）。所以本文件里每个字段都写了注释 —— Kotlin 那边照抄。

use serde::{Deserialize, Serialize};

/// 播放器状态。字面量与前端 `PlayerState` 完全一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerState {
    Idle,
    Loading,
    Ready,
    Playing,
    Paused,
    /// 播放中数据不够。与 `Loading` 两回事：Loading 是"还没开始"，
    /// Buffering 是"开始了又断"。UI 给的提示不同（转圈 vs 网络慢）。
    Buffering,
    Ended,
    Error,
}

impl PlayerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Loading => "loading",
            Self::Ready => "ready",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Buffering => "buffering",
            Self::Ended => "ended",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "idle" => Self::Idle,
            "loading" => Self::Loading,
            "ready" => Self::Ready,
            "playing" => Self::Playing,
            "paused" => Self::Paused,
            "buffering" => Self::Buffering,
            "ended" => Self::Ended,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

/// 错误码。前端据此决定"重试"还是"换个策略重新决策"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    /// 网络断开 / 请求失败。可重试 —— 这是 P6 DoD"网络切换可恢复"的落点。
    Network,
    /// 解码失败（容器或编码后端不支持）。重试无意义，要服务端重新决策。
    Decode,
    /// 源不存在（404）。重试无意义。
    NotFound,
    /// 自动播放被拦截。Android 上没有这条（Media3 不拦），
    /// 保留是为了与前端类型对齐 —— 对齐比省一个变体重要。
    AutoplayBlocked,
    Unknown,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::Decode => "decode",
            Self::NotFound => "not-found",
            Self::AutoplayBlocked => "autoplay-blocked",
            Self::Unknown => "unknown",
        }
    }
}

/// 音轨 / 字幕轨 / 画质档，共用一套结构。
///
/// **id 的规则必须在三侧一致**：ExoPlayer 里轨道没有全局 id，
/// 只有"第 N 个 group 里的第 M 条 format"。所以 id 定义为 `"g{group}:t{track}"`。
/// 这个格式看着丑，但它是唯一能在"Kotlin 收到 id → 反查 group/track"时
/// 不会歧义的表示 —— 用数组下标的话，切了音轨后 group 会重排。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub forced: bool,
    /// 画质档专用：档位高度（1080 / 720…）。音轨字幕轨为 None。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// 画质档专用：码率（kbps）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
}

/// Kotlin 侧从 ExoPlayer 的 `Tracks` 里读出来的**原始**一条轨道。
///
/// 为什么是"原始"的：标签怎么取、语言怎么显示、id 怎么编，
/// 这些是会算错的逻辑，而 Kotlin 侧当前没有测试手段。
/// 所以 Kotlin 只负责把 ExoPlayer 的原值原样打包送过来，
/// 由 `protocol::to_tracks` 生成真正的 `Track`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawTrack {
    /// ExoPlayer 的 group 下标。
    pub group: usize,
    /// group 内的 format 下标。
    pub index: usize,
    /// 容器里带的标题（MKV 的 track name）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 语言标签，可能是 ISO-639-1 / -2 / -3，也可能带地区。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// ExoPlayer 的 codecs 字符串（如 `mp4a.40.2`）。用于诊断，不参与决策。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codecs: Option<String>,
    #[serde(default)]
    pub selected: bool,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub forced: bool,
    /// 画质档：分辨率高度。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// 画质档：码率（kbps）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<u32>,
}

/// Kotlin 一次送回来的三套原始轨道。对应 Kotlin 的 `RawTracksPayload`。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RawTracksPayload {
    #[serde(default)]
    pub audio: Vec<RawTrack>,
    #[serde(default)]
    pub text: Vec<RawTrack>,
    #[serde(default)]
    pub video: Vec<RawTrack>,
}

/// Kotlin 侧发来的**原始**错误事件。
///
/// 注意它和 `ErrorPayload` 不同：Kotlin 只有 Media3 的 int 错误码，
/// 没有我们的 `ErrorCode`。翻译（`classify_error`）发生在 Rust 桥里，
/// 所以这个类型是"解码用"，不该出现在给前端的线上格式里。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RawError {
    #[serde(rename = "errorCode")]
    pub error_code: i32,
    #[serde(default)]
    pub message: String,
}

/// 播放源。与前端 `PlaybackSource` 对应。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    /// Direct Play / Remux：能直接喂给播放器的 URL。
    Progressive { url: String },
    /// 转码：HLS master playlist。
    Hls { url: String },
}

/// 外挂字幕。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalSubtitle {
    pub label: String,
    /// 服务端转换后的 URL。Android 上走网络，与 Web 一样。
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default)]
    pub default: bool,
    /// MIME。Media3 靠它选 extractor；猜错的表现是字幕加载了但一条都不显示。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime: Option<String>,
}

/// 载入请求。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadRequest {
    pub source: Source,
    /// 起始位置（秒）。负数与超过时长的都按 `clamp_seek` 处理，不报错。
    #[serde(default)]
    pub start_at: f64,
    #[serde(default)]
    pub autoplay: bool,
    #[serde(default)]
    pub external_subtitles: Vec<ExternalSubtitle>,
}

/// 完整状态快照。不可变：每次变更都是新对象。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub state: PlayerState,
    /// 秒。
    pub position: f64,
    /// 秒。未知为 0。
    pub duration: f64,
    pub volume: f64,
    pub muted: bool,
    pub rate: f64,
    /// 已缓冲到的位置（秒）。
    pub buffered: f64,
    #[serde(default)]
    pub audio_tracks: Vec<Track>,
    #[serde(default)]
    pub subtitle_tracks: Vec<Track>,
    #[serde(default)]
    pub qualities: Vec<Track>,
    pub current_audio_track_id: Option<String>,
    /// None 表示字幕关闭。
    pub current_subtitle_track_id: Option<String>,
    pub current_quality_id: Option<String>,
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: PlayerState::Idle,
            position: 0.0,
            duration: 0.0,
            volume: 1.0,
            muted: false,
            rate: 1.0,
            buffered: 0.0,
            audio_tracks: Vec::new(),
            subtitle_tracks: Vec::new(),
            qualities: Vec::new(),
            current_audio_track_id: None,
            current_subtitle_track_id: None,
            current_quality_id: None,
            error: None,
        }
    }
}

/// 瞬时事件。与 `state` 的区别见前端 types.ts 的注释。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PlayerEvent {
    StateChange {
        from: PlayerState,
        to: PlayerState,
    },
    TimeUpdate {
        position: f64,
        duration: f64,
    },
    Ended,
    Error {
        error: ErrorPayload,
    },
    TracksListed,
    Waiting,
    Playing,
    Paused {
        by_user: bool,
    },
    /// 画中画状态变化。只有 Android 有，前端据此隐藏控制条。
    PipChanged {
        active: bool,
    },
    /// 遥控器按键。只有 TV 有。见 `RemoteKey`。
    RemoteKey {
        key: RemoteKey,
    },
    /// 网络状态变化。P6 DoD：WiFi ↔ 4G 切换不崩溃。
    NetworkChanged {
        online: bool,
        kind: NetworkKind,
    },
}

/// 遥控器按键。
///
/// 与 `MainActivity.mapKey()` 的输出一一对应。
/// 加键必须两边同时加 —— 少了的键在 TV 上表现为"按了没反应"，
/// 而遥控器按键是 TV 上唯一的操作方式，缺一个就等于这一项功能不存在。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemoteKey {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Menu,
    Toggle,
    Play,
    Pause,
    SeekForward,
    SeekBackward,
    Next,
    Previous,
    Stop,
}

impl RemoteKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::Select => "select",
            Self::Back => "back",
            Self::Menu => "menu",
            Self::Toggle => "toggle",
            Self::Play => "play",
            Self::Pause => "pause",
            Self::SeekForward => "seek-forward",
            Self::SeekBackward => "seek-backward",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::Stop => "stop",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "up" => Self::Up,
            "down" => Self::Down,
            "left" => Self::Left,
            "right" => Self::Right,
            "select" => Self::Select,
            "back" => Self::Back,
            "menu" => Self::Menu,
            "toggle" => Self::Toggle,
            "play" => Self::Play,
            "pause" => Self::Pause,
            "seek-forward" => Self::SeekForward,
            "seek-backward" => Self::SeekBackward,
            "next" => Self::Next,
            "previous" => Self::Previous,
            "stop" => Self::Stop,
            _ => return None,
        })
    }
}

/// 网络类型。用于决定"切换后要不要降码率"—— 不必降，但值得记。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkKind {
    Wifi,
    Cellular,
    Ethernet,
    None,
    Unknown,
}

impl NetworkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wifi => "wifi",
            Self::Cellular => "cellular",
            Self::Ethernet => "ethernet",
            Self::None => "none",
            Self::Unknown => "unknown",
        }
    }

    /// 计量网络（4G）标记。UI 据此提示"正在使用移动数据"。
    pub fn is_metered(self) -> bool {
        matches!(self, Self::Cellular)
    }
}

/// MediaSession 元数据。通知栏 / 锁屏 / 蓝牙耳机上显示。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaMetadata {
    pub title: String,
    /// 副标题（剧集名 / 年份）。
    #[serde(default)]
    pub subtitle: String,
    /// 海报 URL。**必须是 http(s)** —— MediaSession 不走 Tauri 的 asset 协议，
    /// 给 `asset://` 它会静默不显示封面，且不报错。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artwork_url: Option<String>,
    /// 时长（毫秒）。0 表示未知，通知栏上不画进度条。
    #[serde(default)]
    pub duration_ms: i64,
}
