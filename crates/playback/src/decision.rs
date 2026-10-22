//! 播放决策：Client + Media + Server + Network → Direct / Remux / Transcode / Unsupported。
//!
//! 降级链（**只能往前不能往后**）：
//!
//! ```text
//! DirectPlay   原文件直出，零 CPU
//!   ↓ 容器不认 / 音轨不认 / 客户端不能发 Range
//! Remux        换容器，视频不动（音频可以顺带转，很便宜）
//!   ↓ 视频编码本身不认（编码 / 色深 / 分辨率 / HDR / 码率 / 要烧字幕）
//! Transcode    重编码
//!   ↓ 服务端没有可用编码器
//! Unsupported
//! ```
//!
//! 一条铁律：**决策必须能解释自己。** 每个降级都留下 `Reason`，
//! 用户问"为什么我的 4K 原盘在转码"时要有据可查，而不是一个黑盒结果。

use serde::Serialize;
use whatmedia_core::{AudioCodec, PlaybackStrategy, VideoCodec};
use whatmedia_transcoder::{AudioAction, OutputSpec, StreamRequest, VideoAction};

use crate::capability::{ClientCapability, MediaCapability, ServerCapability};

/// 降级原因。API 直接把它序列化给前端做"为什么在转码"的提示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum Reason {
    NoVideoTrack,
    ContainerUnsupported {
        container: String,
    },
    VideoCodecUnsupported {
        codec: String,
    },
    BitDepthTooHigh {
        have: u8,
        max: u8,
    },
    ResolutionTooHigh {
        width: u32,
        height: u32,
    },
    HdrUnsupported {
        hdr: String,
    },
    BitrateTooHigh {
        kbps: u32,
        limit: u32,
    },
    /// 音轨客户端解不了，需要单独转音频（视频仍可 copy）。
    AudioNeedsTranscode,
    /// 客户端既不能 Range 直连也不接受流式。
    ClientCannotReceive,
    FfmpegMissing,
    /// 服务端编不出客户端认的任何格式。
    NoEncoderForClient,
}

impl Reason {
    pub fn code(&self) -> &'static str {
        match self {
            Reason::NoVideoTrack => "no_video_track",
            Reason::ContainerUnsupported { .. } => "container_unsupported",
            Reason::VideoCodecUnsupported { .. } => "video_codec_unsupported",
            Reason::BitDepthTooHigh { .. } => "bit_depth_too_high",
            Reason::ResolutionTooHigh { .. } => "resolution_too_high",
            Reason::HdrUnsupported { .. } => "hdr_unsupported",
            Reason::BitrateTooHigh { .. } => "bitrate_too_high",
            Reason::AudioNeedsTranscode => "audio_needs_transcode",
            Reason::ClientCannotReceive => "client_cannot_receive",
            Reason::FfmpegMissing => "ffmpeg_missing",
            Reason::NoEncoderForClient => "no_encoder_for_client",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Reason::NoVideoTrack => "文件里没有视频轨".into(),
            Reason::ContainerUnsupported { container } => format!("客户端不支持 {container} 容器"),
            Reason::VideoCodecUnsupported { codec } => format!("客户端不支持 {codec} 编码"),
            Reason::BitDepthTooHigh { have, max } => {
                format!("片源是 {have}bit，客户端最高只支持 {max}bit")
            }
            Reason::ResolutionTooHigh { width, height } => {
                format!("分辨率 {width}x{height} 超出客户端能力")
            }
            Reason::HdrUnsupported { hdr } => format!("客户端不支持 {hdr}"),
            Reason::BitrateTooHigh { kbps, limit } => {
                format!("码率 {kbps}kbps 超过上限 {limit}kbps")
            }
            Reason::AudioNeedsTranscode => "音轨格式不被支持，需要转换音频".into(),
            Reason::ClientCannotReceive => "客户端既不支持直连也不支持流式输出".into(),
            Reason::FfmpegMissing => "服务端没有可用的 ffmpeg".into(),
            Reason::NoEncoderForClient => "服务端编不出客户端支持的任何格式".into(),
        }
    }
}

