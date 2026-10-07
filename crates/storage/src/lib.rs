//! StorageProvider 的本地文件系统实现 + 路径安全。
//!
//! **路径安全四件套**（P1 的 DoD 之一）：
//!   1. canonicalize —— 消除 `..`、`.`、符号链接带来的歧义
//!   2. root boundary —— 解析结果必须仍在媒体库根之内
//!   3. symlink check —— 指向库外的符号链接一律拒绝
//!   4. permission check —— 不可读的文件报权限错误，不当成"文件不存在"
//!
//! 这四条不是装饰品：媒体库路径来自用户输入，
//! 而 Mosaic 是一个**长期运行、对外开放 HTTP 端口**的服务。

use async_trait::async_trait;
use mosaic_core::{ByteRange, MosaicError, Result, StorageEntry, StorageProvider};
use std::io::SeekFrom;
use std::path::{Component, Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// 本地文件系统存储。
///
/// `StorageEntry.path` 一律是**绝对路径**且已词法规范化（不含 `\\?\` 前缀）。
/// 远端存储（SMB / S3 / STRM）未来返回各自的虚拟路径，上层不得假设它是真实磁盘路径。
#[derive(Debug, Clone)]
pub struct LocalStorage {
    id: String,
    root: PathBuf,
}

/// 去掉 Windows canonicalize 产生的 `\\?\\` 前缀。
///
/// 这个前缀会让路径在 UI 里显示成一串乱码，也无法在别的机器上解析，
/// 而媒体库路径是要**长期存进数据库**的 —— 必须在入库前剥干净。
pub fn clean_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

impl LocalStorage {
    /// `root` 必须已存在且是目录。
    pub fn new(id: impl Into<String>, root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let canonical = clean_path(&root.canonicalize().map_err(|e| MosaicError::Storage {
            provider: "local".into(),
            message: format!("媒体库根目录不可访问 {}: {e}", root.display()),
        })?);
        if !canonical.is_dir() {
            return Err(MosaicError::Storage {
                provider: "local".into(),
                message: format!("媒体库根不是目录：{}", root.display()),
            });
        }
        Ok(Self {
            id: id.into(),
            root: canonical,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 把相对/绝对路径解析成库内安全路径。**所有读写都必须先过这一关。**
    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let p = Path::new(path);
        let joined = if p.is_absolute() {
            PathBuf::from(p)
        } else {
            self.root.join(p)
        };
        let normalized = normalize_lexically(&joined);

        // ②+③：canonicalize 会解析符号链接；结果落在 root 之外即视为逃逸。
        match normalized.canonicalize() {
            Ok(raw) => {
                // canonicalize 会加回 `\\?\\` 前缀，比较前必须剥掉，否则所有路径都被判为逃逸。
                let canonical = clean_path(&raw);
                if !canonical.starts_with(&self.root) {
                    return Err(MosaicError::PathRejected {
                        message: format!("路径逃逸出媒体库根：{}", normalized.display()),
                    });
                }
                // ④：存在但读不到元信息 → 明确报权限错误，不伪装成 NotFound。
                if std::fs::metadata(&canonical).is_err() {
                    return Err(MosaicError::PathRejected {
                        message: format!("无权访问：{}", normalized.display()),
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 写入场景允许目标尚不存在，但父目录必须在库内。
                if let Some(parent) = normalized.parent() {
                    if let Ok(raw) = parent.canonicalize() {
                        if !clean_path(&raw).starts_with(&self.root) {
                            return Err(MosaicError::PathRejected {
                                message: format!("路径逃逸出媒体库根：{}", normalized.display()),
                            });
                        }
                    }
                }
            }
            Err(e) => {
                return Err(MosaicError::PathRejected {
                    message: format!("无权访问 {}: {e}", normalized.display()),
                });
            }
        }

        Ok(normalized)
    }
}

/// 词法规范化：去掉 `.` 与 `..`，不做 IO。
///
/// 与 `canonicalize` 的区别是它**不解析符号链接**，
/// 因此结果可读、可直接入库（Windows 上也不会带上 `\\?\` 前缀）。
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                // `/..` 保持根，绝不把路径顶成相对路径。
                if !out.pop() {
                    out.push(Component::RootDir);
                }
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(Component::RootDir);
    }
    out
}

fn modified_ns(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as i64)
}

#[async_trait]
impl StorageProvider for LocalStorage {
    fn id(&self) -> &str {
        &self.id
    }

    async fn list(&self, dir: &str) -> Result<Vec<StorageEntry>> {
        let path = self.resolve(dir)?;
        let mut entries = Vec::new();
        let mut rd = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("读取目录失败 {}: {e}", path.display()),
            })?;

        while let Some(entry) = rd.next_entry().await.map_err(|e| MosaicError::Storage {
            provider: self.id.clone(),
            message: format!("遍历目录失败: {e}"),
        })? {
            let full = entry.path();
            // 权限不足或文件已消失 → 跳过，绝不中断整个扫描。
            let meta = match entry.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            entries.push(StorageEntry {
                path: clean_path(&normalize_lexically(&full))
                    .to_string_lossy()
                    .to_string(),
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir: meta.is_dir(),
                size: Some(meta.len() as i64),
                modified_ns: modified_ns(&meta),
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> Result<StorageEntry> {
        let p = self.resolve(path)?;
        let meta = tokio::fs::metadata(&p)
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("读取元信息失败 {}: {e}", p.display()),
            })?;
        Ok(StorageEntry {
            path: clean_path(&p).to_string_lossy().to_string(),
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            is_dir: meta.is_dir(),
            size: Some(meta.len() as i64),
            modified_ns: modified_ns(&meta),
        })
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        // 路径逃逸不算"不存在"，错误必须往上抛。
        let p = self.resolve(path)?;
        Ok(p.exists())
    }

    async fn range_read(&self, path: &str, range: ByteRange) -> Result<Vec<u8>> {
        let p = self.resolve(path)?;
        let mut file = tokio::fs::File::open(&p)
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("打开文件失败 {}: {e}", p.display()),
            })?;

        let total = file
            .metadata()
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("读取文件大小失败: {e}"),
            })?
            .len();

        if range.start >= total {
            return Ok(Vec::new());
        }
        let len = range.len(total);

        let mut buf = vec![0u8; len as usize];
        file.seek(SeekFrom::Start(range.start))
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("定位失败: {e}"),
            })?;
        let n = file
            .read_exact(&mut buf)
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("读取失败: {e}"),
            })?;
        buf.truncate(n);
        Ok(buf)
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        let p = self.resolve(path)?;
        tokio::fs::read(&p).await.map_err(|e| MosaicError::Storage {
            provider: self.id.clone(),
            message: format!("读取文件失败 {}: {e}", p.display()),
        })
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let p = self.resolve(path)?;
        if let Some(parent) = p.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| MosaicError::Storage {
                    provider: self.id.clone(),
                    message: format!("创建目录失败: {e}"),
                })?;
        }
        tokio::fs::write(&p, data)
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("写入失败 {}: {e}", p.display()),
            })
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let a = self.resolve(from)?;
        let b = self.resolve(to)?;
        tokio::fs::rename(&a, &b)
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("重命名失败: {e}"),
            })
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let p = self.resolve(path)?;
        tokio::fs::remove_file(&p)
            .await
            .map_err(|e| MosaicError::Storage {
                provider: self.id.clone(),
                message: format!("删除失败 {}: {e}", p.display()),
            })
    }
}

