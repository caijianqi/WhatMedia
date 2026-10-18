//! `HardwareAccelerationProvider` 的六个实现 + 探测选择逻辑。
//!
//! **两条容易被搞错的事，先写在这儿**：
//!
//! 1. **软件后端必须永远可用。** 硬件转码失败要能静默降级，
//!    如果"没有硬件"时 `detect` 返回空列表，降级链就断了。
//! 2. **ARM64 支持 ≠ RKMPP。** 在 x86 上跑的是 VAAPI / QSV / NVENC，
//!    在 RK3588 上才是 RKMPP。别把"ARM 构建通过"当成"硬件加速可用"。

use whatmedia_core::{
    HardwareAccelerationProvider, HardwareKind, HardwareProfile, HdrType, Result, VideoCodec,
};

use crate::env::Os;
use crate::profile::{MachineProfile, MemoryInfo};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────
// 软件后端
// ─────────────────────────────────────────────────────────────

/// 软件转码。任何环境都必须存在。
#[derive(Debug, Clone)]
pub struct SoftwareAccel {
    max_sessions: u32,
}

impl SoftwareAccel {
    pub fn new(cores: usize, memory: &MemoryInfo) -> Self {
        // 软件转码是纯 CPU 活：一路 1080p 大约吃掉 2~3 个核。
        // 内存太小（<1.5GB）时只给一路，R5S 4GB 版就得靠这个限制。
        let by_cpu = (cores / 3).max(1) as u32;
        // 内存也要卡一道：一路软件转码（含滤镜缓冲）常驻约 768MB，
        // 还要给系统留 512MB。R5S 那种 1.5GB 的小内存机器靠这一条被压到 1 路。
        let by_mem = if memory.total_bytes > 0 {
            let usable = memory.total_bytes.saturating_sub(512 * 1024 * 1024);
            (usable / (768 * 1024 * 1024)).max(1) as u32
        } else {
            by_cpu
        };
        Self {
            max_sessions: by_cpu.min(by_mem).clamp(1, 4),
        }
    }
}

impl HardwareAccelerationProvider for SoftwareAccel {
    fn id(&self) -> &str {
        "software"
    }

    fn kind(&self) -> HardwareKind {
        HardwareKind::Software
    }

    fn probe(&self) -> Result<HardwareProfile> {
        Ok(HardwareProfile {
            kind: HardwareKind::Software,
            decoders: vec![
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Av1,
                VideoCodec::Vp9,
                VideoCodec::Mpeg2,
                VideoCodec::Mpeg4,
                VideoCodec::Vc1,
            ],
            // 只编 H264。libx265 在 ARM 上慢到没有实用价值，
            // 与其给一个"能跑但卡死"的选项，不如不给。
            encoders: vec![VideoCodec::H264],
            supports_zero_copy: false,
            supports_tone_mapping: true,
            max_concurrent_sessions: self.max_sessions,
        })
    }

    fn supports_decode(&self, _codec: &VideoCodec) -> bool {
        // 软件解码器对没见过的 codec 也常常能解（ffmpeg 覆盖面远大于本枚举）。
        true
    }

    fn supports_encode(&self, codec: &VideoCodec) -> bool {
        matches!(codec, VideoCodec::H264)
    }

    fn decoder_args(&self, _codec: &VideoCodec) -> Vec<String> {
        Vec::new()
    }

    fn encoder_args(&self, codec: &VideoCodec) -> Vec<String> {
        match codec {
            VideoCodec::H264 => vec![
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "veryfast".into(),
                "-crf".into(),
                "23".into(),
            ],
            _ => vec!["-c:v".into(), "libx264".into()],
        }
    }

    fn scale_filter(&self, width: u32, height: u32) -> Option<String> {
        Some(format!("scale={width}:{height}"))
    }

    fn tonemap_filter(&self, from: HdrType, to: HdrType) -> Option<String> {
        if from == to || matches!(from, HdrType::Sdr) {
            return None;
        }
        // HDR→SDR 的标准 zscale 链。CPU 上很贵，但它是唯一"到处都能跑"的方案。
        Some(
            "zscale=t=linear:npl=100,format=gbrpf32le,zscale=p=bt709,tonemap=linear:desat=0,\
             zscale=t=bt709:m=bt709:r=tv,format=yuv420p"
                .to_string(),
        )
    }
}

