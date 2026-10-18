//! 转码与流式输出：参数构造 + 进程生命周期 + 并发控制。
//!
//! 模块分层：
//!
//! ```text
//! runner    进程生命周期（启动 / 超时 / 取消 / 收尸 / 硬件失败降级）
//!   ├─ args     命令行构造（顺序即正确性，见模块注释）
//!   ├─ progress stderr 进度解析
//!   ├─ pool     并发上限与播放抢占
//!   ├─ process  可注入的 CommandRunner（离线可测）
//!   └─ temp     临时目录守卫（取消后 100% 清理）
//! ```
//!
//! 本 crate **不决定该不该转码**，只负责"决定好了之后怎么把 ffmpeg 跑起来"。

pub mod args;
pub mod pool;
pub mod process;
pub mod progress;
pub mod runner;
pub mod temp;

pub use args::{
    build_args, master_playlist, media_playlist_entry, AccelArgs, AudioAction, BurnSource,
    OutputSpec, StreamRequest, VideoAction,
};
pub use pool::{Slot, TranscodePool};
pub use process::{CommandRunner, MockChild, MockRunner, TokioRunner};
pub use progress::{parse_line, Progress, ProgressParser};
pub use runner::{
    describe_failure, timeout_for_priority, Exit, FallbackOutcome, FfmpegRunner, RunningProcess,
    FIRST_FRAME_WINDOW,
};
pub use temp::{dir_size, TempDirGuard};

use std::sync::Arc;

use whatmedia_core::{
    HardwareAccelerationProvider, HardwareKind, HardwareProfile, HdrType, VideoCodec,
};
use whatmedia_hardware::{
    MemoryInfo, NvencAccel, QsvAccel, RkmppAccel, SoftwareAccel, VaapiAccel, VideoToolboxAccel,
};

pub use whatmedia_hardware::AccelContext;

/// 由后端类型反查 provider 实例。
///
/// 决策层只持有 `HardwareProfile`（core 类型），不该 import hardware crate
/// 去 new 具体实现——那会把"挑后端"和"造后端"搅在一起。
pub fn provider_for(
    kind: HardwareKind,
    ctx: &AccelContext,
) -> Arc<dyn HardwareAccelerationProvider> {
    let mem = MemoryInfo {
        total_bytes: ctx.memory_bytes,
    };
    match kind {
        HardwareKind::RockchipMpp => Arc::new(RkmppAccel),
        HardwareKind::NvidiaNvenc => Arc::new(NvencAccel::default()),
        HardwareKind::IntelQsv => Arc::new(QsvAccel::new(ctx.render_node.clone())),
        HardwareKind::AmdVaapi => Arc::new(VaapiAccel::new(ctx.render_node.clone())),
        HardwareKind::AppleVideoToolbox => Arc::new(VideoToolboxAccel),
        _ => Arc::new(SoftwareAccel::new(ctx.cpu_cores.max(1), &mem)),
    }
}

/// 后端候选链：能编目标格式的按探测顺序排在前面，**软件永远压在最后**。
///
/// 返回的列表**保证以软件档收尾**（P5 DoD：硬件转码失败时静默降级）。
/// 只有硬件的机器也必须补一个软件档，否则降级链会在最后一级断掉。
pub fn accel_chain(
    profiles: &[HardwareProfile],
    ctx: &AccelContext,
    target: &VideoCodec,
    width: Option<u32>,
    height: Option<u32>,
    tonemap_from: Option<HdrType>,
) -> Vec<AccelArgs> {
    let mut out: Vec<AccelArgs> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    for p in profiles {
        if !p.encoders.contains(target) {
            continue;
        }
        let a = accel_args_for(
            &*provider_for(p.kind, ctx),
            target,
            width,
            height,
            tonemap_from,
        );
        if seen.contains(&a.id) {
            continue;
        }
        seen.push(a.id.clone());
        out.push(a);
    }

    // 兜底：软件档必须在链尾，且只放一次。
    if !seen.iter().any(|id| id == "software") {
        let sw = SoftwareAccel::new(
            ctx.cpu_cores.max(1),
            &MemoryInfo {
                total_bytes: ctx.memory_bytes,
            },
        );
        out.push(accel_args_for(&sw, target, width, height, tonemap_from));
    }
    out
}

