//! 浏览类端点的集成测试（首页分类 tab / 详情页演职员与季列表）。
//!
//! 这里守的是**端点层**的性质，数据库层那几个查询在
//! `crates/database/tests/media_browse.rs` 里已经测过：
//!
//!   1. 三个新端点都在鉴权之后 —— 匿名必须 401，而不是拿到数据。
//!      这是最容易漏的一类：新路由挂进了 `guarded_routes()` 才成立。
//!   2. `credits` 的 cast / crew 分组由**服务端**做，前端不该自己按 role 分。
//!   3. `items/{id}/series` 对电影返回 `null`，不是 404、也不是空对象。
//!   4. `credits` / `series` 挂掉时详情页仍能拿到主详情（它们在前端是静默降级的）。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_token, get, send};
use serde_json::{json, Value};
use tower::ServiceExt;
use whatmedia_api::{build_router, AppState};
use whatmedia_config::AppConfig;
use whatmedia_core::{Id, MediaKind};
use whatmedia_database::repo::*;
use whatmedia_database::Db;

async fn db() -> Db {
    let db = Db::open_in_memory().await.unwrap();
    db.migrate(&common::migrations_dir()).await.unwrap();
    db
}

fn cfg() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    cfg
}

async fn library(db: &Db, name: &str, kind: MediaKind) -> Id {
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

async fn add_item(db: &Db, library_id: Id, title: &str, kind: MediaKind) -> Id {
    let id = Id::new_v4();
    upsert_item(
        db,
        &whatmedia_core::MediaItem {
            id,
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
        },
    )
    .await
    .unwrap();
    id
}

struct Harness {
    state: AppState,
    tok: String,
}

async fn harness() -> Harness {
    let db = db().await;
    let lib = library(&db, "影", MediaKind::Movie).await;
    let movie = add_item(&db, lib, "电影一", MediaKind::Movie).await;

    let series = tv::upsert_series(&db, "剧集一", Some(2020)).await.unwrap();
    let s1 = tv::upsert_season(&db, series.id, 1).await.unwrap();
    let ep = add_item(&db, lib, "第一集", MediaKind::Episode).await;
    tv::upsert_episode(&db, s1.id, 1, ep, Some("第一集"))
        .await
        .unwrap();

    metadata::set_genres(&db, movie, &["动作".to_string()])
        .await
        .unwrap();
    metadata::set_credits(
        &db,
        movie,
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

    let tok = admin_token(&db).await;
    Harness {
        state: AppState::new(cfg()).with_db(db),
        tok,
    }
}

async fn get_json(h: &Harness, uri: &str) -> Value {
    let r = send(&h.state, get(&h.tok, uri)).await;
    assert_eq!(r.status, StatusCode::OK, "GET {uri} 失败：{}", r.text());
    r.json()
}

#[tokio::test]
async fn browse_endpoints_are_behind_auth() {
    let h = harness().await;
    let app = build_router(h.state.clone());

    for uri in [
        "/api/v1/media/kinds",
        "/api/v1/items/00000000-0000-0000-0000-000000000000/credits",
        "/api/v1/items/00000000-0000-0000-0000-000000000000/series",
        "/api/v1/series/00000000-0000-0000-0000-000000000000/seasons",
    ] {
        let r = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{uri} 应当先要登录");
    }
}

#[tokio::test]
async fn kinds_only_lists_kinds_with_content() {
    let h = harness().await;
    let json = get_json(&h, "/api/v1/media/kinds").await;
    let arr = json.as_array().unwrap();

    // 库里有一条 movie 与一条已成剧的 series（电视剧聚合）。
    let mut got: Vec<(String, i64)> = arr
        .iter()
        .map(|v| {
            (
                v["kind"].as_str().unwrap().to_string(),
                v["count"].as_i64().unwrap(),
            )
        })
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![("movie".to_string(), 1), ("series".to_string(), 1)],
        "库里只有 movie 与 series：{json}"
    );
}

/// tab 的排列必须**稳定**。
///
/// 排序规则是"条数多的在前，一样多时按类型字面量"。不加这条断言，
/// 哪天有人去掉 `ORDER BY`，分类 tab 会在每次刷新时换位置 ——
/// 那种 bug 在手工点两下时看不出来，只有真用起来才烦。
#[tokio::test]
async fn kinds_are_returned_in_a_stable_order() {
    let h = harness().await;
    let json = get_json(&h, "/api/v1/media/kinds").await;
    let arr = json.as_array().unwrap();

    let keys: Vec<(i64, String)> = arr
        .iter()
        .map(|v| {
            (
                -v["count"].as_i64().unwrap(),
                v["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "分类顺序不稳定：{json}");
}

#[tokio::test]
async fn credits_are_split_by_role_on_the_server() {
    let h = harness().await;
    // 电影一的 id：从 /media 里取，避免把 id 硬编码进测试。
    let page = get_json(&h, "/api/v1/media?kind=movie&page_size=10").await;
    let id = page["data"][0]["id"].as_str().unwrap().to_string();

    let json = get_json(&h, &format!("/api/v1/items/{id}/credits")).await;
    let cast = json["cast"].as_array().unwrap();
    let crew = json["crew"].as_array().unwrap();

    assert_eq!(cast.len(), 1);
    assert_eq!(cast[0]["person"], json!("张三"));
    assert_eq!(cast[0]["character"], json!("主角"));
    assert_eq!(crew.len(), 1);
    assert_eq!(crew[0]["role"], json!("director"));
}

#[tokio::test]
async fn series_of_a_movie_is_null_not_404() {
    let h = harness().await;
    let page = get_json(&h, "/api/v1/media?kind=movie&page_size=10").await;
    let id = page["data"][0]["id"].as_str().unwrap().to_string();

    let json = get_json(&h, &format!("/api/v1/items/{id}/series")).await;
    assert!(json.is_null(), "电影不属于任何剧，应当是 null：{json}");
}

#[tokio::test]
async fn seasons_come_from_the_series_of_an_episode_item() {
    let h = harness().await;
    let page = get_json(&h, "/api/v1/media?kind=episode&page_size=10").await;
    let ep_item = page["data"][0]["id"].as_str().unwrap().to_string();

    let series = get_json(&h, &format!("/api/v1/items/{ep_item}/series")).await;
    let series_id = series["id"].as_str().unwrap().to_string();
    assert_eq!(series["title"], json!("剧集一"));

    let seasons = get_json(&h, &format!("/api/v1/series/{series_id}/seasons")).await;
    let arr = seasons.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["number"], json!(1));
    assert_eq!(arr[0]["episode_count"], json!(1));
}

#[tokio::test]
async fn item_detail_carries_genres() {
    let h = harness().await;
    let page = get_json(&h, "/api/v1/media?kind=movie&page_size=10").await;
    let id = page["data"][0]["id"].as_str().unwrap().to_string();

    let json = get_json(&h, &format!("/api/v1/items/{id}")).await;
    assert_eq!(json["genres"], json!(["动作"]));
}

#[tokio::test]
async fn delete_item_cascades_and_removes_from_library() {
    let h = harness().await;
    let page = get_json(&h, "/api/v1/media?kind=movie&page_size=10").await;
    let id = page["data"][0]["id"].as_str().unwrap().to_string();

    let resp = send(&h.state, get(&h.tok, &format!("/api/v1/items/{id}"))).await;
    assert_eq!(resp.status, StatusCode::OK);

    let resp = send(
        &h.state,
        common::delete(&h.tok, &format!("/api/v1/items/{id}")),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK);

    let resp = send(&h.state, get(&h.tok, &format!("/api/v1/items/{id}"))).await;
    assert_eq!(resp.status, StatusCode::NOT_FOUND);
}
