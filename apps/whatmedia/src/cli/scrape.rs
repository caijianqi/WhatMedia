//! 元数据刮削与人工审核子命令模块。
//!
//! 负责影片/剧集海报、剧情简介、演职员表刮削抓取，
//! 支持人工候选搜索确认（Identify）、元数据字段锁定（Lock）及审核队列管理。

use anyhow::Result;
use whatmedia_config::AppConfig;
use whatmedia_database::{repo, Db};

use crate::common::{parse_id, resolve_library};

/// 构造元数据刮削器实例。
pub fn build_scraper(
    db: &Db,
    cfg: &AppConfig,
    scrape: whatmedia_metadata::ScrapeConfig,
) -> Result<whatmedia_metadata::Scraper> {
    let chain = whatmedia_metadata::ProviderChainConfig {
        tmdb_api_key: cfg.metadata.tmdb_api_key.clone(),
        tmdb_bearer: cfg.metadata.tmdb_bearer.clone(),
        bangumi_enabled: cfg.metadata.bangumi_enabled,
        language: cfg.metadata.language.clone(),
        requests_per_second: cfg.metadata.requests_per_second,
    };
    let providers = whatmedia_metadata::build_providers(&chain);
    if providers.is_empty() {
        anyhow::bail!(
            "未配置任何元数据 Provider。请设置环境变量 WHATMEDIA_METADATA_TMDB_API_KEY \
             或 WHATMEDIA_METADATA_BANGUMI=1"
        );
    }
    Ok(whatmedia_metadata::Scraper::new(
        db.clone(),
        providers,
        scrape,
    ))
}

/// 执行批量刮削任务。
pub async fn scrape_cmd(
    db: &Db,
    cfg: &AppConfig,
    library: Option<String>,
    retry_failed: bool,
    review_only: bool,
    no_images: bool,
) -> Result<()> {
    let lib = resolve_library(db, library.as_deref()).await?;
    let scraper = build_scraper(
        db,
        cfg,
        whatmedia_metadata::ScrapeConfig {
            auto_accept: !review_only,
            fetch_images: !no_images,
            ..Default::default()
        },
    )?;

    let queued = match (&lib, retry_failed) {
        (Some(l), true) => scraper.requeue_failed(l.id).await?,
        (Some(l), false) => scraper.enqueue(l.id).await?,
        (None, _) => 0,
    };

    // 崩溃遗留的正在运行任务先回收
    let reaped = scraper.reap_running().await?;
    if reaped > 0 {
        println!("已回收 {reaped} 个因异常中断的历史任务");
    }
    println!("已入队 {queued} 项，开始并行刮削元数据…");

    let stats = scraper.run(lib.map(|l| l.id), Default::default()).await?;
    println!(
        "刮削结束：匹配成功 {}，待人工确认 {}，失败 {}，跳过 {} —— 整体命中率 {:.1}%",
        stats.matched,
        stats.review,
        stats.failed,
        stats.skipped,
        stats.hit_rate() * 100.0
    );
    Ok(())
}

/// 查看指定或全部媒体库的刮削队列统计。
pub async fn scrape_stats_cmd(db: &Db, library: Option<String>) -> Result<()> {
    let lib = resolve_library(db, library.as_deref()).await?;
    let s = repo::metadata::job_stats(db, lib.map(|l| l.id)).await?;
    println!(
        "待处理 {}\t运行中 {}\t已完成 {}\t失败 {}\t待确认 {}\t跳过 {}",
        s.pending, s.running, s.done, s.failed, s.review, s.skipped
    );
    Ok(())
}

/// 列出当前待人工审核确认元数据的条目。
pub async fn review_cmd(db: &Db, library: Option<String>) -> Result<()> {
    let lib = resolve_library(db, library.as_deref()).await?;
    let list = repo::metadata::list_review(db, lib.map(|l| l.id), 200).await?;
    if list.is_empty() {
        println!("(当前无待人工确认的条目)");
        return Ok(());
    }
    for t in list {
        println!(
            "{}\t{:?}\t{}\t搜索用标题：{}",
            t.id, t.kind, t.title, t.match_title
        );
    }
    Ok(())
}

/// 针对指定条目在线搜索匹配候选（Candidates）。
pub async fn candidates_cmd(
    db: &Db,
    cfg: &AppConfig,
    item: &str,
    title: Option<String>,
    year: Option<i32>,
) -> Result<()> {
    let scraper = build_scraper(db, cfg, whatmedia_metadata::ScrapeConfig::default())?;
    let id = parse_id(item)?;
    let list = scraper.candidates(id, title, year).await?;
    if list.is_empty() {
        println!("未搜索到任何候选匹配");
        return Ok(());
    }
    for (i, c) in list.iter().enumerate() {
        println!(
            "{}. [{}/{}] {} ({:?})\t得分 {:.3}\t{}",
            i + 1,
            c.provider,
            c.provider_id,
            c.title,
            c.year,
            c.score,
            c.confidence
        );
    }
    Ok(())
}

/// 人工确认指定条目的元数据采用项。
pub async fn identify_cmd(
    db: &Db,
    cfg: &AppConfig,
    item: &str,
    provider: &str,
    provider_id: &str,
) -> Result<()> {
    let scraper = build_scraper(db, cfg, whatmedia_metadata::ScrapeConfig::default())?;
    scraper
        .identify(parse_id(item)?, provider, provider_id)
        .await?;
    println!("已确认采用 {provider}:{provider_id} 作为该条目的元数据源");
    Ok(())
}

/// 锁定或解锁条目的元数据字段，避免后续自动刮削覆盖人工编辑内容。
pub async fn lock_cmd(db: &Db, item: &str, fields: &str, unlock: bool) -> Result<()> {
    let id = parse_id(item)?;
    let current = repo::metadata::locked_fields_of_item(db, id).await?;
    let mut locked = whatmedia_metadata::parse_locked(&current);
    for f in fields.split(',') {
        let f = f.trim();
        if !f.is_empty() && !whatmedia_metadata::set_locked(&mut locked, f, !unlock) {
            println!("跳过非法或未知字段名：{f}");
        }
    }
    repo::metadata::update_item_meta(
        db,
        id,
        &repo::metadata::ItemMetaUpdate {
            locked_fields: Some(whatmedia_metadata::serialize_locked(&locked)),
            ..Default::default()
        },
    )
    .await?;
    let mut out: Vec<String> = locked.into_iter().collect();
    out.sort();
    if unlock {
        println!("已更新锁定状态，当前锁定字段：{:?}", out);
    } else {
        println!("已成功锁定字段：{:?}", out);
    }
    Ok(())
}
