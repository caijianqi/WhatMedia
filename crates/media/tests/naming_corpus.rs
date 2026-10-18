//! 命名语料回归测试。
//!
//! P1 的 DoD 之一：**季/集解析在语料上准确率 ≥ 99%**。
//! 语料在 `test-media/naming/cases.jsonl`，加新语料的规则写在那个文件头部。

use serde::Deserialize;
use std::path::PathBuf;
use whatmedia_media::{parse_file_in_dir, parse_filename};

#[derive(Debug, Deserialize)]
struct Case {
    name: String,
    /// 所在目录名。剧集的"集名 vs 剧名"必须靠它才能分开。
    #[serde(default)]
    dir: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    year: Option<i32>,
    #[serde(default)]
    season: Option<i32>,
    #[serde(default)]
    episode: Option<i32>,
    #[serde(default)]
    episode_end: Option<i32>,
    #[serde(default)]
    episode_title: Option<String>,
    #[serde(default)]
    resolution: Option<String>,
}

/// 语料里省略的字段不做校验。
fn matches<T: PartialEq>(expected: Option<T>, actual: Option<T>) -> bool {
    match expected {
        None => true,
        Some(e) => actual == Some(e),
    }
}

fn corpus_path_impl() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("test-media/naming/cases.jsonl")
}

#[test]
fn corpus_accuracy_is_at_least_99_percent() {
    let path = corpus_path_impl();
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到语料 {}: {e}", path.display()));

    let mut total = 0usize;
    let mut failures: Vec<String> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let case: Case =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("语料格式错误：{e}\n  {line}"));
        total += 1;

        let got = match &case.dir {
            Some(d) => parse_file_in_dir(&case.name, d),
            None => parse_filename(&case.name),
        };
        let mut why = Vec::new();
        if !matches(case.title.clone(), Some(got.title.clone())) {
            why.push(format!("title 期望 {:?} 实得 {:?}", case.title, got.title));
        }
        if !matches(case.year, got.year) {
            why.push(format!("year 期望 {:?} 实得 {:?}", case.year, got.year));
        }
        if !matches(case.season, got.season) {
            why.push(format!(
                "season 期望 {:?} 实得 {:?}",
                case.season, got.season
            ));
        }
        if !matches(case.episode, got.episode) {
            why.push(format!(
                "episode 期望 {:?} 实得 {:?}",
                case.episode, got.episode
            ));
        }
        if !matches(case.episode_end, got.episode_end) {
            why.push(format!(
                "episode_end 期望 {:?} 实得 {:?}",
                case.episode_end, got.episode_end
            ));
        }
        if !matches(case.episode_title.clone(), got.episode_title.clone()) {
            why.push(format!(
                "episode_title 期望 {:?} 实得 {:?}",
                case.episode_title, got.episode_title
            ));
        }
        if !matches(case.resolution.clone(), got.resolution.clone()) {
            why.push(format!(
                "resolution 期望 {:?} 实得 {:?}",
                case.resolution, got.resolution
            ));
        }

        if !why.is_empty() {
            failures.push(format!("  ✗ {}\n      {}", case.name, why.join("；")));
        }
    }

    let accuracy = if total == 0 {
        100.0
    } else {
        (total - failures.len()) as f64 * 100.0 / total as f64
    };

    assert!(
        accuracy >= 99.0,
        "命名解析准确率 {accuracy:.1}% 未达 99%（{}/{} 通过）：\n{}",
        total - failures.len(),
        total,
        failures.join("\n")
    );

    println!("命名语料准确率 {accuracy:.1}%（{total} 条）");
}
