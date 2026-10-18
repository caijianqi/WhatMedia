//! Media3 / ExoPlayer 后端：**原生播放器 + Tauri 插件桥**，不是 WebView 里的 `<video>`。
//!
//! ## 为什么 Android 端必须走原生播放器
//!
//! 桌面端 mpv 走"外部进程 + JSON IPC"（ADR-0003），绕开了"把播放器塞进 WebView"
//! 这个难题。Android 上绕不开：
//!
//! · 浏览器/WebView 不支持 AC3 / DTS / TrueHD —— 国内下载片源里 AC3 是常态。
//! · Android WebView 的 HLS 支持依赖系统实现，不同厂商表现完全不同，
//!   有的机型播不出 fMP4 分片，而且**不报错，只是黑屏**。
//! · MediaSession / PiP / 后台播放 / 蓝牙耳机，全都是原生能力，WebView 一层都拿不到。
//!
//! 所以：播放器是原生 `PlayerView`，浮在 WebView **之下**（Tauri 2 支持
//! 在 WebView 后面插一个原生 Surface），UI 仍由 Vue 渲染在上层。
//! 这条路的三个风险（图层叠加、位图字幕、MediaSession 稳定性）由 S1 Spike 验证。
//!
//! ## 这个 crate 负责什么
//!
//! 与 `crates/mpv` 完全同构：**只负责协议与语义，不负责进程与平台调用**。
//! 真正的 ExoPlayer 调用在 `plugins/tauri-plugin-media3/android/` 的 Kotlin 里。
//!
//! 分工的理由是**可测性**：Kotlin 侧在当前开发环境没有 Android SDK，
//! 一行都跑不了测试。所以凡是会算错的逻辑（错误码分类、语言归一化、
//! seek 收敛、轨道 id 编解码）全部搬到这里，让它至少有一层能验证。
//! Kotlin 侧只做"把 ExoPlayer 的原值传进来、把结果传回去"。
//!
//! ## 三层契约
//!
//! ```text
//!   TypeScript (client/src/player/media3.ts)
//!        │  Tauri invoke / channel
//!   Rust  (本 crate，桥 + 语义)
//!        │  JNI
//!   Kotlin (plugins/tauri-plugin-media3/android)
//! ```
//!
//! **字段名三处必须一致，且 Kotlin 是唯一没有编译期检查的一侧**
//! （JSON 字段不匹配在 kotlinx.serialization 上是运行时缺字段，不报错）。
//! 改动流程：先改本文件 → 再改 Kotlin → 最后改 TypeScript。

pub mod model;
pub mod protocol;
pub mod session;

pub use model::{
    ErrorCode, ErrorPayload, ExternalSubtitle, LoadRequest, MediaMetadata, NetworkKind,
    PlayerEvent, PlayerState, RawError, RawTrack, RawTracksPayload, RemoteKey, Snapshot, Source,
    Track,
};
pub use protocol::{
    build_tracks_view, clamp_seek, classify_error, classify_http_status, is_retryable,
    language_label, normalize_duration, parse_track_id, quality_label, quality_track, selected_id,
    to_error_payload, to_qualities, to_tracks, track_id, track_label, TracksView, AUTO_QUALITY_ID,
    MEDIA3_TIME_UNSET, SEEK_TAIL_MARGIN_SECONDS,
};
pub use session::{sanitize as sanitize_metadata, seek_buttons_enabled, SessionActions};

/// 覆盖的 media3 最低版本。
///
/// 1.4.x 之前 `Tracks` / `MediaSession` 的 API 变动频繁，
/// 且 `media3-exoplayer-hls` 对 fMP4 分片的处理有 bug。
pub const MIN_MEDIA3_VERSION: (u32, u32) = (1, 4);

/// 判断 media3 版本是否满足要求。解析不出来一律 false —— 认出来才敢用。
pub fn media3_version_ok(version: &str) -> bool {
    let Some((maj, min)) = parse_version(version) else {
        return false;
    };
    (maj, min) >= MIN_MEDIA3_VERSION
}

fn parse_version(raw: &str) -> Option<(u32, u32)> {
    let token = raw
        .split_whitespace()
        .find(|t| t.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = token.split('.');
    let maj = parts.next()?.parse::<u32>().ok()?;
    let min = parts.next()?.parse::<u32>().ok()?;
    Some((maj, min))
}

/// 事件通道名。Kotlin → Rust → TS。
///
/// 与 mpv 的 `mpv://event` 保持同样的命名风格，
/// 这样前端的订阅代码能复用同一套写法。
pub const EVENT_CHANNEL: &str = "media3://event";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media3_version_gate() {
        assert!(media3_version_ok("1.4.1"));
        assert!(media3_version_ok("1.5.0"));
        assert!(!media3_version_ok("1.3.1"));
        assert!(!media3_version_ok("beta"));
    }

    #[test]
    fn snapshot_default_is_idle_not_playing() {
        let s = Snapshot::default();
        assert_eq!(s.state, PlayerState::Idle);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.rate, 1.0);
        assert!(s.current_subtitle_track_id.is_none());
    }

    #[test]
    fn state_string_round_trips() {
        for s in [
            PlayerState::Idle,
            PlayerState::Loading,
            PlayerState::Ready,
            PlayerState::Playing,
            PlayerState::Paused,
            PlayerState::Buffering,
            PlayerState::Ended,
            PlayerState::Error,
        ] {
            assert_eq!(PlayerState::parse(s.as_str()), Some(s));
        }
        assert_eq!(PlayerState::parse("nope"), None);
    }

    #[test]
    fn remote_key_round_trips() {
        for k in [
            RemoteKey::Up,
            RemoteKey::Down,
            RemoteKey::Left,
            RemoteKey::Right,
            RemoteKey::Select,
            RemoteKey::Back,
            RemoteKey::Menu,
            RemoteKey::Toggle,
            RemoteKey::Play,
            RemoteKey::Pause,
            RemoteKey::SeekForward,
            RemoteKey::SeekBackward,
            RemoteKey::Next,
            RemoteKey::Previous,
            RemoteKey::Stop,
        ] {
            assert_eq!(RemoteKey::parse(k.as_str()), Some(k));
        }
        assert_eq!(RemoteKey::parse("volume-up"), None);
    }

    #[test]
    fn network_kind_only_cell_is_metered() {
        assert!(NetworkKind::Cellular.is_metered());
        assert!(!NetworkKind::Wifi.is_metered());
        assert!(!NetworkKind::None.is_metered());
    }

    #[test]
    fn wire_format_matches_the_frontend_contract() {
        // 这条测试钉住的是**线格式**，不是 Rust 内部表示。
        // 前端手写了一份 TS 类型，Kotlin 手写了一份 data class，
        // 改了 serde 属性而不同步改那两份，就靠这条测试拦下来。
        let ev = PlayerEvent::TimeUpdate {
            position: 1.5,
            duration: 100.0,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(
            json,
            r#"{"type":"time-update","position":1.5,"duration":100.0}"#
        );

        let err = PlayerEvent::Error {
            error: ErrorPayload {
                code: ErrorCode::Network,
                message: "断网".into(),
            },
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains(r#""code":"network""#), "{json}");

        let pip = PlayerEvent::PipChanged { active: true };
        assert_eq!(
            serde_json::to_string(&pip).unwrap(),
            r#"{"type":"pip-changed","active":true}"#
        );

        let src = Source::Hls { url: "u".into() };
        assert_eq!(
            serde_json::to_string(&src).unwrap(),
            r#"{"kind":"hls","url":"u"}"#
        );
    }
}