/// 扫描指纹：`(mtime_ns, size)`。
///
/// **两者缺一不可** —— 只用 mtime 会漏掉"内容变了但工具回写了 mtime"，
/// 只用 size 会漏掉"同尺寸不同内容"。
pub fn fingerprint(modified_ns: Option<i64>, size: i64) -> String {
    format!("{}:{}", modified_ns.unwrap_or(0), size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_normalization_removes_dot_dot() {
        assert_eq!(
            normalize_lexically(Path::new("/media/../media/movies/./a.mkv")),
            PathBuf::from("/media/movies/a.mkv")
        );
        assert_eq!(
            normalize_lexically(Path::new("/../../etc/passwd")),
            PathBuf::from("/etc/passwd")
        );
    }

    #[test]
    fn fingerprint_pairs_mtime_and_size() {
        assert_eq!(fingerprint(Some(123), 456), "123:456");
        assert_eq!(fingerprint(None, 456), "0:456");
    }
}

#[cfg(test)]
mod safety_tests {
    use super::*;

    /// 路径穿越必须被拒绝，且**不能**表现为"文件不存在"。
    #[tokio::test]
    async fn dot_dot_escape_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("media");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(tmp.path().join("outside.mkv"), b"x").unwrap();

        let s = LocalStorage::new("local", &root).unwrap();
        let err = s.stat("../outside.mkv").await.unwrap_err();
        assert!(
            matches!(err, MosaicError::PathRejected { .. }),
            "应是 PathRejected，实际 {err:?}"
        );
    }

    /// 绝对路径指向库外同样要拒绝。
    #[tokio::test]
    async fn absolute_path_outside_root_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("media");
        std::fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("outside.mkv");
        std::fs::write(&outside, b"x").unwrap();

        let s = LocalStorage::new("local", &root).unwrap();
        let err = s.stat(&outside.to_string_lossy()).await.unwrap_err();
        assert!(matches!(err, MosaicError::PathRejected { .. }), "{err:?}");
    }

    /// 库内路径必须放行，且 Range 读取要正确。
    #[tokio::test]
    async fn in_root_range_read_works() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.bin"), b"0123456789").unwrap();

        let s = LocalStorage::new("local", tmp.path()).unwrap();
        let got = s
            .range_read("a.bin", mosaic_core::ByteRange::new(3, Some(6)))
            .await
            .unwrap();
        assert_eq!(got, b"3456");

        // 越界起点返回空，不报错
        let empty = s
            .range_read("a.bin", mosaic_core::ByteRange::new(999, None))
            .await
            .unwrap();
        assert!(empty.is_empty());
    }

    /// 指向库外的符号链接必须被拒绝。Windows 上创建符号链接通常需要特权，跳过。
    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_escape_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("media");
        std::fs::create_dir_all(&root).unwrap();
        let outside = tmp.path().join("outside.mkv");
        std::fs::write(&outside, b"x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link.mkv")).unwrap();

        let s = LocalStorage::new("local", &root).unwrap();
        let err = s.stat("link.mkv").await.unwrap_err();
        assert!(matches!(err, MosaicError::PathRejected { .. }), "{err:?}");
    }
}