/// 决策输出。
#[derive(Debug, Clone)]
pub struct PlaybackDecision {
    pub strategy: PlaybackStrategy,
    /// 为什么没有走更省的路。**为空表示 DirectPlay**。
    pub reasons: Vec<Reason>,
    /// 需要启动 ffmpeg 时的完整请求。DirectPlay 时为 None。
    pub stream: Option<StreamRequest>,
    /// 选中的音轨序号。
    pub audio_index: Option<i32>,
    /// 转码目标高度。缩放用；None 表示保持原始分辨率。
    pub target_height: Option<u32>,
    /// 硬件后端候选链，索引 0 为首选，**链尾保证是软件档**。
    ///
    /// 转码启动后若在首帧窗口内失败，按这个顺序静默换下一个 —— 用户无感知。
    /// 非转码策略下为空。
    pub accel_chain: Vec<whatmedia_transcoder::AccelArgs>,
}

impl PlaybackDecision {
    /// 一句话解释。给日志与 UI 提示用。
    pub fn summary(&self) -> String {
        if self.reasons.is_empty() {
            return "直接播放".to_string();
        }
        self.reasons
            .iter()
            .map(|r| r.message())
            .collect::<Vec<_>>()
            .join("；")
    }
}

/// 决策输入。
pub struct DecisionInput<'a> {
    pub media: &'a MediaCapability,
    pub client: &'a ClientCapability,
    pub server: &'a ServerCapability,
    /// 实测/上报的可用带宽。弱网时主动降码率。
    pub network_kbps: Option<u32>,
    pub start_ms: i64,
    /// HLS 分片目录。只有 HLS 输出用得上。
    pub hls_dir: Option<String>,
    pub seg_seconds: u32,
}

