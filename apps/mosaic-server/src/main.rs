//! Mosaic Server —— 服务端入口。
//!
//! P0 目标：能启动、能过 /healthz、能优雅退出。

use anyhow::Result;
use mosaic_api::{build_router, AppState};
use mosaic_config::AppConfig;

fn init_logging(cfg: &mosaic_config::LoggingConfig) {
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
    tracing::info!("收到退出信号，开始优雅关闭");
}

#[tokio::main]
async fn main() -> Result<()> {
    // 配置可从 --config 指定；缺省走默认目录布局 + 环境变量覆盖。
    let config_path = std::env::args()
        .nth(1)
        .filter(|a| a.ends_with(".toml"))
        .map(std::path::PathBuf::from);

    let config = AppConfig::load(config_path)?;
    init_logging(&config.logging);
    config.ensure_dirs()?;
    mosaic_diagnostics::mark_started();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        arch = std::env::consts::ARCH,
        "Mosaic 启动"
    );
    tracing::debug!(database = %config.database.url, "数据库连接串已解析");

    let state = AppState::new(config.clone());
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(config.bind_addr()).await?;

    tracing::info!(addr = %config.bind_addr(), "监听中");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    tracing::info!("已退出");
    Ok(())
}
