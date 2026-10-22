//! 媒体库与文件检索子命令模块。
//!
//! 提供目录扫描入库、增量扫描、库列表查看、媒体条目展示以及全文检索功能。

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use whatmedia_config::AppConfig;
use whatmedia_core::MediaKind;
use whatmedia_database::{repo, Db};
use whatmedia_scanner::{ScanConfig, ScanControl, ScanProgress, Scanner};
use whatmedia_storage::LocalStorage;

use crate::common::parse_kind;

/// 扫描指定的本地文件目录并入库。
///
/// 具备自动增量识别能力（基于文件 mtime/大小计算指纹），若文件无变动则直接跳过，
/// 支持多线程并发扫描与可选的 FFprobe 流探测。
pub async fn scan_cmd(
    db: &Db,
    cfg: &AppConfig,
    path: PathBuf,
    name: Option<String>,
    probe: bool,
    concurrency: usize,
    full: bool,
) -> Result<()> {
    let root = whatmedia_storage::clean_path(
        &path
            .canonicalize()
            .with_context(|| format!("媒体目录不可访问或不存在：{}", path.display()))?,
    );

    let lib_name = name.unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "媒体库".into())
    });

    // 根据 (存储驱动, 根路径) 保持幂等：重复扫描同一目录自动复用已有库，不会重复建库
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
        Some(Arc::new(whatmedia_media::FfprobeRunner::default()))
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
                "\r扫描进度 {}/{}  新增 {} 更新 {} 跳过 {} 失败 {}   ",
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
        "扫描完成：发现 {}，新增 {}，更新 {}，跳过 {}，失败 {}，已失效 {}",
        stats.found, stats.added, stats.updated, stats.skipped, stats.failed, stats.missing
    );
    println!("所属数据库：{}", cfg.database.url);
    Ok(())
}

/// 列出已登记的全部媒体库。
pub async fn libraries_cmd(db: &Db) -> Result<()> {
    let libs = repo::list_libraries(db).await?;
    if libs.is_empty() {
        println!("(当前无已登记的媒体库)");
        return Ok(());
    }
    println!("ID\t名称\t根目录路径");
    println!("------------------------------------------------------------");
    for l in libs {
        println!("{}\t{}\t{}", l.id, l.name, l.root_path);
    }
    Ok(())
}

/// 列出数据库中已入库的媒体条目。
pub async fn list_cmd(db: &Db, limit: u32, kind: Option<String>) -> Result<()> {
    let q = repo::ItemQuery {
        kind: kind.as_deref().and_then(parse_kind),
        page_size: limit.min(200),
        ..Default::default()
    };
    let page = repo::list_items(db, &q).await?;
    if page.data.is_empty() {
        println!("(未找到符合条件的媒体条目)");
        return Ok(());
    }
    for it in &page.data {
        println!(
            "{}\t{:?}\t{}\t{:?}",
            it.id,
            it.kind,
            it.title,
            it.year.unwrap_or(0)
        );
    }
    println!(
        "共 {} 条条目（展示前 {} 条）",
        page.page.total,
        page.data.len()
    );
    Ok(())
}

/// 全文检索媒体条目。
pub async fn search_cmd(db: &Db, q: &str) -> Result<()> {
    let page = repo::search_items(db, q, &repo::ItemQuery::default()).await?;
    if page.data.is_empty() {
        println!("未检索到包含 \"{q}\" 的内容");
        return Ok(());
    }
    for it in &page.data {
        println!("{}\t{}", it.id, it.title);
    }
    println!("检索命中 {} 条记录", page.page.total);
    Ok(())
}
