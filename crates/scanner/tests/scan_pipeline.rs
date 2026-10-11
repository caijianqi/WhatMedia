//! 扫描 → 入库 → 查询 的端到端测试。
//!
//! 这是 P1 存在意义的验证：不是"某个函数对不对"，
//! 而是"给一个目录，能不能得到一个能查的媒体库"。

use anyhow::Result;
use whatmedia_core::MediaKind;
use whatmedia_database::{repo, Db};
use whatmedia_scanner::{ScanConfig, ScanControl, Scanner};
use whatmedia_storage::LocalStorage;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("migrations")
}

async fn fresh_db(dir: &Path) -> Db {
    let path = dir.join("whatmedia.db");
    let url = format!("sqlite:{}?mode=rwc", path.display());
    let db = Db::open(&url).await.unwrap();
    db.migrate(&migrations_dir()).await.unwrap();
    db
}

/// 造一个小型但结构完整的样本库。
fn build_sample_library(root: &Path) {
    let movies = root.join("Movies");
    let tv = root.join("TV").join("Breaking Bad").join("Season 01");
    let hidden = root.join(".hidden");
    std::fs::create_dir_all(&movies).unwrap();
    std::fs::create_dir_all(&tv).unwrap();
    std::fs::create_dir_all(&hidden).unwrap();

    touch(
        &movies.join("The.Matrix.1999.1080p.BluRay.x264-FGT.mkv"),
        b"m1",
    );
    touch(&movies.join("Inception.2010.720p.mkv"), b"m2");
    touch(&movies.join("readme.txt"), b"ignored");
    touch(&hidden.join("Should.Not.Be.Scanned.2020.mkv"), b"hidden");

    touch(&tv.join("Breaking.Bad.S01E01.1080p.mkv"), b"e1");
    touch(&tv.join("Breaking.Bad.S01E02.1080p.mkv"), b"e2");

    // NFO 优先级最高：它应当把 Inception 的标题改掉
    std::fs::write(
        movies.join("Inception.2010.720p.nfo"),
        r#"<movie><title>盗梦空间</title><originaltitle>Inception</originaltitle><year>2010</year></movie>"#,
    )
    .unwrap();
}

fn touch(path: &Path, content: &[u8]) {
    std::fs::write(path, content).unwrap();
}

async fn scan_once(db: &Db, root: &Path, cfg: ScanConfig) -> whatmedia_scanner::ScanStats {
    let library = repo::insert_library(
        db,
        "样本库",
        MediaKind::Mixed,
        "local",
        &root.to_string_lossy(),
        &[],
    )
    .await
    .unwrap();
    let storage = Arc::new(LocalStorage::new("local", root).unwrap());
    Scanner::new(storage, db.clone(), cfg)
        .scan(&library, ScanControl::new(), None)
        .await
        .unwrap()
}

#[tokio::test]
async fn scan_builds_a_queryable_library() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    build_sample_library(tmp.path());

    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;
    let stats = scan_once(
        &db,
        tmp.path(),
        ScanConfig {
            probe: false,
            ..Default::default()
        },
    )
    .await;

    // 隐藏目录与 txt 都不该进来
    assert_eq!(stats.found, 4, "只应发现 4 个视频文件：{stats:?}");
    assert_eq!(stats.added, 4);
    assert_eq!(stats.failed, 0);

    let page = repo::list_items(&db, &repo::ItemQuery::default()).await?;
    assert_eq!(page.page.total, 4);

    let titles: Vec<String> = page.data.iter().map(|i| i.title.clone()).collect();
    assert!(titles.contains(&"The Matrix".to_string()), "{titles:?}");
    assert!(
        titles.contains(&"盗梦空间".to_string()),
        "NFO 应覆盖标题：{titles:?}"
    );
    assert_eq!(
        titles.iter().filter(|t| *t == "Breaking Bad").count(),
        2,
        "两集应归到同一部剧：{titles:?}"
    );

    // 剧集结构
    let series = repo::list_series(&db, repo::tv::PageParams::default()).await?;
    assert_eq!(series.page.total, 1);
    assert_eq!(series.data[0].title, "Breaking Bad");
    assert_eq!(series.data[0].season_count, 1);
    assert_eq!(series.data[0].episode_count, 2);

    Ok(())
}

