//! 依赖守卫：把 ADR-0006 的"AI 不可直连业务"钉在编译期之后、运行之前。
//!
//! 为什么需要一个测试来管这件事，而不是靠 code review：
//! AI 是本项目里唯一一个"输入来自模型"的模块。模型会犯错，而人写调用代码时
//! 也会图省事。一旦有人在工具里写了 `sqlx::query(...)` 或 `std::fs::remove_file`，
//! 「Plan → Preview → Confirm → Execute」那道闸就等于被绕过去了 ——
//! 用户根本看不到预览，因为执行根本没经过运行时。
//!
//! 这个测试干两件事：
//!   1. `Cargo.toml` 里不允许出现业务 crate / sqlx / tokio::process
//!   2. 源码里不允许出现 `sqlx::` / `Command::` / `std::fs::` 这类直连调用
//!
//! 想加能力？正确做法是往 `AiHost` trait 上加方法，再包成一个工具。

use std::path::{Path, PathBuf};

fn crate_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn cargo_toml() -> String {
    std::fs::read_to_string(crate_root().join("Cargo.toml")).expect("读不到 Cargo.toml")
}

/// 收集 `src/` 下所有 .rs 的内容（测试代码也在内 —— 后门开在测试里一样是后门）。
fn all_sources() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut dirs = vec![crate_root().join("src")];
    while let Some(dir) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().and_then(|e| e.to_str()) == Some("rs") {
                if let Ok(s) = std::fs::read_to_string(&p) {
                    out.push((p, s));
                }
            }
        }
    }
    out
}

/// 去掉注释和字符串字面量，避免"文档里提到 sqlx"被误判成"代码里用了 sqlx"。
fn strip_comments_and_strings(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                i += 2;
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────
// 1. 依赖表
// ─────────────────────────────────────────────────────────────

const FORBIDDEN_DEPS: &[&str] = &[
    // 业务 crate：AI 只能通过 AiHost 触达它们
    "whatmedia-database",
    "whatmedia-scanner",
    "whatmedia-transcoder",
    "whatmedia-library",
    "whatmedia-metadata",
    "whatmedia-storage",
    "whatmedia-media",
    "whatmedia-api",
    // 直连数据 / 系统
    "sqlx",
    "rusqlite",
    "tokio-process",
];

#[test]
fn no_business_crate_dependencies() {
    let toml = cargo_toml();
    for dep in FORBIDDEN_DEPS {
        // 按整词匹配：`whatmedia-media` 不能误伤 `whatmedia-media3`，
        // 反过来 `whatmedia-media` 也不该被 `whatmedia-media3` 糊过去。
        let hit = toml
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .any(|l| {
                l.split_once('=')
                    .map(|(k, _)| k)
                    .unwrap_or(l)
                    .trim()
                    .trim_matches('"')
                    == *dep
            });
        assert!(
            !hit,
            "crates/ai 不允许依赖 {dep}：AI 只能通过 AiHost trait 触达业务（ADR-0006）"
        );
    }
}

#[test]
fn allowed_dependencies_are_exactly_the_documented_set() {
    // 正向清单，而不是只做黑名单：将来有人随手加一个依赖，这里会先响。
    let toml = cargo_toml();
    let allowed = [
        "whatmedia-core",
        "whatmedia-config",
        "utoipa",
        "async-trait",
        "serde",
        "serde_json",
        "thiserror",
        "tracing",
        "uuid",
        "chrono",
        "reqwest",
        "sha2",
        "tokio",
    ];
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('#') || !l.contains('=') {
            continue;
        }
        let key = l.split_once('=').map(|(k, _)| k).unwrap_or("").trim();
        if key.is_empty() || key.starts_with('[') {
            continue;
        }
        // 只查依赖条目（`name = { ... }`）。
        // 用 `= {` 而不是 `workspace = true`：后者会把
        // `version.workspace = true` / `edition.workspace = true` 也当成依赖。
        if !l.contains("= {") {
            continue;
        }
        let name = key.trim_matches('"');
        assert!(
            allowed.contains(&name),
            "crates/ai 出现了未在白名单里的依赖 {name}。\
             要加请先想清楚它是「模型接入」还是「业务能力」；后者必须走 AiHost。"
        );
    }
}

// ─────────────────────────────────────────────────────────────
// 2. 源码里不许出现直连调用
// ─────────────────────────────────────────────────────────────

const FORBIDDEN_CODE: &[&str] = &[
    "sqlx::",
    "rusqlite::",
    "std::fs::",
    "tokio::fs::",
    "std::process::",
    "tokio::process::",
    "Command::new",
    "std::net::TcpStream",
];

#[test]
fn no_direct_data_or_system_access_in_sources() {
    for (path, raw) in all_sources() {
        let code = strip_comments_and_strings(&raw);
        for pat in FORBIDDEN_CODE {
            assert!(
                !code.contains(pat),
                "{}\n  出现了直连调用 `{pat}`。\
                 AI 工具只能调 AiHost；绕过它等于绕过确认闸（ADR-0006）。",
                path.display()
            );
        }
    }
}

#[test]
fn sources_are_not_empty_sanity_check() {
    // 防止上面那个测试因为"扫不到文件"而虚假通过。
    let n = all_sources().len();
    assert!(n >= 10, "只扫到 {n} 个 .rs 文件，守卫本身可能失效了");
}
