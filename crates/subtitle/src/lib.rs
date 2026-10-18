//! 字幕：识别 / 解析 / 转换 / 降级策略。
//!
//! 模块分层：
//!
//! ```text
//! plan      投递策略（Direct → Convert → BurnIn → Drop）
//!   ├─ lang     语言与标记识别（文件名里挖信息）
//!   ├─ format   SRT / VTT / ASS 与中间表示 Cue 的互转
//!   └─ sidecar  外挂字幕发现（走 StorageProvider，不假设本地磁盘）
//! ```
//!
//! 一条铁律：**字幕问题永远不能让播放起不来。**
//! 字幕解析失败、格式不认识、烧录没预算 —— 统统降级到"没字幕继续播"。

pub mod format;
pub mod lang;
pub mod plan;
pub mod sidecar;

pub use format::{parse_ass, parse_srt, parse_timestamp, parse_vtt, to_srt, to_vtt, Cue};
pub use lang::{codec_from_ext, is_bitmap, is_text, normalize_lang, parse_suffix, SubtitleFlags};
pub use plan::{
    plan_track, requires_burn, BurnCapability, SubtitleMode, SubtitlePlan, SubtitleTarget,
};
pub use sidecar::{discover_sidecars, ExternalSubtitle};

use whatmedia_core::SubtitleCodec;

/// 从 ffprobe 的 codec 名映射字幕格式。
///
/// ffprobe 给的是 `subrip` / `ass` / `hdmv_pgs_subtitle` 这类名字，
/// 与文件扩展名是两套命名，所以要有这一层。
pub fn codec_from_probe_name(name: &str) -> SubtitleCodec {
    match name.to_ascii_lowercase().as_str() {
        "subrip" | "srt" => SubtitleCodec::Srt,
        "ass" => SubtitleCodec::Ass,
        "ssa" => SubtitleCodec::Ssa,
        "webvtt" | "vtt" => SubtitleCodec::Vtt,
        "hdmv_pgs_subtitle" | "pgs" | "sup" => SubtitleCodec::Pgs,
        "dvb_subtitle" | "dvbsub" => SubtitleCodec::DvbSub,
        "dvd_subtitle" | "dvdsub" => SubtitleCodec::DvdSub,
        other => SubtitleCodec::Other(other.to_string()),
    }
}

/// 字幕转格式时目标格式的默认选择：HLS 用 VTT，其余用 SRT。
pub fn default_convert_target(for_hls: bool) -> SubtitleCodec {
    if for_hls {
        SubtitleCodec::Vtt
    } else {
        SubtitleCodec::Srt
    }
}

/// 把字幕渲染进画面所需的 ffmpeg 滤镜片段。
///
/// 外挂字幕用 `subtitles=`，内嵌轨用 `overlay` 之外的 `subtitles=si=N`。
/// 注意：烧录**必然**触发视频重编码，调用方必须先确认真的要烧。
pub fn burn_filter(track: &str) -> String {
    // 文件名里可能有冒号与单引号，得转义；Windows 路径的盘符冒号尤其常见。
    let escaped = track
        .replace('\\', "/")
        .replace(':', "\\:")
        .replace('\'', "\\'");
    format!("subtitles='{escaped}'")
}

/// 内嵌轨烧录用。ffprobe 的流序号是**文件内序号**，不是过滤后的序号。
pub fn burn_filter_for_stream(file_index: i32) -> String {
    format!("subtitles=si={file_index}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_names_map_to_codecs() {
        assert_eq!(codec_from_probe_name("subrip"), SubtitleCodec::Srt);
        assert_eq!(
            codec_from_probe_name("hdmv_pgs_subtitle"),
            SubtitleCodec::Pgs
        );
        assert_eq!(codec_from_probe_name("dvb_subtitle"), SubtitleCodec::DvbSub);
        assert_eq!(
            codec_from_probe_name("mov_text"),
            SubtitleCodec::Other("mov_text".into())
        );
    }

    #[test]
    fn hls_prefers_vtt() {
        assert_eq!(default_convert_target(true), SubtitleCodec::Vtt);
        assert_eq!(default_convert_target(false), SubtitleCodec::Srt);
    }

    #[test]
    fn burn_filter_escapes_windows_paths() {
        // C:\media\a'b.mkv 里的冒号与单引号都会破坏滤镜语法。
        let f = burn_filter(r"C:\media\a'b.mkv");
        assert!(f.starts_with("subtitles='"));
        assert!(f.contains("\\:"));
        assert!(f.contains("\\'"));
        assert_eq!(burn_filter_for_stream(3), "subtitles=si=3");
    }
}
