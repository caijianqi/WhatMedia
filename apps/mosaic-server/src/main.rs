//! Mosaic Server —— 服务端入口。
//!
//! 启动顺序是有讲究的：配置 → 目录 → 日志 → 数据库 → 迁移 → 探测 FFmpeg → 监听。
//! 任何一步失败都要**快速失败并给出人能读懂的原因**，
//! 不要带着半初始化的状态去监听端口 —— 那会让健康检查说谎。

use anyhow::{Context, Result};
use mosaic_api::{build_router, AppState};
use mosaic_config::AppConfig;
use mosaic_database::Db;
use std::sync::Arc;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    let config_path = std::env::args()
        .nth(1)
        .filter(|a| a.ends_with(".toml"))
        .map(std::path::PathBuf::from);

    let config = AppConfig::load(config_path)?;
    config.ensure_dirs()?;
    init_logging(&config.logging);

    let db = Db::open(&config.database.url)
        .await
        .with_context(|| format!("打开数据库失败：{}", config.database.url))?;
    db.migrate(&config.paths.migrations_dir).await?;
    db.ping().await?;

    let ffprobe = detect_ffprobe().await;

    // 图片缓存。缓存目录建不出来不算致命 —— 大不了每张图都回源。
    let images = init_image_cache(&config, db.clone());
    let scraper = mosaic_api::metadata::build_scraper(db.clone(), &config);
    if scraper.is_none() {
        warn!(
            "未配置任何元数据 Provider（TMDB API Key / Bangumi 均未启用），\
             刮削端点将返回 501"
        );
    }

    info!(
        version = env!("CARGO_PKG_VERSION"),
        arch = std::env::consts::ARCH,
        "Mosaic 启动"
    );

    let state = AppState::new(config.clone())
        .with_db(db)
        .with_ffprobe(ffprobe)
        .with_scraper(scraper)
        .with_images(images);

    // 重启后把上次崩溃遗留的 running 任务捞回来，否则它们会永久卡住。
    if let Some(s) = &state.scraper {
        match s.reap_running().await {
            Ok(0) => {}
            Ok(n) => info!("已回收 {n} 个中断的刮削任务"),
            Err(e) => warn!("回收刮削任务失败：{e:#}"),
        }
    }

    let addr = config.bind_addr();
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(%addr, "监听中");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("已退出");
    Ok(())
}

fn init_logging(cfg: &mosaic_config::LoggingConfig) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&cfg.level));

    if cfg.format == "json" {
        // 容器里日志要能被采集，必须是结构化 JSON。
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .json()
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("注册 Ctrl-C 处理器失败");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut s) => s.recv().await,
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
    info!("收到退出信号，开始优雅关闭");
}

/// 初始化图片缓存。
///
/// 缓存目录建不出来只降级不失败：没有缓存只是每张图都回源，
/// 而启动失败是"整个服务不可用"。这两者的严重程度差着一个数量级。
fn init_image_cache(
    config: &AppConfig,
    db: mosaic_database::Db,
) -> Option<Arc<mosaic_metadata::ImageCache>> {
    let dir = config.images_dir();
    let cfg = mosaic_metadata::ImageCacheConfig {
        dir: dir.clone(),
        max_bytes: config.images.max_bytes,
        max_entries: config.images.max_entries,
        max_age: chrono::Duration::days(config.images.max_age_days),
    };
    match mosaic_metadata::ImageCache::new(db, cfg) {
        Ok(c) => {
            info!(dir = %dir.display(), "图片缓存已就绪");
            Some(Arc::new(c))
        }
        Err(e) => {
            warn!("图片缓存不可用（{}）：{e:#}", dir.display());
            None
        }
    }
}

/// 探测 ffprobe 是否可用。
///
/// 不可用不是致命错误 —— 扫描降级为"只做文件名解析"，库照样建得起来，
/// 只是没有时长 / 码率 / 流信息。这对首次体验很关键：
/// 用户装完就发现"扫描报错"，比"少一些信息"糟糕得多。
async fn detect_ffprobe() -> Option<Arc<mosaic_media::FfprobeRunner>> {
    let runner = mosaic_media::FfprobeRunner::default();
    let ok = tokio::process::Command::new(runner.bin())
        .arg("-version")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if ok {
        info!("已检测到 ffprobe，扫描将包含流信息");
        Some(Arc::new(runner))
    } else {
        warn!("未检测到 ffprobe（{}），扫描将只做文件名解析", runner.bin());
        None
    }
}
