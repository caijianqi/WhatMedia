//! 浏览相关查询的集成测试（首页分类 tab / 详情页演职员与季列表的数据源）。
//!
//! 这几个查询是**给 UI 用的**，所以测的是 UI 真正依赖的性质，而不是"SQL 能跑"：
//!
//!   1. `kind_counts` 只返回有内容的类型（空 tab 比没 tab 更糟），且按数量降序。
//!   2. `get_item` 带出 `genres`，**没刮削时是空数组而不是 null** ——
//!      前端拿到 null 会渲染出 "null" 或者要自己判空。
//!   3. `list_seasons` 的集数是真的（不是前端拿季 id 去数），且按季号升序。
//!   4. `series_of_item` 是**反查**：条目 → 剧。电影必须返回 None，
//!      否则电影详情页会冒出一个空的"剧集"区块。

use std::path::{Path, PathBuf};
use whatmedia_core::{MediaItem, MediaKind};
use whatmedia_database::repo::*;
use whatmedia_database::Db;

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("migrations")
}

async fn db() -> Db {
    let db = Db::open_in_memory().await.unwrap();
    db.migrate(&migrations_dir()).await.unwrap();
    db
}

async fn library(db: &Db, name: &str, kind: MediaKind) -> whatmedia_core::Id {
    insert_library(
        db,
        name,
        kind,
        "local",
        &format!("/media/{name}"),
        &["tmdb".to_string()],
    )
    .await
    .unwrap()
    .id
}

fn make_item(library_id: whatmedia_core::Id, title: &str, kind: MediaKind) -> MediaItem {
    MediaItem {
        id: whatmedia_core::Id::new_v4(),
        library_id,
        kind,
        title: title.into(),
        original_title: None,
        sort_title: None,
        year: Some(2020),
        overview: Some("简介".into()),
        runtime_ms: Some(7_200_000),
        rating: Some(7.5),
        locked_fields: Vec::new(),
        provider: None,
        provider_id: None,
        added_at: chrono::Utc::now(),
    }
}

async fn add_item(
    db: &Db,
    library_id: whatmedia_core::Id,
    title: &str,
    kind: MediaKind,
) -> whatmedia_core::Id {
    let item = make_item(library_id, title, kind);
    upsert_item(db, &item).await.unwrap();
    item.id
}

#[tokio::test]
async fn kind_counts_only_reports_kinds_that_have_items() {
    let db = db().await;
    let lib = library(&db, "影", MediaKind::Movie).await;
    let _ = add_item(&db, lib, "电影一", MediaKind::Movie).await;
    let _ = add_item(&db, lib, "电影二", MediaKind::Movie).await;
    let _ = add_item(&db, lib, "第一集", MediaKind::Episode).await;

    let counts = kind_counts(&db).await.unwrap();

    // 只有 movie 与 episode 有内容 —— 库里没有的类型不该出现在 tab 上。
    assert_eq!(counts.len(), 2);
    // 数量多的排前面：最常用的分类在最左。
    assert_eq!(counts[0].kind, MediaKind::Movie);
    assert_eq!(counts[0].count, 2);
    assert_eq!(counts[1].kind, MediaKind::Episode);
    assert_eq!(counts[1].count, 1);
}

#[tokio::test]
async fn kind_counts_is_empty_on_a_fresh_library() {
    let db = db().await;
    assert!(kind_counts(&db).await.unwrap().is_empty());
}

#[tokio::test]
async fn kind_counts_reports_series_when_episodes_are_linked() {
    let db = db().await;
    let lib = library(&db, "剧", MediaKind::Series).await;
    let ep1 = add_item(&db, lib, "第一集", MediaKind::Episode).await;
    let ep2 = add_item(&db, lib, "第二集", MediaKind::Episode).await;

    let series = tv::upsert_series(&db, "早春晴朗", None).await.unwrap();
    let season = tv::upsert_season(&db, series.id, 1).await.unwrap();
    tv::upsert_episode(&db, season.id, 1, ep1, None).await.unwrap();
    tv::upsert_episode(&db, season.id, 2, ep2, None).await.unwrap();

    let counts = kind_counts(&db).await.unwrap();
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0].kind, MediaKind::Series);
    assert_eq!(counts[0].count, 1);

    // 显式聚合查询列表时，聚合为 1 个 series 卡片
    let page = list_items(&db, &ItemQuery { aggregate_series: Some(true), ..Default::default() }).await.unwrap();
    assert_eq!(page.page.total, 1);
    assert_eq!(page.data[0].kind, MediaKind::Series);
    assert_eq!(page.data[0].title, "早春晴朗");
    assert_eq!(page.data[0].episode_count, Some(2));

    // 按 Series 过滤时，自动聚合为 1 个 series 卡片
    let series_page = list_items(&db, &ItemQuery { kind: Some(MediaKind::Series), ..Default::default() }).await.unwrap();
    assert_eq!(series_page.page.total, 1);
    assert_eq!(series_page.data[0].kind, MediaKind::Series);
    assert_eq!(series_page.data[0].title, "早春晴朗");
    assert_eq!(series_page.data[0].episode_count, Some(2));

    // 默认底层查询返回 2 个原始 episode 记录
    let raw_page = list_items(&db, &ItemQuery::default()).await.unwrap();
    assert_eq!(raw_page.page.total, 2);
    assert_eq!(raw_page.data[0].kind, MediaKind::Episode);
}

