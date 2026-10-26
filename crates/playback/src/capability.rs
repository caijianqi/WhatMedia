//! 播放决策的四个输入：客户端能力 / 片源能力 / 服务端能力 / 网络。
//!
//! 决策引擎的**全部输入都是"事实"**，不含策略：
//! 客户端能解什么、片源是什么、服务端有什么、带宽多少。
//! "该走哪条路"是 `decision` 模块的事。
//!
//! 分开的理由：能力可以从 HTTP 头 / ffprobe / 启动自检里各自拿到，
//! 而策略只有一份。混在一起就没法单独测"这个客户端能不能放 AV1"。

use serde::{Deserialize, Serialize};
use whatmedia_core::{
    AudioCodec, Container, HdrType, MediaFile, MediaStream, StreamType, VideoCodec,
};

// ─────────────────────────────────────────────────────────────
// 客户端能力
// ─────────────────────────────────────────────────────────────

/// 一个播放端的能力声明。
///
/// 由客户端上报（或从 User-Agent / 设备画像推断），服务端据此决策。
/// **宁可低估也不要高估**：报高了会得到一个放不出来的流，
/// 报低了最多多转一次码。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientCapability {
    /// 能力指纹，如 `web-chrome` / `tauri-mpv` / `android-exoplayer`。
    pub client_id: String,
    /// 能直接解封装的容器。
    pub containers: Vec<Container>,
    pub video_codecs: Vec<VideoCodec>,
    pub audio_codecs: Vec<AudioCodec>,
    pub max_bitrate_kbps: Option<u32>,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    /// 最大色深。8 位的浏览器解不了 10bit HEVC。
    pub max_bit_depth: u8,
    /// 支持的 HDR 类型。空表示只支持 SDR。
    pub hdr: Vec<HdrType>,
    /// 最大声道数。立体声设备拿到 7.1 会没声音。
    pub audio_channels: u8,
    /// 能否自己发 HTTP Range 请求（Direct Play 的前提）。
    pub supports_range: bool,
    /// 能否播流式输出（remux / HLS / fMP4）。
    pub supports_streaming: bool,
}

impl ClientCapability {
    pub fn supports_container(&self, c: &Container) -> bool {
        match c {
            Container::Other(s) => {
                let s_lower = s.to_ascii_lowercase();
                if matches!(
                    s_lower.as_str(),
                    "m3u8" | "hls" | "application/x-mpegurl" | "application/vnd.apple.mpegurl"
                ) && self.supports_streaming
                {
                    return true;
                }
                self.containers.iter().any(|item| match item {
                    Container::Other(o) => o.eq_ignore_ascii_case(&s_lower),
                    _ => false,
                })
            }
            _ => self.containers.contains(c),
        }
    }

    pub fn supports_video(&self, c: &VideoCodec) -> bool {
        self.video_codecs.contains(c)
    }

    pub fn supports_audio(&self, c: &AudioCodec) -> bool {
        self.audio_codecs.contains(c)
    }

    pub fn supports_hdr(&self, h: HdrType) -> bool {
        matches!(h, HdrType::Sdr) || self.hdr.contains(&h)
    }

    /// 能解的目标分辨率是否被客户端接受。
    pub fn fits_resolution(&self, w: u32, h: u32) -> bool {
        // 宽度用 max_width 卡，高度用 max_height 卡；缺失即不限制。
        // 只卡一边就够了：等比例缩放时另一边自然满足。
        self.max_width.map(|m| w <= m).unwrap_or(true)
            && self.max_height.map(|m| h <= m).unwrap_or(true)
    }
}

impl Default for ClientCapability {
    fn default() -> Self {
        // 默认给"浏览器"档而不是"全能"档：
        // 未知客户端按最保守处理，最多多转码，不会放不出来。
        Self::preset_web()
    }
}

// ── 预置能力档 ──────────────────────────────────────────────

impl ClientCapability {
    /// Web 浏览器（`<video>` + hls.js）。
    ///
    /// 现实约束：H264/AAC/VP9/Opus 是通用集，HEVC 只有部分平台，
    /// AV1 要新显卡，**位图字幕一律不支持**，外挂字幕只能走 VTT。
    pub fn preset_web() -> Self {
        Self {
            client_id: "web".into(),
            containers: vec![Container::Mp4, Container::Webm],
            video_codecs: vec![VideoCodec::H264, VideoCodec::Vp9],
            audio_codecs: vec![AudioCodec::Aac, AudioCodec::Opus, AudioCodec::Mp3],
            max_bitrate_kbps: None,
            max_width: None,
            max_height: None,
            max_bit_depth: 8,
            hdr: vec![],
            audio_channels: 2,
            supports_range: true,
            supports_streaming: true,
        }
    }

