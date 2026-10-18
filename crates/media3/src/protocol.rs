//! 契约之外的**语义**：轨道映射、语言显示名、画质命名、错误分类、seek 收敛。
//!
//! 这一层存在的理由和 `crates/mpv` 一样：Kotlin 侧现在跑不了测试，
//! 所以凡是"会算错的逻辑"都搬到 Rust 里来，让它至少有一层能验证。
//! Kotlin 侧只负责把 ExoPlayer 的原始值（errorCode、format.language…）
//! 原样传进来，由这里决定它们意味着什么。

use crate::model::{ErrorCode, ErrorPayload, RawError, RawTrack, RawTracksPayload, Track};

/// ExoPlayer / Media3 的错误码。
///
/// 数值来自 `androidx.media3.common.PlaybackException`。
/// **不要拍脑袋改这些数** —— 改错了表现是"断网被当成解码失败"，
/// 于是 UI 提示"换个播放方式"，而实际只要等网络回来重连就行。
pub mod error_code {
    pub const UNSPECIFIED: i32 = 1000;
    pub const REMOTE_ERROR: i32 = 1001;
    pub const IO_BAD_HTTP_STATUS: i32 = 1002;
    pub const IO_FILE_NOT_FOUND: i32 = 1003;
    pub const IO_NO_PERMISSION: i32 = 1004;
    pub const IO_UNSPECIFIED: i32 = 1005;
    pub const IO_READ_POSITION_OUT_OF_RANGE: i32 = 1006;
    pub const IO_CLEARTEXT_NOT_PERMITTED: i32 = 1007;
    pub const IO_NETWORK_CONNECTION_FAILED: i32 = 1008;
    pub const IO_NETWORK_CONNECTION_TIMEOUT: i32 = 1009;
    pub const IO_INVALID_HTTP_CONTENT_TYPE: i32 = 1010;
    pub const PARSING_CONTAINER_MALFORMED: i32 = 3001;
    pub const PARSING_MANIFEST_MALFORMED: i32 = 3002;
    pub const PARSING_CONTAINER_UNSUPPORTED: i32 = 3003;
    pub const PARSING_MANIFEST_UNSUPPORTED: i32 = 3004;
    pub const DECODER_INIT_FAILED: i32 = 4001;
    pub const DECODING_FAILED: i32 = 4002;
    pub const DECODING_FORMAT_UNSUPPORTED: i32 = 4003;
    pub const DECODING_FORMAT_EXCEEDS_CAPABILITIES: i32 = 4004;
    pub const AUDIO_TRACK_INIT_FAILED: i32 = 5001;
    pub const AUDIO_TRACK_WRITE_FAILED: i32 = 5002;
    pub const TIMEOUT: i32 = 9000;
}

/// 把 Media3 的错误码翻译成我们的 `ErrorCode`。
///
/// 认不出来的码一律归 `Unknown` 而不是 `Network`。
/// 归 Network 会触发自动重连，而重连对"解码不支持"是纯浪费
/// —— 还会让用户等三轮退避才看到"这个片子你的手机放不了"。
pub fn classify_error(code: i32) -> ErrorCode {
    match code {
        error_code::IO_NETWORK_CONNECTION_FAILED
        | error_code::IO_NETWORK_CONNECTION_TIMEOUT
        | error_code::REMOTE_ERROR
        | error_code::TIMEOUT
        | error_code::IO_UNSPECIFIED => ErrorCode::Network,

        error_code::IO_FILE_NOT_FOUND | error_code::IO_BAD_HTTP_STATUS => ErrorCode::NotFound,

        error_code::DECODING_FORMAT_UNSUPPORTED
        | error_code::DECODING_FORMAT_EXCEEDS_CAPABILITIES
        | error_code::DECODER_INIT_FAILED
        | error_code::PARSING_CONTAINER_UNSUPPORTED
        | error_code::PARSING_MANIFEST_UNSUPPORTED
        | error_code::PARSING_CONTAINER_MALFORMED
        | error_code::PARSING_MANIFEST_MALFORMED => ErrorCode::Decode,

        _ => ErrorCode::Unknown,
    }
}

