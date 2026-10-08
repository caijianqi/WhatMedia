//! Mosaic CLI —— 运维与排障入口。
//!
//! 服务端调试基本全靠它：看配置、看迁移、扫一个目录、看看扫出来什么。
//! 它和 HTTP API 走**同一套** repo，不存在"CLI 能查到但 API 查不到"的分叉。

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use mosaic_config::AppConfig;
use mosaic_core::MediaKind;
use mosaic_database::{repo, Db};
use mosaic_scanner::{ScanConfig, ScanControl, ScanProgress, Scanner};
use mosaic_storage::LocalStorage;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "mosaic-cli", about = "Mosaic 运维命令行", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 打印解析后的最终配置（含环境变量覆盖），用于排障。
    Config {
        /// 指定 TOML 配置文件
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// 列出迁移文件及其执行顺序。
    Migrations {
        /// 迁移目录，默认取配置里的值
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// 扫描一个目录并入库。**P1 的主交付物就是这条命令。**
    Scan {
        /// 要扫描的目录
        #[arg(long)]
        path: PathBuf,
        /// 媒体库名称，缺省用目录名
        #[arg(long)]
        name: Option<String>,
        /// 调用 ffprobe 读取流信息（需要本机装有 FFmpeg）
        #[arg(long)]
        probe: bool,
        /// 并发入库上限
        #[arg(long, default_value = "4")]
        concurrency: usize,
        /// 强制全量重扫（忽略增量指纹）
        #[arg(long)]
        full: bool,
    },
    /// 列出已入库的媒体条目。
    List {
        #[arg(long, default_value = "50")]
        limit: u32,
        #[arg(long)]
        kind: Option<String>,
    },
    /// 全文检索。
    Search {
        #[arg(long)]
        q: String,
    },
    /// 列出媒体库。
    Libraries,
    /// 刮削元数据。**P2 的主交付物。**
    Scrape {
        /// 媒体库 id；不给则处理全部库
        #[arg(long)]
        library: Option<String>,
        /// 只重跑失败项
        #[arg(long)]
        retry_failed: bool,
        /// 关闭自动采用，全部进人工确认队列
        #[arg(long)]
        review_only: bool,
        /// 不下载图片（压测匹配率时用）
        #[arg(long)]
        no_images: bool,
    },
    /// 查看刮削进度。
    ScrapeStats {
        #[arg(long)]
        library: Option<String>,
    },
    /// 列出待人工确认的条目。
    Review {
        #[arg(long)]
        library: Option<String>,
    },
    /// 给某个条目搜索候选。
    Candidates {
        #[arg(long)]
        item: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        year: Option<i32>,
    },
    /// 人工确认：采用指定 Provider 的指定 id。
    Identify {
        #[arg(long)]
        item: String,
        #[arg(long, default_value = "tmdb")]
        provider: String,
        #[arg(long)]
        provider_id: String,
    },
    /// 锁定 / 解锁字段，使其不被刮削覆盖。
    Lock {
        #[arg(long)]
        item: String,
        /// 字段名，逗号分隔
        #[arg(long)]
        fields: String,
        #[arg(long)]
        unlock: bool,
    },
    /// 图片缓存统计与清理。
    Images {
        #[arg(long)]
        stats: bool,
        /// 清理超出预算的缓存
        #[arg(long)]
        evict: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = AppConfig::load(None)?;
    init_logging(&cfg);

    match cli.command {
        Command::Config { file } => {
            let c = AppConfig::load(file)?;
            println!("{}", toml::to_string_pretty(&c)?);
        }
        Command::Migrations { dir } => {
            let dir = dir.unwrap_or(cfg.paths.migrations_dir.clone());
            let files = mosaic_database::migration_files(&dir)?;
            if files.is_empty() {
                println!("(无迁移文件：{})", dir.display());
            }
            for f in files {
                println!("{}", f.display());
            }
        }
        Command::Libraries => {
            let db = open_db(&cfg).await?;
            for l in repo::list_libraries(&db).await? {
                println!("{}\t{}\t{}", l.id, l.name, l.root_path);
            }
        }
        Command::List { limit, kind } => {
            let db = open_db(&cfg).await?;
            let q = repo::ItemQuery {
                kind: kind.as_deref().and_then(parse_kind),
                page_size: limit.min(200),
                ..Default::default()
            };
            let page = repo::list_items(&db, &q).await?;
            for it in &page.data {
                println!("{}\t{:?}\t{}\t{:?}", it.id, it.kind, it.title, it.year);
            }
            println!("共 {} 条", page.page.total);
        }
        Command::Search { q } => {
            let db = open_db(&cfg).await?;
            let page = repo::search_items(&db, &q, &repo::ItemQuery::default()).await?;
            for it in &page.data {
                println!("{}\t{}", it.id, it.title);
            }
            println!("命中 {} 条", page.page.total);
        }
        Command::Scan {
            path,
            name,
            probe,
            concurrency,
            full,
        } => {
            let db = open_db(&cfg).await?;
            scan_cmd(&db, &cfg, path, name, probe, concurrency, full).await?;
        }
        Command::Scrape {
            library,
            retry_failed,
            review_only,
            no_images,
        } => {
            let db = open_db(&cfg).await?;
            scrape_cmd(&db, &cfg, library, retry_failed, review_only, no_images).await?;
        }
        Command::ScrapeStats { library } => {
            let db = open_db(&cfg).await?;
            let lib = resolve_library(&db, library.as_deref()).await?;
            let s = repo::metadata::job_stats(&db, lib.map(|l| l.id)).await?;
            println!(
                "待处理 {}\t运行中 {}\t已完成 {}\t失败 {}\t待确认 {}\t跳过 {}",
                s.pending, s.running, s.done, s.failed, s.review, s.skipped
            );
        }
        Command::Review { library } => {
            let db = open_db(&cfg).await?;
            let lib = resolve_library(&db, library.as_deref()).await?;
            for t in repo::metadata::list_review(&db, lib.map(|l| l.id), 200).await? {
                println!(
                    "{}\t{:?}\t{}\t搜索用标题：{}",
                    t.id, t.kind, t.title, t.match_title
                );
            }
        }
        Command::Candidates { item, title, year } => {
            let db = open_db(&cfg).await?;
            let scraper = build_scraper(&db, &cfg, mosaic_metadata::ScrapeConfig::default())?;
            let id = parse_id(&item)?;
            for (i, c) in scraper
                .candidates(id, title, year)
                .await?
                .iter()
                .enumerate()
            {
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
        }
        Command::Identify {
            item,
            provider,
            provider_id,
        } => {
            let db = open_db(&cfg).await?;
            let scraper = build_scraper(&db, &cfg, mosaic_metadata::ScrapeConfig::default())?;
            scraper
                .identify(parse_id(&item)?, &provider, &provider_id)
                .await?;
            println!("已采用 {provider}:{provider_id}");
        }
        Command::Lock {
            item,
            fields,
            unlock,
        } => {
            let db = open_db(&cfg).await?;
            let id = parse_id(&item)?;
            let current = repo::metadata::locked_fields_of_item(&db, id).await?;
            let mut locked = mosaic_metadata::parse_locked(&current);
            for f in fields.split(',') {
                let f = f.trim();
                if !f.is_empty() && !mosaic_metadata::set_locked(&mut locked, f, !unlock) {
                    println!("跳过非法字段名：{f}");
                }
            }
            repo::metadata::update_item_meta(
                &db,
                id,
                &repo::metadata::ItemMetaUpdate {
                    locked_fields: Some(mosaic_metadata::serialize_locked(&locked)),
                    ..Default::default()
                },
            )
            .await?;
            let mut out: Vec<String> = locked.into_iter().collect();
            out.sort();
            println!("已锁定：{:?}", out);
        }
        Command::Images { stats, evict } => {
            let db = open_db(&cfg).await?;
            let cache = mosaic_metadata::ImageCache::new(
                db,
                mosaic_metadata::ImageCacheConfig {
                    dir: cfg.images_dir(),
                    max_bytes: cfg.images.max_bytes,
                    max_entries: cfg.images.max_entries,
                    max_age: chrono::Duration::days(cfg.images.max_age_days),
                },
            )?;
            if stats {
                let s = cache.stats().await?;
                println!(
                    "条目 {}\t占用 {:.1} MB\t累计命中 {}",
                    s.entries,
                    s.bytes as f64 / 1024.0 / 1024.0,
                    s.hits
                );
            }
            if evict {
                let r = cache.evict().await?;
                println!(
                    "清理 {} 项，释放 {:.1} MB（按时间 {}，按体积 {}）",
                    r.removed,
                    r.bytes_freed as f64 / 1024.0 / 1024.0,
                    r.by_age,
                    r.by_size
                );
            }
        }
    }
    Ok(())
}

fn parse_id(s: &str) -> Result<mosaic_core::Id> {
    uuid::Uuid::parse_str(s).with_context(|| format!("不是合法的 UUID：{s}"))
}

async fn resolve_library(
    db: &Db,
    name_or_id: Option<&str>,
) -> Result<Option<mosaic_core::Library>> {
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

fn build_scraper(
    db: &Db,
    cfg: &AppConfig,
    scrape: mosaic_metadata::ScrapeConfig,
) -> Result<mosaic_metadata::Scraper> {
    let chain = mosaic_metadata::ProviderChainConfig {
        tmdb_api_key: cfg.metadata.tmdb_api_key.clone(),
        tmdb_bearer: cfg.metadata.tmdb_bearer.clone(),
        bangumi_enabled: cfg.metadata.bangumi_enabled,
        language: cfg.metadata.language.clone(),
        requests_per_second: cfg.metadata.requests_per_second,
    };
    let providers = mosaic_metadata::build_providers(&chain);
    if providers.is_empty() {
        anyhow::bail!(
            "未配置任何元数据 Provider。设置 MOSAIC_METADATA_TMDB_API_KEY \
             或 MOSAIC_METADATA_BANGUMI=1"
        );
    }
    Ok(mosaic_metadata::Scraper::new(db.clone(), providers, scrape))
}

async fn scrape_cmd(
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
        mosaic_metadata::ScrapeConfig {
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
    // 崩溃遗留的 running 任务先捞回来
    let reaped = scraper.reap_running().await?;
    if reaped > 0 {
        println!("已回收 {reaped} 个中断的任务");
    }
    println!("入队 {queued} 项，开始刮削…");

    let stats = scraper.run(lib.map(|l| l.id), Default::default()).await?;
    println!(
        "完成：匹配 {}，待确认 {}，失败 {}，跳过 {} —— 命中率 {:.1}%",
        stats.matched,
        stats.review,
        stats.failed,
        stats.skipped,
        stats.hit_rate() * 100.0
    );
    Ok(())
}

async fn open_db(cfg: &AppConfig) -> Result<Db> {
    let db = Db::open(&cfg.database.url).await?;
    db.migrate(&cfg.paths.migrations_dir).await?;
    Ok(db)
}

async fn scan_cmd(
    db: &Db,
    cfg: &AppConfig,
    path: PathBuf,
    name: Option<String>,
    probe: bool,
    concurrency: usize,
    full: bool,
) -> Result<()> {
    let root = mosaic_storage::clean_path(
        &path
            .canonicalize()
            .with_context(|| format!("目录不可访问：{}", path.display()))?,
    );

    let lib_name = name.unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "媒体库".into())
    });

    // 按 (存储, 根路径) 幂等：重复扫同一个目录不会产生第二个库。
    let library = match repo::find_library_by_root(db, "local", &root.to_string_lossy()).await? {
        Some(l) => {
            println!("复用已有媒体库：{}（{}）", l.name, l.root_path);
            l
        }
        None => {
            repo::insert_library(
                db,
                &lib_name,
                MediaKind::Mixed,
                "local",
                &root.to_string_lossy(),
                &[],
            )
            .await?
        }
    };

    let storage = Arc::new(LocalStorage::new("local", &root)?);
    let ffprobe = if probe {
        Some(Arc::new(mosaic_media::FfprobeRunner::default()))
    } else {
        None
    };

    let scanner = Scanner::new(
        storage,
        db.clone(),
        ScanConfig {
            concurrency: concurrency.max(1),
            incremental: !full,
            probe,
            ffprobe,
            ..Default::default()
        },
    );

    let (tx, mut rx) = tokio::sync::broadcast::channel::<ScanProgress>(64);
    let printer = tokio::spawn(async move {
        use std::io::Write;
        while let Ok(p) = rx.recv().await {
            print!(
                "\r扫描中 {}/{}  新增 {} 更新 {} 跳过 {} 失败 {}   ",
                p.done, p.total, p.added, p.updated, p.skipped, p.failed
            );
            let _ = std::io::stdout().flush();
            if p.finished {
                println!();
                break;
            }
        }
    });

    let stats = scanner.scan(&library, ScanControl::new(), Some(tx)).await?;
    let _ = printer.await;

    println!(
        "完成：发现 {}，新增 {}，更新 {}，跳过 {}，失败 {}，已失效 {}",
        stats.found, stats.added, stats.updated, stats.skipped, stats.failed, stats.missing
    );
    println!("数据库：{}", cfg.database.url);
    Ok(())
}

fn parse_kind(s: &str) -> Option<MediaKind> {
    serde_json::from_str::<MediaKind>(&format!("\"{s}\"")).ok()
}

fn init_logging(cfg: &AppConfig) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.logging.level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