// ─────────────────────────────────────────────────────────────
// VA-API（Linux：Intel / AMD / 部分 ARM）
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct VaapiAccel {
    render_node: Option<String>,
}

impl VaapiAccel {
    pub fn new(render_node: Option<String>) -> Self {
        Self { render_node }
    }

    fn device_args(&self) -> Vec<String> {
        match &self.render_node {
            Some(n) => vec!["-vaapi_device".into(), n.clone()],
            None => Vec::new(),
        }
    }
}

impl HardwareAccelerationProvider for VaapiAccel {
    fn id(&self) -> &str {
        "vaapi"
    }

    fn kind(&self) -> HardwareKind {
        HardwareKind::AmdVaapi
    }

    fn probe(&self) -> Result<HardwareProfile> {
        Ok(HardwareProfile {
            kind: HardwareKind::AmdVaapi,
            decoders: vec![
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Vp9,
                VideoCodec::Mpeg2,
                VideoCodec::Vc1,
            ],
            encoders: vec![VideoCodec::H264, VideoCodec::Hevc],
            supports_zero_copy: true,
            supports_tone_mapping: true,
            max_concurrent_sessions: 4,
        })
    }

    fn supports_decode(&self, codec: &VideoCodec) -> bool {
        matches!(
            codec,
            VideoCodec::H264
                | VideoCodec::Hevc
                | VideoCodec::Vp9
                | VideoCodec::Mpeg2
                | VideoCodec::Vc1
        )
    }

    fn supports_encode(&self, codec: &VideoCodec) -> bool {
        matches!(codec, VideoCodec::H264 | VideoCodec::Hevc)
    }

    fn decoder_args(&self, _codec: &VideoCodec) -> Vec<String> {
        let mut a = vec!["-hwaccel".into(), "vaapi".into()];
        a.extend(self.device_args());
        a
    }

    fn encoder_args(&self, codec: &VideoCodec) -> Vec<String> {
        let name = match codec {
            VideoCodec::Hevc => "hevc_vaapi",
            _ => "h264_vaapi",
        };
        let mut a = vec!["-c:v".into(), name.into()];
        a.extend(self.device_args());
        a
    }

    fn scale_filter(&self, width: u32, height: u32) -> Option<String> {
        Some(format!("format=nv12,hwupload,scale_vaapi={width}:{height}"))
    }

    fn tonemap_filter(&self, from: HdrType, to: HdrType) -> Option<String> {
        if from == to || matches!(from, HdrType::Sdr) {
            return None;
        }
        Some("format=nv12,hwupload,tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709".to_string())
    }
}

// ─────────────────────────────────────────────────────────────
// NVENC / NVDEC
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct NvencAccel {
    /// 消费级驱动的并发编码路数限制（GeForce 是 3 / 5，专业卡不限）。
    pub session_limit: Option<u32>,
}

impl HardwareAccelerationProvider for NvencAccel {
    fn id(&self) -> &str {
        "nvenc"
    }

    fn kind(&self) -> HardwareKind {
        HardwareKind::NvidiaNvenc
    }

    fn probe(&self) -> Result<HardwareProfile> {
        Ok(HardwareProfile {
            kind: HardwareKind::NvidiaNvenc,
            decoders: vec![
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Av1,
                VideoCodec::Vp9,
                VideoCodec::Mpeg2,
                VideoCodec::Vc1,
            ],
            encoders: vec![VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1],
            supports_zero_copy: true,
            supports_tone_mapping: true,
            // 不知道型号时按消费级最保守值给。
            max_concurrent_sessions: self.session_limit.unwrap_or(3),
        })
    }

    fn supports_decode(&self, codec: &VideoCodec) -> bool {
        !matches!(codec, VideoCodec::Mpeg4)
    }

