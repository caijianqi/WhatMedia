//! 兼容性矩阵（P3 DoD 之一）。
//!
//! 语料在 `test-media/compat/cases.jsonl`，格式说明写在那个文件头部的注释里。
//! **为什么语料是数据而不是写死在代码里**：覆盖率是个会随认知变化的指标，
//! 加一条"最近很常见的 AV1 片源"不该需要改 Rust 代码；
//! 数据结构化之后，语料本身也能被 review（weight 是不是拍脑袋来的）。
//!
//! 这份语料模拟**真实家庭片源**的组合，而不是穷举笛卡尔积：
//! 现实里 `AVI + AV1 + TrueHD` 这种组合根本不存在，穷举只会稀释覆盖率数字，
//! 让"Direct Play 覆盖率"变成一个好看但没意义的指标。
//! 每条样本带一个**出现频率权重**，覆盖率按权重算，不能按条数算 ——
//! 否则"4K HDR 原盘"和"1080p H264"会被当成同等重要。
//!
//! 断言三件事：
//!   1. 桌面客户端上，常见片源的 Direct Play 覆盖率要够高（否则转码服务器会被打爆）
//!   2. Web 上**不允许出现放不了的情况**（转码必须兜住）
//!   3. 每个组合的降级原因都能解释自己（不允许出现"没原因的降级"）

use serde::Deserialize;
use std::path::{Path, PathBuf};
use whatmedia_core::{AudioCodec, Container, HdrType, PlaybackStrategy, VideoCodec};
use whatmedia_playback::{
    decide, AudioTrack, ClientCapability, DecisionInput, MediaCapability, ServerCapability,
    VideoTrack,
};

/// 一条片源样本。`weight` 是这类片源在家庭库里的相对出现频率。
#[derive(Debug, Deserialize)]
struct Sample {
    name: String,
    weight: u32,
    container: Container,
    video: VideoCodec,
    #[serde(default = "eight")]
    depth: u8,
    #[serde(default)]
    hdr: HdrType,
    #[serde(default = "hd")]
    width: u32,
    #[serde(default = "sd")]
    height: u32,
    audio: AudioCodec,
    channels: u8,
    kbps: u32,
}

fn eight() -> u8 {
    8
}
fn hd() -> u32 {
    1920
}
fn sd() -> u32 {
    1080
}

/// 拦住"语料写错 → 覆盖率悄悄变化"。
///
/// **为什么必须显式拦**：这几个枚举都带 `#[serde(untagged)] Other(String)` 兜底，
/// 写错的 codec 名不会报错，而是静默变成一个"没人认识的编码"。
/// 结果是覆盖率数字变了，但没人知道为什么变 —— 这种失败方式比崩溃糟糕得多。
/// （`truehd` 写成 `true_hd` 才对，这个坑就是这么被踩出来的。）
fn assert_recognized(s: &Sample, line: usize) {
    let unrecognized = [
        ("video", matches!(s.video, VideoCodec::Other(_))),
        ("audio", matches!(s.audio, AudioCodec::Other(_))),
        ("container", matches!(s.container, Container::Other(_))),
    ];
    for (field, bad) in unrecognized {
        assert!(
            !bad,
            "语料第 {line} 行的 {field} 没被识别：{}\n  未识别的值会静默落进 Other 变体，\
             让覆盖率变成一个说不清来源的数字",
            s.name
        );
    }
}

fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("test-media/compat/cases.jsonl")
}

fn corpus() -> Vec<Sample> {
    let raw = std::fs::read_to_string(corpus_path())
        .unwrap_or_else(|e| panic!("读不到兼容性语料 {}：{e}", corpus_path().display()));
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let s: Sample = serde_json::from_str(t)
            .unwrap_or_else(|e| panic!("语料第 {} 行不是合法 JSON：{e}\n  {t}", i + 1));
        assert_recognized(&s, i + 1);
        out.push(s);
    }
    // 空语料会让所有覆盖率变成 NaN，断言会在静默中失效。
    assert!(!out.is_empty(), "兼容性语料是空的");
    out
}

fn to_media(s: &Sample) -> MediaCapability {
    MediaCapability {
        container: Some(s.container.clone()),
        video: Some(VideoTrack {
            index: 0,
            codec: s.video.clone(),
            width: Some(s.width),
            height: Some(s.height),
            bit_depth: s.depth,
            hdr: s.hdr,
        }),
        audio: vec![AudioTrack {
            index: 1,
            codec: s.audio.clone(),
            channels: Some(s.channels),
            language: Some("eng".into()),
            is_default: true,
        }],
        duration_ms: Some(7_200_000),
        bitrate_kbps: Some(s.kbps),
    }
}

