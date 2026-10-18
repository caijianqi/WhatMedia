//! Media3 / ExoPlayer 播放器插件 —— Rust 一半。
//!
//! ## 这一半负责什么
//!
//! 只有两件事：
//!
//! 1. **转发**：把前端的 `invoke` 转给 Kotlin，把 Kotlin 的通道事件转给前端。
//! 2. **翻译**：把 Kotlin 送来的原始值（Media3 的 int 错误码、没有 label 的轨道）
//!    变成前端 `MediaPlayer` 契约里的类型。
//!
//! **不负责任何决策。** 错误码怎么分类、标签怎么取、seek 怎么收敛，
//! 全在 `whatmedia-media3`（`crates/media3`）里，那边有测试。
//! 这里再写一遍"如果错误码是 1008 就当断网"就等于把逻辑放进没有测试的地方。
//!
//! ## 为什么要有这一层，而不是前端直接调 Kotlin
//!
//! Tauri 的 Kotlin 插件 API 只能给前端 JSON。若前端直连，
//! "错误码 → 错误类型"的翻译就得写在 TypeScript 里 ——
//! 而 Kotlin 侧的行为在桌面端（Windows，本项目当前开发环境）上无法验证，
//! TypeScript 侧又拿不到真实的 Media3 错误码来测。
//! 放在 Rust 里至少能被 `cargo test` 覆盖。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use whatmedia_media3::{
    build_tracks_view, clamp_seek, to_error_payload, ErrorPayload, PlayerEvent as WireEvent,
    PlayerState, RawError, RawTracksPayload, RemoteKey, Snapshot,
};

/// Kotlin 送来的原始事件。**不是**给前端的线格式。
///
/// 之所以单独一套：`errorCode` 是 Media3 的 int，前端认的是
/// `"network"` / `"decode"` 这样的字符串。翻译在这里发生。
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RawEvent {
    StateChange { from: String, to: String },
    TimeUpdate { position: f64, duration: f64 },
    Playing,
    Paused { by_user: bool },
    Waiting,
    Ended,
    TracksListed,
    Error { error: RawError },
    PipChanged { active: bool },
    RemoteKey { key: String },
    NetworkChanged { online: bool, kind: String },
}

/// 原始事件 → 前端事件。翻译不了返回 None（**不猜**）。
pub fn translate(raw: &RawEvent) -> Option<WireEvent> {
    Some(match raw {
        RawEvent::StateChange { from, to } => WireEvent::StateChange {
            from: PlayerState::parse(from)?,
            to: PlayerState::parse(to)?,
        },
        RawEvent::TimeUpdate { position, duration } => WireEvent::TimeUpdate {
            position: *position,
            duration: *duration,
        },
        RawEvent::Playing => WireEvent::Playing,
        RawEvent::Paused { by_user } => WireEvent::Paused { by_user: *by_user },
        RawEvent::Waiting => WireEvent::Waiting,
        RawEvent::Ended => WireEvent::Ended,
        RawEvent::TracksListed => WireEvent::TracksListed,
        RawEvent::Error { error } => WireEvent::Error {
            error: to_error_payload(error),
        },
        RawEvent::PipChanged { active } => WireEvent::PipChanged { active: *active },
        RawEvent::RemoteKey { key } => WireEvent::RemoteKey {
            key: RemoteKey::parse(key)?,
        },
        RawEvent::NetworkChanged { online, kind } => WireEvent::NetworkChanged {
            online: *online,
            kind: parse_network_kind(kind),
        },
    })
}

fn parse_network_kind(s: &str) -> whatmedia_media3::NetworkKind {
    match s {
        "wifi" => whatmedia_media3::NetworkKind::Wifi,
        "cellular" => whatmedia_media3::NetworkKind::Cellular,
        "ethernet" => whatmedia_media3::NetworkKind::Ethernet,
        "none" => whatmedia_media3::NetworkKind::None,
        _ => whatmedia_media3::NetworkKind::Unknown,
    }
}