    fn supports_encode(&self, codec: &VideoCodec) -> bool {
        matches!(codec, VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::Av1)
    }

    fn decoder_args(&self, _codec: &VideoCodec) -> Vec<String> {
        vec![
            "-hwaccel".into(),
            "cuda".into(),
            "-hwaccel_output_format".into(),
            "cuda".into(),
        ]
    }

    fn encoder_args(&self, codec: &VideoCodec) -> Vec<String> {
        let name = match codec {
            VideoCodec::Hevc => "hevc_nvenc",
            VideoCodec::Av1 => "av1_nvenc",
            _ => "h264_nvenc",
        };
        vec![
            "-c:v".into(),
            name.into(),
            "-preset".into(),
            "p4".into(),
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            "23".into(),
        ]
    }

    fn scale_filter(&self, width: u32, height: u32) -> Option<String> {
        Some(format!("scale_cuda={width}:{height}"))
    }

    fn tonemap_filter(&self, from: HdrType, to: HdrType) -> Option<String> {
        if from == to || matches!(from, HdrType::Sdr) {
            return None;
        }
        Some("tonemap_cuda=p=bt709:t=bt709:m=bt709:tonemap=linear:format=nv12".to_string())
    }
}

// ─────────────────────────────────────────────────────────────
// Intel QSV
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct QsvAccel {
    render_node: Option<String>,
}

impl QsvAccel {
    pub fn new(render_node: Option<String>) -> Self {
        Self { render_node }
    }
}

impl HardwareAccelerationProvider for QsvAccel {
    fn id(&self) -> &str {
        "qsv"
    }

    fn kind(&self) -> HardwareKind {
        HardwareKind::IntelQsv
    }

    fn probe(&self) -> Result<HardwareProfile> {
        Ok(HardwareProfile {
            kind: HardwareKind::IntelQsv,
            decoders: vec![
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Vp9,
                VideoCodec::Av1,
                VideoCodec::Mpeg2,
                VideoCodec::Vc1,
            ],
            encoders: vec![VideoCodec::H264, VideoCodec::Hevc, VideoCodec::Av1],
            supports_zero_copy: true,
            supports_tone_mapping: true,
            max_concurrent_sessions: 4,
        })
    }

    fn supports_decode(&self, codec: &VideoCodec) -> bool {
        !matches!(codec, VideoCodec::Mpeg4)
    }

    fn supports_encode(&self, codec: &VideoCodec) -> bool {
        matches!(codec, VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::Av1)
    }

    fn decoder_args(&self, _codec: &VideoCodec) -> Vec<String> {
        let mut a = vec!["-hwaccel".into(), "qsv".into()];
        if let Some(n) = &self.render_node {
            a.push("-qsv_device".into());
            a.push(n.clone());
        }
        a
    }

    fn encoder_args(&self, codec: &VideoCodec) -> Vec<String> {
        let name = match codec {
            VideoCodec::Hevc => "hevc_qsv",
            VideoCodec::Av1 => "av1_qsv",
            _ => "h264_qsv",
        };
        vec!["-c:v".into(), name.into()]
    }

    fn scale_filter(&self, width: u32, height: u32) -> Option<String> {
        Some(format!("scale_qsv={width}:{height}"))
    }

    fn tonemap_filter(&self, from: HdrType, to: HdrType) -> Option<String> {
        if from == to || matches!(from, HdrType::Sdr) {
            return None;
        }
        Some("vpp_qsv=tonemap=1:format=nv12".to_string())
    }
}

// ─────────────────────────────────────────────────────────────
// Apple VideoToolbox
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct VideoToolboxAccel;

impl HardwareAccelerationProvider for VideoToolboxAccel {
    fn id(&self) -> &str {
        "videotoolbox"
    }

    fn kind(&self) -> HardwareKind {
        HardwareKind::AppleVideoToolbox
    }