/// 核心决策函数。纯函数，无 IO —— 全部输入都是已经探测好的事实。
pub fn decide(input: DecisionInput<'_>) -> PlaybackDecision {
    let media = input.media;
    let client = input.client;
    let server = input.server;

    let Some(video) = &media.video else {
        return PlaybackDecision {
            strategy: PlaybackStrategy::Unsupported,
            reasons: vec![Reason::NoVideoTrack],
            stream: None,
            audio_index: None,
            target_height: None,
            accel_chain: Vec::new(),
        };
    };

    // ── 逐项检查视频能不能原样播 ─────────────────────────
    let mut reasons: Vec<Reason> = Vec::new();

    let container_ok = media
        .container
        .as_ref()
        .map(|c| client.supports_container(c))
        .unwrap_or(false);
    let video_ok = client.supports_video(&video.codec);
    let width = video.width.unwrap_or(0);
    let height = video.height.unwrap_or(0);
    let depth_ok = video.bit_depth <= client.max_bit_depth;
    let res_ok = client.fits_resolution(width, height);
    let hdr_ok = client.supports_hdr(video.hdr);

    // 码率上限取"客户端能力"与"网络带宽"里更紧的那个。
    let bitrate = media.bitrate_kbps.unwrap_or(0);
    let limit = match (client.max_bitrate_kbps, input.network_kbps) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let bitrate_ok = limit.map(|l| bitrate <= l).unwrap_or(true);

    if !video_ok {
        reasons.push(Reason::VideoCodecUnsupported {
            codec: codec_label(&video.codec),
        });
    }
    if !depth_ok {
        reasons.push(Reason::BitDepthTooHigh {
            have: video.bit_depth,
            max: client.max_bit_depth,
        });
    }
    if !res_ok {
        reasons.push(Reason::ResolutionTooHigh { width, height });
    }
    if !hdr_ok {
        reasons.push(Reason::HdrUnsupported {
            hdr: format!("{:?}", video.hdr),
        });
    }
    if !bitrate_ok {
        if let Some(l) = limit {
            reasons.push(Reason::BitrateTooHigh {
                kbps: bitrate,
                limit: l,
            });
        }
    }
    // 音轨：能直解就直解，否则单独转音频。
    let audio_direct = media.pick_audio(client);
    let audio_action = match audio_direct {
        Some(a) => AudioAction::Copy {
            index: Some(a.index),
        },
        None if !media.audio.is_empty() => {
            reasons.push(Reason::AudioNeedsTranscode);
            AudioAction::Encode {
                index: media.audio.first().map(|a| a.index),
                codec: pick_audio_codec(client),
                channels: client.audio_channels.clamp(1, 2),
                bitrate_kbps: 192,
            }
        }
        None => AudioAction::None,
    };
    let audio_index = match &audio_action {
        AudioAction::Copy { index } | AudioAction::Encode { index, .. } => *index,
        AudioAction::None => None,
    };

    // ── ③ Direct Play ───────────────────────────────────────
    let video_passthrough = video_ok && depth_ok && res_ok && hdr_ok && bitrate_ok;
    if video_passthrough
        && container_ok
        && client.supports_range
        && matches!(audio_action, AudioAction::Copy { .. })
    {
        return PlaybackDecision {
            strategy: PlaybackStrategy::DirectPlay,
            reasons: Vec::new(),
            stream: None,
            audio_index,
            target_height: None,
            accel_chain: Vec::new(),
        };
    }

    if !client.supports_range && !client.supports_streaming {
        return PlaybackDecision {
            strategy: PlaybackStrategy::Unsupported,
            reasons: vec![Reason::ClientCannotReceive],
            stream: None,
            audio_index,
            target_height: None,
            accel_chain: Vec::new(),
        };
    }

    // ── ④ Remux：视频原样，只换容器 / 转音频 ────────────────
    if video_passthrough && client.supports_streaming {
        let mut reasons = reasons.clone();
        if !container_ok {
            reasons.insert(
                0,
                Reason::ContainerUnsupported {
                    container: media
                        .container
                        .as_ref()
                        .map(|c| format!("{c:?}"))
                        .unwrap_or_else(|| "未知".into()),
                },
            );
        }
        if !client.supports_range && container_ok {
            // 能直解但不能发 Range（比如某些内嵌 WebView）→ 走流式。
            reasons.insert(0, Reason::ClientCannotReceive);
        }
        let output = output_for(client, input.hls_dir.clone(), input.seg_seconds);
        return PlaybackDecision {
            strategy: PlaybackStrategy::Remux,
            reasons,
            stream: Some(StreamRequest {
                input: String::new(), // 由调用方填入真实路径
                seek_ms: input.start_ms,
                video: VideoAction::Copy,
                audio: audio_action,
                output,
                accel: None,
                // 决策层只产出点播请求。直播不经过 decide()，见 crates/api/src/live.rs。
                live: false,
            }),
            audio_index,
            target_height: None,
            accel_chain: Vec::new(),
        };
    }

    // ── ⑤ Transcode ────────────────────────────────────────
    if !server.ffmpeg_available {
        return PlaybackDecision {
            strategy: PlaybackStrategy::Unsupported,
            reasons: vec![Reason::FfmpegMissing],
            stream: None,
            audio_index,
            target_height: None,
            accel_chain: Vec::new(),
        };
    }

    let Some(target_codec) = pick_video_codec(client, server) else {
        return PlaybackDecision {
            strategy: PlaybackStrategy::Unsupported,
            reasons: vec![Reason::NoEncoderForClient],
            stream: None,
            audio_index,
            target_height: None,
            accel_chain: Vec::new(),
        };
    };

    let target_height = if res_ok {
        None
    } else {
        // 缩到客户端能接受的高度；客户端没给上限时退到 720p。
        Some(client.max_height.unwrap_or(720))
    };
    let target_width = target_height.map(|h| scale_width(width, height, h));

    let tonemap_to = if hdr_ok || matches!(video.hdr, whatmedia_core::HdrType::Sdr) {
        None
    } else {
        Some(whatmedia_core::HdrType::Sdr)
    };

    let output = output_for(client, input.hls_dir.clone(), input.seg_seconds);

    // 硬件后端候选链：硬件优先，软件兜底。转码跑不起来时按序静默降级。
    // tone mapping 的**源**是片源的 HDR 类型；目标是 SDR（决策里只做 HDR→SDR）。
    let accel_chain = whatmedia_transcoder::accel_chain(
        &server.encoders,
        &server.accel_ctx,
        &target_codec,
        target_width,
        target_height,
        if tonemap_to.is_some() {
            Some(video.hdr)
        } else {
            None
        },
    );

    PlaybackDecision {
        strategy: PlaybackStrategy::Transcode,
        reasons,
        stream: Some(StreamRequest {
            input: String::new(),
            seek_ms: input.start_ms,
            video: VideoAction::Encode {
                codec: target_codec,
                max_width: target_width,
                max_height: target_height,
                bitrate_kbps: limit,
                tonemap_to,
            },
            audio: audio_action,
            output,
            accel: accel_chain.first().cloned(),
            live: false,
        }),
        audio_index,
        target_height,
        accel_chain,
    }
}

