//! 找 mpv。
//!
//! **为什么做成可注入的**：`mpv 没装` 是绝大多数桌面端的真实状态，
//! 而"没装时应当降级到 Web 播放器"这条路径必须能测 ——
//! 不能只有装了 mpv 的机器才能验证。

use std::path::PathBuf;

pub use crate::ipc::IpcPath;

/// 操作系统。本 crate 不依赖 `whatmedia-hardware`：
/// 它要能被 Tauri 壳单独引用，不该顺带把硬件探测整包拖进来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
    Other,
}

impl Os {
    pub fn from_target() -> Self {
        if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }
    pub fn exe_suffix(self) -> &'static str {
        match self {
            Os::Windows => ".exe",
            _ => "",
        }
    }
}

/// 环境副作用。
pub trait MpvEnv {
    fn os(&self) -> Os;
    fn env_var(&self, key: &str) -> Option<String>;
    fn path_exists(&self, path: &str) -> bool;
    /// 跑一条命令拿 stdout。不存在 / 失败一律 None。
    fn run_cmd(&self, bin: &str, args: &[&str]) -> Option<String>;
}

/// 真实环境。
pub struct RealMpvEnv;

impl MpvEnv for RealMpvEnv {
    fn os(&self) -> Os {
        Os::from_target()
    }
    fn env_var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
    fn path_exists(&self, path: &str) -> bool {
        std::path::Path::new(path).exists()
    }
    fn run_cmd(&self, bin: &str, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new(bin).args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8(out.stdout).ok()
    }
}

/// 找到的 mpv。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpvBinary {
    pub path: PathBuf,
    /// `mpv --version` 的第一行输出，原样留着给诊断页看。
    pub version_line: String,
}

/// 探测 mpv。
///
/// 返回 None 表示**用不了**：没装、装了但版本太低、或者装了但跑不动。
/// 上层据此降级，不要尝试"凑合着用"。
pub fn detect(env: &dyn MpvEnv) -> Option<MpvBinary> {
    detect_with_override(env, None)
}

/// 带显式路径的探测。配置里写了 `mpv_path` 时用它。
///
/// 显式指定却不可用 → 返回 None 并让上层降级。
/// 不做"显式路径找不到就悄悄回落到 PATH"—— 用户明确指定了路径，
/// 静默用另一个 mpv 会让排障变成猜谜。
pub fn detect_with_override(env: &dyn MpvEnv, override_path: Option<&str>) -> Option<MpvBinary> {
    if let Some(p) = override_path {
        if p.is_empty() {
            return None;
        }
        return probe(env, p);
    }
    candidates(env).iter().find_map(|c| probe(env, c))
}

fn probe(env: &dyn MpvEnv, bin: &str) -> Option<MpvBinary> {
    let raw = env.run_cmd(bin, &["--version"])?;
    let version_line = raw.lines().next().unwrap_or("").trim().to_string();
    if !crate::version_ok(&version_line) {
        return None;
    }
    Some(MpvBinary {
        path: PathBuf::from(bin),
        version_line,
    })
}

/// 候选路径，按"最可能是用户装的"排序。
fn candidates(env: &dyn MpvEnv) -> Vec<String> {
    let os = env.os();
    let exe = "mpv".to_string() + os.exe_suffix();
    let mut out = vec![exe.clone()];

    // PATH 里的每一项。这比"直接跑 mpv"可靠：
    // 某些 GUI 启动方式下 PATH 不完整，靠 Command::new 会失败。
    if let Some(path) = env.env_var("PATH") {
        let sep = if os == Os::Windows { ';' } else { ':' };
        for dir in path.split(sep) {
            if dir.is_empty() {
                continue;
            }
            let p = if os == Os::Windows {
                let trimmed = dir.trim_end_matches(['/', '\\']);
                format!("{trimmed}\\{exe}")
            } else {
                let trimmed = dir.trim_end_matches('/');
                format!("{trimmed}/{exe}")
            };
            out.push(p);
        }
    }

    // 常见安装位置。PATH 没配好时靠这些兜住。
    for known in known_locations(os) {
        out.push(known.to_string());
    }
    out
}

fn known_locations(os: Os) -> Vec<String> {
    match os {
        Os::Windows => vec![
            "C:\\Program Files\\mpv\\mpv.exe".to_string(),
            "C:\\Program Files (x86)\\mpv\\mpv.exe".to_string(),
            "C:\\Program Files\\mpv.net\\mpv.exe".to_string(),
        ],
        Os::Macos => vec![
            "/opt/homebrew/bin/mpv".to_string(),
            "/usr/local/bin/mpv".to_string(),
            "/Applications/mpv.app/Contents/MacOS/mpv".to_string(),
        ],
        Os::Linux | Os::Other => vec![
            "/usr/bin/mpv".to_string(),
            "/usr/local/bin/mpv".to_string(),
            "/snap/bin/mpv".to_string(),
            "/run/current-system/sw/bin/mpv".to_string(), // NixOS
            "/var/lib/flatpak/exports/bin/io.mpv.Mpv".to_string(),
        ],
    }
}