    fn probe(&self) -> Result<HardwareProfile> {
        Ok(HardwareProfile {
            kind: HardwareKind::AppleVideoToolbox,
            decoders: vec![
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Av1,
                VideoCodec::Vp9,
                VideoCodec::Mpeg2,
            ],
            encoders: vec![VideoCodec::H264, VideoCodec::Hevc],
            supports_zero_copy: true,
            // VideoToolbox 没有 tonemap 滤镜，HDR→SDR 得退回 CPU 链路。
            supports_tone_mapping: false,
            max_concurrent_sessions: 4,
        })
    }

    fn supports_decode(&self, codec: &VideoCodec) -> bool {
        !matches!(codec, VideoCodec::Vc1 | VideoCodec::Mpeg4)
    }

    fn supports_encode(&self, codec: &VideoCodec) -> bool {
        matches!(codec, VideoCodec::H264 | VideoCodec::Hevc)
    }

    fn decoder_args(&self, _codec: &VideoCodec) -> Vec<String> {
        vec!["-hwaccel".into(), "videotoolbox".into()]
    }

    fn encoder_args(&self, codec: &VideoCodec) -> Vec<String> {
        let name = match codec {
            VideoCodec::Hevc => "hevc_videotoolbox",
            _ => "h264_videotoolbox",
        };
        vec!["-c:v".into(), name.into()]
    }

    fn scale_filter(&self, width: u32, height: u32) -> Option<String> {
        Some(format!("scale={width}:{height}"))
    }

    fn tonemap_filter(&self, _from: HdrType, _to: HdrType) -> Option<String> {
        // 明确返回 None：调用方据此回落到软件 tone mapping。
        // 宁可慢，也不要给出一条根本拼不出来的滤镜链。
        None
    }
}

// ─────────────────────────────────────────────────────────────
// Rockchip RKMPP
// ─────────────────────────────────────────────────────────────

/// Rockchip MPP。**FFmpeg 主线支持不完整**（§48），
/// 生产上可能要切到 ffmpeg-rockchip 后端，所以参数一律集中在这一处。
#[derive(Debug, Clone, Default)]
pub struct RkmppAccel;

impl HardwareAccelerationProvider for RkmppAccel {
    fn id(&self) -> &str {
        "rkmpp"
    }

    fn kind(&self) -> HardwareKind {
        HardwareKind::RockchipMpp
    }

    fn probe(&self) -> Result<HardwareProfile> {
        Ok(HardwareProfile {
            kind: HardwareKind::RockchipMpp,
            decoders: vec![
                VideoCodec::H264,
                VideoCodec::Hevc,
                VideoCodec::Vp9,
                VideoCodec::Av1,
            ],
            encoders: vec![VideoCodec::H264, VideoCodec::Hevc],
            supports_zero_copy: true,
            // MPP 本身不做 tone mapping，HDR→SDR 要走 RGA —— P5 再接。
            supports_tone_mapping: false,
            max_concurrent_sessions: 2,
        })
    }

    fn supports_decode(&self, codec: &VideoCodec) -> bool {
        matches!(
            codec,
            VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::Vp9 | VideoCodec::Av1
        )
    }

    fn supports_encode(&self, codec: &VideoCodec) -> bool {
        matches!(codec, VideoCodec::H264 | VideoCodec::Hevc)
    }

    fn decoder_args(&self, _codec: &VideoCodec) -> Vec<String> {
        vec![
            "-hwaccel".into(),
            "rkmpp".into(),
            "-hwaccel_output_format".into(),
            "drm_prime".into(),
        ]
    }

    fn encoder_args(&self, codec: &VideoCodec) -> Vec<String> {
        let name = match codec {
            VideoCodec::Hevc => "hevc_rkmpp",
            _ => "h264_rkmpp",
        };
        vec!["-c:v".into(), name.into()]
    }

    fn scale_filter(&self, width: u32, height: u32) -> Option<String> {
        Some(format!("scale_rkrga={width}:{height}"))
    }

    fn tonemap_filter(&self, _from: HdrType, _to: HdrType) -> Option<String> {
        None
    }
}

// ─────────────────────────────────────────────────────────────
// 探测与选择
// ─────────────────────────────────────────────────────────────