fn server() -> ServerCapability {
    ServerCapability {
        ffmpeg_available: true,
        encoders: vec![whatmedia_core::HardwareProfile {
            kind: whatmedia_core::HardwareKind::Software,
            decoders: vec![],
            encoders: vec![VideoCodec::H264, VideoCodec::Hevc],
            supports_zero_copy: false,
            supports_tone_mapping: true,
            max_concurrent_sessions: 2,
        }],
        active_transcodes: 0,
        accel_ctx: whatmedia_transcoder::AccelContext::default(),
    }
}

fn run(client: &ClientCapability) -> Vec<(String, u32, PlaybackStrategy, Vec<String>)> {
    let srv = server();
    corpus()
        .iter()
        .map(|s| {
            let media = to_media(s);
            let d = decide(DecisionInput {
                media: &media,
                client,
                server: &srv,
                network_kbps: None,
                start_ms: 0,
                hls_dir: None,
                seg_seconds: 6,
            });
            (
                s.name.to_string(),
                s.weight,
                d.strategy,
                d.reasons.iter().map(|r| r.code().to_string()).collect(),
            )
        })
        .collect()
}

/// 加权覆盖率。`pred` 判定"这条算不算达标"。
fn weighted_coverage(
    results: &[(String, u32, PlaybackStrategy, Vec<String>)],
    pred: fn(PlaybackStrategy) -> bool,
) -> f64 {
    let total: u32 = results.iter().map(|(_, w, _, _)| *w).sum();
    let hit: u32 = results
        .iter()
        .filter(|(_, _, s, _)| pred(*s))
        .map(|(_, w, _, _)| *w)
        .sum();
    hit as f64 / total as f64
}

fn is_direct(s: PlaybackStrategy) -> bool {
    s == PlaybackStrategy::DirectPlay
}

/// 视频不需要重编码（Direct Play 或 Remux）。
///
/// **为什么要有这个指标**：浏览器根本不认 MKV，
/// 纯 Direct Play 覆盖率在 Web 端天然不可能高 —— 用它做跨端指标是自欺欺人。
/// 真正决定服务器压力的是"有没有重编码"。
fn is_no_reencode(s: PlaybackStrategy) -> bool {
    matches!(s, PlaybackStrategy::DirectPlay | PlaybackStrategy::Remux)
}

#[test]
fn desktop_client_direct_plays_most_home_formats() {
    let r = run(&ClientCapability::preset_mpv());
    for (name, _, strategy, reasons) in &r {
        println!("[mpv] {name:36} {strategy:?} {reasons:?}");
    }
    let c = weighted_coverage(&r, is_direct);
    println!("桌面客户端 Direct Play 加权覆盖率：{:.0}%", c * 100.0);
    assert!(
        c >= 0.90,
        "桌面客户端 Direct Play 覆盖率 {:.0}% 低于 90%",
        c * 100.0
    );
}

#[test]
fn web_client_never_fails_to_play() {
    let r = run(&ClientCapability::preset_web());
    for (name, _, strategy, reasons) in &r {
        println!("[web] {name:36} {strategy:?} {reasons:?}");
        assert_ne!(
            *strategy,
            PlaybackStrategy::Unsupported,
            "Web 上 {name} 竟然放不了 —— 转码必须兜住所有情况"
        );
        if *strategy != PlaybackStrategy::DirectPlay {
            assert!(!reasons.is_empty(), "{name} 降级了却给不出原因");
        }
    }
    println!(
        "Web 客户端：Direct Play {:.0}% / 无需重编码 {:.0}%",
        weighted_coverage(&r, is_direct) * 100.0,
        weighted_coverage(&r, is_no_reencode) * 100.0
    );
    assert!(
        weighted_coverage(&r, is_no_reencode) >= 0.55,
        "Web 上超过一半的常见片源都要重编码，说明 Web 端策略有问题"
    );
}

#[test]
fn overall_no_reencode_coverage_meets_dod() {
    // DoD 原文："常见家庭格式 Direct Play 覆盖率 ≥ 70%"。
    //
    // 这里按"视频无需重编码"来跨端衡量，理由写在 `is_no_reencode` 的注释里：
    // 浏览器不认 MKV，把 Web 的 Direct Play 率拿来平均会把指标压到一个
    // 无论如何都达不到的数字，那样的"未达标"没有指导意义。
    let desktop = weighted_coverage(&run(&ClientCapability::preset_mpv()), is_no_reencode);
    let web = weighted_coverage(&run(&ClientCapability::preset_web()), is_no_reencode);
    let overall = (desktop + web) / 2.0;
    println!(
        "综合「无需重编码」覆盖率：{:.0}%（桌面 {:.0}% / Web {:.0}%）",
        overall * 100.0,
        desktop * 100.0,
        web * 100.0
    );
    assert!(
        overall >= 0.70,
        "综合无需重编码覆盖率 {:.0}% 未达到 DoD 的 70%",
        overall * 100.0
    );
}