/// 客户端只要能 Range 就用管道直出；不能 Range 的（多数移动端 WebView）走 HLS。
///
/// HLS 需要 `hls_dir`；没给就退化成 TS 管道，调用方自行决定是否拒绝。
fn output_for(client: &ClientCapability, hls_dir: Option<String>, seg: u32) -> OutputSpec {
    match hls_dir {
        Some(dir) if !client.supports_range => OutputSpec::Hls {
            dir,
            seg_seconds: seg.max(2),
        },
        _ => OutputSpec::PipeTs,
    }
}

/// 挑一个"客户端认 且 服务端编得出"的视频格式。
fn pick_video_codec(client: &ClientCapability, server: &ServerCapability) -> Option<VideoCodec> {
    // 目标格式必须**同时**被客户端和服务端接受。
    // H264 是事实上的最大公约数，客户端认它时就优先它。
    let mut order: Vec<VideoCodec> = Vec::new();
    if client.video_codecs.contains(&VideoCodec::H264) {
        order.push(VideoCodec::H264);
    }
    order.extend(
        client
            .video_codecs
            .iter()
            .filter(|c| **c != VideoCodec::H264)
            .cloned(),
    );
    order.into_iter().find(|c| server.can_encode(c))
}

/// 挑音频编码目标。同样以 AAC 为最大公约数。
fn pick_audio_codec(client: &ClientCapability) -> AudioCodec {
    if client.supports_audio(&AudioCodec::Aac) {
        return AudioCodec::Aac;
    }
    client
        .audio_codecs
        .first()
        .cloned()
        .unwrap_or(AudioCodec::Aac)
}

fn scale_width(width: u32, height: u32, target_h: u32) -> u32 {
    if height == 0 || width == 0 {
        return 0;
    }
    // 偶数对齐：奇数宽高会让 yuv420p 编码失败。
    let w = (width as u64 * target_h as u64 / height as u64) as u32;
    w.max(2) & !1
}