    /// 桌面 mpv（Tauri 插件）。几乎什么都能放。
    pub fn preset_mpv() -> Self {
        Self {
            client_id: "tauri-mpv".into(),
            containers: vec![
                Container::Mkv,
                Container::Mp4,
                Container::Ts,
                Container::Webm,
                Container::M2ts,
                Container::Avi,
            ],
            video_codecs: vec![
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Av1,
                VideoCodec::Vp9,
                VideoCodec::Mpeg2,
                VideoCodec::Vc1,
            ],
            audio_codecs: vec![
                AudioCodec::Aac,
                AudioCodec::Ac3,
                AudioCodec::Eac3,
                AudioCodec::Dts,
                AudioCodec::DtsHd,
                AudioCodec::TrueHd,
                AudioCodec::Flac,
                AudioCodec::Opus,
                AudioCodec::Mp3,
            ],
            max_bitrate_kbps: None,
            max_width: None,
            max_height: None,
            max_bit_depth: 12,
            hdr: vec![
                HdrType::Hdr10,
                HdrType::Hdr10Plus,
                HdrType::Hlg,
                HdrType::DolbyVision,
            ],
            audio_channels: 8,
            supports_range: true,
            supports_streaming: true,
        }
    }

    /// Android / ExoPlayer（Media3）。硬件解码能力随机型差异极大，取交集。
    pub fn preset_android() -> Self {
        Self {
            client_id: "android-exoplayer".into(),
            containers: vec![
                Container::Mp4,
                Container::Mkv,
                Container::Ts,
                Container::Webm,
            ],
            video_codecs: vec![VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Vp9],
            audio_codecs: vec![
                AudioCodec::Aac,
                AudioCodec::Ac3,
                AudioCodec::Eac3,
                AudioCodec::Opus,
            ],
            max_bitrate_kbps: Some(40_000),
            max_width: Some(3840),
            max_height: Some(2160),
            max_bit_depth: 10,
            hdr: vec![HdrType::Hdr10, HdrType::Hlg],
            audio_channels: 6,
            supports_range: false,
            supports_streaming: true,
        }
    }

    /// Apple 平台（AVFoundation / Safari）。
    pub fn preset_apple() -> Self {
        Self {
            client_id: "apple".into(),
            containers: vec![Container::Mp4],
            video_codecs: vec![VideoCodec::H264, VideoCodec::Hevc],
            audio_codecs: vec![
                AudioCodec::Aac,
                AudioCodec::Ac3,
                AudioCodec::Eac3,
                AudioCodec::Flac,
            ],
            max_bitrate_kbps: None,
            max_width: None,
            max_height: None,
            max_bit_depth: 10,
            hdr: vec![HdrType::Hdr10, HdrType::DolbyVision, HdrType::Hlg],
            audio_channels: 2,
            supports_range: true,
            supports_streaming: true,
        }
    }

    /// 最保守：只认 HLS + AAC。给未知/老旧设备兜底。
    pub fn preset_minimal() -> Self {
        Self {
            client_id: "minimal".into(),
            containers: vec![Container::Ts],
            video_codecs: vec![VideoCodec::H264],
            audio_codecs: vec![AudioCodec::Aac],
            max_bitrate_kbps: Some(8_000),
            max_width: Some(1280),
            max_height: Some(720),
            max_bit_depth: 8,
            hdr: vec![],
            audio_channels: 2,
            supports_range: false,
            supports_streaming: true,
        }
    }

