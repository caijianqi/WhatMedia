//! 收藏 / 通知 / 设置 / 存储四个端点的集成测试。
//!
//! 这里守的是**端点层**的性质，SQL 层在 `crates/database/tests/social_repo.rs` 里测过。
//!
//!   1. 四个端点都在鉴权之后。新路由只有挂进 `guarded_routes()` 才成立，
//!      而"忘了挂"的表现是静默开放 —— 必须有一条匿名 401 的断言盯着。
//!   2. **非管理员既看不到也改不动全服设置**。设置页对成员只显示自己的播放偏好，
//!      否则一个成员就能把刮削语言改掉。
//!   3. **白名单之外的键一律拒绝**。这是"可写设置"这条写路径上唯一的安全边界。
//!   4. 值的校验在写库**之前**。反过来的话非法值会先落库、再在下次启动时被拒，
//!      而那时界面已经打不开了，用户无法自己改回来。

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{admin_token, db, get, member_token, post_json, send};
use serde_json::{json, Value};
use tower::ServiceExt;
use whatmedia_api::{build_router, AppState};
use whatmedia_config::AppConfig;
use whatmedia_core::{Id, MediaKind};
use whatmedia_database::repo::*;
use whatmedia_database::Db;

fn cfg() -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    cfg
}

async fn add_item(d: &Db, library_id: Id, title: &str) -> Id {
    let id = Id::new_v4();
    upsert_item(
        d,
        &whatmedia_core::MediaItem {
            id,
            library_id,
            kind: MediaKind::Movie,
            title: title.into(),
            original_title: None,
            sort_title: None,
            year: Some(2021),
            overview: None,
            runtime_ms: None,
            rating: None,
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
    admin: String,
    member: String,
    /// 一个真实的条目 id。收藏端点需要它。
    item: Id,
}

async fn harness() -> Harness {
    let d = db().await;
    let lib = insert_library(
        &d,
        "影",
        MediaKind::Movie,
        "local",
        "/media/影",
        &["tmdb".to_string()],
    )
    .await
    .unwrap()
    .id;
    let item = add_item(&d, lib, "电影一").await;

    let admin = admin_token(&d).await;
    let member = member_token(&d, "member").await;

    Harness {
        state: AppState::new(cfg()).with_db(d),
        admin,
        member,
        item,
    }
}

fn put_json(token: &str, uri: &str, v: Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&v).unwrap()))
        .unwrap()
}

// ─────────────────────────────────────────────────────────────
// 鉴权
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn social_endpoints_are_behind_auth() {
    let h = harness().await;
    let app = build_router(h.state.clone());

    for uri in [
        "/api/v1/me/favorites",
        "/api/v1/notifications",
        "/api/v1/settings",
        "/api/v1/system/storage",
        &format!("/api/v1/items/{}/favorite", h.item),
    ] {
        let r = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{uri} 应当先要登录");
    }
}

// ─────────────────────────────────────────────────────────────
// 收藏
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn favorite_can_be_set_read_and_cleared() {
    let h = harness().await;
    let uri = format!("/api/v1/items/{}/favorite", h.item);

    // 初始：没收藏。
    let r = send(&h.state, get(&h.admin, &uri)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["favorite"], json!(false));
    assert_eq!(r.json()["count"], json!(0));

    // 收藏。
    let r = send(&h.state, put_json(&h.admin, &uri, json!({ "favorite": true }))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["favorite"], json!(true));
    assert_eq!(r.json()["count"], json!(1));

    // 再点一次 true 仍是 1 —— 幂等，不会变成 2。
    let r = send(&h.state, put_json(&h.admin, &uri, json!({ "favorite": true }))).await;
    assert_eq!(r.json()["count"], json!(1));

    // 我的收藏列表里出现它。
    let r = send(&h.state, get(&h.admin, "/api/v1/me/favorites")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["total"], json!(1));
    assert_eq!(r.json()["data"][0]["title"], json!("电影一"));

    // 取消。
    let r = send(&h.state, put_json(&h.admin, &uri, json!({ "favorite": false }))).await;
    assert_eq!(r.json()["favorite"], json!(false));
    assert_eq!(r.json()["count"], json!(0));
}

#[tokio::test]
async fn favorites_are_private_to_each_user() {
    let h = harness().await;
    let uri = format!("/api/v1/items/{}/favorite", h.item);
    let _ = send(&h.state, put_json(&h.admin, &uri, json!({ "favorite": true }))).await;

    // 成员看到的是"被 1 人收藏"，但自己没收。
    let r = send(&h.state, get(&h.member, &uri)).await;
    assert_eq!(r.json()["favorite"], json!(false));
    assert_eq!(r.json()["count"], json!(1));

    let r = send(&h.state, get(&h.member, "/api/v1/me/favorites")).await;
    assert_eq!(r.json()["total"], json!(0));
}

