//! 多源聚合搜索与采集源管理 API 端点集成测试。

mod common;

use axum::http::StatusCode;
use common::*;
use serde_json::json;
use whatmedia_api::AppState;
use whatmedia_config::AppConfig;

#[tokio::test]
async fn aggregator_settings_and_search_endpoints() {
    let db = db().await;
    let token = admin_token(&db).await;
    let mut state = AppState::new(AppConfig::default());
    state.db = Some(db);

    // 1. 采集源列表初始为空
    let res = send(&state, get_authed(&token, "/api/v1/settings/vod_sources")).await;
    assert_eq!(res.status, StatusCode::OK);
    let list: Vec<serde_json::Value> = serde_json::from_value(res.json()).unwrap();
    assert!(list.is_empty());

    // 2. 导入 MoonTV 格式的配置 (兼容 MoonTV config.json 规范)
    let moontv_config = json!({
        "api_site": {
            "lz_source": {
                "name": "量子资源站",
                "api": "https://cj.lziapi.com/api.php/provide/vod/",
                "active": true
            },
            "ff_source": {
                "name": "非凡资源站",
                "api": "https://cj.ffzyapi.com/api.php/provide/vod/",
                "active": false
            }
        }
    });

    let res = send(
        &state,
        post_authed(&token, "/api/v1/settings/vod_sources", moontv_config),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json()["imported"], 2);

    // 3. 再次查询采集源列表，验证已导入
    let res = send(&state, get_authed(&token, "/api/v1/settings/vod_sources")).await;
    assert_eq!(res.status, StatusCode::OK);
    let list: Vec<serde_json::Value> = serde_json::from_value(res.json()).unwrap();
    assert_eq!(list.len(), 2);

    // 4. 切换状态 (Toggle)
    let res = send(
        &state,
        put_authed(
            &token,
            "/api/v1/settings/vod_sources/ff_source/toggle",
            json!({}),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    assert_eq!(res.json()["is_active"], true);

    // 5. 聚合搜索接口验证 (空关键词)
    let res = send(&state, get_authed(&token, "/api/v1/search/aggregate?q=")).await;
    assert_eq!(res.status, StatusCode::OK);
    let search_res = res.json();
    assert_eq!(search_res["local"]["data"].as_array().unwrap().len(), 0);
    assert_eq!(search_res["remote"].as_array().unwrap().len(), 0);

    // 6. 流媒体代理非法 URL 校验
    let res = send(
        &state,
        get_authed(&token, "/api/v1/proxy/vod_stream?url=not_a_valid_url"),
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST);

    // 7. 删除采集源
    let res = send(
        &state,
        delete_authed(&token, "/api/v1/settings/vod_sources/lz_source"),
    )
    .await;
    assert_eq!(res.status, StatusCode::NO_CONTENT);

    // 验证只剩 1 个
    let res = send(&state, get_authed(&token, "/api/v1/settings/vod_sources")).await;
    let list_after: Vec<serde_json::Value> = serde_json::from_value(res.json()).unwrap();
    assert_eq!(list_after.len(), 1);
    assert_eq!(list_after[0]["id"], "ff_source");
}

#[tokio::test]
async fn child_and_member_role_permissions_for_vod() {
    let db = db().await;
    let _admin_tok = admin_token(&db).await;
    let member_tok = member_token(&db, "test_member").await;
    let child_tok = child_token(&db, "test_child").await;

    let mut state = AppState::new(AppConfig::default());
    state.db = Some(db);

    // 1. 普通用户与儿童无权管理采集源 (403 FORBIDDEN)
    let res = send(
        &state,
        get_authed(&member_tok, "/api/v1/settings/vod_sources"),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    let res = send(
        &state,
        get_authed(&child_tok, "/api/v1/settings/vod_sources"),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    let res = send(
        &state,
        post_authed(&member_tok, "/api/v1/settings/vod_sources", json!([])),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    // 2. 儿童拉取三方在线流被严格拦截 (403 CHILD_RESTRICTED)
    let res = send(
        &state,
        get_authed(
            &child_tok,
            "/api/v1/proxy/vod_stream?url=https://example.com/stream.m3u8",
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let err_json = res.json();
    assert_eq!(err_json["error"]["code"], "CHILD_RESTRICTED");

    // 3. 儿童进行聚合搜索时，三方 remote 结果强制屏蔽置空
    let res = send(
        &state,
        get_authed(&child_tok, "/api/v1/search/aggregate?q=星际穿越"),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let search_res = res.json();
    // remote 数组应严格为空，杜绝未分级网络流触达儿童
    assert!(search_res["remote"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn search_with_string_query_parameters_succeeds() {
    let db = db().await;
    let token = admin_token(&db).await;

    let mut state = AppState::new(AppConfig::default());
    state.db = Some(db);

    // 验证 /api/v1/search?q=狂飙&page_size=6 正常反序列化并不再报 "invalid type: string 6, expected u32"
    let res = send(
        &state,
        get_authed(&token, "/api/v1/search?q=%E7%8B%82%E9%A3%99&page_size=6"),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let json = res.json();
    assert_eq!(json["page"]["page_size"], 6);

    // 验证 /api/v1/search/aggregate?q=狂飙&page_size=6 同样正常反序列化
    let res = send(
        &state,
        get_authed(
            &token,
            "/api/v1/search/aggregate?q=%E7%8B%82%E9%A3%99&page_size=6",
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK);
    let json = res.json();
    assert_eq!(json["local"]["page"]["page_size"], 6);
}