/// HTTP 状态码 → 错误码。Media3 把 4xx/5xx 都报成 `IO_BAD_HTTP_STATUS`，
/// 具体码要自己从 cause 里抠。抠不到就按 Unknown，别猜。
pub fn classify_http_status(status: i32) -> ErrorCode {
    match status {
        404 | 410 => ErrorCode::NotFound,
        401 | 403 => ErrorCode::Unknown, // 鉴权问题，重试只会再失败一次
        _ => ErrorCode::Unknown,
    }
}

/// 这个错误**值不值得自动重连**。
///
/// 只有网络类值得。解码类重连一百次还是解不了，
/// 而用户会看到播放器反复转圈 —— 这是比直接报错更糟的体验。
pub fn is_retryable(code: ErrorCode) -> bool {
    matches!(code, ErrorCode::Network)
}

/// 轨道 id。格式 `g{group}:t{index}`。
///
/// 为什么不用纯数字下标：ExoPlayer 的 `Tracks` 在切换音轨后可能重排 group，
/// 而且字幕轨与音轨的 group 是两套编号。
/// 用 `g:t` 让 Kotlin 侧能反查出确定的 group 与 track，不需要维护映射表。
pub fn track_id(group: usize, index: usize) -> String {
    format!("g{group}:t{index}")
}

/// 反解轨道 id。解析不出返回 None，**不猜** ——
/// 猜错的后果是选中了另一条音轨，用户会听到莫名其妙的语言。
pub fn parse_track_id(id: &str) -> Option<(usize, usize)> {
    let (g, t) = id.split_once(':')?;
    let g = g.strip_prefix('g')?.parse::<usize>().ok()?;
    let t = t.strip_prefix('t')?.parse::<usize>().ok()?;
    Some((g, t))
}

/// 自动画质档的 id。ExoPlayer 的 track selector 支持自动适配，
/// 给它一个保留 id，别和真实档位冲突（真实档位永远是 `g*:t*`）。
pub const AUTO_QUALITY_ID: &str = "auto";

/// 语言标签 → 显示名。
///
/// 输入来源混杂：MKV 里常见 ISO-639-2（`chi` `eng`），MP4 里常见 639-1（`zh` `en`），
/// 有的刮削器还会写 `zh-CN` 这种带地区的。ExoPlayer 原样透传，不做归一化。
/// 所以这里必须有三级兜底，否则字幕菜单会显示一排 `und`。
pub fn language_label(tag: Option<&str>) -> Option<String> {
    let tag = tag?.trim();
    if tag.is_empty() {
        return None;
    }
    let base = tag.split(['-', '_']).next().unwrap_or(tag).to_lowercase();
    let name = match base.as_str() {
        "zh" | "chi" | "zho" | "cmn" => "中文",
        "en" | "eng" => "英语",
        "ja" | "jpn" => "日语",
        "ko" | "kor" => "韩语",
        "fr" | "fre" | "fra" => "法语",
        "de" | "ger" | "deu" => "德语",
        "es" | "spa" => "西班牙语",
        "ru" | "rus" => "俄语",
        "it" | "ita" => "意大利语",
        "pt" | "por" => "葡萄牙语",
        "ar" | "ara" => "阿拉伯语",
        "hi" | "hin" => "印地语",
        "th" | "tha" => "泰语",
        "vi" | "vie" => "越南语",
        "id" | "ind" => "印尼语",
        // `und` 是"未定义"的标准写法。返回 None 让上层走"轨道 N"的兜底，
        // 而不是把 `und` 当成一种语言显示给用户。
        "und" | "mul" | "zxx" => return None,
        _ => return Some(tag.to_uppercase()),
    };
    Some(name.to_string())
}

/// 轨道显示名。
///
/// 优先级：**容器里带的标题 > 语言显示名 > "轨道 N"**。
/// 用容器标题是因为它往往写着"导演评论音轨""简体中文（特效）"这类
/// 比语言码有用得多的信息。
pub fn track_label(title: Option<&str>, language: Option<&str>, fallback_index: usize) -> String {
    if let Some(t) = title.map(str::trim).filter(|t| !t.is_empty()) {
        return t.to_string();
    }
    if let Some(l) = language_label(language) {
        return l;
    }
    format!("轨道 {}", fallback_index + 1)
}

