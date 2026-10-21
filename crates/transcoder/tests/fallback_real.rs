//! 降级链的**真机回归**。
//!
//! 单元层（`runner.rs` 里用 MockRunner）能证明"逻辑会换后端"，
//! 但证明不了"真 ffmpeg 的失败确实发生在首帧窗口内" —— 那是一个时间假设，
//! 只有真跑才知道 10 秒够不够、退出码是不是非 0。
//!
//! **没有 ffmpeg 就跳过，不算失败。** CI（尤其自托管 arm64 runner）不一定装了，
//! 让环境缺失变成红会让这个测试失去"回归"的意义 —— 它会在每次装环境时被忽略。

use std::process::Command;
use std::time::Duration;

use whatmedia_core::VideoCodec;
use whatmedia_transcoder::{
    AccelArgs, AudioAction, FfmpegRunner, OutputSpec, StreamRequest, VideoAction,
};

fn ffmpeg_available() -> bool {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 造一段 1 秒素材。用 lavfi 而不是下载样本：任何机器都能跑出同一口径。
fn make_clip(dir: &std::path::Path) -> Option<String> {
    let out = dir.join("clip.mp4");
    let ok = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=15:duration=1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
        ])
        .arg(out.to_string_lossy().as_ref())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    ok.then(|| out.to_string_lossy().to_string())
}

fn request(input: &str) -> StreamRequest {
    StreamRequest {
        input: input.to_string(),
        seek_ms: 0,
        video: VideoAction::Encode {
            codec: VideoCodec::H264,
            max_width: None,
            max_height: None,
            bitrate_kbps: None,
            tonemap_to: None,
        },
        audio: AudioAction::None,
        output: OutputSpec::PipeTs,
        accel: None,
        live: false,
    }
}

/// 一个必然启动失败的"硬件"后端。
///
/// 不用 vaapi/qsv 这类真实后端：它们的成败取决于机器有没有对应硬件，
/// 会导致这个测试在一部分机器上红。`no_such_hwaccel` 在任何平台上都立刻失败。
fn bogus_accel() -> AccelArgs {
    AccelArgs {
        id: "bogus".into(),
        decoder: vec!["-hwaccel".into(), "no_such_hwaccel".into()],
        encoder: vec!["-c:v".into(), "h264_bogus".into()],
        scale: None,
        tonemap: None,
    }
}

fn software_accel() -> AccelArgs {
    AccelArgs {
        id: "software".into(),
        decoder: Vec::new(),
        encoder: Vec::new(),
        scale: None,
        tonemap: None,
    }
}

/// 真 ffmpeg：硬件档在首帧窗口内挂掉 → 必须静默换到软件档跑完。
#[tokio::test]
async fn real_ffmpeg_degrades_to_software() {
    if !ffmpeg_available() {
        eprintln!("跳过：本机没有 ffmpeg");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Some(input) = make_clip(dir.path()) else {
        eprintln!("跳过：造不出素材（ffmpeg 缺 lavfi/libx264？）");
        return;
    };

    let runner = FfmpegRunner::new("ffmpeg");
    let out = runner
        .spawn_with_fallback(
            &request(&input),
            &[bogus_accel(), software_accel()],
            Some(Duration::from_secs(15)),
        )
        .await
        .expect("降级链不该整体失败");

    assert_eq!(out.used, 1, "硬件档失败后必须换到软件档");
    assert!(out.degraded());
    // 被放弃那次的 stderr 必须留下来 —— 用户问"为什么没走硬件"全靠它。
    assert!(
        out.dropped
            .first()
            .map(|(_, s)| s.contains("no_such_hwaccel"))
            .unwrap_or(false),
        "首帧失败的 stderr 没被采集：{:?}",
        out.dropped
    );

    // 换过去的软件档要真能把片子转完 —— 否则降级只是"换了个方式失败"。
    let mut proc = out.process;
    let exit = proc.wait_with_timeout(None).await.unwrap();
    assert_eq!(
        exit,
        whatmedia_transcoder::Exit::Completed,
        "软件档应当跑完"
    );
}

/// 首帧窗口必须足够短，让"硬件挂了"这件事不至于让用户等太久。
#[tokio::test]
async fn degraded_start_is_fast_enough_to_be_imperceptible() {
    if !ffmpeg_available() {
        eprintln!("跳过：本机没有 ffmpeg");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let Some(input) = make_clip(dir.path()) else {
        return;
    };

    let runner = FfmpegRunner::new("ffmpeg");
    let start = std::time::Instant::now();
    let out = runner
        .spawn_with_fallback(
            &request(&input),
            &[bogus_accel(), software_accel()],
            Some(Duration::from_secs(15)),
        )
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(out.used, 1);
    // ffmpeg 不认识的 hwaccel 是立刻退出的，不该耗掉整个窗口。
    // 留 5 秒余量是为了容忍慢盘上的进程启动开销。
    assert!(
        elapsed < Duration::from_secs(5),
        "降级耗时 {elapsed:?} 过长 —— 用户会看到明显的起播卡顿"
    );
}
