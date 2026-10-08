//! 刮削流水线端到端测试。
//!
//! 全部用 `FakeProvider` 离线跑 —— 命中率、字段锁定、断点续跑这些
//! 最重要的行为不能依赖"有没有 TMDB 的 API Key"。

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use mosaic_core::{
    deterministic_id, MediaItem, MediaKind, MetadataImage, MetadataProvider, MetadataQuery,
    MetadataRecord, MosaicError, Result as CoreResult,
};
use mosaic_database::{repo, Db};
use mosaic_metadata::{matcher::MatchThresholds, ScrapeConfig, Scraper};
use std::path::Path;

fn migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("migrations")
}

async fn fresh_db(dir: &Path) -> Db {
    let db = Db::open(&format!(
        "sqlite:{}?mode=rwc",
        dir.join("mosaic.db").display()
    ))
    .await
    .unwrap();
    db.migrate(&migrations_dir()).await.unwrap();
    db
}

async fn make_movie(
    db: &Db,
    library_id: mosaic_core::Id,
    title: &str,
    year: Option<i32>,
) -> mosaic_core::Id {
    let id = deterministic_id("item", &["test", title, &year.unwrap_or(0).to_string()]);
    let item = MediaItem {
        id,
        library_id,
        kind: MediaKind::Movie,
        title: title.into(),
        original_title: None,
        sort_title: None,
        year,
        overview: None,
        runtime_ms: None,
        rating: None,
        locked_fields: Vec::new(),
        provider: None,
        provider_id: None,
        added_at: Utc::now(),
    };
    repo::upsert_item(db, &item).await.unwrap();
    id
}

/// 一个"什么都返回"的假 Provider。打分与筛选交给真正的逻辑做。
struct FakeProvider {
    records: Vec<MetadataRecord>,
}

impl FakeProvider {
    fn new(records: Vec<MetadataRecord>) -> Self {
        Self { records }
    }
}

#[async_trait::async_trait]
impl MetadataProvider for FakeProvider {
    fn id(&self) -> &str {
        "fake"
    }
    async fn search_movie(&self, _q: &MetadataQuery) -> CoreResult<Vec<MetadataRecord>> {
        Ok(self.records.clone())
    }
    async fn search_series(&self, _q: &MetadataQuery) -> CoreResult<Vec<MetadataRecord>> {
        Ok(self.records.clone())
    }
    async fn get_movie(&self, id: &str) -> CoreResult<MetadataRecord> {
        self.records
            .iter()
            .find(|r| r.provider_id == id)
            .cloned()
            .ok_or_else(|| MosaicError::Metadata {
                provider: "fake".into(),
                message: format!("没有 {id}"),
            })
    }
    async fn get_series(&self, id: &str) -> CoreResult<MetadataRecord> {
        self.get_movie(id).await
    }
    async fn get_episode(&self, _s: &str, _se: i32, ep: i32) -> CoreResult<MetadataRecord> {
        Ok(MetadataRecord {
            title: format!("第 {ep} 集"),
            ..Default::default()
        })
    }
}

fn matrix_record() -> MetadataRecord {
    MetadataRecord {
        provider: "fake".into(),
        provider_id: "603".into(),
        title: "The Matrix".into(),
        original_title: Some("The Matrix".into()),
        sort_title: Some("Matrix, The".into()),
        year: Some(1999),
        tagline: Some("Welcome to the Real World".into()),
        overview: Some("一名黑客发现世界是模拟的".into()),
        runtime_ms: Some(8_160_000),
        rating: Some(8.2),
        genres: vec!["动作".into(), "科幻".into()],
        images: vec![MetadataImage {
            kind: "poster".into(),
            url: "https://example.com/poster.jpg".into(),
            width: Some(500),
            height: Some(750),
            vote: None,
        }],
        credits: vec![
            mosaic_core::MetadataCredit {
                name: "基努·里维斯".into(),
                role: "actor".into(),
                character: Some("Neo".into()),
                sort_order: 0,
                provider_id: Some("6384".into()),
            },
            mosaic_core::MetadataCredit {
                name: "沃卓斯基".into(),
                role: "director".into(),
                character: None,
                sort_order: 1000,
                provider_id: None,
            },
        ],
    }
}

