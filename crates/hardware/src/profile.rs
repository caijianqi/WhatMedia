//! CPU / 内存 / 设备节点的探测。
//!
//! 所有 IO 都走 `ProbeEnv`，因此"在 R5S 上应该识别成什么"是可测的断言，
//! 而不是"等我把机器接上再看"。

use serde::{Deserialize, Serialize};

use crate::env::{Os, ProbeEnv};

/// CPU 摘要。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CpuInfo {
    pub model: Option<String>,
    pub vendor: Option<String>,
    pub cores: usize,
    /// 是否支持 x86 的 SSE4.1/AVX2 之类的加速。ARM 上是 NEON。
    ///
    /// 这个字段目前只用于报告；真正的加速由 ffmpeg 自己决定。
    pub simd: Vec<String>,
}

/// 内存摘要（字节）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
}

/// 设备节点探测结果。
///
/// 只判断"节点在不在"，不去打开设备 —— 打开设备需要权限，
/// 而"探测"这件事本身必须能在没有权限的情况下跑完（§47）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceNodes {
    /// `/dev/dri/renderD*`。VA-API 与 QSV 都需要它。
    pub drm_render_nodes: Vec<String>,
    /// `/dev/dri/card*`。
    pub drm_cards: Vec<String>,
    /// `/dev/nvidia*`。
    pub nvidia: Vec<String>,
    /// `/dev/mpp_service`。Rockchip MPP 的用户态入口。
    pub mpp_service: bool,
    /// `/dev/rga`。Rockchip 2D 加速（缩放 / 色彩转换）。
    pub rga: bool,
}

impl DeviceNodes {
    pub fn has_drm(&self) -> bool {
        !self.drm_render_nodes.is_empty() || !self.drm_cards.is_empty()
    }

    pub fn has_nvidia(&self) -> bool {
        !self.nvidia.is_empty()
    }

    pub fn has_rkmp_p(&self) -> bool {
        self.mpp_service
    }
}

/// 整机画像。诊断页与 `hardware/profile` 端点都返回它。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MachineProfile {
    pub os: String,
    pub arch: String,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub devices: DeviceNodes,
}

