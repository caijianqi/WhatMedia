//! WhatMedia 公共基础工具模块。
//!
//! 本模块汇集了服务端与命令行工具共享的底层逻辑，包括：
//! - 日志追踪系统初始化（支持终端格式化输出与容器 JSON 结构化输出）
//! - 数据库连接池与迁移运行
//! - FFmpeg / FFprobe 跨平台可执行文件检测与就绪探测
//! - 媒体库与文件解析辅助函数（UUID 解析、存储目录映射等）

use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{info, warn};
use whatmedia_config::AppConfig;
use whatmedia_core::{Id, Library, MediaKind};
use whatmedia_database::{repo, Db};
use whatmedia_storage::LocalStorage;

/// 初始化全局日志追踪系统。
///
/// 根据配置文件的日志级别与格式进行初始化：
/// - 若环境变量 `RUST_LOG` 已指定，则优先遵循环境变量；
/// - 若格式为 `json`，启用结构化 JSON 格式（便于 Docker 容器日志收集）；
/// - 否则启用带终端色彩的紧凑控制台输出。
pub fn init_logging(cfg: &whatmedia_config::LoggingConfig) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.level));

    if cfg.format == "json" {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

/// 打开数据库连接池并自动执行待运行的迁移脚本。
///
/// # 错误
/// 如果数据库文件不可写、URL 非法或迁移脚本执行失败，将直接抛出异常并阻断启动，
/// 避免系统在半就绪状态下运行。
pub async fn open_db(cfg: &AppConfig) -> Result<Db> {
    let db = Db::open(&cfg.database.url)
        .await
        .with_context(|| format!("打开数据库失败：{}", cfg.database.url))?;
    db.migrate(&cfg.paths.migrations_dir)
        .await
        .with_context(|| format!("执行数据库迁移失败：{}", cfg.paths.migrations_dir.display()))?;
    Ok(db)
}

/// 探测 FFmpeg 可执行文件是否可用。
///
/// 探测规则：
/// 1. 优先读取环境变量 `WHATMEDIA_FFMPEG_PATH`；
/// 2. 其次读取配置文件中的 `transcode.ffmpeg_path`；
/// 3. 最后在系统 `PATH` 中寻找 `ffmpeg`。
///
/// 注意：FFmpeg 不可用不属于致命错误。Direct Play 依然可用，
/// 仅涉及转码或 Remux 的流式接口会返回 501。
pub async fn detect_ffmpeg(config: &AppConfig) -> Option<Arc<whatmedia_transcoder::FfmpegRunner>> {
    let bin = std::env::var("WHATMEDIA_FFMPEG_PATH").unwrap_or_else(|_| {
        config
            .transcode
            .ffmpeg_path
            .clone()
            .unwrap_or_else(|| "ffmpeg".to_string())
    });
    let runner = Arc::new(whatmedia_transcoder::FfmpegRunner::new(bin));
    if runner.is_available().await {
        info!(bin = runner.bin(), "已检测到 ffmpeg，转码服务可用");
        Some(runner)
    } else {
        warn!(bin = runner.bin(), "未检测到 ffmpeg，Direct Play 之外的转码路径将不可用");
        None
    }
}

/// 探测 FFprobe 是否可用。
///
/// 规则与 FFmpeg 一致，若未检测到 FFprobe，扫描流程降级为“纯文件名解析”，
/// 依然可以建库，只是缺少编码流详情与时长信息。
pub async fn detect_ffprobe() -> Option<Arc<whatmedia_media::FfprobeRunner>> {
    let runner = whatmedia_media::FfprobeRunner::default();
    let ok = tokio::process::Command::new(runner.bin())
        .arg("-version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ok {
        info!("已检测到 ffprobe，扫描入库将包含精确音视频流信息");
        Some(Arc::new(runner))
    } else {
        warn!("未检测到 ffprobe（{}），扫描入库将只做文件名规则解析", runner.bin());
        None
    }
}

/// 初始化图片缓存管理实例。
///
/// 即使缓存目录由于权限等原因创建失败，系统仅记录告警并降级为无缓存模式，
/// 确保核心服务正常可用。
pub fn init_image_cache(
    config: &AppConfig,
    db: Db,
) -> Option<Arc<whatmedia_metadata::ImageCache>> {
    let dir = config.images_dir();
    let cfg = whatmedia_metadata::ImageCacheConfig {
        dir: dir.clone(),
        max_bytes: config.images.max_bytes,
        max_entries: config.images.max_entries,
        max_age: chrono::Duration::days(config.images.max_age_days),
    };
    match whatmedia_metadata::ImageCache::new(db, cfg) {
        Ok(c) => {
            info!(dir = %dir.display(), "图片缓存池已就绪");
            Some(Arc::new(c))
        }
        Err(e) => {
            warn!("图片缓存不可用（{}）：{e:#}", dir.display());
            None
        }
    }
}

/// 将用户输入的 UUID 字符串解析为系统内核标准 `Id`。
pub fn parse_id(s: &str) -> Result<Id> {
    uuid::Uuid::parse_str(s).with_context(|| format!("不是合法的 UUID：{s}"))
}

/// 解析媒体类型字符串（movie / series / other 等）。
pub fn parse_kind(s: &str) -> Option<MediaKind> {
    serde_json::from_str::<MediaKind>(&format!("\"{s}\"")).ok()
}

/// 将毫秒数转换为便于人类阅读的时间字符串（HH:MM:SS）。
pub fn human_ms(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// 查找指定文件路径所属的媒体库本地存储实例。
pub async fn storage_for(db: &Db, path: &str) -> Result<Option<Arc<LocalStorage>>> {
    let p = std::path::Path::new(path);
    for lib in repo::list_libraries(db).await? {
        if p.starts_with(&lib.root_path) {
            return Ok(Some(Arc::new(LocalStorage::new(
                "local",
                std::path::PathBuf::from(&lib.root_path),
            )?)));
        }
    }
    Ok(None)
}

/// 根据 ID 或库名称/路径在数据库中解析媒体库。
pub async fn resolve_library(db: &Db, name_or_id: Option<&str>) -> Result<Option<Library>> {
    let Some(raw) = name_or_id else {
        return Ok(None);
    };
    if let Ok(id) = uuid::Uuid::parse_str(raw) {
        if let Some(l) = repo::get_library(db, id).await? {
            return Ok(Some(l));
        }
    }
    for l in repo::list_libraries(db).await? {
        if l.name == raw || l.root_path == raw {
            return Ok(Some(l));
        }
    }
    anyhow::bail!("找不到媒体库：{raw}")
}
