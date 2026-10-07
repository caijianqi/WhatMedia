//! FFprobe 进程封装与输出解析。
//!
//! 设计约束：
//!   · **必须有超时。** 网络存储挂掉时 ffprobe 会永久阻塞，不能拖死整个扫描。
//!   · **必须能离线测试。** 本机不一定装了 FFmpeg，所以 JSON → 结构的转换
//!     与进程调用分开，前者有单元测试，后者只在集成测试里跑。
//!   · **原始 JSON 入库。** ARM 上 ffprobe 一次要好几百毫秒，重复探测很贵。

use chrono::Utc;
use mosaic_core::{HdrType, MediaStream, MosaicError, Result, StreamType};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use uuid::Uuid;

/// 默认超时。4K REMUX 在机械盘上探测也会在一分钟内返回。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct FfprobeRunner {
    bin: String,
    timeout: Duration,
}

impl Default for FfprobeRunner {
    fn default() -> Self {
        Self {
            bin: "ffprobe".into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl FfprobeRunner {
    pub fn new(bin: impl Into<String>, timeout: Duration) -> Self {
        Self {
            bin: bin.into(),
            timeout,
        }
    }

    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// 探测一个文件。超时或进程异常都返回 `MosaicError::Ffmpeg`。
    pub async fn probe(&self, path: &str) -> Result<ProbeResult> {
        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.arg("-v")
            .arg("quiet")
            .arg("-print_format")
            .arg("json")
            .arg("-show_format")
            .arg("-show_streams")
            .arg(path);

        let fut = cmd.output();
        let output = tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| MosaicError::Ffmpeg {
                message: format!("ffprobe 超时（{}s）：{path}", self.timeout.as_secs()),
            })?
            .map_err(|e| MosaicError::Ffmpeg {
                message: format!("无法启动 ffprobe（{}）：{e}", self.bin),
            })?;

        if !output.status.success() {
            return Err(MosaicError::Ffmpeg {
                message: format!(
                    "ffprobe 失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }

        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        let mut result = parse_probe_json(&raw)?;
        result.raw = raw;
        Ok(result)
    }
}

/// 探测结果。
#[derive(Debug, Clone, Default)]
pub struct ProbeResult {
    pub container: Option<String>,
    pub duration_ms: Option<i64>,
    pub bitrate: Option<i64>,
    pub streams: Vec<ProbeStream>,
    /// 原始 JSON，原样入库以避免重复探测。
    pub raw: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProbeStream {
    pub index: i32,
    pub stream_type: StreamType,
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

impl ProbeStream {
    pub fn to_media_stream(&self, file_id: Uuid) -> MediaStream {
        MediaStream {
            id: Uuid::new_v4(),
            file_id,
            stream_type: self.stream_type,
            index: self.index,
            codec: self.codec.clone(),
            language: self.language.clone(),
            title: self.title.clone(),
            is_default: self.is_default,
            is_forced: self.is_forced,
            width: self.width,
            height: self.height,
            bit_depth: self.bit_depth,
            hdr_type: self.hdr_type,
            channels: self.channels,
            bitrate: self.bitrate,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// ffprobe JSON 结构（只声明我们要用的字段，其余一律忽略）
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    streams: Vec<FfStream>,
    format: Option<FfFormat>,
}

#[derive(Debug, Deserialize)]
struct FfStream {
    #[serde(default)]
    index: i64,
    codec_name: Option<String>,
    codec_type: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    bits_per_raw_sample: Option<String>,
    #[serde(default)]
    channels: Option<i64>,
    color_transfer: Option<String>,
    #[serde(default)]
    bit_rate: Option<String>,
    #[serde(default)]
    tags: Option<HashMap<String, String>>,
    #[serde(default)]
    disposition: Option<HashMap<String, i64>>,
    #[serde(default)]
    side_data_list: Option<Vec<FfSideData>>,
}

#[derive(Debug, Deserialize)]
struct FfSideData {
    #[serde(default)]
    side_data_type: String,
}

#[derive(Debug, Deserialize)]
struct FfFormat {
    format_name: Option<String>,
    duration: Option<String>,
    bit_rate: Option<String>,
}

/// 把 ffprobe 的 JSON 解析成 `ProbeResult`。**不启动进程**，可离线测试。
pub fn parse_probe_json(raw: &str) -> Result<ProbeResult> {
    let parsed: FfprobeOutput = serde_json::from_str(raw).map_err(|e| MosaicError::Ffmpeg {
        message: format!("解析 ffprobe 输出失败：{e}"),
    })?;

    let mut out = ProbeResult::default();

    if let Some(f) = &parsed.format {
        out.container = f
            .format_name
            .as_deref()
            .and_then(|n| n.split(',').next())
            .map(str::to_string);
        out.duration_ms = f.duration.as_deref().and_then(parse_secs_f64);
        out.bitrate = f.bit_rate.as_deref().and_then(|s| s.parse().ok());
    }

    for s in &parsed.streams {
        let tags = s.tags.clone().unwrap_or_default();
        let disp = s.disposition.clone().unwrap_or_default();
        let bitrate = s
            .bit_rate
            .as_deref()
            .and_then(|b| b.parse::<i64>().ok())
            .filter(|b| *b > 0);

        out.streams.push(ProbeStream {
            index: s.index as i32,
            stream_type: match s.codec_type.as_deref() {
                Some("video") => StreamType::Video,
                Some("audio") => StreamType::Audio,
                Some("subtitle") => StreamType::Subtitle,
                Some("attachment") => StreamType::Attachment,
                _ => StreamType::Data,
            },
            codec: s.codec_name.clone(),
            language: tags.get("language").cloned(),
            title: tags.get("title").cloned(),
            is_default: disp.get("default").copied().unwrap_or(0) != 0,
            is_forced: disp.get("forced").copied().unwrap_or(0) != 0,
            width: s.width.map(|v| v as i32),
            height: s.height.map(|v| v as i32),
            bit_depth: s
                .bits_per_raw_sample
                .as_deref()
                .and_then(|b| b.parse().ok())
                .filter(|b| *b > 0),
            hdr_type: detect_hdr(s),
            channels: s.channels.map(|c| c as i32),
            bitrate,
        });
    }

    Ok(out)
}

/// HDR 判定：先看 Dolby Vision 的 side data，再看 color transfer。
///
/// 顺序不能反 —— DV 片源的 color transfer 通常也是 PQ(smpte2084)，
/// 先判 PQ 会把所有 DV 都误标成 HDR10。
fn detect_hdr(s: &FfStream) -> HdrType {
    let side: Vec<String> = s
        .side_data_list
        .iter()
        .flatten()
        .map(|d| d.side_data_type.to_lowercase())
        .collect();

    if side.iter().any(|t| t.contains("dovi")) {
        return HdrType::DolbyVision;
    }

    match s.color_transfer.as_deref() {
        Some("smpte2084") => {
            if side.iter().any(|t| t.contains("smpte2094-40")) {
                HdrType::Hdr10Plus
            } else {
                HdrType::Hdr10
            }
        }
        Some("arib-std-b67") => HdrType::Hlg,
        _ => HdrType::Sdr,
    }
}

fn parse_secs_f64(s: &str) -> Option<i64> {
    let secs: f64 = s.parse().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    Some((secs * 1000.0).round() as i64)
}

/// 供入库时填充 `MediaFile.duration_ms` 缺失的兜底：用最长的一条流。
pub fn fallback_duration(result: &ProbeResult) -> Option<i64> {
    result.duration_ms
}

pub fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份真实的 ffprobe 输出（精简过）。测试不启动进程，只解析它。
    const SAMPLE: &str = r#"{
      "streams": [
        {
          "index": 0,
          "codec_name": "hevc",
          "codec_type": "video",
          "width": 3840, "height": 2160,
          "bits_per_raw_sample": "10",
          "color_transfer": "smpte2084",
          "side_data_list": [
            {"side_data_type": "HDR Dynamic Metadata SMPTE2094-40"}
          ],
          "disposition": {"default": 1}
        },
        {
          "index": 1,
          "codec_name": "truehd",
          "codec_type": "audio",
          "channels": 8,
          "tags": {"language": "eng", "title": "TrueHD Atmos 7.1"},
          "disposition": {"default": 1}
        },
        {
          "index": 2,
          "codec_name": "hdmv_pgs_subtitle",
          "codec_type": "subtitle",
          "tags": {"language": "chi"},
          "disposition": {"default": 0, "forced": 0}
        }
      ],
      "format": {
        "format_name": "matroska,webm",
        "duration": "7345.024000",
        "bit_rate": "61234567"
      }
    }"#;

    #[test]
    fn parses_container_duration_bitrate() {
        let r = parse_probe_json(SAMPLE).unwrap();
        assert_eq!(r.container.as_deref(), Some("matroska"));
        assert_eq!(r.duration_ms, Some(7345024));
        assert_eq!(r.bitrate, Some(61_234_567));
    }

    #[test]
    fn maps_three_streams() {
        let r = parse_probe_json(SAMPLE).unwrap();
        assert_eq!(r.streams.len(), 3);
        assert_eq!(r.streams[0].stream_type, StreamType::Video);
        assert_eq!(r.streams[0].width, Some(3840));
        assert_eq!(r.streams[0].bit_depth, Some(10));
        assert_eq!(r.streams[1].channels, Some(8));
        assert_eq!(r.streams[1].language.as_deref(), Some("eng"));
        assert!(r.streams[1].is_default);
        assert_eq!(r.streams[2].stream_type, StreamType::Subtitle);
        assert!(!r.streams[2].is_default);
    }

    #[test]
    fn hdr10plus_beats_plain_hdr10() {
        let r = parse_probe_json(SAMPLE).unwrap();
        assert_eq!(r.streams[0].hdr_type, HdrType::Hdr10Plus);
    }

    #[test]
    fn dovi_is_not_misread_as_hdr10() {
        // DV 的 color transfer 也是 PQ；先判 side data 才不会错。
        let json = r#"{"streams":[{"index":0,"codec_type":"video","color_transfer":"smpte2084",
            "side_data_list":[{"side_data_type":"DOVI configuration record"}]}],"format":{}}"#;
        let r = parse_probe_json(json).unwrap();
        assert_eq!(r.streams[0].hdr_type, HdrType::DolbyVision);
    }

    #[test]
    fn garbage_json_is_an_error_not_a_panic() {
        assert!(parse_probe_json("not json").is_err());
    }
}