/// 探测结果：按优先级从高到低排列的可用后端。**列表永不为空**（至少含软件）。
#[derive(Debug, Clone)]
pub struct HardwareReport {
    pub machine: MachineProfile,
    pub profiles: Vec<(String, HardwareProfile)>,
}

impl HardwareReport {
    /// 首选后端。探测失败时静默降级到下一个。
    pub fn preferred(&self, accels: &[Arc<dyn HardwareAccelerationProvider>]) -> Option<usize> {
        self.profiles
            .first()
            .and_then(|(id, _)| accels.iter().position(|a| a.id() == id))
    }
}

/// 按机器画像推断可用的加速后端，优先级从高到低。
///
/// 顺序的依据：专用硬件 > 通用 GPU > CPU。
/// RKMPP 放在 VAAPI 之前是因为 RK3588 上也有 /dev/dri（panfrost），
/// 但 MPP 的解码能力明显更强。
///
/// **"有没有设备节点"不等于"能不能用"。** Windows 上没有 `/dev/dri`，
/// 但 Intel QSV 走 D3D11 一样能用 —— 只看设备节点会把整台机器的硬件能力
/// 判成 0，用户只会觉得"转码莫名其妙很慢"。所以候选先列全，
/// 再用 `is_relevant` 按 OS 砍掉真正不可能的那些。
pub fn detect_backends(m: &MachineProfile) -> Vec<String> {
    let os = match m.os.as_str() {
        "linux" => Os::Linux,
        "macos" => Os::Macos,
        "windows" => Os::Windows,
        _ => Os::Other,
    };

    let is_intel = m
        .cpu
        .vendor
        .as_deref()
        .map(|v| v.eq_ignore_ascii_case("GenuineIntel"))
        .unwrap_or(false);

    // (后端 id, 类型) —— 类型用来过 is_relevant。
    let mut candidates: Vec<(String, HardwareKind)> = Vec::new();

    if m.devices.mpp_service {
        candidates.push(("rkmpp".into(), HardwareKind::RockchipMpp));
    }
    if m.devices.has_nvidia() {
        candidates.push(("nvenc".into(), HardwareKind::NvidiaNvenc));
    }
    if is_intel && (m.devices.has_drm() || m.os == "windows") {
        candidates.push(("qsv".into(), HardwareKind::IntelQsv));
    }
    if m.devices.has_drm() {
        candidates.push(("vaapi".into(), HardwareKind::AmdVaapi));
    }
    if m.os == "macos" {
        candidates.push(("videotoolbox".into(), HardwareKind::AppleVideoToolbox));
    }
    // 软件永远在最后，保证降级链不会断。
    candidates.push(("software".into(), HardwareKind::Software));

    candidates
        .into_iter()
        .filter(|(_, k)| is_relevant(os, *k))
        .map(|(id, _)| id)
        .collect()
}

/// 构造探测到的后端实例（含软件兜底）。
pub fn build_accels(m: &MachineProfile) -> Vec<Arc<dyn HardwareAccelerationProvider>> {
    let render_node = m.devices.drm_render_nodes.first().cloned();
    detect_backends(m)
        .into_iter()
        .filter_map(|id| -> Option<Arc<dyn HardwareAccelerationProvider>> {
            match id.as_str() {
                "rkmpp" => Some(Arc::new(RkmppAccel)),
                "nvenc" => Some(Arc::new(NvencAccel::default())),
                "qsv" => Some(Arc::new(QsvAccel::new(render_node.clone()))),
                "vaapi" => Some(Arc::new(VaapiAccel::new(render_node.clone()))),
                "videotoolbox" => Some(Arc::new(VideoToolboxAccel)),
                "software" => Some(Arc::new(SoftwareAccel::new(m.cpu.cores, &m.memory))),
                _ => None,
            }
        })
        .collect()
}

/// 一次性探测整机 + 后端画像。启动自检与 `/hardware/profile` 都用它。
pub fn report(m: MachineProfile) -> HardwareReport {
    let accels = build_accels(&m);
    let profiles = accels
        .iter()
        .filter_map(|a| a.probe().ok().map(|p| (a.id().to_string(), p)))
        .collect();
    HardwareReport {
        machine: m,
        profiles,
    }
}

