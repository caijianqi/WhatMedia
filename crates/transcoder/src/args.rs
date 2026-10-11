//! FFmpeg 参数构造。
//!
//! **参数顺序不是风格问题，是正确性问题。** 三条硬规则：
//!
//! 1. `-ss` 放在 `-i` **之前**：关键帧快速定位，首帧时间从十几秒降到 1 秒内
//!    （DoD 要求转码首帧 < 3s）。代价是定位精度到关键帧，播放场景下可以接受。
//! 2. 硬件解码参数（`-hwaccel` / `-vaapi_device`）必须在 `-i` 之前，
//!    编码器参数（`-c:v h264_vaapi`）必须在 `-i` 之后。
//! 3. `-map` 显式指定轨道。不写 `-map` 时 ffmpeg 只挑"它认为最好的"一路，
//!    多音轨 / 多字幕的片源会丢轨。
//!
//! 每一条都有对应的测试守护，改参数顺序前先看测试为什么那么写。

use whatmedia_core::{AudioCodec, HdrType, SubtitleCodec, VideoCodec};

/// 硬件加速参数片段。由 `whatmedia-hardware` 的 provider 产出，这里只负责拼。
#[derive(Debug, Clone, Default)]
pub struct AccelArgs {
    pub id: String,
    /// `-i` 之前的参数（解码侧）。
    pub decoder: Vec<String>,
    /// `-i` 之后的编码器参数。
    pub encoder: Vec<String>,
    /// 缩放滤镜片段（不含 `-vf`）。
    pub scale: Option<String>,
    /// tone mapping 滤镜片段（不含 `-vf`）。
    pub tonemap: Option<String>,
}

impl AccelArgs {
    pub fn is_hw(&self) -> bool {
        !matches!(self.id.as_str(), "" | "software")
    }
}

/// 视频动作。
#[derive(Debug, Clone)]
pub enum VideoAction {
    /// 不重编码。
    Copy,
    Encode {
        codec: VideoCodec,
        max_width: Option<u32>,
        max_height: Option<u32>,
        bitrate_kbps: Option<u32>,
        /// 需要 HDR→SDR（或反向）时给目标类型。
        tonemap_to: Option<HdrType>,
    },
}

/// 音频动作。
#[derive(Debug, Clone)]
pub enum AudioAction {
    /// 不重编码。`index` 为 None 时取默认音轨。
    Copy { index: Option<i32> },
    Encode {
        index: Option<i32>,
        codec: AudioCodec,
        channels: u8,
        bitrate_kbps: u32,
    },
    /// 不要声音。极少数场景（静音预览）用。
    None,
}

/// 烧录字幕的来源。
#[derive(Debug, Clone)]
pub enum BurnSource {
    /// 外挂字幕文件路径。
    External(String),
    /// 内嵌轨在**文件中的序号**（ffprobe 的序号，不是过滤后的序号）。
    Internal(i32),
}

/// 输出形态。
#[derive(Debug, Clone)]
pub enum OutputSpec {
    /// 管道直出 MPEG-TS。适合"换容器不换编码"的流式播放。
    PipeTs,
    /// 管道直出 fMP4。浏览器友好，但需要整个 moov 前置。
    PipeFmp4,
    /// HLS：写分片到磁盘，返回 m3u8。
    Hls { dir: String, seg_seconds: u32 },
}

/// 一次流式输出 / 转码的完整请求。
#[derive(Debug, Clone)]
pub struct StreamRequest {
    pub input: String,
    /// 起播位置（毫秒）。0 表示从头。
    pub seek_ms: i64,
    pub video: VideoAction,
    pub audio: AudioAction,
    pub burn: Option<BurnSource>,
    pub output: OutputSpec,
    pub accel: Option<AccelArgs>,
}

impl StreamRequest {
    /// 是否需要重编码视频。Remux 与 Transcode 的分界线就是它。
    pub fn reencodes_video(&self) -> bool {
        matches!(self.video, VideoAction::Encode { .. }) || self.burn.is_some()
    }
}