fn codec_label(c: &VideoCodec) -> String {
    match c {
        VideoCodec::Other(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{AudioTrack, MediaCapability, VideoTrack};
    use whatmedia_core::{Container, HdrType};

    fn media(
        video: VideoTrack,
        audio: Vec<AudioTrack>,
        container: Option<Container>,
    ) -> MediaCapability {
        MediaCapability {
            container,
            video: Some(video),
            audio,
            duration_ms: Some(7_200_000),
            bitrate_kbps: Some(20_000),
        }
    }

    fn h264_1080p() -> VideoTrack {
        VideoTrack {
            index: 0,
            codec: VideoCodec::H264,
            width: Some(1920),
            height: Some(1080),
            bit_depth: 8,
            hdr: HdrType::Sdr,
        }
    }

    fn aac() -> AudioTrack {
        AudioTrack {
            index: 1,
            codec: AudioCodec::Aac,
            channels: Some(2),
            language: Some("eng".into()),
            is_default: true,
        }
    }

    fn server() -> ServerCapability {
        ServerCapability {
            ffmpeg_available: true,
            encoders: vec![whatmedia_core::HardwareProfile {
                kind: whatmedia_core::HardwareKind::Software,
                decoders: vec![],
                encoders: vec![VideoCodec::H264],
                supports_zero_copy: false,
                supports_tone_mapping: true,
                max_concurrent_sessions: 2,
            }],
            active_transcodes: 0,
            accel_ctx: whatmedia_transcoder::AccelContext::default(),
        }
    }

    fn input<'a>(
        m: &'a MediaCapability,
        c: &'a ClientCapability,
        s: &'a ServerCapability,
    ) -> DecisionInput<'a> {
        DecisionInput {
            media: m,
            client: c,
            server: s,
            network_kbps: None,
            start_ms: 0,
            hls_dir: None,
            seg_seconds: 6,
        }
    }

    #[test]
    fn mp4_h264_aac_direct_plays_on_web() {
        let m = media(h264_1080p(), vec![aac()], Some(Container::Mp4));
        let d = decide(input(&m, &ClientCapability::preset_web(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::DirectPlay);
        assert!(d.reasons.is_empty());
        assert!(d.stream.is_none());
        assert_eq!(d.summary(), "直接播放");
    }

    #[test]
    fn mkv_h264_remuxes_for_web() {
        // 浏览器不认 MKV，但认 H264 → 只需换容器，不重编码。
        let m = media(h264_1080p(), vec![aac()], Some(Container::Mkv));
        let d = decide(input(&m, &ClientCapability::preset_web(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Remux);
        assert!(d
            .reasons
            .iter()
            .any(|r| r.code() == "container_unsupported"));
        assert!(matches!(
            d.stream.as_ref().unwrap().video,
            VideoAction::Copy
        ));
    }

    #[test]
    fn hevc_transcodes_for_web() {
        let mut v = h264_1080p();
        v.codec = VideoCodec::Hevc;
        let m = media(v, vec![aac()], Some(Container::Mkv));
        let d = decide(input(&m, &ClientCapability::preset_web(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Transcode);
        assert!(d
            .reasons
            .iter()
            .any(|r| r.code() == "video_codec_unsupported"));
        assert!(matches!(
            d.stream.as_ref().unwrap().video,
            VideoAction::Encode { .. }
        ));
    }

    #[test]
    fn hevc_direct_plays_on_mpv() {
        let mut v = h264_1080p();
        v.codec = VideoCodec::Hevc;
        v.bit_depth = 10;
        v.hdr = HdrType::Hdr10;
        let m = media(v, vec![aac()], Some(Container::Mkv));
        let d = decide(input(&m, &ClientCapability::preset_mpv(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::DirectPlay);
    }

    #[test]
    fn ten_bit_forces_transcode_on_web() {
        let mut v = h264_1080p();
        v.bit_depth = 10;
        let m = media(v, vec![aac()], Some(Container::Mp4));
        let d = decide(input(&m, &ClientCapability::preset_web(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Transcode);
        assert!(d.reasons.iter().any(|r| r.code() == "bit_depth_too_high"));
    }

    #[test]
    fn four_k_downscales_for_minimal_client() {
        let v = VideoTrack {
            width: Some(3840),
            height: Some(2160),
            ..h264_1080p()
        };
        let m = media(v, vec![aac()], Some(Container::Mp4));
        let d = decide(input(&m, &ClientCapability::preset_minimal(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Transcode);
        assert_eq!(d.target_height, Some(720));
        // 宽度必须偶数，否则 yuv420p 编码会失败。
        let w = match &d.stream.as_ref().unwrap().video {
            VideoAction::Encode { max_width, .. } => *max_width,
            _ => panic!("应当是转码"),
        };
        assert_eq!(w, Some(1280));
        assert_eq!(w.unwrap() % 2, 0);
    }

    #[test]
    fn dts_audio_only_transcodes_audio() {
        // MKV + H264 + DTS：视频完全没问题，只转音频。
        let a = AudioTrack {
            index: 1,
            codec: AudioCodec::Dts,
            channels: Some(6),
            language: Some("eng".into()),
            is_default: true,
        };
        let m = media(h264_1080p(), vec![a], Some(Container::Mkv));
        let d = decide(input(&m, &ClientCapability::preset_web(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Remux);
        assert!(d
            .reasons
            .iter()
            .any(|r| r.code() == "audio_needs_transcode"));
        assert!(
            matches!(d.stream.as_ref().unwrap().video, VideoAction::Copy),
            "视频必须保持 copy —— 只转音频就够了"
        );
        assert!(matches!(
            d.stream.as_ref().unwrap().audio,
            AudioAction::Encode { .. }
        ));
    }

    #[test]
    fn weak_network_lowers_bitrate_and_transcodes() {
        let m = media(h264_1080p(), vec![aac()], Some(Container::Mp4));
        let binding = server();
        let web = ClientCapability::preset_web();
        let mut i = input(&m, &web, &binding);
        i.network_kbps = Some(2_000);
        let d = decide(i);
        assert_eq!(d.strategy, PlaybackStrategy::Transcode);
        assert!(d.reasons.iter().any(|r| r.code() == "bitrate_too_high"));
    }

    #[test]
    fn no_ffmpeg_means_unsupported_not_crash() {
        let mut v = h264_1080p();
        v.codec = VideoCodec::Hevc;
        let m = media(v, vec![aac()], Some(Container::Mkv));
        let s = ServerCapability::without_ffmpeg();
        let d = decide(input(&m, &ClientCapability::preset_web(), &s));
        assert_eq!(d.strategy, PlaybackStrategy::Unsupported);
        assert!(d.reasons.iter().any(|r| r.code() == "ffmpeg_missing"));
    }

    #[test]
    fn client_without_compatible_encoder_is_unsupported() {
        // 客户端只认 AV1，服务端只能编 H264 → 无路可走。
        let mut c = ClientCapability::preset_web();
        c.video_codecs = vec![VideoCodec::Av1];
        let m = media(h264_1080p(), vec![aac()], Some(Container::Mp4));
        let d = decide(input(&m, &c, &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Unsupported);
        assert!(d
            .reasons
            .iter()
            .any(|r| r.code() == "no_encoder_for_client"));
    }

    #[test]
    fn android_without_range_gets_hls() {
        let m = media(h264_1080p(), vec![aac()], Some(Container::Mkv));
        let binding = server();
        let android = ClientCapability::preset_android();
        let mut i = input(&m, &android, &binding);
        i.hls_dir = Some("/cache/hls/j1".into());
        let d = decide(i);
        // Android 预设不能发 Range → 即使能直解也要走 HLS 流式。
        assert_eq!(d.strategy, PlaybackStrategy::Remux);
        assert!(matches!(
            d.stream.as_ref().unwrap().output,
            OutputSpec::Hls { .. }
        ));
    }

    #[test]
    fn no_video_track_is_unsupported() {
        let m = MediaCapability::default();
        let d = decide(input(&m, &ClientCapability::preset_mpv(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Unsupported);
        assert_eq!(d.reasons[0].code(), "no_video_track");
    }

    #[test]
    fn hdr_downgrade_sets_tonemap_target() {
        let mut v = h264_1080p();
        v.hdr = HdrType::Hdr10;
        let m = media(v, vec![aac()], Some(Container::Mp4));
        let d = decide(input(&m, &ClientCapability::preset_web(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::Transcode);
        match &d.stream.as_ref().unwrap().video {
            VideoAction::Encode { tonemap_to, .. } => assert_eq!(*tonemap_to, Some(HdrType::Sdr)),
            _ => panic!("应当是转码"),
        }
    }

    #[test]
    fn transcode_carries_an_accel_chain_ending_in_software() {
        // P5 DoD：链尾必须是软件档，否则硬件一挂就没有下一级可退。
        let mut s = server();
        s.encoders = vec![whatmedia_core::HardwareProfile {
            kind: whatmedia_core::HardwareKind::IntelQsv,
            decoders: vec![VideoCodec::H264],
            encoders: vec![VideoCodec::H264],
            supports_zero_copy: true,
            supports_tone_mapping: true,
            max_concurrent_sessions: 4,
        }];
        s.accel_ctx = whatmedia_transcoder::AccelContext {
            render_node: Some("/dev/dri/renderD128".into()),
            cpu_cores: 8,
            memory_bytes: 8 << 30,
        };

        let m = media(h264_1080p(), vec![aac()], Some(Container::Mkv));
        let d = decide(input(&m, &ClientCapability::preset_minimal(), &s));
        assert_eq!(d.strategy, PlaybackStrategy::Transcode);
        assert!(!d.accel_chain.is_empty());
        assert_eq!(d.accel_chain.first().unwrap().id, "qsv");
        assert_eq!(d.accel_chain.last().unwrap().id, "software");
        // 首选后端必须已经写进 StreamRequest —— 否则降级机制拿到的是空链。
        assert_eq!(d.stream.as_ref().unwrap().accel.as_ref().unwrap().id, "qsv");
    }

    #[test]
    fn direct_play_needs_no_accel_chain() {
        let m = media(h264_1080p(), vec![aac()], Some(Container::Mp4));
        let d = decide(input(&m, &ClientCapability::preset_web(), &server()));
        assert_eq!(d.strategy, PlaybackStrategy::DirectPlay);
        assert!(d.accel_chain.is_empty(), "直出不起 ffmpeg，不该有后端链");
    }
}