/// 判断某类硬件在该 OS 上是否可能存在。
///
/// **砍掉"不可能"的，留下的交给降级链兜底。** 探测是廉价的猜测，
/// ffmpeg 真跑起来才是事实：这条过滤只挡住"给了也一定跑不起来"的组合，
/// 剩下的（比如有 /dev/dri 但驱动坏了）由首帧窗口内的失败降级来接。
pub fn is_relevant(os: Os, kind: HardwareKind) -> bool {
    match (os, kind) {
        (_, HardwareKind::Software) => true,
        (Os::Macos, HardwareKind::AppleVideoToolbox) => true,
        (Os::Macos, _) => false,
        (Os::Linux, HardwareKind::AppleVideoToolbox) => false,
        // Windows 上没有 /dev/dri。实测 ffmpeg 在 Windows 上拿到
        // `-hwaccel vaapi` 会在 1 秒内 "Failed to initialise VAAPI connection"，
        // 与其每次起播都白试一次，不如一开始就不列。
        (Os::Windows, HardwareKind::AmdVaapi | HardwareKind::RockchipMpp) => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{FakeEnv, Os};
    use crate::profile::{detect_machine, DeviceNodes};
    use whatmedia_core::VideoCodec;

    fn machine(os: &str, devices: DeviceNodes, vendor: Option<&str>) -> MachineProfile {
        MachineProfile {
            os: os.to_string(),
            arch: "x86_64".to_string(),
            cpu: crate::profile::CpuInfo {
                model: None,
                vendor: vendor.map(|s| s.to_string()),
                cores: 8,
                simd: vec![],
            },
            memory: MemoryInfo {
                total_bytes: 8 << 30,
            },
            devices,
        }
    }

    #[test]
    fn software_is_always_available() {
        let accels = build_accels(&machine("linux", DeviceNodes::default(), None));
        assert_eq!(accels.last().unwrap().id(), "software");
        assert!(!accels.is_empty());
    }

    #[test]
    fn software_caps_concurrency_by_cpu_and_memory() {
        // 8 核 8GB → by_cpu = 2。R5S 这种 1.5GB 的小内存会被压到 1。
        let a = SoftwareAccel::new(
            8,
            &MemoryInfo {
                total_bytes: 8 << 30,
            },
        );
        assert_eq!(a.probe().unwrap().max_concurrent_sessions, 2);
        let b = SoftwareAccel::new(
            8,
            &MemoryInfo {
                total_bytes: 1536 << 20,
            },
        );
        assert_eq!(b.probe().unwrap().max_concurrent_sessions, 1);
    }

    #[test]
    fn nvidia_machine_prefers_nvenc() {
        let m = machine(
            "linux",
            DeviceNodes {
                nvidia: vec!["/dev/nvidia0".into()],
                ..Default::default()
            },
            None,
        );
        assert_eq!(detect_backends(&m)[0], "nvenc");
        let accels = build_accels(&m);
        let p = accels[0].probe().unwrap();
        assert_eq!(p.kind, HardwareKind::NvidiaNvenc);
        assert!(accels[0].supports_encode(&VideoCodec::Hevc));
    }

    #[test]
    fn intel_drm_machine_prefers_qsv_then_vaapi() {
        let m = machine(
            "linux",
            DeviceNodes {
                drm_render_nodes: vec!["/dev/dri/renderD128".into()],
                ..Default::default()
            },
            Some("GenuineIntel"),
        );
        let b = detect_backends(&m);
        assert_eq!(b[0], "qsv");
        assert!(b.contains(&"vaapi".to_string()));
        assert_eq!(b.last().unwrap(), "software");
    }

    #[test]
    fn rk3588_prefers_rkmpp() {
        // R5S 的画像：有 /dev/dri、有 mpp_service、有 rga。
        // 关键断言是 **RKMPP 排在 VAAPI 之前** —— 否则等于把 VPU 晾在一边用 CPU 软解。
        let m = machine(
            "linux",
            DeviceNodes {
                drm_render_nodes: vec!["/dev/dri/renderD128".into()],
                mpp_service: true,
                rga: true,
                ..Default::default()
            },
            None,
        );
        let b = detect_backends(&m);
        assert_eq!(b[0], "rkmpp");
        assert!(b.contains(&"vaapi".to_string()));
    }

    #[test]
    fn windows_intel_still_gets_qsv_without_drm() {
        // Windows 上没有 /dev/dri，只看设备节点会把 QSV 判成不存在。
        // 实测这台机器上 h264_qsv 是能出片的 —— 漏掉它等于白白放弃硬件转码。
        let m = machine("windows", DeviceNodes::default(), Some("GenuineIntel"));
        let b = detect_backends(&m);
        assert_eq!(b[0], "qsv");
        assert!(!b.contains(&"vaapi".to_string()), "Windows 上没有 VAAPI");
        assert_eq!(b.last().unwrap(), "software");
    }

    #[test]
    fn windows_amd_only_gets_software() {
        // 没有 Intel / NVIDIA 就没有可用硬件后端 —— 降级链只剩软件也不能断。
        let m = machine("windows", DeviceNodes::default(), Some("AuthenticAMD"));
        assert_eq!(detect_backends(&m), vec!["software"]);
    }

    #[test]
    fn vaapi_is_irrelevant_on_windows() {
        assert!(!is_relevant(Os::Windows, HardwareKind::AmdVaapi));
        assert!(!is_relevant(Os::Windows, HardwareKind::RockchipMpp));
        assert!(is_relevant(Os::Windows, HardwareKind::IntelQsv));
        assert!(is_relevant(Os::Linux, HardwareKind::AmdVaapi));
        // 软件在任何系统上都必须相关，否则降级链会断。
        assert!(is_relevant(Os::Windows, HardwareKind::Software));
    }

    #[test]
    fn macos_gets_videotoolbox() {
        let m = machine("macos", DeviceNodes::default(), Some("Apple"));
        let b = detect_backends(&m);
        assert!(b.contains(&"videotoolbox".to_string()));
        let accels = build_accels(&m);
        let vt = accels.iter().find(|a| a.id() == "videotoolbox").unwrap();
        // VT 没有 tonemap 滤镜 → 必须返回 None 让上层回落 CPU。
        assert!(vt.tonemap_filter(HdrType::Hdr10, HdrType::Sdr).is_none());
        assert!(!vt.probe().unwrap().supports_tone_mapping);
    }

    #[test]
    fn vaapi_args_include_device_node() {
        let a = VaapiAccel::new(Some("/dev/dri/renderD128".into()));
        let dec = a.decoder_args(&VideoCodec::Hevc);
        assert_eq!(dec[0], "-hwaccel");
        assert!(dec.contains(&"/dev/dri/renderD128".to_string()));
        let enc = a.encoder_args(&VideoCodec::H264);
        assert!(enc.contains(&"h264_vaapi".to_string()));
    }

    #[test]
    fn scale_and_tonemap_filters_are_wellformed() {
        let sw = SoftwareAccel::new(
            8,
            &MemoryInfo {
                total_bytes: 8 << 30,
            },
        );
        assert_eq!(sw.scale_filter(1280, 720).unwrap(), "scale=1280:720");
        assert!(sw
            .tonemap_filter(HdrType::Hdr10, HdrType::Sdr)
            .unwrap()
            .contains("tonemap"));
        // SDR→SDR 不需要任何滤镜，返回 None 避免多拼一条空链。
        assert!(sw.tonemap_filter(HdrType::Sdr, HdrType::Sdr).is_none());
    }

    #[test]
    fn report_lists_all_profiles() {
        let env = FakeEnv::new()
            .with_os(Os::Linux)
            .with_dir("/dev/dri", &["renderD128"])
            .with_file("/proc/cpuinfo", "vendor_id\t: GenuineIntel\n")
            .with_cpus(8);
        let m = detect_machine(&env);
        let r = report(m);
        assert!(r.profiles.iter().any(|(id, _)| id == "qsv"));
        assert!(r.profiles.iter().any(|(id, _)| id == "software"));
    }
}