/// 画质档显示名。
///
/// 480/576 归到"标清"而不是显示 `480p`？
/// 不 —— 用户认的是数字。这里只在**高度缺失**时按码率给个粗分档。
pub fn quality_label(height: Option<u32>, bitrate_kbps: Option<u32>) -> String {
    if let Some(h) = height {
        return match h {
            0 => "自动".to_string(),
            h if h >= 2160 => "4K".to_string(),
            h if h >= 1440 => "2K".to_string(),
            h if h >= 1080 => "1080P".to_string(),
            h if h >= 720 => "720P".to_string(),
            h if h >= 480 => "480P".to_string(),
            h if h >= 360 => "360P".to_string(),
            _ => format!("{h}P"),
        };
    }
    match bitrate_kbps {
        Some(b) if b >= 8000 => "高清".to_string(),
        Some(b) if b >= 2500 => "标清".to_string(),
        Some(_) => "流畅".to_string(),
        None => "自动".to_string(),
    }
}

/// 构造一个画质档轨道。
pub fn quality_track(id: String, height: Option<u32>, bitrate_kbps: Option<u32>) -> Track {
    Track {
        label: quality_label(height, bitrate_kbps),
        id,
        language: None,
        default: false,
        forced: false,
        height,
        bitrate_kbps,
    }
}

/// 把 Kotlin 送来的原始轨道列表变成前端要的 `Track` 列表。
///
/// 放在 Rust 里而不是 Kotlin 里，理由见 `RawTrack` 的注释：
/// 标签与 id 的编法是最容易算错的部分，必须落在能测的一侧。
///
/// `fallback_start` 是"轨道 N"的起始序号 —— 音轨与字幕轨各自从 1 开始编号，
/// 由调用方传，别在这里猜。
pub fn to_tracks(raw: &[RawTrack], fallback_start: usize) -> Vec<Track> {
    raw.iter()
        .enumerate()
        .map(|(i, r)| Track {
            id: track_id(r.group, r.index),
            label: track_label(
                r.title.as_deref(),
                r.language.as_deref(),
                fallback_start + i,
            ),
            language: r.language.clone(),
            default: r.default,
            forced: r.forced,
            height: r.height,
            bitrate_kbps: r.bitrate_kbps,
        })
        .collect()
}

/// 从原始视频轨里挑出画质档。
///
/// **只保留不同高度的档**：HLS master playlist 里同一个高度常常有多个码率变体
/// （尤其服务端开了多码率）。全列出来会得到"1080P / 1080P / 720P"这种菜单。
/// 去重时保留**码率最高**的那条 —— 同一高度下码率高的画质更好，
/// 而用户看菜单时看到的是同一个标签，取更好的那条没有坏处。
pub fn to_qualities(raw: &[RawTrack]) -> Vec<Track> {
    let mut best: std::collections::BTreeMap<u32, &RawTrack> = std::collections::BTreeMap::new();
    for r in raw {
        let Some(h) = r.height.filter(|h| *h > 0) else {
            continue;
        };
        match best.get(&h) {
            Some(prev) if prev.bitrate_kbps.unwrap_or(0) >= r.bitrate_kbps.unwrap_or(0) => {}
            _ => {
                best.insert(h, r);
            }
        }
    }
    // 从高到低：菜单顺序要符合直觉。
    best.iter()
        .rev()
        .map(|(_, r)| quality_track(track_id(r.group, r.index), r.height, r.bitrate_kbps))
        .collect()
}

/// 当前选中的轨道 id。没有选中（比如字幕被关掉）返回 None。
pub fn selected_id(raw: &[RawTrack]) -> Option<String> {
    raw.iter()
        .find(|r| r.selected)
        .map(|r| track_id(r.group, r.index))
}

/// Kotlin 送回来的三套原始轨道 → 前端要的三套 `Track`。
///
/// 音轨与字幕轨各自从 1 开始编号（"轨道 N"），因为它们是**两个菜单**，
/// 用户在字幕菜单里看到"轨道 3"指的是第三条字幕，不该是全局第三条轨道。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TracksView {
    pub audio: Vec<Track>,
    pub subtitle: Vec<Track>,
    pub qualities: Vec<Track>,
    pub current_audio_id: Option<String>,
    pub current_subtitle_id: Option<String>,
    pub current_quality_id: Option<String>,
}