    pub fn by_id(id: &str) -> Self {
        match id {
            "web" | "web-chrome" | "web-firefox" | "web-safari" => Self::preset_web(),
            "tauri-mpv" | "mpv" | "desktop" => Self::preset_mpv(),
            "android" | "android-exoplayer" | "android-tv" => Self::preset_android(),
            "apple" | "ios" | "tvos" => Self::preset_apple(),
            "minimal" => Self::preset_minimal(),
            _ => Self::preset_web(),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 片源能力
// ─────────────────────────────────────────────────────────────

/// 片源的视频轨。
#[derive(Debug, Clone)]
pub struct VideoTrack {
    pub index: i32,
    pub codec: VideoCodec,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bit_depth: u8,
    pub hdr: HdrType,
}

/// 片源的音频轨。
#[derive(Debug, Clone)]
pub struct AudioTrack {
    /// **文件内的流序号**，用于 ffmpeg 的 `-map 0:{index}`。
    pub index: i32,
    pub codec: AudioCodec,
    pub channels: Option<u8>,
    pub language: Option<String>,
    pub is_default: bool,
}

/// 从 `MediaFile + MediaStream[]` 派生出的片源画像。
#[derive(Debug, Clone, Default)]
pub struct MediaCapability {
    pub container: Option<Container>,
    pub video: Option<VideoTrack>,
    pub audio: Vec<AudioTrack>,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<u32>,
}

impl MediaCapability {
    /// 从入库的流信息构造。
    ///
    /// 注意 codec 名可能是不认识的（媒体世界的 codec 永远比我们的枚举多），
    /// 这里一律走 `Other(...)`，绝不 panic —— 扫描 10 万个文件时崩在一个
    /// 没见过的 codec 上是不可接受的。
    pub fn from_streams(file: &MediaFile, streams: &[MediaStream]) -> Self {
        Self::from_parts(
            file.container.clone(),
            file.duration_ms,
            file.bitrate.map(|b| (b / 1000) as u32),
            streams,
        )
    }

    /// 从零散字段构造。
    ///
    /// `from_streams` 要求一个完整的 `MediaFile`，但 API 层常从
    /// `FileWithStreams`（不含 container / bitrate）这类视图里取数，
    /// 所以留一个不需要完整实体的入口。
    pub fn from_parts(
        container: Option<Container>,
        duration_ms: Option<i64>,
        bitrate_kbps: Option<u32>,
        streams: &[MediaStream],
    ) -> Self {
        let mut cap = MediaCapability {
            container,
            duration_ms,
            bitrate_kbps,
            ..Default::default()
        };

        for s in streams {
            match s.stream_type {
                StreamType::Video => {
                    if cap.video.is_none() {
                        cap.video = Some(VideoTrack {
                            index: s.index,
                            codec: whatmedia_database::to_video_codec(
                                s.codec.as_deref().unwrap_or(""),
                            ),
                            width: s.width.map(|w| w as u32),
                            height: s.height.map(|h| h as u32),
                            bit_depth: s.bit_depth.unwrap_or(8) as u8,
                            hdr: s.hdr_type,
                        });
                    }
                }
                StreamType::Audio => {
                    cap.audio.push(AudioTrack {
                        index: s.index,
                        codec: whatmedia_database::to_audio_codec(s.codec.as_deref().unwrap_or("")),
                        channels: s.channels.map(|c| c as u8),
                        language: s.language.clone(),
                        is_default: s.is_default,
                    });
                }
                _ => {}
            }
        }

        // 没有视频轨的"媒体文件"（纯音频）也要能构造出来，
        // 只不过决策时会因为没有画面而走 Unsupported。
        cap
    }

    /// 挑一个客户端能直接解的音轨。优先 default，其次第一条。
    pub fn pick_audio(&self, client: &ClientCapability) -> Option<&AudioTrack> {
        self.audio
            .iter()
            .filter(|a| a.channels.unwrap_or(2) <= client.audio_channels)
            .find(|a| a.is_default && client.supports_audio(&a.codec))
            .or_else(|| self.audio.iter().find(|a| client.supports_audio(&a.codec)))
    }
}

// ─────────────────────────────────────────────────────────────
// 服务端能力
// ─────────────────────────────────────────────────────────────

/// 服务端当前能拿出什么。
#[derive(Debug, Clone)]
pub struct ServerCapability {
    pub ffmpeg_available: bool,
    /// 可用后端（含软件）。按顺序尝试。
    pub encoders: Vec<whatmedia_core::HardwareProfile>,
    /// 拼硬件参数所需的机器细节（render 节点 / 核数 / 内存）。
    ///
    /// `HardwareProfile` 只说"有 VAAPI"，拼 `-vaapi_device` 还得知道是
    /// renderD128 还是 renderD129。这些是探测时顺手就能拿到的，别丢。
    pub accel_ctx: whatmedia_transcoder::AccelContext,
    /// 正在跑的转码数。
    pub active_transcodes: usize,
}

impl ServerCapability {
    pub fn without_ffmpeg() -> Self {
        Self {
            ffmpeg_available: false,
            encoders: Vec::new(),
            accel_ctx: whatmedia_transcoder::AccelContext::default(),
            active_transcodes: 0,
        }
    }

    /// 有没有后端能编出目标格式。
    pub fn can_encode(&self, codec: &VideoCodec) -> bool {
        self.ffmpeg_available && self.encoders.iter().any(|p| p.encoders.contains(codec))
    }

    /// 还能不能再开一路（烧录字幕要看这个）。
    pub fn has_budget(&self) -> bool {
        let max = self
            .encoders
            .iter()
            .map(|p| p.max_concurrent_sessions as usize)
            .max()
            .unwrap_or(0);
        self.ffmpeg_available && self.active_transcodes < max.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_client_rejects_10bit() {
        let c = ClientCapability::preset_web();
        assert!(c.supports_video(&VideoCodec::H264));
        assert!(!c.supports_video(&VideoCodec::Hevc));
        assert_eq!(c.max_bit_depth, 8);
        assert!(!c.supports_hdr(HdrType::Hdr10));
        // SDR 永远支持。
        assert!(c.supports_hdr(HdrType::Sdr));
    }

    #[test]
    fn mpv_plays_almost_everything() {
        let c = ClientCapability::preset_mpv();
        assert!(c.supports_video(&VideoCodec::Av1));
        assert!(c.supports_audio(&AudioCodec::TrueHd));
        assert!(c.supports_container(&Container::Mkv));
        assert!(c.supports_hdr(HdrType::DolbyVision));
    }

    #[test]
    fn unknown_client_id_falls_back_to_web() {
        // 未知设备按最保守处理：宁可多转码。
        assert_eq!(ClientCapability::by_id("toaster").client_id, "web");
        assert_eq!(ClientCapability::by_id("tauri-mpv").client_id, "tauri-mpv");
    }

    #[test]
    fn resolution_fit_checks_both_axes() {
        let c = ClientCapability::preset_minimal();
        assert!(c.fits_resolution(1280, 720));
        assert!(!c.fits_resolution(1920, 1080));
        // 未限制时一律通过。
        let web = ClientCapability::preset_web();
        assert!(web.fits_resolution(3840, 2160));
    }

    #[test]
    fn audio_pick_prefers_compatible_default_track() {
        let cap = MediaCapability {
            audio: vec![
                AudioTrack {
                    index: 1,
                    codec: AudioCodec::TrueHd,
                    channels: Some(8),
                    language: Some("eng".into()),
                    is_default: true,
                },
                AudioTrack {
                    index: 2,
                    codec: AudioCodec::Aac,
                    channels: Some(2),
                    language: Some("eng".into()),
                    is_default: false,
                },
            ],
            ..Default::default()
        };
        let web = ClientCapability::preset_web();
        // default 是 TrueHD，浏览器解不了 → 退到 AAC 那条。
        assert_eq!(cap.pick_audio(&web).unwrap().index, 2);
        // mpv 两条都能解 → 选 default。
        assert_eq!(
            cap.pick_audio(&ClientCapability::preset_mpv())
                .unwrap()
                .index,
            1
        );
    }

    #[test]
    fn server_budget_is_respected() {
        let mut s = ServerCapability::without_ffmpeg();
        assert!(!s.has_budget());
        s.ffmpeg_available = true;
        assert!(s.has_budget());
        s.encoders = vec![whatmedia_core::HardwareProfile {
            kind: whatmedia_core::HardwareKind::Software,
            decoders: vec![],
            encoders: vec![VideoCodec::H264],
            supports_zero_copy: false,
            supports_tone_mapping: false,
            max_concurrent_sessions: 1,
        }];
        assert!(s.can_encode(&VideoCodec::H264));
        assert!(!s.can_encode(&VideoCodec::Av1));
        s.active_transcodes = 1;
        assert!(!s.has_budget());
    }

    #[test]
    fn web_and_streaming_clients_support_m3u8_container() {
        let web = ClientCapability::preset_web();
        assert!(web.supports_container(&Container::Other("m3u8".into())));
        assert!(web.supports_container(&Container::Other("M3U8".into())));
        assert!(web.supports_container(&Container::Other("hls".into())));
        assert!(web.supports_container(&Container::Other("application/x-mpegURL".into())));
    }
}