/// 构造完整命令行。
pub fn build_args(req: &StreamRequest) -> Vec<String> {
    let mut a = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        // 不探测输入时长，省掉一次完整的 demux（大文件上能省几百毫秒）。
        "-noaccurate_seek".to_string(),
    ];

    // ── ① 全局 / 解码侧（必须在 -i 之前）──────────────────────

    if let Some(acc) = &req.accel {
        a.extend(acc.decoder.iter().cloned());
    }

    if req.seek_ms > 0 {
        a.push("-ss".into());
        a.push(format!("{:.3}", req.seek_ms as f64 / 1000.0));
    }

    a.push("-i".into());
    a.push(req.input.clone());

    // ── ② 轨道选择 ──────────────────────────────────────────
    a.push("-map".into());
    a.push("0:v:0".into());
    match &req.audio {
        AudioAction::Copy { index } | AudioAction::Encode { index, .. } => {
            a.push("-map".into());
            a.push(match index {
                Some(i) => format!("0:{i}"),
                None => "0:a:0".into(),
            });
        }
        AudioAction::None => {
            a.push("-an".into());
        }
    }

    // ── ③ 视频 ──────────────────────────────────────────────
    let mut filters: Vec<String> = Vec::new();

    match &req.video {
        VideoAction::Copy if req.burn.is_none() => {
            a.push("-c:v".into());
            a.push("copy".into());
        }
        VideoAction::Copy => {
            // 有烧录就必须重编码 —— 这条覆盖了 `-c copy`。
            a.extend(encode_video_args(req, &mut filters));
        }
        VideoAction::Encode { .. } => {
            a.extend(encode_video_args(req, &mut filters));
        }
    }

    // ── ④ 滤镜链：scale → tonemap → 烧录 ────────────────────
    if let Some(acc) = &req.accel {
        if let VideoAction::Encode {
            max_width,
            max_height,
            ..
        } = &req.video
        {
            if let (Some(w), Some(h)) = (max_width, max_height) {
                if let Some(f) = &acc.scale {
                    let _ = (w, h);
                    filters.push(f.clone());
                } else {
                    filters.push(format!("scale={w}:{h}"));
                }
            }
        }
        if let VideoAction::Encode { tonemap_to, .. } = &req.video {
            if tonemap_to.is_some() {
                if let Some(f) = &acc.tonemap {
                    filters.push(f.clone());
                }
            }
        }
    }

    if let Some(burn) = &req.burn {
        filters.push(match burn {
            BurnSource::External(p) => {
                let esc = p
                    .replace('\\', "/")
                    .replace(':', "\\:")
                    .replace('\'', "\\'");
                format!("subtitles='{esc}'")
            }
            BurnSource::Internal(i) => format!("subtitles=si={i}"),
        });
    }

    if !filters.is_empty() {
        a.push("-vf".into());
        a.push(filters.join(","));
    }

    // ── ⑤ 音频 ──────────────────────────────────────────────
    match &req.audio {
        AudioAction::Copy { .. } => {
            a.push("-c:a".into());
            a.push("copy".into());
        }
        AudioAction::Encode {
            codec,
            channels,
            bitrate_kbps,
            ..
        } => {
            a.push("-c:a".into());
            a.push(audio_encoder_name(codec).to_string());
            a.push("-ac".into());
            a.push(channels.to_string());
            a.push("-b:a".into());
            a.push(format!("{bitrate_kbps}k"));
        }
        AudioAction::None => {}
    }

    // ── ⑥ 输出 ──────────────────────────────────────────────
    // 音视频不同步时宁可丢帧也不要让容器队列无限增长。
    a.push("-max_muxing_queue_size".into());
    a.push("4096".into());

    match &req.output {
        OutputSpec::PipeTs => {
            a.push("-f".into());
            a.push("mpegts".into());
            a.push("pipe:1".into());
        }
        OutputSpec::PipeFmp4 => {
            a.push("-f".into());
            a.push("mp4".into());
            a.push("-movflags".into());
            a.push("+frag_keyframe+empty_moov+default_base_moof".into());
            a.push("pipe:1".into());
        }
        OutputSpec::Hls { dir, seg_seconds } => {
            a.push("-f".into());
            a.push("hls".into());
            a.push("-hls_time".into());
            a.push(seg_seconds.to_string());
            // VOD：播放列表一次性列出所有分片，不做滑动窗口。
            a.push("-hls_playlist_type".into());
            a.push("vod".into());
            a.push("-hls_flags".into());
            a.push("independent_segments".into());
            a.push("-hls_segment_filename".into());
            a.push(format!("{dir}/seg_%05d.ts"));
            a.push(format!("{dir}/index.m3u8"));
        }
    }

    a
}