#[cfg(test)]
pub struct FakeEnv {
    pub os: Os,
    pub path: Option<String>,
    pub existing: Vec<String>,
    pub cmd_out: Option<String>,
    pub requested: std::cell::RefCell<Vec<String>>,
}

#[cfg(test)]
impl Default for FakeEnv {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl FakeEnv {
    pub fn new() -> Self {
        Self {
            os: Os::Linux,
            path: None,
            existing: vec![],
            cmd_out: None,
            requested: std::cell::RefCell::new(vec![]),
        }
    }
    pub fn with_os(mut self, os: Os) -> Self {
        self.os = os;
        self
    }
    pub fn with_path(mut self, p: &str) -> Self {
        self.path = Some(p.to_string());
        self
    }
    pub fn with_cmd(mut self, out: &str) -> Self {
        self.cmd_out = Some(out.to_string());
        self
    }
    /// 限制"真实存在"的二进制。未列出的一律认为不存在 ——
    /// 否则 `run_cmd` 会对任何路径都成功，测不出"指定路径不可用"这类分支。
    pub fn with_existing(mut self, paths: &[&str]) -> Self {
        self.existing = paths.iter().map(|s| s.to_string()).collect();
        self
    }
}

#[cfg(test)]
impl MpvEnv for FakeEnv {
    fn os(&self) -> Os {
        self.os
    }
    fn env_var(&self, key: &str) -> Option<String> {
        (key == "PATH").then(|| self.path.clone()).flatten()
    }
    fn path_exists(&self, path: &str) -> bool {
        self.existing.iter().any(|p| p == path)
    }
    fn run_cmd(&self, bin: &str, _args: &[&str]) -> Option<String> {
        self.requested.borrow_mut().push(bin.to_string());
        if !self.existing.is_empty() && !self.existing.iter().any(|p| p == bin) {
            return None;
        }
        self.cmd_out.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "mpv 0.37.0 Copyright © 2000-2023 mpv/MPlayer/mplayer2 projects";

    #[test]
    fn found_in_path() {
        let env = FakeEnv::new().with_path("/usr/bin:/bin").with_cmd(GOOD);
        let b = detect(&env).expect("应当找到");
        assert_eq!(b.version_line, GOOD);
    }

    #[test]
    fn missing_mpv_yields_none() {
        let env = FakeEnv::new().with_path("/usr/bin");
        assert!(detect(&env).is_none());
    }

    #[test]
    fn too_old_version_is_rejected() {
        // 装了但太老：应当降级，而不是带着半套能力去播。
        let env = FakeEnv::new().with_cmd("mpv 0.32.0");
        assert!(detect(&env).is_none());
    }

    #[test]
    fn override_path_is_used_verbatim() {
        let env = FakeEnv::new().with_cmd(GOOD);
        let b = detect_with_override(&env, Some("/opt/mpv/bin/mpv")).unwrap();
        assert_eq!(b.path, PathBuf::from("/opt/mpv/bin/mpv"));
        assert_eq!(*env.requested.borrow(), vec!["/opt/mpv/bin/mpv"]);
    }

    #[test]
    fn broken_override_does_not_silently_fall_back() {
        // 用户明确指定了路径却不可用 —— 不悄悄改用 PATH 里的另一个 mpv。
        let env = FakeEnv::new()
            .with_existing(&["/usr/bin/mpv"])
            .with_path("/usr/bin")
            .with_cmd(GOOD);
        assert!(detect_with_override(&env, Some("/does/not/exist/mpv")).is_none());
    }

    #[test]
    fn empty_override_is_treated_as_unset_value() {
        let env = FakeEnv::new().with_cmd(GOOD);
        assert!(detect_with_override(&env, Some("")).is_none());
    }

    #[test]
    fn windows_candidates_include_program_files() {
        let env = FakeEnv::new().with_os(Os::Windows).with_cmd("");
        let c = candidates(&env);
        assert!(c.iter().any(|x| x.ends_with("mpv.exe")), "{c:?}");
        assert!(
            c.iter().any(|x| x == "C:\\Program Files\\mpv\\mpv.exe"),
            "{c:?}"
        );
    }

    #[test]
    fn path_is_split_by_platform_separator() {
        let env = FakeEnv::new()
            .with_os(Os::Windows)
            .with_path("C:\\bin;D:\\tools");
        let c = candidates(&env);
        assert!(c.iter().any(|x| x == "C:\\bin\\mpv.exe"), "{c:?}");
        assert!(c.iter().any(|x| x == "D:\\tools\\mpv.exe"), "{c:?}");
    }
}