/// 把 Kotlin 送来的原始轨道 + 播放器状态拼成前端要的 `Snapshot`。
///
/// 状态由 Kotlin 给（它才知道 ExoPlayer 现在是什么状态），
/// 轨道列表由这里生成（标签与 id 的编法要能被测试）。
pub fn build_snapshot(state: &str, raw: &RawTracksPayload) -> Option<Snapshot> {
    let view = build_tracks_view(raw);
    Some(Snapshot {
        state: PlayerState::parse(state)?,
        position: 0.0,
        duration: 0.0,
        volume: 1.0,
        muted: false,
        rate: 1.0,
        buffered: 0.0,
        audio_tracks: view.audio,
        subtitle_tracks: view.subtitle,
        qualities: view.qualities,
        current_audio_track_id: view.current_audio_id,
        current_subtitle_track_id: view.current_subtitle_id,
        current_quality_id: view.current_quality_id,
        error: None,
    })
}

/// seek 前的收敛。前端传来的秒数可能是 NaN 或超过时长。
#[tauri::command]
fn media3_clamp_seek(target: f64, duration: f64) -> f64 {
    clamp_seek(target, duration)
}

// ── 命令 ──────────────────────────────────────────────────
//
// **命名规则（重要）**：这里的命令名不能与 Kotlin 侧
// （`plugins/tauri-plugin-media3/android/.../Media3Plugin.kt`）重名。
// 两侧同属 `plugin:media3|` 命名空间，同名命令由谁处理取决于 Tauri 的
// 路由顺序 —— 而那个顺序在 Android 与桌面上还不一样。
// 撞名的表现是"命令有时生效有时不生效"，排查起来极其费时。
//
// 所以：播放器**动作类**命令全在 Kotlin（media3_load / play / seek / …），
// **翻译类**命令全在 Rust（media3_snapshot / media3_emit / media3_clamp_seek）。

/// 取一份带类型的快照。Kotlin 只给原值，标签在这里生成。
#[tauri::command]
fn media3_snapshot(state: String, raw: RawTracksPayload) -> Result<Snapshot, Media3Error> {
    build_snapshot(&state, &raw).ok_or_else(|| Media3Error::BadState(state))
}

/// 把一个原始事件翻译成前端事件并发给前端。
///
/// 由 Kotlin 侧通道回调用。走 `Emitter` 而不是返回值，
/// 因为事件是 Kotlin 主动推的，不是前端请求来的。
#[tauri::command]
fn media3_emit<R: Runtime>(app: AppHandle<R>, raw: RawEvent) -> Result<(), Media3Error> {
    let Some(ev) = translate(&raw) else {
        return Err(Media3Error::Untranslatable);
    };
    app.emit(whatmedia_media3::EVENT_CHANNEL, &ev)
        .map_err(|e| Media3Error::Bridge(e.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum Media3Error {
    #[error("media3 桥接失败：{0}")]
    Bridge(String),
    #[error("未知播放器状态：{0}")]
    BadState(String),
    #[error("事件无法翻译（未知的枚举字面量）")]
    Untranslatable,
}

impl Serialize for Media3Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("media3")
        .invoke_handler(tauri::generate_handler![
            media3_snapshot,
            media3_emit,
            media3_clamp_seek
        ])
        .build()
}

/// 供 Kotlin 侧反查：`EventEmitter` 的目标通道名。
pub fn event_channel() -> &'static str {
    whatmedia_media3::EVENT_CHANNEL
}

/// 共享状态占位。当前实现是无状态的（播放器实例在 Kotlin 侧），
/// 留这个类型是为了将来要在 Rust 侧缓存轨道列表时不必改注册方式。
pub struct Media3State {
    pub last_error: std::sync::Mutex<Option<ErrorPayload>>,
}

impl Default for Media3State {
    fn default() -> Self {
        Self {
            last_error: std::sync::Mutex::new(None),
        }
    }
}

/// 把状态装进 app。桌面端（Windows）上这个插件不注册 —— 见 `init` 的调用点。
pub fn manage(app: &AppHandle<impl Runtime>) -> Arc<Media3State> {
    let state = Arc::new(Media3State::default());
    app.manage(state.clone());
    state
}
