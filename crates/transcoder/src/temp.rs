//! 临时目录守卫。
//!
//! **DoD 里有一条是"取消后进程与临时文件 100% 清理"**，
//! 靠调用方记得删目录是做不到的 —— 取消、panic、early return 都会漏。
//!
//! 用 Drop 兜底：只要 `TempDirGuard` 离开作用域，目录就没了。
//! 想留下目录必须显式 `persist()`。

use std::path::{Path, PathBuf};

/// 一个"离开作用域就删掉"的目录。
#[derive(Debug)]
pub struct TempDirGuard {
    path: Option<PathBuf>,
}

impl TempDirGuard {
    /// 创建一个新目录。父目录不存在时会自动建立。
    pub fn create(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        std::fs::create_dir_all(&path)?;
        Ok(Self { path: Some(path) })
    }

    /// 接管一个已经存在的目录（比如进程重启后从库里捞回来的残留）。
    pub fn adopt(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    pub fn path(&self) -> &Path {
        self.path.as_deref().expect("guard 已被 persist 消耗")
    }

    /// 放弃自动删除，把目录交给调用方。
    pub fn persist(mut self) -> PathBuf {
        self.path.take().expect("guard 已被 persist 消耗")
    }

    /// 立即删除。幂等：目录不存在也不报错。
    pub fn cleanup(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// 计算一个目录下的总字节数与文件数。回收器用它统计清理量。
pub fn dir_size(path: &Path) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut files = 0usize;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            match e.metadata() {
                Ok(m) if m.is_dir() => stack.push(e.path()),
                Ok(m) => {
                    bytes += m.len();
                    files += 1;
                }
                Err(_) => {}
            }
        }
    }
    (bytes, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_removes_dir_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("hls").join("job1");
        {
            let g = TempDirGuard::create(&target).unwrap();
            std::fs::write(g.path().join("seg_00000.ts"), b"data").unwrap();
            assert!(target.exists());
        }
        assert!(!target.exists(), "离开作用域后目录必须消失");
    }

    #[test]
    fn persist_keeps_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("job2");
        let kept = {
            let g = TempDirGuard::create(&target).unwrap();
            g.persist()
        };
        assert!(kept.exists());
    }

    #[test]
    fn cleanup_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("job3");
        let mut g = TempDirGuard::create(&target).unwrap();
        g.cleanup();
        g.cleanup();
        assert!(!target.exists());
    }

    #[test]
    fn dir_size_counts_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a")).unwrap();
        std::fs::write(dir.path().join("a").join("x.ts"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.path().join("y.ts"), vec![0u8; 50]).unwrap();
        let (bytes, files) = dir_size(dir.path());
        assert_eq!(bytes, 150);
        assert_eq!(files, 2);
    }
}
