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

    info!(
        version = env!("CARGO_PKG_VERSION"),
        arch = std::env::consts::ARCH,
        "Mosaic 启动"
    );

    let state = AppState::new(config.clone())
        .with_db(db)
        .with_ffprobe(ffprobe);

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