#[tokio::test]
async fn incremental_scan_skips_unchanged_files() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    build_sample_library(tmp.path());
    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;

    let cfg = ScanConfig {
        probe: false,
        ..Default::default()
    };
    let first = scan_once(&db, tmp.path(), cfg.clone()).await;
    assert_eq!(first.added, 4);

    // 第二次扫描走增量分支：指纹没变，一个都不该重新入库。
    let library = repo::list_libraries(&db).await?.pop().unwrap();
    let storage = Arc::new(LocalStorage::new("local", tmp.path())?);
    let second = Scanner::new(storage, db.clone(), cfg)
        .scan(&library, ScanControl::new(), None)
        .await?;

    assert_eq!(second.found, 4);
    assert_eq!(second.skipped, 4, "无变更应全部跳过：{second:?}");
    assert_eq!(second.added, 0);
    assert_eq!(second.updated, 0);

    // 重复扫描不得让条目数增长
    let page = repo::list_items(&db, &repo::ItemQuery::default()).await?;
    assert_eq!(page.page.total, 4);

    Ok(())
}

#[tokio::test]
async fn changed_file_is_updated_not_duplicated() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    build_sample_library(tmp.path());
    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;

    let cfg = ScanConfig {
        probe: false,
        ..Default::default()
    };
    scan_once(&db, tmp.path(), cfg.clone()).await;

    // 改动一个文件：指纹变了 → 走更新分支
    std::fs::write(
        tmp.path()
            .join("Movies")
            .join("The.Matrix.1999.1080p.BluRay.x264-FGT.mkv"),
        b"changed content",
    )?;

    let library = repo::list_libraries(&db).await?.pop().unwrap();
    let storage = Arc::new(LocalStorage::new("local", tmp.path())?);
    let s = Scanner::new(storage, db.clone(), cfg)
        .scan(&library, ScanControl::new(), None)
        .await?;

    assert_eq!(s.updated, 1, "改动的文件应被更新：{s:?}");
    let page = repo::list_items(&db, &repo::ItemQuery::default()).await?;
    assert_eq!(page.page.total, 4, "更新不得产生重复条目");
    Ok(())
}

#[tokio::test]
async fn removed_file_is_marked_unavailable_not_deleted() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    build_sample_library(tmp.path());
    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;

    let cfg = ScanConfig {
        probe: false,
        ..Default::default()
    };
    scan_once(&db, tmp.path(), cfg.clone()).await;

    std::fs::remove_file(
        tmp.path()
            .join("TV")
            .join("Breaking Bad")
            .join("Season 01")
            .join("Breaking.Bad.S01E02.1080p.mkv"),
    )?;

    let library = repo::list_libraries(&db).await?.pop().unwrap();
    let storage = Arc::new(LocalStorage::new("local", tmp.path())?);
    let s = Scanner::new(storage, db.clone(), cfg)
        .scan(&library, ScanControl::new(), None)
        .await?;

    assert_eq!(s.missing, 1, "消失的文件应被标记为失效：{s:?}");
    // 条目仍在库里 —— 移动硬盘拔掉不该让用户的元数据消失
    let page = repo::list_items(&db, &repo::ItemQuery::default()).await?;
    assert_eq!(page.page.total, 4);
    Ok(())
}

#[tokio::test]
async fn scan_is_cancellable() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    build_sample_library(tmp.path());
    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;

    let library = repo::insert_library(
        &db,
        "x",
        MediaKind::Mixed,
        "local",
        &tmp.path().to_string_lossy(),
        &[],
    )
    .await?;
    let storage = Arc::new(LocalStorage::new("local", tmp.path())?);
    let ctrl = ScanControl::new();
    ctrl.cancel();

    let s = Scanner::new(
        storage,
        db.clone(),
        ScanConfig {
            probe: false,
            ..Default::default()
        },
    )
    .scan(&library, ctrl, None)
    .await?;

    assert!(s.cancelled, "取消标记应被传回：{s:?}");
    Ok(())
}

