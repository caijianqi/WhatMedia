//! 数据库连接与迁移。
//!
//! 两个决定值得记下来：
//!
//! 1. **不用 `sqlx::query!` 编译期宏。** 它要求 `DATABASE_URL` 与 `.sqlx` 离线缓存，
//!    在跨架构 CI（尤其自托管 arm64 runner）上是个持续制造麻烦的点。
//!    改用运行期 `sqlx::query()`，代价是失去编译期 SQL 校验，
//!    补偿手段是 `repo` 层每个函数都要有集成测试（`crates/database/tests/`）。
//!
//! 2. **所有行结构体用 `String` 存 id 与时间。** sqlx 对 SQLite 的 Uuid/DateTime
//!    编解码在不同版本间行为不一致，显式转换最省心，也让"SQLite → PostgreSQL"
//!    后期切换时差异集中在一处。

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// 数据库连接池句柄。可 Clone，Clone 后共享同一池。
#[derive(Clone, Debug)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// 打开一个 SQLite 数据库。
    ///
    /// `url` 形式：`sqlite:/path/to/mosaic.db?mode=rwc` 或 `sqlite::memory:`。
    pub async fn open(url: &str) -> Result<Self> {
        let is_memory = url.contains(":memory:") || url.contains("mode=memory");

        let mut opts = SqliteConnectOptions::from_str(url)
            .with_context(|| format!("解析数据库 URL 失败：{url}"))?
            .create_if_missing(true)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        // WAL 让"扫描写入"与"播放读取"互不阻塞 —— §107 "扫描不影响播放"的地基之一。
        // 内存库不支持 WAL。
        if !is_memory {
            opts = opts.journal_mode(SqliteJournalMode::Wal);
        }
        opts = opts.disable_statement_logging();

        if !is_memory {
            if let Some(parent) = db_file_path(url).and_then(|p| p.parent().map(PathBuf::from)) {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(&parent)
                        .with_context(|| format!("创建数据库目录失败：{}", parent.display()))?;
                }
            }
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(opts)
            .await
            .with_context(|| format!("连接数据库失败：{url}"))?;

        Ok(Self { pool })
    }

    /// 内存数据库。仅供测试与 `mosaic-cli` 的一次性诊断使用。
    pub async fn open_in_memory() -> Result<Self> {
        Self::open("sqlite::memory:").await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// 执行 `dir` 下所有未应用的迁移。重复调用是幂等的。
    ///
    /// **为什么运行时读目录而不是 `sqlx::migrate!` 编译期嵌入**：
    /// 容器里迁移文件位于 `/usr/local/share/mosaic/migrations`，进程 CWD 却在 `/config`，
    /// 任何相对路径的编译期常量都会在生产环境失效。运行时按配置传入目录才是对的。
    pub async fn migrate(&self, dir: &Path) -> Result<()> {
        let migrator = sqlx::migrate::Migrator::new(dir.to_path_buf())
            .await
            .with_context(|| format!("读取迁移目录失败：{}", dir.display()))?;

        migrator
            .run(&self.pool)
            .await
            .context("执行数据库迁移失败")?;
        Ok(())
    }

    pub async fn ping(&self) -> Result<()> {
        sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .context("数据库连通性检查失败")?;
        Ok(())
    }
}

/// 从 `sqlite:` URL 里取出文件路径，用于自动创建父目录。
fn db_file_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("sqlite:")?;
    let path_part = rest.split('?').next().unwrap_or(rest);
    if path_part.starts_with(':') || path_part.is_empty() {
        return None;
    }
    Some(PathBuf::from(path_part))
}

/// 按文件名升序列出目录下的所有 `.sql` 迁移。顺序即执行顺序。
pub fn migration_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
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
    fn parses_db_file_path() {
        assert_eq!(
            db_file_path("sqlite:/config/mosaic.db?mode=rwc"),
            Some(PathBuf::from("/config/mosaic.db"))
        );
        assert_eq!(db_file_path("sqlite::memory:"), None);
    }

    fn workspace_migrations_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("migrations")
    }

    #[tokio::test]
    async fn memory_db_migrates_and_pings() {
        let db = Db::open_in_memory().await.unwrap();
        db.migrate(&workspace_migrations_dir()).await.unwrap();
        db.ping().await.unwrap();
    }

    #[tokio::test]
    async fn migration_is_idempotent() {
        let db = Db::open_in_memory().await.unwrap();
        let dir = workspace_migrations_dir();
        db.migrate(&dir).await.unwrap();
        db.migrate(&dir).await.unwrap();
    }
}
