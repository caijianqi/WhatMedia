//! MediaSession 元数据与通知栏行为。
//!
//! 通知栏 / 锁屏 / 蓝牙耳机 / Android Auto 上显示什么，全由这里决定。
//! 这层看着只是"填几个字符串"，但它是 Android 上**唯一**能在应用退到后台后
//! 还让用户控制播放的地方 —— 填错了（标题空、封面给了 asset://）不会报错，
//! 只是通知栏上不显示，而用户根本不知道该去哪找。

use crate::model::MediaMetadata;

/// 元数据回填默认值。
///
/// 三件事：
///   1. 标题为空得有个兜底 —— 空标题的通知栏在 Android 上会被系统折叠成
///      "正在播放"，用户看不出在播什么。
///   2. 封面 URL 必须是 http(s)。`asset://` / `file://` 在 MediaSession 里
///      **静默失败**：不报错、不显示、日志里也没有。这是最容易漏的一条。
///   3. 负时长归 0 —— 负值会让通知栏的进度条画到屏幕外。
pub fn sanitize(meta: &MediaMetadata, fallback_title: &str) -> MediaMetadata {
    let title = match meta.title.trim() {
        "" => {
            if fallback_title.trim().is_empty() {
                "未知标题".to_string()
            } else {
                fallback_title.trim().to_string()
            }
        }
        t => t.to_string(),
    };

    let artwork = meta
        .artwork_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))
        .map(|u| u.to_string());

    MediaMetadata {
        title,
        subtitle: meta.subtitle.trim().to_string(),
        artwork_url: artwork,
        duration_ms: if meta.duration_ms > 0 {
            meta.duration_ms
        } else {
            0
        },
    }
}

/// MediaSession 上要不要显示"快进/快退"。
///
/// 直播（duration 未知 / 为 0）不能显示 seek 按钮 —— 按下去会跳到一个不存在的位置，
/// Media3 会把它当成一个永远填不满的缓冲，表现是"卡住不动"。
pub fn seek_buttons_enabled(duration_ms: i64) -> bool {
    duration_ms > 0
}

/// 通知栏要显示的动作集合。
///
/// **按设备形态区分**：TV 上没有通知栏，手机上没有 D-Pad，
/// 给 TV 下发"上一首/下一首"会导致遥控器按上下键切集数而不是移动焦点 ——
/// 这是一个真实会发生且极难定位的冲突。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionActions {
    /// 手机：完整一套。
    Handheld,
    /// TV：只保留播放/暂停与停止，方向键归焦点系统。
    Tv,
}

impl SessionActions {
    /// 是否暴露"上一集 / 下一集"。
    pub fn has_skip(self) -> bool {
        matches!(self, Self::Handheld)
    }

    /// 是否暴露快进快退（10 秒）。
    pub fn has_seek_jump(self) -> bool {
        matches!(self, Self::Handheld)
    }

    /// 是否显示通知栏。TV 上没有通知栏这一层 UI。
    pub fn has_notification(self) -> bool {
        matches!(self, Self::Handheld)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> MediaMetadata {
        MediaMetadata {
            title: "  ".to_string(),
            subtitle: "  ".to_string(),
            artwork_url: None,
            duration_ms: 0,
        }
    }

    #[test]
    fn empty_title_falls_back() {
        let out = sanitize(&meta(), "星际穿越");
        assert_eq!(out.title, "星际穿越");
    }

    #[test]
    fn empty_title_without_fallback_is_still_readable() {
        let out = sanitize(&meta(), "");
        assert_eq!(out.title, "未知标题");
    }

    #[test]
    fn real_title_is_trimmed() {
        let mut m = meta();
        m.title = "  星际穿越  ".to_string();
        assert_eq!(sanitize(&m, "x").title, "星际穿越");
    }

    #[test]
    fn asset_urls_are_dropped_silently_but_safely() {
        // MediaSession 只认 http(s)。给 asset:// 会静默不显示封面，
        // 与其静默失败，不如在这里就明确丢掉 → 上层可以换一个 http 地址。
        let mut m = meta();
        m.title = "x".to_string();
        m.artwork_url = Some("asset://localhost/poster.jpg".to_string());
        assert_eq!(sanitize(&m, "x").artwork_url, None);
    }

    #[test]
    fn http_artwork_is_kept() {
        let mut m = meta();
        m.title = "x".to_string();
        m.artwork_url = Some("https://cdn.example.com/p.jpg".to_string());
        assert!(sanitize(&m, "x").artwork_url.is_some());
    }

    #[test]
    fn negative_duration_becomes_zero() {
        let mut m = meta();
        m.duration_ms = -1;
        assert_eq!(sanitize(&m, "x").duration_ms, 0);
    }

    #[test]
    fn live_streams_hide_seek_buttons() {
        assert!(!seek_buttons_enabled(0));
        assert!(!seek_buttons_enabled(-1));
        assert!(seek_buttons_enabled(7_200_000));
    }

    #[test]
    fn tv_does_not_expose_skip_actions() {
        // 遥控器上下键必须留给焦点移动，不能去切集数。
        assert!(!SessionActions::Tv.has_skip());
        assert!(!SessionActions::Tv.has_seek_jump());
        assert!(!SessionActions::Tv.has_notification());
        assert!(SessionActions::Handheld.has_skip());
    }
}
