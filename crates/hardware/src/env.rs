//! 运行环境探测的抽象层。
//!
//! **为什么要有它**：硬件探测本质上是"读 /proc、看 /dev、跑 nvidia-smi"。
//! 这些全是副作用，本机跑得通不代表 CI 上跑得通，
//! 更不代表能测出"在 R5S 上应该识别成 RKMPP"这类分支。
//!
//! 把副作用收进 `ProbeEnv`，实现就变成纯函数：`FakeEnv` 喂什么就识别成什么。

use std::collections::HashMap;
use std::path::Path;

/// 操作系统。三家的探测路径完全不同，必须显式区分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
    /// 未知平台一律按 Linux 处理（容器内绝大多是 Linux）。
    Other,
}

impl Os {
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
            Os::Other => "other",
        }
    }
}

/// 环境副作用集合。所有探测只能通过这些方法触达外部世界。
pub trait ProbeEnv: Send + Sync {
    fn os(&self) -> Os;

    /// 读文本文件。不存在或读不了返回 None，不抛错。
    fn read_file(&self, path: &str) -> Option<String>;

    fn path_exists(&self, path: &str) -> bool;

    /// 列出目录下的条目名。目录不存在返回空。
    fn list_dir(&self, path: &str) -> Vec<String>;

    /// 跑一条命令拿 stdout。失败 / 超时 / 命令不存在都返回 None。
    fn run_cmd(&self, bin: &str, args: &[&str]) -> Option<String>;

    fn env_var(&self, key: &str) -> Option<String>;

    /// 逻辑 CPU 数。软件转码并发上限依赖它。
    fn cpu_count(&self) -> usize;
}

/// 真实环境。
#[derive(Debug, Default)]
pub struct RealEnv;

impl RealEnv {
    pub fn new() -> Self {
        Self
    }
}

impl ProbeEnv for RealEnv {
    fn os(&self) -> Os {
        match std::env::consts::OS {
            "linux" => Os::Linux,
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            _ => Os::Other,
        }
    }

    fn read_file(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn path_exists(&self, path: &str) -> bool {
        Path::new(path).exists()
    }

    fn list_dir(&self, path: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                out.push(e.file_name().to_string_lossy().to_string());
            }
        }
        out.sort();
        out
    }

    fn run_cmd(&self, bin: &str, args: &[&str]) -> Option<String> {
        let out = std::process::Command::new(bin).args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn env_var(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn cpu_count(&self) -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }
}

/// 假环境。测试里喂什么文件就有什么文件。
#[derive(Debug, Clone, Default)]
pub struct FakeEnv {
    pub os: Option<Os>,
    files: HashMap<String, String>,
    paths: HashMap<String, bool>,
    dirs: HashMap<String, Vec<String>>,
    cmds: HashMap<String, String>,
    envs: HashMap<String, String>,
    cpus: Option<usize>,
}

impl FakeEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_os(mut self, os: Os) -> Self {
        self.os = Some(os);
        self
    }

    pub fn with_file(mut self, path: &str, content: &str) -> Self {
        self.files.insert(path.to_string(), content.to_string());
        // 有内容的文件默认也存在，省得每个测试都写两遍。
        self.paths.entry(path.to_string()).or_insert(true);
        self
    }

    pub fn with_path(mut self, path: &str, exists: bool) -> Self {
        self.paths.insert(path.to_string(), exists);
        self
    }

    pub fn with_dir(mut self, path: &str, entries: &[&str]) -> Self {
        self.dirs.insert(
            path.to_string(),
            entries.iter().map(|s| s.to_string()).collect(),
        );
        self.paths.entry(path.to_string()).or_insert(true);
        self
    }

    pub fn with_cmd(mut self, bin: &str, output: &str) -> Self {
        self.cmds.insert(bin.to_string(), output.to_string());
        self
    }

    pub fn with_env(mut self, k: &str, v: &str) -> Self {
        self.envs.insert(k.to_string(), v.to_string());
        self
    }

    pub fn with_cpus(mut self, n: usize) -> Self {
        self.cpus = Some(n);
        self
    }
}

impl ProbeEnv for FakeEnv {
    fn os(&self) -> Os {
        self.os.unwrap_or(Os::Linux)
    }

    fn read_file(&self, path: &str) -> Option<String> {
        self.files.get(path).cloned()
    }

    fn path_exists(&self, path: &str) -> bool {
        // 目录里的条目也算存在：`/dev/dri/renderD128` 通常只通过 list_dir 登记。
        if let Some(v) = self.paths.get(path) {
            return *v;
        }
        if let Some((parent, name)) = path.rsplit_once('/') {
            if let Some(entries) = self.dirs.get(parent) {
                if entries.iter().any(|e| e == name) {
                    return true;
                }
            }
        }
        false
    }

    fn list_dir(&self, path: &str) -> Vec<String> {
        self.dirs.get(path).cloned().unwrap_or_default()
    }

    fn run_cmd(&self, bin: &str, _args: &[&str]) -> Option<String> {
        self.cmds.get(bin).cloned()
    }

    fn env_var(&self, key: &str) -> Option<String> {
        self.envs.get(key).cloned()
    }

    fn cpu_count(&self) -> usize {
        self.cpus.unwrap_or(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_env_dir_entries_are_visible_as_paths() {
        let env = FakeEnv::new().with_dir("/dev/dri", &["card0", "renderD128"]);
        assert!(env.path_exists("/dev/dri/renderD128"));
        assert!(!env.path_exists("/dev/dri/renderD129"));
    }

    #[test]
    fn real_env_reports_some_os() {
        // 这条测试只是保证真实实现不会因为 unwrap 之类的原因崩掉。
        let env = RealEnv::new();
        let _ = env.os();
        let _ = env.cpu_count();
    }
}