pub fn build_tracks_view(raw: &RawTracksPayload) -> TracksView {
    TracksView {
        audio: to_tracks(&raw.audio, 0),
        subtitle: to_tracks(&raw.text, 0),
        qualities: to_qualities(&raw.video),
        current_audio_id: selected_id(&raw.audio),
        current_subtitle_id: selected_id(&raw.text),
        current_quality_id: selected_id(&raw.video),
    }
}

/// Kotlin 的原始错误码 → 前端要的错误载荷。
///
/// 这一步是**整个降级链的关键分叉点**：归 Network 会触发自动重连，
/// 归 Decode 会触发"让服务端重新决策"。分错了用户会看到一个
/// 反复重试却永远失败的播放器。
pub fn to_error_payload(raw: &RawError) -> ErrorPayload {
    ErrorPayload {
        code: classify_error(raw.error_code),
        message: raw.message.clone(),
    }
}

/// seek 位置收敛。
///
/// 三件事：负数归 0、超过时长压到"时长 - 尾部余量"、NaN 归 0。
///
/// **为什么要留尾部余量**：seek 到正好 `duration` 会直接触发 ended，
/// 而不是停在最后一帧。用户从进度条拖到最右端，期待的是"看到片尾画面"，
/// 不是"播放结束、弹出下一集"。
///
/// **NaN 必须处理**：ExoPlayer 在元数据还没到手时 `duration` 是 `TIME_UNSET`（-9223372036854775807），
/// 除一下就出 NaN，再传给 seek 会让 Media3 抛 IllegalArgumentException。
pub const SEEK_TAIL_MARGIN_SECONDS: f64 = 0.5;

pub fn clamp_seek(target: f64, duration: f64) -> f64 {
    if !target.is_finite() || target < 0.0 {
        return 0.0;
    }
    if !duration.is_finite() || duration <= 0.0 {
        // 时长未知时不能压上限，但也不能让 target 无限大。
        return target;
    }
    let max = (duration - SEEK_TAIL_MARGIN_SECONDS).max(0.0);
    target.min(max)
}

/// Media3 用 `C.TIME_UNSET` 表示"时长未知"。前端只认 0。
pub const MEDIA3_TIME_UNSET: i64 = i64::MIN;