fn encode_video_args(req: &StreamRequest, filters: &mut Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let VideoAction::Encode {
        codec,
        max_width,
        max_height,
        bitrate_kbps,
        tonemap_to,
    } = &req.video
    else {
        // 被烧录逼着重编码，但没给编码参数 → 用最保守的 H264 软件档。
        out.push("-c:v".into());
        out.push("libx264".into());
        out.push("-preset".into());
        out.push("veryfast".into());
        out.push("-crf".into());
        out.push("23".into());
        return out;
    };

    if let Some(acc) = &req.accel {
        if acc.is_hw() && !acc.encoder.is_empty() {
            out.extend(acc.encoder.iter().cloned());
        } else {
            out.push("-c:v".into());
            out.push(video_encoder_name(codec).to_string());
            out.push("-preset".into());
            out.push("veryfast".into());
            out.push("-crf".into());
            out.push("23".into());
        }
        // 软件后端（或硬件没给 scale 片段）时用通用 scale。
        if acc.scale.is_none() {
            if let (Some(w), Some(h)) = (max_width, max_height) {
                filters.push(format!("scale={w}:{h}"));
            }
        }
    } else {
        out.push("-c:v".into());
        out.push(video_encoder_name(codec).to_string());
        out.push("-preset".into());
        out.push("veryfast".into());
        out.push("-crf".into());
        out.push("23".into());
        if let (Some(w), Some(h)) = (max_width, max_height) {
            filters.push(format!("scale={w}:{h}"));
        }
    }

    if let Some(kbps) = bitrate_kbps {
        // 有码率上限时改成 ABR：CBR 更可控，直播/弱网更稳。
        out.push("-maxrate".into());
        out.push(format!("{kbps}k"));
        out.push("-bufsize".into());
        out.push(format!("{}k", kbps * 2));
    }

    // 软件后端没有 tonemap 片段时，补一条 CPU zscale 链。
    if tonemap_to.is_some() {
        let has = req.accel.as_ref().and_then(|a| a.tonemap.clone());
        if has.is_none() {
            filters.push(
                "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=linear:desat=0,\
                 zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
                    .to_string(),
            );
        }
    }

    out
}

pub fn video_encoder_name(c: &VideoCodec) -> &'static str {
    match c {
        VideoCodec::Hevc => "libx265",
        VideoCodec::Av1 => "libsvtav1",
        VideoCodec::Vp9 => "libvpx-vp9",
        VideoCodec::Mpeg2 => "mpeg2video",
        VideoCodec::Mpeg4 => "libxvid",
        _ => "libx264",
    }
}

pub fn audio_encoder_name(c: &AudioCodec) -> &'static str {
    match c {
        AudioCodec::Ac3 => "ac3",
        AudioCodec::Eac3 => "eac3",
        AudioCodec::Opus => "libopus",
        AudioCodec::Mp3 => "libmp3lame",
        AudioCodec::Flac => "flac",
        AudioCodec::Vorbis => "libvorbis",
        _ => "aac",
    }
}

pub fn subtitle_codec_name(c: &SubtitleCodec) -> &'static str {
    match c {
        SubtitleCodec::Ass | SubtitleCodec::Ssa => "ass",
        SubtitleCodec::Vtt => "webvtt",
        SubtitleCodec::Pgs => "pgs",
        SubtitleCodec::DvbSub => "dvbsub",
        SubtitleCodec::DvdSub => "dvdsub",
        _ => "srt",
    }
}