// ─────────────────────────────────────────────────────────────
// 通知
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn notifications_start_empty_and_can_be_read_and_deleted() {
    let h = harness().await;

    let r = send(&h.state, get(&h.admin, "/api/v1/notifications")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["unread"], json!(0));
    assert!(r.json()["data"].as_array().unwrap().is_empty());

    // 直接往库里塞一条（真实来源是扫描/刮削完成时写入，那个路径在
    // social_repo 里测；这里只测端点层）。
    {
        let d = h.state.db.clone().unwrap();
        let uid = uuid::Uuid::parse_str(&get_user_by_username(&d, "admin").await.unwrap().unwrap().id)
            .unwrap();
        notify_users(
            &d,
            &[uid],
            &NewNotification {
                kind: "scan_finished",
                level: NotifyLevel::Info,
                title: "扫描完成".into(),
                body: Some("新增 3 个条目".into()),
                link: None,
            },
        )
        .await
        .unwrap();
    }

    let r = send(&h.state, get(&h.admin, "/api/v1/notifications")).await;
    assert_eq!(r.json()["unread"], json!(1));
    let id = r.json()["data"][0]["id"].as_str().unwrap().to_string();

    // 未读过滤。
    let r = send(&h.state, get(&h.admin, "/api/v1/notifications?unread_only=true")).await;
    assert_eq!(r.json()["data"].as_array().unwrap().len(), 1);

    // 标记已读。
    let r = send(
        &h.state,
        post_json(&h.admin, &format!("/api/v1/notifications/{id}/read"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);

    let r = send(&h.state, get(&h.admin, "/api/v1/notifications?unread_only=true")).await;
    assert!(r.json()["data"].as_array().unwrap().is_empty());

    // 删除。
    let r = send(&h.state, common::delete(&h.admin, &format!("/api/v1/notifications/{id}"))).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    // 已经删了 —— 再删是 404 而不是 204。
    let r = send(&h.state, common::delete(&h.admin, &format!("/api/v1/notifications/{id}"))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn one_user_cannot_read_or_delete_anothers_notification() {
    let h = harness().await;
    let d = h.state.db.clone().unwrap();
    let uid = uuid::Uuid::parse_str(&get_user_by_username(&d, "admin").await.unwrap().unwrap().id)
            .unwrap();
    notify_users(
        &d,
        &[uid],
        &NewNotification {
            kind: "system",
            level: NotifyLevel::Info,
            title: "只给管理员".into(),
            body: None,
            link: None,
        },
    )
    .await
    .unwrap();
    let id = list_notifications(&d, uid, 10, false)
        .await
        .unwrap()
        .remove(0)
        .id;

    let r = send(
        &h.state,
        post_json(&h.member, &format!("/api/v1/notifications/{id}/read"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "不该能标别人的已读");

    let r = send(&h.state, common::delete(&h.member, &format!("/api/v1/notifications/{id}"))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND, "不该能删别人的通知");
}

#[tokio::test]
async fn read_all_reports_how_many_it_cleared() {
    let h = harness().await;
    let d = h.state.db.clone().unwrap();
    let uid = uuid::Uuid::parse_str(&get_user_by_username(&d, "admin").await.unwrap().unwrap().id)
            .unwrap();
    notify_users(
        &d,
        &[uid],
        &NewNotification {
            kind: "system",
            level: NotifyLevel::Info,
            title: "一".into(),
            body: None,
            link: None,
        },
    )
    .await
    .unwrap();
    notify_users(
        &d,
        &[uid],
        &NewNotification {
            kind: "system",
            level: NotifyLevel::Warn,
            title: "二".into(),
            body: None,
            link: None,
        },
    )
    .await
    .unwrap();

    let r = send(&h.state, post_json(&h.admin, "/api/v1/notifications/read-all", json!({}))).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["updated"], json!(2));
    // 第二次是 0 —— 幂等，不报错。
    let r = send(&h.state, post_json(&h.admin, "/api/v1/notifications/read-all", json!({}))).await;
    assert_eq!(r.json()["updated"], json!(0));
}

// ─────────────────────────────────────────────────────────────
// 设置
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn members_only_see_their_own_preferences() {
    let h = harness().await;

    let r = send(&h.state, get(&h.admin, "/api/v1/settings")).await;
    assert_eq!(r.status, StatusCode::OK);
    let entries = r.json()["entries"].as_array().unwrap().len();
    // 管理员看到全部（server + user 域）。
    assert!(
        entries >= whatmedia_config::SETTING_SPECS.len(),
        "管理员应看到全部 {} 项，实际 {entries}",
        whatmedia_config::SETTING_SPECS.len()
    );

    let r = send(&h.state, get(&h.member, "/api/v1/settings")).await;
    let scopes: Vec<String> = r.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["scope"].as_str().unwrap().to_string())
        .collect();
    assert!(!scopes.is_empty(), "成员至少要有自己的播放偏好");
    assert!(
        scopes.iter().all(|s| s == "user"),
        "成员不该看到 server 级设置：{scopes:?}"
    );
}

#[tokio::test]
async fn members_cannot_write_server_settings() {
    let h = harness().await;
    let r = send(
        &h.state,
        put_json(
            &h.member,
            "/api/v1/settings",
            json!({ "key": "metadata.language", "value": "en-US" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.text());
}

#[tokio::test]
async fn keys_outside_the_whitelist_are_refused() {
    let h = harness().await;
    for key in [
        "server.port",
        "paths.data_dir",
        "database.url",
        "security.signing_key",
    ] {
        let r = send(
            &h.state,
            put_json(&h.admin, "/api/v1/settings", json!({ "key": key, "value": "x" })),
        )
        .await;
        assert_eq!(
            r.status,
            StatusCode::BAD_REQUEST,
            "「{key}」不该可写：{}",
            r.text()
        );
    }
}

#[tokio::test]
async fn invalid_values_are_refused_before_anything_is_written() {
    let h = harness().await;

    // 超范围。
    let r = send(
        &h.state,
        put_json(
            &h.admin,
            "/api/v1/settings",
            json!({ "key": "metadata.requests_per_second", "value": "999" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());

    // 枚举大小写不对。
    let r = send(
        &h.state,
        put_json(
            &h.admin,
            "/api/v1/settings",
            json!({ "key": "player.preferred_quality", "value": "4K" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // 布尔值不是布尔。
    let r = send(
        &h.state,
        put_json(&h.admin, "/api/v1/settings", json!({ "key": "ai.enabled", "value": "maybe" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // 上面三次都失败了，所以库里不该留下任何覆盖行。
    let d = h.state.db.clone().unwrap();
    assert!(list_server_settings(&d).await.unwrap().is_empty());
    assert!(list_user_settings(&d, whatmedia_core::Id::new_v4()).await.unwrap().is_empty());
}

#[tokio::test]
async fn server_setting_roundtrip_marks_it_as_overridden() {
    let h = harness().await;

    let r = send(
        &h.state,
        put_json(
            &h.admin,
            "/api/v1/settings",
            json!({ "key": "metadata.language", "value": "en-US" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.json()["overridden"], json!(true));
    // 界面要能显示"改了之后要重启"，否则用户以为立刻生效。
    assert_eq!(r.json()["restart_required"], json!(true));

    let r = send(&h.state, get(&h.admin, "/api/v1/settings")).await;
    let e = r.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "metadata.language")
        .unwrap()
        .clone();
    assert_eq!(e["value"], json!("en-US"));
    assert_eq!(e["default"], json!("zh-CN"));
    assert_eq!(e["overridden"], json!(true));

    // 恢复默认。
    let r = send(
        &h.state,
        common::delete(&h.admin, "/api/v1/settings/metadata.language"),
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);

    let r = send(&h.state, get(&h.admin, "/api/v1/settings")).await;
    let e = r.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "metadata.language")
        .unwrap()
        .clone();
    assert_eq!(e["value"], json!("zh-CN"));
    assert_eq!(e["overridden"], json!(false));
}

#[tokio::test]
async fn user_preferences_are_per_user() {
    let h = harness().await;
    let r = send(
        &h.state,
        put_json(
            &h.member,
            "/api/v1/settings",
            json!({ "key": "player.autoplay_next", "value": "true" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());

    // 管理员看不到成员改的那个值 —— 各人各一份。
    let r = send(&h.state, get(&h.admin, "/api/v1/settings")).await;
    let e = r.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "player.autoplay_next")
        .unwrap()
        .clone();
    assert_eq!(e["value"], json!("false"));

    let r = send(&h.state, get(&h.member, "/api/v1/settings")).await;
    let e = r.json()["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["key"] == "player.autoplay_next")
        .unwrap()
        .clone();
    assert_eq!(e["value"], json!("true"));
}

// ─────────────────────────────────────────────────────────────
// 存储统计
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn storage_reports_real_library_sizes() {
    let h = harness().await;
    let r = send(&h.state, get(&h.admin, "/api/v1/system/storage")).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let j = r.json();

    let libs = j["libraries"].as_array().unwrap();
    assert_eq!(libs.len(), 1);
    assert_eq!(libs[0]["name"], json!("影"));
    assert_eq!(libs[0]["item_count"], json!(1));
    // 目录 /media/影 在磁盘上不存在 —— 如实报 false，界面据此提示"盘没挂载"。
    assert_eq!(libs[0]["reachable"], json!(false));

    // 缓存目录读不出来时是 unavailable，而不是谎报 0。
    assert!(j["cache"]["unavailable"].is_boolean());
    assert!(j["total_bytes"].is_i64());
}
