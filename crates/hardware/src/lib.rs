//! 硬件能力探测：CPU / 内存 / 设备节点 → `HardwareProfile`。
//!
//! 本 crate 回答两个问题：
//!   1. 这台机器能跑什么？（`HardwareProfile`）
//!   2. 要达成某个目标该往 ffmpeg 命令行里追加什么？（`decoder_args` / `encoder_args` / `*_filter`）
//!
//! 它**不负责**决定"该不该转码"——那是 `whatmedia-playback` 的决策引擎的事。
//! 这里只提供能力事实，避免"能力探测"与"策略判断"搅在一起。

pub mod accel;
pub mod env;
pub mod profile;

pub use accel::{
    build_accels, detect_backends, is_relevant, report, HardwareReport, NvencAccel, QsvAccel,
    RkmppAccel, SoftwareAccel, VaapiAccel, VideoToolboxAccel,
};
pub use env::{FakeEnv, Os, ProbeEnv, RealEnv};
pub use profile::{
    detect_cpu, detect_devices, detect_machine, detect_memory, CpuInfo, DeviceNodes,
    MachineProfile, MemoryInfo,
};

use whatmedia_core::HardwareProfile;

/// 一次性拿到整机画像与后端列表。启动自检用。
pub fn probe_all(env: &dyn ProbeEnv) -> (MachineProfile, Vec<HardwareProfile>) {
    let machine = detect_machine(env);
    let profiles = build_accels(&machine)
        .iter()
        .filter_map(|a| a.probe().ok())
        .collect();
    (machine, profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_all_never_returns_empty_backends() {
        // "没有任何硬件"的机器上必须至少还有软件后端 —— 降级链不能断。
        let env = FakeEnv::new().with_os(Os::Linux);
        let (_, profiles) = probe_all(&env);
        assert!(!profiles.is_empty());
        assert_eq!(
            profiles.last().unwrap().kind,
            whatmedia_core::HardwareKind::Software
        );
    }
}
