//! FFmpeg 缩略图与多尺寸生成。
//!
//! P2 需要两类图：
//!   · **视频缩略图** —— 片源自己截一帧（用户更想看到画面，而不是官方海报）
//!   · **图片变体**   —— 把下载来的海报压出 w185 / w300 / w500 等多个尺寸
//!
//! 这里只负责"构造参数 + 管进程"，不负责缓存（在 `images` 模块）。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// 常用尺寸变体。UI 按场景挑一个，不要每次都拉原图。
pub const VARIANTS: &[(&str, u32)] =
    &[("w185", 185), ("w300", 300), ("w500", 500), ("w1280", 1280)];

#[derive(Debug, Clone)]
pub struct FfmpegThumbRunner {
    pub bin: PathBuf,
    pub timeout: Duration,
}

impl Default for FfmpegThumbRunner {
    fn default() -> Self {
        Self {
            bin: PathBuf::from("ffmpeg"),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// 截一帧。
///
/// `-ss` 放在 `-i` **前面**是关键：这样 FFmpeg 走关键帧快速定位，
/// 放在后面会先解码到目标时间点，一部 2 小时的片能慢上十倍。
///
/// `-frames:v 1` 保证只输出一张，避免某些容器下输出一堆帧。
pub fn thumbnail_args(input: &str, output: &str, at_ms: i64, width: u32) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-ss".into(),
        format!("{:.3}", at_ms as f64 / 1000.0),
        "-i".into(),
        input.to_string(),
        "-frames:v".into(),
        "1".into(),
        "-vf".into(),
        format!("scale={width}:-2:flags=lanczos"),
        "-q:v".into(),
        "5".into(),
        output.to_string(),
    ]
}

/// 图片变体。
pub fn resize_args(input: &str, output: &str, width: u32) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        input.to_string(),
        "-vf".into(),
        format!("scale={width}:-2:flags=lanczos"),
        output.to_string(),
    ]
}

impl FfmpegThumbRunner {
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Self {
            bin: bin.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// 截取 `at_ms` 处的一帧，宽度 `width`。返回输出路径。
    pub async fn thumbnail(
        &self,
        input: &Path,
        output: &Path,
        at_ms: i64,
        width: u32,
    ) -> Result<PathBuf> {
        let args = thumbnail_args(
            &input.to_string_lossy(),
            &output.to_string_lossy(),
            at_ms,
            width,
        );
        self.run(&args).await?;
        Ok(output.to_path_buf())
    }

    pub async fn resize(&self, input: &Path, output: &Path, width: u32) -> Result<PathBuf> {
        let args = resize_args(&input.to_string_lossy(), &output.to_string_lossy(), width);
        self.run(&args).await?;
        Ok(output.to_path_buf())
    }

    /// 起进程、等超时、收 stderr。
    ///
    /// **必须 kill 而不是等它自己退出**：转码/截图卡住时 FFmpeg 会一直挂着，
    /// 等到天荒地老会把 worker 占满。
    async fn run(&self, args: &[String]) -> Result<()> {
        let mut child = tokio::process::Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| {
                format!(
                    "无法启动 FFmpeg（{}）—— 未安装或不在 PATH 中",
                    self.bin.display()
                )
            })?;

        let stderr = child.stderr.take();
        let read = async {
            let mut buf = Vec::new();
            if let Some(mut s) = stderr {
                use tokio::io::AsyncReadExt;
                let _ = s.read_to_end(&mut buf).await;
            }
            buf
        };

        let output = tokio::time::timeout(self.timeout, child.wait()).await;
        match output {
            Ok(Ok(status)) => {
                if !status.success() {
                    let err = read.await;
                    anyhow::bail!(
                        "FFmpeg 退出码 {:?}：{}",
                        status.code(),
                        String::from_utf8_lossy(&err)
                    );
                }
                Ok(())
            }
            Ok(Err(e)) => Err(e).context("等待 FFmpeg 失败"),
            Err(_) => {
                let _ = child.kill().await;
                anyhow::bail!("FFmpeg 超时（{}s），已终止", self.timeout.as_secs())
            }
        }
    }

    pub async fn available(&self) -> bool {
        tokio::process::Command::new(&self.bin)
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// 取缩略图的时间点：太靠前是黑场或制作方 logo，太靠后会剧透。
///
/// 默认取时长 10%，夹在 [5s, 5min] 之间。
pub fn thumbnail_timestamp(duration_ms: Option<i64>) -> i64 {
    match duration_ms {
        Some(d) if d > 0 => (d / 10).clamp(5_000, 300_000),
        _ => 30_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_is_before_input() {
        let a = thumbnail_args("in.mkv", "out.jpg", 60_000, 400);
        let ss = a.iter().position(|x| x == "-ss").unwrap();
        let ii = a.iter().position(|x| x == "-i").unwrap();
        assert!(ss < ii, "`-ss` 必须在 `-i` 之前，否则会退化成全量解码");
        assert_eq!(a[ss + 1], "60.000");
        assert!(a.contains(&"-frames:v".to_string()));
        assert!(a.iter().any(|x| x.contains("scale=400")));
    }

    #[test]
    fn resize_uses_lanczos() {
        let a = resize_args("in.png", "out.png", 300);
        assert!(a.iter().any(|x| x.contains("scale=300:-2")));
        assert!(a.contains(&"-y".to_string()));
    }

    #[test]
    fn timestamp_is_bounded() {
        assert_eq!(thumbnail_timestamp(None), 30_000);
        // 短片：10% 太靠前，抬到 5s
        assert_eq!(thumbnail_timestamp(Some(20_000)), 5_000);
        // 2 小时：10% = 12 分钟，但被 5 分钟上限夹住 —— 太靠后会剧透
        assert_eq!(thumbnail_timestamp(Some(7_200_000)), 300_000);
        // 40 分钟：10% = 4 分钟，未触发上限
        assert_eq!(thumbnail_timestamp(Some(2_400_000)), 240_000);
        // 超长片：封顶 5 分钟
        assert_eq!(thumbnail_timestamp(Some(3_600_000 * 3)), 300_000);
    }

    #[test]
    fn variants_are_ordered() {
        let widths: Vec<u32> = VARIANTS.iter().map(|(_, w)| *w).collect();
        assert_eq!(widths, vec![185, 300, 500, 1280]);
    }

    /// 没有 FFmpeg 时要给出可诊断的错误，而不是 panic。
    #[tokio::test]
    async fn missing_binary_reports_clearly() {
        let r = FfmpegThumbRunner::new("definitely-not-ffmpeg-xyz");
        let err = r
            .thumbnail(Path::new("a.mkv"), Path::new("b.jpg"), 1000, 200)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("无法启动 FFmpeg"), "{msg}");
    }
}