fn other_record() -> MetadataRecord {
    MetadataRecord {
        provider: "fake".into(),
        provider_id: "1".into(),
        title: "Mad Max".into(),
        year: Some(1979),
        ..Default::default()
    }
}

async fn scraper(db: Db, cfg: ScrapeConfig) -> Scraper {
    let p: Arc<dyn MetadataProvider> =
        Arc::new(FakeProvider::new(vec![matrix_record(), other_record()]));
    Scraper::new(db, vec![p], cfg)
}

#[tokio::test]
async fn scrape_writes_full_metadata() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = fresh_db(dir.path()).await;
    let lib = repo::insert_library(&db, "测试库", MediaKind::Movie, "local", "/media", &[]).await?;
    let id = make_movie(&db, lib.id, "The Matrix", Some(1999)).await;

    let s = scraper(db.clone(), ScrapeConfig::default()).await;
    s.enqueue(lib.id).await?;
    let stats = s.run(Some(lib.id), Default::default()).await?;

    assert_eq!(stats.matched, 1, "{stats:?}");
    assert_eq!(stats.hit_rate(), 1.0);

    let detail = repo::get_item(&db, id).await?.unwrap();
    assert_eq!(detail.item.title, "The Matrix");
    assert_eq!(detail.overview.as_deref(), Some("一名黑客发现世界是模拟的"));
    assert_eq!(detail.item.rating, Some(8.2));
    assert_eq!(detail.item.runtime_ms, Some(8_160_000));

    let genres = repo::metadata::genres_of(&db, id).await?;
    assert_eq!(genres, vec!["动作", "科幻"]);

    let credits = repo::metadata::credits_of(&db, id).await?;
    assert_eq!(credits.len(), 2);
    assert!(credits
        .iter()
        .any(|c| c.role == "director" && c.person == "沃卓斯基"));
    assert!(credits
        .iter()
        .any(|c| c.role == "actor" && c.character.as_deref() == Some("Neo")));

    let images = repo::image::list_for_item(&db, id).await?;
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].kind, "poster");
    Ok(())
}

/// DoD 对应项：元数据写入后不得覆盖用户手动修改的字段。
#[tokio::test]
async fn locked_title_survives_rescrape() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = fresh_db(dir.path()).await;
    let lib = repo::insert_library(&db, "测试库", MediaKind::Movie, "local", "/media", &[]).await?;
    // 扫描器按文件名建条目（match_title 由此固定为 The Matrix）
    let id = make_movie(&db, lib.id, "The Matrix", Some(1999)).await;

    // 用户手动改成自己喜欢的译名并锁定
    repo::metadata::update_item_meta(
        &db,
        id,
        &repo::metadata::ItemMetaUpdate {
            title: Some("黑客帝国".into()),
            locked_fields: Some(r#"["title"]"#.into()),
            ..Default::default()
        },
    )
    .await?;

    let s = scraper(db.clone(), ScrapeConfig::default()).await;
    s.enqueue(lib.id).await?;
    let stats = s.run(Some(lib.id), Default::default()).await?;
    assert_eq!(stats.matched, 1, "锁定不该导致匹配失败：{stats:?}");

    let detail = repo::get_item(&db, id).await?.unwrap();
    assert_eq!(detail.item.title, "黑客帝国", "锁定字段被覆盖了");
    // 未锁定的字段照常更新
    assert_eq!(detail.overview.as_deref(), Some("一名黑客发现世界是模拟的"));
    Ok(())
}

/// DoD 对应项：刮削任务可断点续跑。
#[tokio::test]
async fn second_run_does_not_reprocess() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = fresh_db(dir.path()).await;
    let lib = repo::insert_library(&db, "测试库", MediaKind::Movie, "local", "/media", &[]).await?;
    for i in 0..5 {
        make_movie(&db, lib.id, &format!("The Matrix {i}"), Some(1999)).await;
    }

    let s = scraper(db.clone(), ScrapeConfig::default()).await;
    s.enqueue(lib.id).await?;

    let first = s.run(Some(lib.id), Default::default()).await?;
    assert_eq!(first.matched + first.review, 5, "{first:?}");

    let second = s.run(Some(lib.id), Default::default()).await?;
    assert_eq!(second.matched, 0);
    assert_eq!(second.review, 0);
    assert_eq!(second.skipped, 0, "已完成的条目不该被重跑：{second:?}");

    // 只有"重试失败项"才会重新入队
    let requeued = s.requeue_failed(lib.id).await?;
    assert_eq!(requeued, 0);
    Ok(())
}

