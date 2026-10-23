use anyhow::Result;
use std::path::{Path, PathBuf};
use whatmedia_aggregator::VodSourceConfig;
use whatmedia_database::{db::Db, repo};

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("migrations")
}

async fn test_db() -> Db {
    let db = Db::open_in_memory().await.unwrap();
    db.migrate(&migrations_dir()).await.unwrap();
    db
}

#[tokio::test]
async fn test_vod_source_and_cache_crud() -> Result<()> {
    let db = test_db().await;

    // 初始状态应为空
    let initial = repo::list_vod_sources(&db).await?;
    assert!(initial.is_empty());

    // 写入第一个源
    let s1 = VodSourceConfig {
        id: "api_lz".into(),
        name: "量子资源".into(),
        api_url: "https://cj.lziapi.com/api.php/provide/vod/".into(),
        is_active: true,
        weight: 10,
        timeout_ms: 2500,
    };
    repo::upsert_vod_source(&db, &s1).await?;

    // 写入第二个源
    let s2 = VodSourceConfig {
        id: "api_ff".into(),
        name: "非凡资源".into(),
        api_url: "https://cj.ffzyapi.com/api.php/provide/vod/".into(),
        is_active: false,
        weight: 20,
        timeout_ms: 3000,
    };
    repo::upsert_vod_source(&db, &s2).await?;

    // 查询所有源 (应为 2 个)
    let all = repo::list_vod_sources(&db).await?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, "api_lz");
    assert_eq!(all[1].id, "api_ff");

    // 查询启用源 (应为 1 个)
    let active = repo::list_active_vod_sources(&db).await?;
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "api_lz");

    // 单个查询
    let fetched = repo::get_vod_source(&db, "api_lz").await?;
    assert!(fetched.is_some());
    assert_eq!(fetched.unwrap().name, "量子资源");

    // 更新 s2 为启用
    let mut s2_updated = s2.clone();
    s2_updated.is_active = true;
    repo::upsert_vod_source(&db, &s2_updated).await?;
    let active_after = repo::list_active_vod_sources(&db).await?;
    assert_eq!(active_after.len(), 2);

    // 删除 s1
    let deleted = repo::delete_vod_source(&db, "api_lz").await?;
    assert!(deleted);
    let after_del = repo::list_vod_sources(&db).await?;
    assert_eq!(after_del.len(), 1);

    // 测试搜索缓存
    let now = chrono::Utc::now().timestamp();
    repo::set_search_cache(&db, "hash_123", "{\"res\":1}", now + 100).await?;
    let cache_hit = repo::get_search_cache(&db, "hash_123", now).await?;
    assert_eq!(cache_hit, Some("{\"res\":1}".into()));

    // 过期清理
    let cleaned = repo::cleanup_expired_cache(&db, now + 200).await?;
    assert_eq!(cleaned, 1);
    let cache_after_clean = repo::get_search_cache(&db, "hash_123", now).await?;
    assert_eq!(cache_after_clean, None);

    Ok(())
}