/// 把 `HardwareAccelerationProvider` 的参数片段打包成 `AccelArgs`。
///
/// 目标编码格式由决策引擎定，硬件后端只提供"怎么编"。
/// 硬件不支持目标格式时**返回软件档而不是报错** —— 降级是常态，不是异常。
pub fn accel_args_for(
    accel: &dyn HardwareAccelerationProvider,
    target: &VideoCodec,
    width: Option<u32>,
    height: Option<u32>,
    tonemap_from: Option<whatmedia_core::HdrType>,
) -> AccelArgs {
    // 软件后端不给编码/解码参数：让 args.rs 走它的默认软件档，避免两处各写一套。
    // 硬件不支持目标格式时也一样退化 —— 降级是常态，不是异常。
    let use_hw = accel.supports_encode(target) && accel.kind() != HardwareKind::Software;
    AccelArgs {
        id: accel.id().to_string(),
        decoder: if use_hw {
            accel.decoder_args(target)
        } else {
            Vec::new()
        },
        encoder: if use_hw {
            accel.encoder_args(target)
        } else {
            Vec::new()
        },
        scale: match (width, height) {
            (Some(w), Some(h)) => Some(
                accel
                    .scale_filter(w, h)
                    .unwrap_or_else(|| format!("scale={w}:{h}")),
            ),
            _ => None,
        },
        tonemap: tonemap_from.and_then(|from| accel.tonemap_filter(from, HdrType::Sdr)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatmedia_core::HdrType;
    use whatmedia_hardware::{SoftwareAccel, VideoToolboxAccel};

    fn mem() -> whatmedia_hardware::MemoryInfo {
        whatmedia_hardware::MemoryInfo {
            total_bytes: 8 << 30,
        }
    }

    #[test]
    fn software_accel_has_no_hw_decoder_args() {
        let sw = SoftwareAccel::new(8, &mem());
        let a = accel_args_for(&sw, &VideoCodec::H264, Some(1280), Some(720), None);
        assert_eq!(a.id, "software");
        assert!(a.decoder.is_empty());
        assert!(a.encoder.is_empty());
        assert_eq!(a.scale.as_deref(), Some("scale=1280:720"));
    }

    #[test]
    fn hw_accel_falls_back_to_software_encoder_when_target_unsupported() {
        // VideoToolbox 不能编 AV1 → 参数应当退化，而不是生成一条跑不起来的命令。
        let vt = VideoToolboxAccel;
        let a = accel_args_for(&vt, &VideoCodec::Av1, None, None, None);
        assert_eq!(a.id, "videotoolbox");
        assert!(a.encoder.is_empty(), "不支持的目标格式不应产出编码器参数");
        assert!(a.decoder.is_empty());
    }

    #[test]
    fn tonemap_is_none_for_sdr_source() {
        let sw = SoftwareAccel::new(8, &mem());
        let a = accel_args_for(&sw, &VideoCodec::H264, None, None, Some(HdrType::Sdr));
        // SDR→SDR 不需要任何滤镜。
        assert!(a.tonemap.is_none());
    }

    fn profile(kind: HardwareKind, enc: Vec<VideoCodec>) -> HardwareProfile {
        HardwareProfile {
            kind,
            decoders: enc.clone(),
            encoders: enc,
            supports_zero_copy: true,
            supports_tone_mapping: true,
            max_concurrent_sessions: 2,
        }
    }

    #[test]
    fn chain_puts_software_last_even_when_only_hw_probed() {
        // P5 DoD：降级链的最后一级必须是软件，否则"硬件失败"就等于"播不了"。
        let ctx = AccelContext {
            render_node: Some("/dev/dri/renderD128".into()),
            cpu_cores: 8,
            memory_bytes: 8 << 30,
        };
        let chain = accel_chain(
            &[profile(HardwareKind::AmdVaapi, vec![VideoCodec::H264])],
            &ctx,
            &VideoCodec::H264,
            Some(1280),
            Some(720),
            None,
        );
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].id, "vaapi");
        assert_eq!(chain[1].id, "software");
        // VAAPI 的设备参数要靠 context 带下来，不能拍脑袋写死。
        assert!(chain[0]
            .decoder
            .contains(&"/dev/dri/renderD128".to_string()));
    }

    #[test]
    fn chain_skips_backends_that_cannot_encode_target() {
        // VideoToolbox 编不出 AV1，不该出现在候选链里（否则白试一次）。
        let ctx = AccelContext::default();
        let chain = accel_chain(
            &[
                profile(HardwareKind::AppleVideoToolbox, vec![VideoCodec::H264]),
                profile(HardwareKind::Software, vec![VideoCodec::H264]),
            ],
            &ctx,
            &VideoCodec::Av1,
            None,
            None,
            None,
        );
        assert_eq!(chain.len(), 1, "只剩软件档");
        assert_eq!(chain[0].id, "software");
    }

    #[test]
    fn chain_does_not_duplicate_software() {
        let ctx = AccelContext::default();
        let chain = accel_chain(
            &[profile(HardwareKind::Software, vec![VideoCodec::H264])],
            &ctx,
            &VideoCodec::H264,
            None,
            None,
            None,
        );
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].id, "software");
    }
}