/// 进程崩溃时卡在 running 的任务必须能被捞回来，否则会永久丢失。
#[tokio::test]
async fn crashed_jobs_are_reaped() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = fresh_db(dir.path()).await;
    let lib = repo::insert_library(&db, "测试库", MediaKind::Movie, "local", "/media", &[]).await?;
    make_movie(&db, lib.id, "The Matrix", Some(1999)).await;

    let s = scraper(db.clone(), ScrapeConfig::default()).await;
    s.enqueue(lib.id).await?;
    // 模拟"领了任务就崩了"
    let claimed = repo::metadata::claim_jobs(&db, Some(lib.id), 10).await?;
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].status, "running");

    let s2 = scraper(db.clone(), ScrapeConfig::default()).await;
    let _ = s2.run(Some(lib.id), Default::default()).await?; // 此时不该处理任何东西
    let after_first = repo::metadata::job_stats(&db, Some(lib.id)).await?;
    assert_eq!(after_first.running, 1);

    let reaped = s2.reap_running().await?;
    assert_eq!(reaped, 1);

    let stats = s2.run(Some(lib.id), Default::default()).await?;
    assert_eq!(stats.matched, 1, "崩溃的任务应当被重新处理：{stats:?}");
    Ok(())
}

/// 关掉自动采用后，所有结果都进人工确认队列。
#[tokio::test]
async fn conservative_mode_routes_to_review() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = fresh_db(dir.path()).await;
    let lib = repo::insert_library(&db, "测试库", MediaKind::Movie, "local", "/media", &[]).await?;
    make_movie(&db, lib.id, "The Matrix", Some(1999)).await;

    let s = scraper(
        db.clone(),
        ScrapeConfig {
            auto_accept: false,
            ..Default::default()
        },
    )
    .await;
    s.enqueue(lib.id).await?;
    let stats = s.run(Some(lib.id), Default::default()).await?;
    assert_eq!(stats.review, 1, "{stats:?}");
    assert_eq!(stats.matched, 0);

    let pending = repo::metadata::list_review(&db, Some(lib.id), 10).await?;
    assert_eq!(pending.len(), 1);

    // 人工确认后落库
    s.identify(pending[0].id, "fake", "603").await?;
    let after = repo::metadata::job_stats(&db, Some(lib.id)).await?;
    assert_eq!(after.done, 1);
    Ok(())
}

/// Provider 里根本没有这部片 → skipped，不是 failed。
#[tokio::test]
async fn unrelated_library_is_skipped() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = fresh_db(dir.path()).await;
    let lib = repo::insert_library(&db, "测试库", MediaKind::Movie, "local", "/media", &[]).await?;
    make_movie(&db, lib.id, "一个完全不存在的片子", Some(2020)).await;

    let s = scraper(
        db.clone(),
        ScrapeConfig {
            thresholds: MatchThresholds {
                auto_accept: 0.86,
                reject: 0.8,
                margin: 0.06,
            },
            ..Default::default()
        },
    )
    .await;
    s.enqueue(lib.id).await?;
    let stats = s.run(Some(lib.id), Default::default()).await?;
    assert_eq!(stats.skipped, 1, "{stats:?}");
    assert_eq!(stats.matched, 0);
    assert_eq!(stats.hit_rate(), 0.0);
    Ok(())
}

/// 候选列表要按分数降序给 UI，且带 confidence。
#[tokio::test]
async fn candidates_are_sorted_for_ui() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = fresh_db(dir.path()).await;
    let lib = repo::insert_library(&db, "测试库", MediaKind::Movie, "local", "/media", &[]).await?;
    let id = make_movie(&db, lib.id, "The Matrix", Some(1999)).await;

    let s = scraper(db.clone(), ScrapeConfig::default()).await;
    let cands = s.candidates(id, None, None).await?;
    assert!(cands.len() >= 2, "{cands:?}");
    assert_eq!(cands[0].title, "The Matrix");
    assert!(cands[0].score > cands[1].score);
    assert!(cands[0].poster.is_some());
    assert!(!cands[0].confidence.is_empty());
    Ok(())
}