#[tokio::test]
async fn item_detail_carries_genres_and_empty_array_when_never_scraped() {
    let db = db().await;
    let lib = library(&db, "影", MediaKind::Movie).await;
    let id = add_item(&db, lib, "电影一", MediaKind::Movie).await;

    // 刮削之前：空数组，不是 null。
    let before = get_item(&db, id).await.unwrap().unwrap();
    assert!(before.genres.is_empty());
    assert_eq!(before.overview.as_deref(), Some("简介"));

    metadata::set_genres(&db, id, &["动作".to_string(), "科幻".to_string()])
        .await
        .unwrap();

    let after = get_item(&db, id).await.unwrap().unwrap();
    assert_eq!(after.genres, vec!["动作".to_string(), "科幻".to_string()]);
}

#[tokio::test]
async fn seasons_are_ordered_and_carry_real_episode_counts() {
    let db = db().await;
    let lib = library(&db, "剧", MediaKind::Episode).await;
    let series = tv::upsert_series(&db, "剧集一", Some(2020)).await.unwrap();

    // 故意先建第 2 季：返回顺序必须是季号升序，而不是插入顺序。
    let s2 = tv::upsert_season(&db, series.id, 2).await.unwrap();
    let s1 = tv::upsert_season(&db, series.id, 1).await.unwrap();
    for n in 1..=3 {
        let item = add_item(&db, lib, &format!("第{n}集"), MediaKind::Episode).await;
        tv::upsert_episode(&db, s1.id, n, item, None).await.unwrap();
    }
    let item = add_item(&db, lib, "第二季第一集", MediaKind::Episode).await;
    tv::upsert_episode(&db, s2.id, 1, item, None).await.unwrap();

    let seasons = list_seasons(&db, series.id).await.unwrap();
    assert_eq!(seasons.len(), 2);
    assert_eq!(seasons[0].id, s1.id);
    assert_eq!(seasons[0].number, 1);
    assert_eq!(seasons[0].episode_count, 3);
    assert_eq!(seasons[1].number, 2);
    assert_eq!(seasons[1].episode_count, 1);
}

#[tokio::test]
async fn series_of_item_is_a_reverse_lookup_and_none_for_movies() {
    let db = db().await;
    let lib = library(&db, "剧", MediaKind::Episode).await;
    let series = tv::upsert_series(&db, "剧集一", Some(2020)).await.unwrap();
    let s1 = tv::upsert_season(&db, series.id, 1).await.unwrap();
    let ep_item = add_item(&db, lib, "第一集", MediaKind::Episode).await;
    tv::upsert_episode(&db, s1.id, 1, ep_item, Some("第一集"))
        .await
        .unwrap();

    let found = series_of_item(&db, ep_item).await.unwrap().unwrap();
    assert_eq!(found.id, series.id);
    assert_eq!(found.title, "剧集一");

    // 电影不属于任何剧。返回 None 而不是 Some(空)，
    // 否则电影详情页会渲染出一个空的"剧集"区块。
    let movie_item = add_item(&db, lib, "电影一", MediaKind::Movie).await;
    assert!(series_of_item(&db, movie_item).await.unwrap().is_none());
}

#[tokio::test]
async fn credits_split_into_cast_and_crew_by_role() {
    let db = db().await;
    let lib = library(&db, "影", MediaKind::Movie).await;
    let id = add_item(&db, lib, "电影一", MediaKind::Movie).await;

    metadata::set_credits(
        &db,
        id,
        "tmdb",
        &[
            whatmedia_core::provider::MetadataCredit {
                name: "张三".into(),
                role: "actor".into(),
                character: Some("主角".into()),
                sort_order: 0,
                provider_id: None,
            },
            whatmedia_core::provider::MetadataCredit {
                name: "王五".into(),
                role: "director".into(),
                character: None,
                sort_order: 1,
                provider_id: None,
            },
        ],
    )
    .await
    .unwrap();

    let rows = metadata::credits_of(&db, id).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].person, "张三");
    assert_eq!(rows[0].character.as_deref(), Some("主角"));
    assert_eq!(rows[1].role, "director");
}
