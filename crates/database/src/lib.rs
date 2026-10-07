//! SQLx 数据访问与 migration 管理。
//!
//! P0 只负责 migration 文件的发现与顺序校验；SQLx 连接池在 P1 接入，
//! 届时同时启用 `cargo sqlx prepare` 离线校验（跨架构 CI 依赖它）。

use std::path::{Path, PathBuf};

/// 迁移文件根目录（相对 workspace 根）。
pub const MIGRATIONS_DIR: &str = "migrations";

/// 按文件名升序列出所有 `.sql` 迁移。顺序即执行顺序，文件名前缀必须是序号。
pub fn migration_files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let dir = root.join(MIGRATIONS_DIR);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_ordered_by_filename() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let files = migration_files(&root).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }
}
