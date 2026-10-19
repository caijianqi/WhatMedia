//! WhatMedia HTTP 服务端模块。
//!
//! 负责 WhatMedia Web 服务端的核心生命周期管理：
//! 1. 配置加载与目录就绪检查；
//! 2. 数据库连接池就绪与自动结构迁移；
//! 3. 环境变量预置管理员（Bootstrap）账户创建；
//! 4. 数据库持久化设置覆盖（Setting Overrides）；
//! 5. 垃圾会话与过期通知定时清理；
//! 6. FFmpeg / FFprobe 与底层硬件加速档位（VAAPI / NVENC / QuickSync / VideoToolbox）自检；
//! 7. 刮削引擎与图片缓存构建；
//! 8. 崩溃遗留状态（HLS 分片目录与中断的刮削任务）自动回收；
//! 9. Axum HTTP 路由挂载与跨平台优雅停机（Graceful Shutdown）。

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{info, warn};
use whatmedia_api::{build_router, AppState};
use whatmedia_config::AppConfig;
use whatmedia_database::Db;

use crate::common::{detect_ffmpeg, detect_ffprobe, init_image_cache, init_logging};

/// 启动 WhatMedia HTTP 服务器并保持监听，直到接收到终止信号。
///
/// # 参数
/// - `config_path`: 可选的外部 TOML 配置文件路径。未提供时按内置策略加载（当前目录 -> 系统配置目录）。
pub async fn run_server(config_path: Option<PathBuf>) -> Result<()> {
    let mut config = AppConfig::load(config_path)?;
    config.ensure_dirs()?;
    init_logging(&config.logging);

    // 记录系统启动时间戳，用于 `/healthz` 的 `uptime_secs` 报告
    whatmedia_diagnostics::mark_started();

    let db = Db::open(&config.database.url)
        .await
        .with_context(|| format!("打开数据库失败：{}", config.database.url))?;
    db.migrate(&config.paths.migrations_dir).await?;
    db.ping().await?;

    // ── 首次运行引导账户 ──────────────────────────────────────
    // 部署者可以用 WHATMEDIA_BOOTSTRAP_USER / _PASSWORD 直接准备管理员
    match whatmedia_api::auth::bootstrap_dev_user(&db).await {
        Ok(Some((username, _))) => {
            info!(%username, "已根据环境变量自动创建初始管理员账户");
        }
        Ok(None) => {}
        Err(e) => warn!("初始管理员账户自动创建失败：{e:#}"),
    }

    // ── 数据库持久化设置覆盖 ──────────────────────────────────
    // 允许管理员在 Web 界面保存的设置覆盖系统默认 TOML 配置
    let rejected = match whatmedia_database::repo::list_server_settings(&db).await {
        Ok(rows) => {
            let overrides: Vec<(String, String)> =
                rows.into_iter().map(|r| (r.key, r.value)).collect();
            if overrides.is_empty() {
                Vec::new()
            } else {
                let r = config.apply_setting_overrides(&overrides);
                if r.is_empty() {
                    info!(n = overrides.len(), "已套用数据库保存的服务端配置覆盖");
                }
                r
            }
        }
        Err(e) => {
            warn!("读取数据库设置覆盖失败，使用默认配置继续启动：{e:#}");
            Vec::new()
        }
    };
    for r in &rejected {
        warn!("设置覆盖项被忽略（不在白名单内或值不合法）：{r}");
    }

    // ── 启动时例行维护清理 ────────────────────────────────────
    whatmedia_api::auth::prune_sessions(&db, 30).await;
    match whatmedia_database::repo::prune_notifications(&db, 90).await {
        Ok(0) => {}
        Ok(n) => info!("已清理 {n} 条过期系统通知"),
        Err(e) => warn!("清理过期通知失败：{e:#}"),
    }

    let ffprobe = detect_ffprobe().await;

    // ── 硬件编解码加速自检 ────────────────────────────────────
    let (machine, profiles) = whatmedia_hardware::probe_all(&whatmedia_hardware::RealEnv::new());
    let ffmpeg = detect_ffmpeg(&config).await;
    if ffmpeg.is_none() {
        warn!("未检测到 ffmpeg，Direct Play 之外的转码路径将返回 501");
    }

    // 图片缓存与元数据刮削器初始化
    let images = init_image_cache(&config, db.clone());
    let scraper = whatmedia_api::metadata::build_scraper(db.clone(), &config);
    if scraper.is_none() {
        warn!("未配置任何元数据 Provider（TMDB / Bangumi），刮削功能将不可用");
    }

    info!(
        version = env!("CARGO_PKG_VERSION"),
        arch = std::env::consts::ARCH,
        cpu = machine.cpu.model.as_deref().unwrap_or("unknown"),
        cores = machine.cpu.cores,
        memory_mb = machine.memory.total_bytes / (1024 * 1024),
        "WhatMedia 媒体服务已就绪"
    );
    for p in &profiles {
        info!(
            backend = ?p.kind,
            encoders = ?p.encoders,
            max_sessions = p.max_concurrent_sessions,
            "检测到硬件加速后端"
        );
    }

    let state = AppState::new(config.clone())
        .with_db(db.clone())
        .with_rejected_settings(rejected)
        .with_ffprobe(ffprobe)
        .with_scraper(scraper)
        .with_images(images)
        .with_playback(ffmpeg, profiles, machine.accel_context())
        .with_ai();

    // ── 重启后脏数据回收 ──────────────────────────────────────
    // 1. 清除崩溃遗留的 HLS 分片临时目录，避免磁盘膨胀
    match whatmedia_api::playback::reap_orphan_streams(&db, &config.paths.cache_dir.join("hls"))
        .await
    {
        Ok(0) => {}
        Ok(n) => info!("已清理 {n} 个上次运行遗留的 HLS 缓存目录"),
        Err(e) => warn!("清理遗留 HLS 目录失败：{e:#}"),
    }

    // 2. 将上次崩溃中断的 running 刮削任务恢复为可重试状态
    if let Some(s) = &state.scraper {
        match s.reap_running().await {
            Ok(0) => {}
            Ok(n) => info!("已恢复 {n} 个中断的后台刮削任务"),
            Err(e) => warn!("恢复刮削任务失败：{e:#}"),
        }
    }

    let addr = config.bind_addr();
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(%addr, "HTTP 服务已在监听，访问 Web 界面即可开始使用");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("WhatMedia 服务端已安全退出");
    Ok(())
}

/// 监听系统中断信号以支持优雅停机。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("注册 Ctrl-C 信号监听失败");
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
    info!("收到停机信号，正在等待处理中的请求并优雅退出...");
}
