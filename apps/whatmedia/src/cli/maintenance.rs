//! 图片缓存与临时目录清理维护子命令模块。
//!
//! 提供图片缓存容量统计与主动驱逐（Evict），以及清理因崩溃遗留的 HLS 分片目录与僵尸转码会话。

use anyhow::Result;
use whatmedia_config::AppConfig;
use whatmedia_database::{repo, Db};

/// 查看图片缓存使用统计并支持主动清理超出预算的缓存。
pub async fn images_cmd(db: Db, cfg: &AppConfig, stats: bool, evict: bool) -> Result<()> {
    let cache = whatmedia_metadata::ImageCache::new(
        db,
        whatmedia_metadata::ImageCacheConfig {
            dir: cfg.images_dir(),
            max_bytes: cfg.images.max_bytes,
            max_entries: cfg.images.max_entries,
            max_age: chrono::Duration::days(cfg.images.max_age_days),
        },
    )?;
    if stats {
        let s = cache.stats().await?;
        println!(
            "缓存条目数 {}\t占用磁盘 {:.1} MB\t累计服务命中次数 {}",
            s.entries,
            s.bytes as f64 / 1024.0 / 1024.0,
            s.hits
        );
    }
    if evict {
        let r = cache.evict().await?;
        println!(
            "清理 {} 项缓存，共释放 {:.1} MB 磁盘空间（超时驱逐 {} 项，超容驱逐 {} 项）",
            r.removed,
            r.bytes_freed as f64 / 1024.0 / 1024.0,
            r.by_age,
            r.by_size
        );
    }
    Ok(())
}

/// 清理遗留的流式转码输出目录（如上次非正常退出遗留的 HLS 分片）。
pub async fn reap_streams_cmd(db: &Db, cfg: &AppConfig, idle_secs: Option<i64>) -> Result<()> {
    let idle = idle_secs.unwrap_or(cfg.transcode.stream_idle_timeout_secs);
    let orphan =
        whatmedia_api::playback::reap_orphan_streams(db, &cfg.paths.cache_dir.join("hls")).await?;
    let stale = repo::list_idle_stream_sessions(db, idle).await?;
    let mut n = 0;
    for s in stale {
        let dir = std::path::Path::new(&s.output_dir);
        if dir.starts_with(cfg.paths.cache_dir.join("hls")) {
            let _ = std::fs::remove_dir_all(dir);
            n += 1;
        }
        repo::delete_stream_session(db, s.job_id).await?;
    }
    println!("清理遗留无主 HLS 目录 {orphan} 个，已回收空闲僵尸流 {n} 个");
    Ok(())
}
