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
    pub metadata: MetadataConfig,
    pub images: ImagesConfig,
    pub transcode: TranscodeConfig,
}

impl AppConfig {
    /// 图片缓存目录。未显式配置时落在 `<cache_dir>/images`。
    ///
    /// 做成方法而不是字段，是因为它依赖另一个字段的值 ——
    /// 若存成字段，改了 `cache_dir` 之后这里就过期了。
    pub fn images_dir(&self) -> PathBuf {
        self.images
            .dir
            .clone()
            .unwrap_or_else(|| self.paths.cache_dir.join("images"))
    }
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
    /// 容器里是 `/usr/local/share/whatmedia/migrations`，进程 CWD 却是 `/config`，
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

/// 元数据刮削（P2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataConfig {
    /// TMDB v3 API Key。
    pub tmdb_api_key: Option<String>,
    /// TMDB v4 Bearer Token。两个都配时用它。
    pub tmdb_bearer: Option<String>,
    /// 启用 Bangumi（动漫）。中文日番在 Bangumi 上命中率更高。
    pub bangumi_enabled: bool,
    pub language: String,
    /// 每秒请求上限。默认 4 —— 保守一点换不被封。
    pub requests_per_second: f64,
    /// 关掉后所有结果都进人工确认队列。新库第一次刮建议关掉。
    pub auto_accept: bool,
    pub concurrency: usize,
    pub fetch_images: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesConfig {
    /// 缓存根目录。默认 `<cache_dir>/images`。
    pub dir: Option<PathBuf>,
    /// 字节上限。超出后按 LRU 清理。
    pub max_bytes: u64,
    pub max_entries: u64,
    /// 多少天没访问就清掉。
    pub max_age_days: i64,
}

/// 转码（P3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeConfig {
    /// ffmpeg 可执行文件路径。None 时用 PATH 里的 `ffmpeg`，
    /// 环境变量 `WHATMEDIA_FFMPEG_PATH` 可以覆盖（ffmpeg-rockchip 就靠它切换）。
    pub ffmpeg_path: Option<String>,
    /// HLS 分片时长（秒）。太短会让分片数量爆炸，太长会让 Seek 变慢。
    pub hls_segment_seconds: u32,
    /// 并发上限。0 表示由硬件能力自动决定。
    pub max_concurrent: u32,
    /// 流式输出多久没被访问就算僵尸（秒）。
    pub stream_idle_timeout_secs: i64,
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
                // 开发默认指向仓库里的 migrations；容器里由 WHATMEDIA_MIGRATIONS_DIR 覆盖。
                migrations_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("migrations"),
            },
            database: DatabaseConfig {
                kind: "sqlite".into(),
                url: "sqlite:/config/whatmedia.db?mode=rwc".into(),
                max_connections: 8,
            },
            logging: LoggingConfig {
                level: "info".into(),
                format: "pretty".into(),
            },
            metadata: MetadataConfig {
                tmdb_api_key: None,
                tmdb_bearer: None,
                bangumi_enabled: false,
                language: "zh-CN".into(),
                requests_per_second: 4.0,
                auto_accept: true,
                concurrency: 4,
                fetch_images: true,
            },
            images: ImagesConfig {
                dir: None,
                max_bytes: 1024 * 1024 * 1024,
                max_entries: 50_000,
                max_age_days: 90,
            },
            transcode: TranscodeConfig {
                ffmpeg_path: None,
                hls_segment_seconds: 6,
                max_concurrent: 0,
                stream_idle_timeout_secs: 300,
            },
        }
    }
}

impl AppConfig {
    /// 从 TOML 加载，随后用 `WHATMEDIA_*` 环境变量覆盖。
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
        if let Ok(v) = std::env::var("WHATMEDIA_HOST") {
            self.server.host = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_PORT") {
            if let Ok(p) = v.parse() {
                self.server.port = p;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_CONFIG_DIR") {
            self.paths.config_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_DATA_DIR") {
            self.paths.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_CACHE_DIR") {
            self.paths.cache_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_LOG_DIR") {
            self.paths.log_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_MIGRATIONS_DIR") {
            self.paths.migrations_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_DATABASE_URL") {
            if v.starts_with("postgres") {
                self.database.kind = "postgres".into();
            }
            self.database.url = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_LOG_LEVEL") {
            self.logging.level = v;
        }
        // ── 元数据 ────────────────────────────────────────────
        // API Key 走环境变量而不是写进 TOML：TOML 常被误提交到仓库。
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_TMDB_API_KEY") {
            self.metadata.tmdb_api_key = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_TMDB_BEARER") {
            self.metadata.tmdb_bearer = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_BANGUMI") {
            self.metadata.bangumi_enabled =
                matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_LANGUAGE") {
            self.metadata.language = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_RPS") {
            if let Ok(n) = v.parse() {
                self.metadata.requests_per_second = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_AUTO_ACCEPT") {
            self.metadata.auto_accept =
                matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(v) = std::env::var("WHATMEDIA_IMAGES_DIR") {
            self.images.dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("WHATMEDIA_IMAGES_MAX_MB") {
            if let Ok(n) = v.parse::<u64>() {
                self.images.max_bytes = n * 1024 * 1024;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_LOG_FORMAT") {
            self.logging.format = v;
        }
        // ── 转码 ────────────────────────────────────────────
        if let Ok(v) = std::env::var("WHATMEDIA_FFMPEG_PATH") {
            self.transcode.ffmpeg_path = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_HLS_SEGMENT_SECONDS") {
            if let Ok(n) = v.parse() {
                self.transcode.hls_segment_seconds = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_TRANSCODE_MAX_CONCURRENT") {
            if let Ok(n) = v.parse() {
                self.transcode.max_concurrent = n;
            }
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
        // 并发上限默认 0 = 交给硬件能力决定，不要写死一个数字。
        assert_eq!(cfg.transcode.max_concurrent, 0);
        assert_eq!(cfg.transcode.hls_segment_seconds, 6);
    }

    #[test]
    fn byte_range_len_is_inclusive() {
        use whatmedia_core::ByteRange;
        assert_eq!(ByteRange::new(0, Some(9)).len(100), 10);
        assert_eq!(ByteRange::new(50, None).len(100), 50);
    }
}