/// 探测 CPU。三家路径完全不同：
///   · Linux    /proc/cpuinfo
///   · macOS    sysctl
///   · Windows  PROCESSOR_IDENTIFIER 环境变量
pub fn detect_cpu(env: &dyn ProbeEnv) -> CpuInfo {
    let cores = env.cpu_count();
    let (model, vendor) = match env.os() {
        Os::Linux => {
            let raw = env.read_file("/proc/cpuinfo").unwrap_or_default();
            let model = raw
                .lines()
                .find(|l| l.starts_with("model name") || l.starts_with("Hardware"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string());
            let vendor = raw
                .lines()
                .find(|l| l.starts_with("vendor_id"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string());
            (model, vendor)
        }
        Os::Macos => {
            let model = env
                .run_cmd("sysctl", &["-n", "machdep.cpu.brand_string"])
                .map(|s| s.trim().to_string());
            // macOS 只有 Apple Silicon 与 Intel 两种，vendor 从 model 里推。
            let vendor = model.as_deref().map(|m| {
                if m.contains("Apple") {
                    "Apple".to_string()
                } else {
                    "GenuineIntel".to_string()
                }
            });
            (model, vendor)
        }
        Os::Windows | Os::Other => {
            let raw = env.env_var("PROCESSOR_IDENTIFIER").unwrap_or_default();
            let vendor = if raw.contains("Intel") {
                Some("GenuineIntel".to_string())
            } else if raw.contains("AMD") {
                Some("AuthenticAMD".to_string())
            } else {
                None
            };
            let model = if raw.is_empty() { None } else { Some(raw) };
            (model, vendor)
        }
    };

    let mut simd = Vec::new();
    if matches!(env.os(), Os::Linux) {
        let flags = env
            .read_file("/proc/cpuinfo")
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("Features") || l.starts_with("flags"))
            .map(|l| l.to_string())
            .unwrap_or_default();
        for f in ["avx2", "sse4_1", "neon", "asimd"] {
            if flags.contains(f) {
                simd.push(f.to_string());
            }
        }
    } else if std::env::consts::ARCH == "aarch64" {
        // ARM64 一定有 NEON（ARMv8 基线）。
        simd.push("neon".to_string());
    }

    CpuInfo {
        model,
        vendor,
        cores,
        simd,
    }
}

/// 探测物理内存总量。探不到就返回 0 —— 上层据此降级而不是崩掉。
///
/// **Windows 上为什么主用 PowerShell 而不是 `wmic`**：
/// `wmic` 从 Windows 11 24H2 / Server 2025 起被移除，在相当一部分现役机器上
/// `wmic` 直接不存在（实测本机即如此）。只用它，内存就会恒为 0，
/// 于是软件转码并发数被压到最低档 —— 一个**不会报错、只会悄悄变慢**的退化。
/// 所以先试 PowerShell CIM，`wmic` 只作回退。
pub fn detect_memory(env: &dyn ProbeEnv) -> MemoryInfo {
    let bytes = match env.os() {
        Os::Linux => env
            .read_file("/proc/meminfo")
            .and_then(|raw| {
                raw.lines()
                    .find(|l| l.starts_with("MemTotal"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .map(|kb| kb * 1024),
        Os::Macos => env
            .run_cmd("sysctl", &["-n", "hw.memsize"])
            .and_then(|s| s.trim().parse::<u64>().ok()),
        Os::Windows | Os::Other => detect_memory_windows(env),
    };
    MemoryInfo {
        total_bytes: bytes.unwrap_or(0),
    }
}

/// Windows 内存探测：PowerShell CIM 优先，`wmic` 回退，都失败才认命返回 None。
fn detect_memory_windows(env: &dyn ProbeEnv) -> Option<u64> {
    // 取最后一行里第一串数字：PowerShell 可能先吐出进度/空行，
    // 硬编码 `lines().nth(1)` 在带横幅输出的环境里会读到空串。
    if let Some(out) = env.run_cmd(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory",
        ],
    ) {
        if let Some(v) = out.lines().rev().find_map(|l| {
            l.split_whitespace()
                .next()
                .and_then(|t| t.parse::<u64>().ok())
        }) {
            return Some(v);
        }
    }

    env.run_cmd("wmic", &["computersystem", "get", "TotalPhysicalMemory"])
        .and_then(|s| s.lines().nth(1).and_then(|l| l.trim().parse::<u64>().ok()))
}

/// 探测设备节点。
pub fn detect_devices(env: &dyn ProbeEnv) -> DeviceNodes {
    let mut nodes = DeviceNodes::default();

    if env.path_exists("/dev/dri") {
        for name in env.list_dir("/dev/dri") {
            if name.starts_with("renderD") {
                nodes.drm_render_nodes.push(format!("/dev/dri/{name}"));
            } else if name.starts_with("card") {
                nodes.drm_cards.push(format!("/dev/dri/{name}"));
            }
        }
    }

    // NVIDIA 设备节点在容器里不一定存在（只挂了驱动库），
    // 所以再退一步看 nvidia-smi 能不能跑。
    if env.path_exists("/dev") {
        for name in env.list_dir("/dev") {
            if name.starts_with("nvidia") {
                nodes.nvidia.push(format!("/dev/{name}"));
            }
        }
    }
    if nodes.nvidia.is_empty() && env.run_cmd("nvidia-smi", &["-L"]).is_some() {
        nodes.nvidia.push("nvidia-smi".to_string());
    }

    nodes.mpp_service = env.path_exists("/dev/mpp_service");
    nodes.rga = env.path_exists("/dev/rga");

    nodes
}

pub fn detect_machine(env: &dyn ProbeEnv) -> MachineProfile {
    MachineProfile {
        os: env.os().as_str().to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu: detect_cpu(env),
        memory: detect_memory(env),
        devices: detect_devices(env),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{FakeEnv, Os};

    const X86_CPUINFO: &str = "\
processor\t: 0
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i5-8250U CPU @ 1.60GHz
flags\t\t: fpu vme sse4_1 avx2
";

    const RK_CPUINFO: &str = "\
processor\t: 0
Hardware\t: Rockchip RK3588
Features\t: fp asimd neon
";

    #[test]
    fn intel_linux_cpu_is_recognized() {
        let env = FakeEnv::new()
            .with_os(Os::Linux)
            .with_file("/proc/cpuinfo", X86_CPUINFO);
        let cpu = detect_cpu(&env);
        assert_eq!(cpu.vendor.as_deref(), Some("GenuineIntel"));
        assert!(cpu.model.unwrap().contains("i5-8250U"));
        assert!(cpu.simd.contains(&"avx2".to_string()));
    }

    #[test]
    fn rockchip_cpu_is_recognized() {
        let env = FakeEnv::new()
            .with_os(Os::Linux)
            .with_file("/proc/cpuinfo", RK_CPUINFO);
        let cpu = detect_cpu(&env);
        assert!(cpu.model.unwrap().contains("RK3588"));
        assert!(cpu.simd.contains(&"neon".to_string()));
    }

    #[test]
    fn macos_uses_sysctl() {
        let env = FakeEnv::new()
            .with_os(Os::Macos)
            .with_cmd("sysctl", "Apple M2 Pro\n");
        let cpu = detect_cpu(&env);
        assert_eq!(cpu.vendor.as_deref(), Some("Apple"));
        assert!(cpu.model.unwrap().contains("M2"));
    }

    #[test]
    fn windows_uses_env_var() {
        let env = FakeEnv::new()
            .with_os(Os::Windows)
            .with_env("PROCESSOR_IDENTIFIER", "Intel64 Family 6 Model 158");
        let cpu = detect_cpu(&env);
        assert_eq!(cpu.vendor.as_deref(), Some("GenuineIntel"));
    }

    #[test]
    fn memory_parses_meminfo_kb() {
        let env = FakeEnv::new().with_os(Os::Linux).with_file(
            "/proc/meminfo",
            "MemTotal:       16384000 kB\nMemFree: 1 kB\n",
        );
        assert_eq!(detect_memory(&env).total_bytes, 16_384_000 * 1024);
    }

    #[test]
    fn memory_missing_file_is_zero_not_panic() {
        let env = FakeEnv::new().with_os(Os::Linux);
        assert_eq!(detect_memory(&env).total_bytes, 0);
    }

    #[test]
    fn windows_memory_uses_powershell_instead_of_wmic() {
        // wmic 从 Win11 24H2 起被移除。只依赖它会让内存恒为 0，
        // 进而把软件转码并发压到最低 —— 一个不会报错、只会悄悄变慢的退化。
        let env = FakeEnv::new()
            .with_os(Os::Windows)
            .with_cmd("powershell", "34359738368\n");
        assert_eq!(detect_memory(&env).total_bytes, 34_359_738_368);
    }

    #[test]
    fn windows_memory_falls_back_to_wmic() {
        let env = FakeEnv::new()
            .with_os(Os::Windows)
            .with_cmd("wmic", "TotalPhysicalMemory  \r\n34359738368  \r\n");
        assert_eq!(detect_memory(&env).total_bytes, 34_359_738_368);
    }

    #[test]
    fn windows_memory_tolerates_powershell_banner_lines() {
        // PowerShell 有时先吐一行空行或进度条；硬编码 nth(1) 会读到空串。
        let env = FakeEnv::new()
            .with_os(Os::Windows)
            .with_cmd("powershell", "\n\n34359738368\n\n");
        assert_eq!(detect_memory(&env).total_bytes, 34_359_738_368);
    }

    #[test]
    fn r5s_devices_are_detected() {
        // R5S 的特征：有 /dev/dri（罗嗦的 panfrost 也会给），有 mpp_service 与 rga。
        let env = FakeEnv::new()
            .with_os(Os::Linux)
            .with_dir("/dev/dri", &["card0", "renderD128"])
            .with_path("/dev/mpp_service", true)
            .with_path("/dev/rga", true);
        let dev = detect_devices(&env);
        assert!(dev.has_drm());
        assert_eq!(dev.drm_render_nodes, vec!["/dev/dri/renderD128"]);
        assert!(dev.has_rkmp_p());
        assert!(dev.rga);
        assert!(!dev.has_nvidia());
    }

    #[test]
    fn nvidia_detected_via_smi_when_nodes_missing() {
        // 容器里常见：只挂了驱动库，没有 /dev/nvidia*。
        let env = FakeEnv::new()
            .with_os(Os::Linux)
            .with_cmd("nvidia-smi", "GPU 0: NVIDIA GeForce RTX 3060\n");
        let dev = detect_devices(&env);
        assert!(dev.has_nvidia());
        assert_eq!(dev.nvidia, vec!["nvidia-smi"]);
    }

    #[test]
    fn machine_profile_composes() {
        let env = FakeEnv::new()
            .with_os(Os::Linux)
            .with_file("/proc/cpuinfo", X86_CPUINFO)
            .with_dir("/dev/dri", &["renderD128"])
            .with_cpus(8);
        let m = detect_machine(&env);
        assert_eq!(m.os, "linux");
        assert_eq!(m.cpu.cores, 8);
        assert!(m.devices.has_drm());
    }
}
