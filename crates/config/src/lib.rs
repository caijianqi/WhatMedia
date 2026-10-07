//! 配置加载：TOML 文件 + 环境变量覆盖。
//!
//! 默认目录布局（容器内）：
//!   /config  配置与数据库
//!   /data    运行时数据
//!   /cache   元数据 / 图片 / 转码缓存
//!   /logs    日志

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub paths: PathsConfig,
    pub database: DatabaseConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    /// SQL 迁移文件目录。
    ///
    /// 容器里是 `/usr/local/share/mosaic/migrations`，进程 CWD 却是 `/config`，
    /// 所以必须可配置 —— 这也是 `Db::migrate` 用运行时路径而非编译期常量的原因。
    pub migrations_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// "sqlite" | "postgres"。v1.0 之前只用 sqlite，postgres 为预留分支。
    pub kind: String,
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    /// "pretty" 本地开发，"json" 容器与生产。
    pub format: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
            },
            paths: PathsConfig {
                config_dir: PathBuf::from("/config"),
                data_dir: PathBuf::from("/data"),
                cache_dir: PathBuf::from("/cache"),
                log_dir: PathBuf::from("/logs"),
                // 开发默认指向仓库里的 migrations；容器里由 MOSAIC_MIGRATIONS_DIR 覆盖。
                migrations_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("migrations"),
            },
            database: DatabaseConfig {
                kind: "sqlite".into(),
                url: "sqlite:/config/mosaic.db?mode=rwc".into(),
                max_connections: 8,
            },
            logging: LoggingConfig {
                level: "info".into(),
                format: "pretty".into(),
            },
        }
    }
}

impl AppConfig {
    /// 从 TOML 加载，随后用 `MOSAIC_*` 环境变量覆盖。
    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let mut cfg = match path {
            Some(p) if p.exists() => {
                let raw = std::fs::read_to_string(&p)?;
                toml::from_str(&raw)?
            }
            _ => Self::default(),
        };
        cfg.apply_env();
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("MOSAIC_HOST") {
            self.server.host = v;
        }
        if let Ok(v) = std::env::var("MOSAIC_PORT") {
            if let Ok(p) = v.parse() {
                self.server.port = p;
            }
        }
        if let Ok(v) = std::env::var("MOSAIC_CONFIG_DIR") {
            self.paths.config_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("MOSAIC_DATA_DIR") {
            self.paths.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("MOSAIC_CACHE_DIR") {
            self.paths.cache_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("MOSAIC_LOG_DIR") {
            self.paths.log_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("MOSAIC_MIGRATIONS_DIR") {
            self.paths.migrations_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("MOSAIC_DATABASE_URL") {
            if v.starts_with("postgres") {
                self.database.kind = "postgres".into();
            }
            self.database.url = v;
        }
        if let Ok(v) = std::env::var("MOSAIC_LOG_LEVEL") {
            self.logging.level = v;
        }
        if let Ok(v) = std::env::var("MOSAIC_LOG_FORMAT") {
            self.logging.format = v;
        }
    }

    /// 创建所有必需目录。启动阶段调用，失败即退出。
    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        for dir in [
            &self.paths.config_dir,
            &self.paths.data_dir,
            &self.paths.cache_dir,
            &self.paths.log_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_binds_to_8080() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.bind_addr(), "0.0.0.0:8080");
        assert_eq!(cfg.database.kind, "sqlite");
    }

    #[test]
    fn byte_range_len_is_inclusive() {
        use mosaic_core::ByteRange;
        assert_eq!(ByteRange::new(0, Some(9)).len(100), 10);
        assert_eq!(ByteRange::new(50, None).len(100), 50);
    }
}