#[tokio::test]
async fn chinese_search_hits_by_substring() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    build_sample_library(tmp.path());
    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;
    scan_once(
        &db,
        tmp.path(),
        ScanConfig {
            probe: false,
            ..Default::default()
        },
    )
    .await;

    // trigram 分词器让中文子串可检索
    let hit = repo::search_items(&db, "盗梦", &repo::ItemQuery::default()).await?;
    assert_eq!(
        hit.page.total,
        1,
        "中文子串检索应命中：{:?}",
        hit.data.len()
    );
    assert_eq!(hit.data[0].title, "盗梦空间");

    // 短词（< 3 字符）降级到 LIKE，也必须能命中
    let short = repo::search_items(&db, "Matrix", &repo::ItemQuery::default()).await?;
    assert!(short.page.total >= 1);
    Ok(())
}

/// 10,000 文件基线。**默认不跑**（需要几分钟），用于建立 P1 DoD 里的性能基线：
/// `cargo test -p whatmedia-scanner -- --ignored --nocapture scan_10k_baseline`
#[tokio::test]
#[ignore]
async fn scan_10k_baseline() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    for i in 0..10_000 {
        let dir = tmp.path().join(format!("d{:03}", i % 100));
        std::fs::create_dir_all(&dir)?;
        touch(
            &dir.join(format!("Movie.{i:05}.2019.1080p.BluRay.x264.mkv")),
            b"x",
        );
    }

    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;
    let start = std::time::Instant::now();
    let stats = scan_once(
        &db,
        tmp.path(),
        ScanConfig {
            probe: false,
            concurrency: 8,
            ..Default::default()
        },
    )
    .await;
    let full = start.elapsed();

    let library = repo::list_libraries(&db).await?.pop().unwrap();
    let storage = Arc::new(LocalStorage::new("local", tmp.path())?);
    let start = std::time::Instant::now();
    let inc = Scanner::new(
        storage,
        db.clone(),
        ScanConfig {
            probe: false,
            concurrency: 8,
            ..Default::default()
        },
    )
    .scan(&library, ScanControl::new(), None)
    .await?;
    let incremental = start.elapsed();

    println!(
        "\n=== 10k 基线 ===\n全量扫描 {:?}（新增 {}）\n增量扫描 {:?}（跳过 {}）\n",
        full, stats.added, incremental, inc.skipped
    );
    // DoD：无变更增量扫描 ≤ 30s
    assert!(
        incremental.as_secs() <= 30,
        "增量扫描 {:?} 超过 30s 预算",
        incremental
    );
    Ok(())
}

/// 并发压力：20 集同时入库，不得产生重复的 series / season。
///
/// 这条测试守护的是"先 SELECT 再 INSERT"那个经典竞态 ——
/// 没有唯一约束时，20 个并发任务会创建出 20 条同名 series。
#[tokio::test]
async fn concurrent_episodes_do_not_duplicate_series() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let season = tmp.path().join("Some Show").join("Season 02");
    std::fs::create_dir_all(&season)?;
    for i in 1..=20 {
        touch(
            &season.join(format!("Some.Show.S02E{i:02}.1080p.WEB-DL.mkv")),
            b"x",
        );
    }

    let db_dir = tempfile::tempdir()?;
    let db = fresh_db(db_dir.path()).await;
    let library = repo::insert_library(
        &db,
        "压力库",
        MediaKind::Mixed,
        "local",
        &tmp.path().to_string_lossy(),
        &[],
    )
    .await?;

    let storage = Arc::new(LocalStorage::new("local", tmp.path())?);
    let stats = Scanner::new(
        storage,
        db.clone(),
        ScanConfig {
            concurrency: 8,
            probe: false,
            ..Default::default()
        },
    )
    .scan(&library, ScanControl::new(), None)
    .await?;

    assert_eq!(stats.found, 20, "{stats:?}");
    assert_eq!(stats.failed, 0, "{stats:?}");

    let series = repo::list_series(&db, repo::tv::PageParams::default()).await?;
    assert_eq!(
        series.page.total, 1,
        "并发不得产生重复剧集：{:?}",
        series.data
    );
    assert_eq!(series.data[0].season_count, 1);
    assert_eq!(series.data[0].episode_count, 20);

    let episodes = repo::list_episodes(&db, &repo::tv::EpisodeQuery::default()).await?;
    assert_eq!(episodes.page.total, 20);
    Ok(())
}
