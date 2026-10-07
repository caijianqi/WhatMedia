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
    }
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