pub fn normalize_duration(raw_ms: i64) -> f64 {
    if raw_ms == MEDIA3_TIME_UNSET || raw_ms <= 0 {
        0.0
    } else {
        raw_ms as f64 / 1000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_failures_are_retryable() {
        assert_eq!(
            classify_error(error_code::IO_NETWORK_CONNECTION_FAILED),
            ErrorCode::Network
        );
        assert_eq!(
            classify_error(error_code::IO_NETWORK_CONNECTION_TIMEOUT),
            ErrorCode::Network
        );
        assert!(is_retryable(ErrorCode::Network));
    }

    #[test]
    fn decode_failures_are_not_retryable() {
        // 这是最关键的一条：解码失败重试只会让播放器反复转圈。
        assert_eq!(
            classify_error(error_code::DECODING_FORMAT_UNSUPPORTED),
            ErrorCode::Decode
        );
        assert!(!is_retryable(ErrorCode::Decode));
        assert!(!is_retryable(ErrorCode::NotFound));
    }

    #[test]
    fn unknown_codes_are_not_assumed_to_be_network() {
        // 认不出来就认不出来。猜成 Network 会启动无意义的重连。
        assert_eq!(
            classify_error(error_code::AUDIO_TRACK_WRITE_FAILED),
            ErrorCode::Unknown
        );
        assert_eq!(classify_error(-1), ErrorCode::Unknown);
    }

    #[test]
    fn http_404_is_not_found() {
        assert_eq!(classify_http_status(404), ErrorCode::NotFound);
        assert_eq!(classify_http_status(410), ErrorCode::NotFound);
        // 403 重试没用，但也不是"资源不存在" —— 归 Unknown，让 UI 报原文。
        assert_eq!(classify_http_status(403), ErrorCode::Unknown);
    }

    #[test]
    fn track_id_round_trips() {
        let id = track_id(2, 5);
        assert_eq!(id, "g2:t5");
        assert_eq!(parse_track_id(&id), Some((2, 5)));
    }

    #[test]
    fn malformed_track_id_is_rejected_not_guessed() {
        assert_eq!(parse_track_id("2:5"), None);
        assert_eq!(parse_track_id("g2"), None);
        assert_eq!(parse_track_id(""), None);
        assert_eq!(parse_track_id("ga:tb"), None);
        // 真实的档位 id 不能被误解析成轨道
        assert_eq!(parse_track_id(AUTO_QUALITY_ID), None);
    }

    #[test]
    fn language_labels_cover_iso639_2_and_1() {
        assert_eq!(language_label(Some("chi")).as_deref(), Some("中文"));
        assert_eq!(language_label(Some("zh")).as_deref(), Some("中文"));
        assert_eq!(language_label(Some("zho")).as_deref(), Some("中文"));
        assert_eq!(language_label(Some("zh-CN")).as_deref(), Some("中文"));
        assert_eq!(language_label(Some(" ZH_CN ")).as_deref(), Some("中文"));
        assert_eq!(language_label(Some("eng")).as_deref(), Some("英语"));
    }

    #[test]
    fn undefined_language_is_not_shown_as_a_language() {
        assert_eq!(language_label(Some("und")), None);
        assert_eq!(language_label(None), None);
        assert_eq!(language_label(Some("  ")), None);
    }

    #[test]
    fn unknown_language_falls_back_to_the_raw_tag() {
        assert_eq!(language_label(Some("yue")).as_deref(), Some("YUE"));
    }

    #[test]
    fn track_label_prefers_container_title() {
        assert_eq!(
            track_label(Some("导演评论音轨"), Some("eng"), 0),
            "导演评论音轨"
        );
        assert_eq!(track_label(None, Some("jpn"), 0), "日语");
        assert_eq!(track_label(None, None, 2), "轨道 3");
        // 空标题不算标题
        assert_eq!(track_label(Some("   "), Some("eng"), 0), "英语");
    }

    #[test]
    fn quality_labels_use_numbers_users_recognize() {
        assert_eq!(quality_label(Some(2160), None), "4K");
        assert_eq!(quality_label(Some(1080), None), "1080P");
        assert_eq!(quality_label(Some(720), None), "720P");
        assert_eq!(quality_label(Some(480), None), "480P");
    }

    #[test]
    fn quality_falls_back_to_bitrate_when_height_missing() {
        assert_eq!(quality_label(None, Some(12000)), "高清");
        assert_eq!(quality_label(None, Some(3000)), "标清");
        assert_eq!(quality_label(None, Some(800)), "流畅");
        assert_eq!(quality_label(None, None), "自动");
    }

    fn raw() -> Vec<RawTrack> {
        vec![
            RawTrack {
                group: 0,
                index: 0,
                language: Some("eng".into()),
                selected: true,
                ..Default::default()
            },
            RawTrack {
                group: 0,
                index: 1,
                language: Some("chi".into()),
                ..Default::default()
            },
        ]
    }

    #[test]
    fn raw_tracks_become_labelled_tracks() {
        let out = to_tracks(&raw(), 0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "g0:t0");
        assert_eq!(out[0].label, "英语");
        assert_eq!(out[1].id, "g0:t1");
        assert_eq!(out[1].label, "中文");
    }

    #[test]
    fn track_numbering_starts_at_the_given_offset() {
        let unnamed = vec![RawTrack {
            group: 1,
            index: 3,
            ..Default::default()
        }];
        assert_eq!(to_tracks(&unnamed, 4)[0].label, "轨道 5");
    }

    #[test]
    fn selected_id_none_when_all_deselected() {
        // 字幕被关掉时 ExoPlayer 一条都不选，不是"选了第一条"。
        let mut none = raw();
        none[0].selected = false;
        assert_eq!(selected_id(&none), None);
    }

    #[test]
    fn selected_id_finds_the_selected_one() {
        assert_eq!(selected_id(&raw()).as_deref(), Some("g0:t0"));
    }

    #[test]
    fn qualities_deduplicate_by_height_keeping_the_best_bitrate() {
        let v = vec![
            RawTrack {
                group: 0,
                index: 0,
                height: Some(1080),
                bitrate_kbps: Some(3000),
                ..Default::default()
            },
            RawTrack {
                group: 0,
                index: 1,
                height: Some(1080),
                bitrate_kbps: Some(8000),
                ..Default::default()
            },
            RawTrack {
                group: 0,
                index: 2,
                height: Some(720),
                bitrate_kbps: Some(1500),
                ..Default::default()
            },
        ];
        let q = to_qualities(&v);
        // 两个 1080 合并成一个，且留码率高的那条。
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].label, "1080P");
        assert_eq!(q[0].bitrate_kbps, Some(8000));
        assert_eq!(q[1].label, "720P");
    }

    #[test]
    fn qualities_are_sorted_high_to_low() {
        let v = vec![
            RawTrack {
                group: 0,
                index: 0,
                height: Some(480),
                ..Default::default()
            },
            RawTrack {
                group: 0,
                index: 1,
                height: Some(2160),
                ..Default::default()
            },
            RawTrack {
                group: 0,
                index: 2,
                height: Some(720),
                ..Default::default()
            },
        ];
        let q = to_qualities(&v);
        assert_eq!(
            q.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(),
            vec!["4K", "720P", "480P"]
        );
    }

    #[test]
    fn tracks_without_height_are_not_qualities() {
        // 音轨没有 height，不能混进画质菜单。
        let v = vec![RawTrack {
            group: 0,
            index: 0,
            height: None,
            bitrate_kbps: Some(128),
            ..Default::default()
        }];
        assert!(to_qualities(&v).is_empty());
    }

    #[test]
    fn tracks_view_numbers_each_menu_from_one() {
        let raw = RawTracksPayload {
            audio: vec![
                RawTrack {
                    group: 0,
                    index: 0,
                    ..Default::default()
                },
                RawTrack {
                    group: 0,
                    index: 1,
                    ..Default::default()
                },
            ],
            text: vec![RawTrack {
                group: 1,
                index: 0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let v = build_tracks_view(&raw);
        assert_eq!(v.audio[0].label, "轨道 1");
        assert_eq!(v.audio[1].label, "轨道 2");
        // 字幕菜单独立编号
        assert_eq!(v.subtitle[0].label, "轨道 1");
    }

    #[test]
    fn tracks_view_picks_current_ids() {
        let raw = RawTracksPayload {
            audio: vec![RawTrack {
                group: 0,
                index: 1,
                selected: true,
                ..Default::default()
            }],
            video: vec![RawTrack {
                group: 2,
                index: 0,
                height: Some(1080),
                selected: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let v = build_tracks_view(&raw);
        assert_eq!(v.current_audio_id.as_deref(), Some("g0:t1"));
        assert_eq!(v.current_quality_id.as_deref(), Some("g2:t0"));
        assert_eq!(v.current_subtitle_id, None);
        assert_eq!(v.qualities.len(), 1);
    }

    #[test]
    fn raw_error_is_classified_before_reaching_the_ui() {
        let e = to_error_payload(&RawError {
            error_code: error_code::IO_NETWORK_CONNECTION_FAILED,
            message: "断网了".into(),
        });
        assert_eq!(e.code, ErrorCode::Network);
        assert_eq!(e.message, "断网了");

        let e = to_error_payload(&RawError {
            error_code: error_code::DECODING_FORMAT_UNSUPPORTED,
            message: "DTS 不支持".into(),
        });
        assert_eq!(e.code, ErrorCode::Decode);
    }

    #[test]
    fn seek_is_clamped_before_the_end() {
        assert_eq!(clamp_seek(-5.0, 100.0), 0.0);
        assert_eq!(clamp_seek(100.0, 100.0), 99.5);
        assert_eq!(clamp_seek(50.0, 100.0), 50.0);
    }

    #[test]
    fn nan_seek_is_rejected() {
        assert_eq!(clamp_seek(f64::NAN, 100.0), 0.0);
        assert_eq!(clamp_seek(f64::INFINITY, 0.0), 0.0);
    }

    #[test]
    fn unset_duration_is_zero_not_a_huge_number() {
        assert_eq!(normalize_duration(MEDIA3_TIME_UNSET), 0.0);
        assert_eq!(normalize_duration(-1), 0.0);
        assert_eq!(normalize_duration(90_000), 90.0);
    }
}