/// 单码率 HLS 的 master playlist。
///
/// 现在只有一路，但**master 与 media playlist 必须分开** ——
/// 后面加 ABR（多码率）时客户端不用改任何代码。
pub fn master_playlist(
    uri: &str,
    bandwidth: u64,
    width: Option<u32>,
    height: Option<u32>,
    codecs: &str,
) -> String {
    let res = match (width, height) {
        (Some(w), Some(h)) => format!(",RESOLUTION={w}x{h}"),
        _ => String::new(),
    };
    format!(
        "#EXTM3U\n\
         #EXT-X-VERSION:6\n\
         #EXT-X-STREAM-INF:BANDWIDTH={bandwidth}{res},CODECS=\"{codecs}\"\n\
         {uri}\n"
    )
}

/// HLS 的备用音轨 / 字幕轨声明。
pub fn media_playlist_entry(
    kind: &str,
    group: &str,
    name: &str,
    uri: &str,
    lang: Option<&str>,
    default: bool,
) -> String {
    let lang = lang
        .map(|l| format!(",LANGUAGE=\"{l}\""))
        .unwrap_or_default();
    let dflt = if default {
        ",DEFAULT=YES"
    } else {
        ",DEFAULT=NO"
    };
    format!(
        "#EXT-X-MEDIA:TYPE={kind},GROUP-ID=\"{group}\",NAME=\"{name}\"{lang}{dflt},URI=\"{uri}\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remux_req() -> StreamRequest {
        StreamRequest {
            input: "/media/a.mkv".into(),
            seek_ms: 0,
            video: VideoAction::Copy,
            audio: AudioAction::Copy { index: Some(1) },
            burn: None,
            output: OutputSpec::PipeTs,
            accel: None,
        }
    }

    fn idx(args: &[String], s: &str) -> usize {
        args.iter().position(|a| a == s).expect(s)
    }

    #[test]
    fn remux_copies_both_streams() {
        let a = build_args(&remux_req());
        assert!(a.contains(&"copy".to_string()));
        assert!(a.contains(&"-map".to_string()));
        assert!(a.contains(&"0:1".to_string()));
        assert!(a.contains(&"mpegts".to_string()));
        assert_eq!(a.last().unwrap(), "pipe:1");
    }

    #[test]
    fn seek_comes_before_input() {
        // DoD：首帧 < 3s。快速定位必须靠 "-ss 在 -i 之前"。
        let mut req = remux_req();
        req.seek_ms = 600_000;
        let a = build_args(&req);
        assert!(idx(&a, "-ss") < idx(&a, "-i"));
        assert_eq!(a[idx(&a, "-ss") + 1], "600.000");
    }

    #[test]
    fn hw_decode_args_precede_input_and_encoder_follows_it() {
        let mut req = remux_req();
        req.video = VideoAction::Encode {
            codec: VideoCodec::H264,
            max_width: Some(1280),
            max_height: Some(720),
            bitrate_kbps: Some(4000),
            tonemap_to: None,
        };
        req.accel = Some(AccelArgs {
            id: "vaapi".into(),
            decoder: vec!["-hwaccel".into(), "vaapi".into()],
            encoder: vec!["-c:v".into(), "h264_vaapi".into()],
            scale: Some("scale_vaapi=1280:720".into()),
            tonemap: None,
        });
        let a = build_args(&req);
        assert!(idx(&a, "-hwaccel") < idx(&a, "-i"));
        assert!(idx(&a, "h264_vaapi") > idx(&a, "-i"));
        assert!(a.contains(&"scale_vaapi=1280:720".to_string()));
        assert!(a.contains(&"4000k".to_string()));
    }

    #[test]
    fn burn_forces_video_reencode() {
        let mut req = remux_req();
        req.burn = Some(BurnSource::Internal(3));
        let a = build_args(&req);
        // 即使 video 动作是 Copy，烧录也必须重编码。
        assert!(!a.windows(2).any(|w| w[0] == "-c:v" && w[1] == "copy"));
        assert!(a.iter().any(|x| x.contains("subtitles=si=3")));
        assert!(req.reencodes_video());
    }

    #[test]
    fn external_burn_escapes_windows_path() {
        let mut req = remux_req();
        req.burn = Some(BurnSource::External(r"C:\media\a'b.srt".into()));
        let a = build_args(&req);
        let f = a.iter().find(|x| x.starts_with("subtitles=")).unwrap();
        assert!(f.contains("C\\:/media/a\\'b.srt"));
    }

    #[test]
    fn hls_output_writes_segments_to_dir() {
        let mut req = remux_req();
        req.output = OutputSpec::Hls {
            dir: "/cache/hls/j1".into(),
            seg_seconds: 6,
        };
        let a = build_args(&req);
        assert!(a.contains(&"hls".to_string()));
        assert!(a.contains(&"/cache/hls/j1/seg_%05d.ts".to_string()));
        assert!(a.contains(&"/cache/hls/j1/index.m3u8".to_string()));
        assert_eq!(a.last().unwrap(), "/cache/hls/j1/index.m3u8");
    }

    #[test]
    fn fmp4_uses_fragmented_movflags() {
        let mut req = remux_req();
        req.output = OutputSpec::PipeFmp4;
        let a = build_args(&req);
        let i = idx(&a, "-movflags");
        assert!(a[i + 1].contains("frag_keyframe"));
        assert!(a[i + 1].contains("empty_moov"));
    }

    #[test]
    fn no_audio_emits_an() {
        let mut req = remux_req();
        req.audio = AudioAction::None;
        let a = build_args(&req);
        assert!(a.contains(&"-an".to_string()));
        assert!(!a.contains(&"-c:a".to_string()));
    }

    #[test]
    fn audio_transcode_targets_aac_stereo() {
        let mut req = remux_req();
        req.audio = AudioAction::Encode {
            index: None,
            codec: AudioCodec::Aac,
            channels: 2,
            bitrate_kbps: 192,
        };
        let a = build_args(&req);
        assert!(a.contains(&"aac".to_string()));
        assert!(a.contains(&"192k".to_string()));
        assert!(a.contains(&"0:a:0".to_string()));
    }

    #[test]
    fn software_tonemap_chain_is_appended() {
        let mut req = remux_req();
        req.video = VideoAction::Encode {
            codec: VideoCodec::H264,
            max_width: None,
            max_height: None,
            bitrate_kbps: None,
            tonemap_to: Some(HdrType::Sdr),
        };
        let a = build_args(&req);
        let vf = a.iter().find(|x| x.contains("tonemap")).unwrap();
        assert!(vf.contains("zscale"));
    }

    #[test]
    fn encoder_names_are_stable() {
        assert_eq!(video_encoder_name(&VideoCodec::H264), "libx264");
        assert_eq!(video_encoder_name(&VideoCodec::Hevc), "libx265");
        assert_eq!(audio_encoder_name(&AudioCodec::Aac), "aac");
        assert_eq!(audio_encoder_name(&AudioCodec::Eac3), "eac3");
    }

    #[test]
    fn master_playlist_shape() {
        let m = master_playlist(
            "index.m3u8",
            8_000_000,
            Some(1920),
            Some(1080),
            "avc1.640028,mp4a.40.2",
        );
        assert!(m.starts_with("#EXTM3U"));
        assert!(m.contains("RESOLUTION=1920x1080"));
        assert!(m.contains("CODECS=\"avc1.640028,mp4a.40.2\""));
        assert!(m.trim_end().ends_with("index.m3u8"));
    }

    #[test]
    fn media_entry_marks_default_and_language() {
        let e = media_playlist_entry("SUBTITLES", "subs", "中文", "sub_0.m3u8", Some("zh"), true);
        assert!(e.contains("TYPE=SUBTITLES"));
        assert!(e.contains("LANGUAGE=\"zh\""));
        assert!(e.contains("DEFAULT=YES"));
    }
}
