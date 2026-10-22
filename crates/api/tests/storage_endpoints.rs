//! 存储源账号与目录浏览 API 端点集成测试。

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use whatmedia_api::AppState;
use whatmedia_config::AppConfig;

#[tokio::test]
async fn storage_account_endpoints_and_remote_library_creation() {
    let db = db().await;
    let token = admin_token(&db).await;
    let mut state = AppState::new(AppConfig::default());
    state.db = Some(db);

    // 1. 列表初始为空
    let res = send(&state, get_authed(&token, "/api/v1/storages")).await;
    assert_eq!(res.status, StatusCode::OK);
    let list: Vec<serde_json::Value> = serde_json::from_value(res.json()).unwrap();
    assert!(list.is_empty());

    // 2. 添加一个 WebDAV 存储源（挂载百度网盘）
    let res = send(
        &state,
        post_authed(
            &token,
            "/api/v1/storages",
            json!({
                "name": "百度网盘-影视库",
                "driver": "webdav",
                "endpoint": "http://127.0.0.1:5244/dav/baidu",
                "auth_type": "basic",
                "username": "admin",
                "password": "password123",
                "root_path": "/影视",
                "direct_stream": true
            }),
        ),
    )
    .await;

    assert_eq!(res.status, StatusCode::CREATED);
    let created = res.json();
    let storage_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["name"], "百度网盘-影视库");
    assert_eq!(created["driver"], "webdav");
    assert_eq!(created["direct_stream"], true);

    // 3. 详情查询
    let res = send(
        &state,
        get_authed(&token, &format!("/api/v1/storages/{storage_id}")),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let detail = res.json();
    assert_eq!(detail["id"], storage_id);

    // 4. 更新配置
    let res = send(
        &state,
        put_authed(
            &token,
            &format!("/api/v1/storages/{storage_id}"),
            json!({
                "name": "百度网盘-4K高清库",
                "direct_stream": false
            }),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let updated = res.json();
    assert_eq!(updated["name"], "百度网盘-4K高清库");
    assert_eq!(updated["direct_stream"], false);

    // 5. 浏览本地存储目录
    let res = send(&state, get_authed(&token, "/api/v1/storages/local/browse")).await;
    assert_eq!(res.status, StatusCode::OK);
    let local_entries: Vec<serde_json::Value> = serde_json::from_value(res.json()).unwrap();
    assert!(!local_entries.is_empty());

    // 6. 核心联动：基于远程存储源创建媒体库 (不再受限于本地磁盘 Path::is_dir 校验)
    let res = send(
        &state,
        post_authed(
            &token,
            "/api/v1/libraries",
            json!({
                "name": "百度网盘电影",
                "path": "/影视/电影",
                "kind": "movie",
                "storage_provider": storage_id
            }),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::CREATED);
    let lib = res.json();
    assert_eq!(lib["name"], "百度网盘电影");
    assert_eq!(lib["storage_provider"], storage_id);
    assert_eq!(lib["root_path"], "/影视/电影");

    // 7. 删除存储源
    let res = send(
        &state,
        delete_authed(&token, &format!("/api/v1/storages/{storage_id}")),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);

    let res = send(
        &state,
        get_authed(&token, &format!("/api/v1/storages/{storage_id}")),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND);
}
