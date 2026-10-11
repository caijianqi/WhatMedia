//! 图片缓存行为测试。
//!
//! DoD 要求"二次访问命中率 ≥ 90%"，这条只能靠缓存策略保证，
//! 而策略的正确性（谁该被清、什么时候清）必须离线可测。

use std::path::Path;

use anyhow::Result;
use chrono::Duration;
use whatmedia_database::Db;
use whatmedia_metadata::{ImageCache, ImageCacheConfig};

fn migrations_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("migrations")
}

async fn fresh_db(dir: &Path) -> Db {
    let db = Db::open(&format!("sqlite:{}?mode=rwc", dir.join("m.db").display()))
        .await
        .unwrap();
    db.migrate(&migrations_dir()).await.unwrap();
    db
}

fn cfg(dir: &Path) -> ImageCacheConfig {
    ImageCacheConfig {
        dir: dir.join("images"),
        max_bytes: 1024 * 1024 * 1024,
        max_entries: 50_000,
        max_age: Duration::days(90),
    }
}

#[tokio::test]
async fn put_then_get_roundtrips() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db = fresh_db(tmp.path()).await;
    let cache = ImageCache::new(db.clone(), cfg(tmp.path()))?;

    let url = "https://image.tmdb.org/t/p/w500/poster.jpg";
    let key = ImageCache::key_for(url, "w500");

    assert!(cache.get(&key).await.is_none(), "冷缓存不该命中");

    let path = cache.put("poster", url, "w500", "jpg", b"hello").await?;
    assert!(path.exists());
    assert_eq!(std::fs::read(&path)?, b"hello");

    let hit = cache.get(&key).await.expect("刚写进去的必须能读到");
    assert_eq!(hit, path);

    // 二次访问命中，hits 应当增加
    let _ = cache.get(&key).await;
    let stats = cache.stats().await?;
    assert_eq!(stats.entries, 1);
    assert!(stats.hits >= 2, "命中计数应当累加：{:?}", stats);
    Ok(())
}

/// 文件被外部删掉后必须当未命中，否则会返回一个指向空气的路径。
#[tokio::test]
async fn missing_file_is_treated_as_miss() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db = fresh_db(tmp.path()).await;
    let cache = ImageCache::new(db.clone(), cfg(tmp.path()))?;

    let path = cache
        .put("poster", "https://x/a.jpg", "w500", "jpg", b"x")
        .await?;
    std::fs::remove_file(&path)?;

    let key = ImageCache::key_for("https://x/a.jpg", "w500");
    assert!(cache.get(&key).await.is_none(), "文件没了就该算未命中");
    assert_eq!(cache.stats().await?.entries, 0, "顺带应当清掉脏记录");
    Ok(())
}

/// 超出字节上限时按 LRU 清理。
#[tokio::test]
async fn evicts_when_over_budget() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db = fresh_db(tmp.path()).await;
    let mut c = cfg(tmp.path());
    c.max_bytes = 1000;
    let cache = ImageCache::new(db.clone(), c)?;

    for i in 0..10 {
        cache
            .put(
                "poster",
                &format!("https://x/{i}.jpg"),
                "w500",
                "jpg",
                &[b'x'; 200],
            )
            .await?;
        // 让后写入的访问时间明显更晚，保证 LRU 顺序稳定
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let before = cache.stats().await?;
    assert_eq!(before.entries, 10);
    assert_eq!(before.bytes, 2000);

    let report = cache.evict().await?;
    assert!(report.removed >= 5, "至少清掉一半：{report:?}");

    let after = cache.stats().await?;
    assert!(after.bytes <= 1000, "清理后不应仍超预算：{after:?}");
    Ok(())
}

/// 清理顺序：still 先于 poster。海报墙的骨架不能先没。
#[tokio::test]
async fn stills_are_evicted_before_posters() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db = fresh_db(tmp.path()).await;
    let mut c = cfg(tmp.path());
    c.max_bytes = 1000;
    let cache = ImageCache::new(db.clone(), c)?;

    // 先写 poster（更早访问），再写 still（更晚访问）
    cache
        .put(
            "poster",
            "https://x/poster.jpg",
            "w500",
            "jpg",
            &[b'p'; 500],
        )
        .await?;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    cache
        .put("still", "https://x/still.jpg", "w1280", "jpg", &[b's'; 600])
        .await?;

    cache.evict().await?;

    let poster = ImageCache::key_for("https://x/poster.jpg", "w500");
    let still = ImageCache::key_for("https://x/still.jpg", "w1280");
    assert!(cache.get(&poster).await.is_some(), "poster 应当保留");
    assert!(cache.get(&still).await.is_none(), "still 应当先被清掉");
    Ok(())
}

/// 过期条目按时间清理，即便总量没超。
#[tokio::test]
async fn expired_entries_are_purged() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db = fresh_db(tmp.path()).await;
    let mut c = cfg(tmp.path());
    c.max_age = Duration::days(1);
    let cache = ImageCache::new(db.clone(), c)?;

    let url = "https://x/old.jpg";
    let key = ImageCache::key_for(url, "w500");
    cache.put("poster", url, "w500", "jpg", b"x").await?;

    // 手动把访问时间推到 10 天前
    let old = (chrono::Utc::now() - Duration::days(10)).to_rfc3339();
    sqlx_set_accessed(&db, &key, &old).await?;

    let report = cache.evict().await?;
    assert_eq!(report.by_age, 1, "{report:?}");
    assert!(cache.get(&key).await.is_none());
    Ok(())
}

async fn sqlx_set_accessed(db: &Db, key: &str, when: &str) -> Result<()> {
    whatmedia_database::repo::image::set_accessed_for_test(db, key, when).await
}

/// 同一张图的不同尺寸互不顶替。
#[tokio::test]
async fn variants_do_not_collide() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let db = fresh_db(tmp.path()).await;
    let cache = ImageCache::new(db.clone(), cfg(tmp.path()))?;

    let url = "https://x/p.jpg";
    let a = cache.put("poster", url, "w185", "jpg", b"small").await?;
    let b = cache.put("poster", url, "w500", "jpg", b"large").await?;

    assert_ne!(a, b);
    assert_eq!(cache.stats().await?.entries, 2);
    Ok(())
}
