//! P7 AI 端点集成测试。
//!
//! 跑真实 HTTP 往返：真的 router、真的 SQLite（迁移后的内存库）。
//! 三条必须被证明的事：
//!
//!   1. **没配模型也能用。** `ai.enabled = true` 但 provider 不存在时，
//!      `/ai/chat` 靠规则引擎回答并如实标注 `degraded`。这是 P7 的立身之本 ——
//!      家里跑不起模型的人不该被挡在门外。
//!   2. **AI 关掉时端点 503，且只影响 `/ai/*`。** `/api/v1/media` 照常 200。
//!   3. **危险操作必须两步。** chat 只出工单，confirm 才真的动库；
//!      令牌用过一次即失效。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use std::path::Path;
use tower::ServiceExt;
use whatmedia_api::{build_router, AppState};
use whatmedia_config::AppConfig;
use whatmedia_core::Id;
use whatmedia_database::Db;

mod common;
use common::{admin_token, db, get, post_json};

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

async fn send(state: &AppState, req: Request<Body>) -> Reply {
    let res = build_router(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    Reply { status, body }
}

/// 建一个库 + 一部没海报、没字幕、没正文的电影。
///
/// 三个"没有"是刻意的：规则引擎那条"缺海报的电影"正是靠它们命中的，
/// 顺带也验证了 `has_poster` 的判别方向没写反。
async fn seed(db: &Db, root: &Path) -> Id {
    let lib = Id::new_v4();
    sqlx::query(
        "INSERT INTO library (id, name, kind, storage_provider, root_path, created_at)
         VALUES (?,'测试库','movie','local',?,'2026-01-01T00:00:00Z')",
    )
    .bind(lib.to_string())
    .bind(root.to_string_lossy().to_string())
    .execute(db.pool())
    .await
    .unwrap();

    let item = Id::new_v4();
    // 与既有测试同一套写法：从 library 里取 id，顺带把 FTS 列填上，
    // 免得触发器在插入时抱怨列数对不上。
    sqlx::query(
        "INSERT INTO media_item (id, library_id, kind, title, year, locked_fields, added_at, match_title)
         SELECT ?, id, 'movie', ?, 2014, '[]', '2026-01-02T00:00:00Z', ? FROM library LIMIT 1",
    )
    .bind(item.to_string())
    .bind("星际穿越")
    .bind("星际穿越")
    .execute(db.pool())
    .await
    .unwrap();
    item
}

async fn state_with_ai(allow_dangerous: bool) -> (AppState, String, Id, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = db().await;
    let tok = admin_token(&db).await;
    let item = seed(&db, dir.path()).await;
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    cfg.ai.enabled = true;
    cfg.ai.allow_dangerous = allow_dangerous;
    cfg.ai.provider = "nonexistent".into();
    let state = AppState::new(cfg)
        .with_db(db)
        .with_playback(None, Vec::new(), Default::default())
        .with_ai();
    (state, tok, item, dir)
}

// ─────────────────────────────────────────────────────────────
// 降级
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn chat_answers_via_rules_when_no_model_is_configured() {
    let (state, tok, item, _dir) = state_with_ai(false).await;
    let r = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/chat",
            json!({ "message": "找出没有海报的电影" }),
        ),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let j = r.json();
    // 没有可用模型（provider 名不认识），必须走规则引擎并如实标注
    assert_eq!(j["degraded"], true, "没模型时必须标 degraded");
    assert!(
        j["traces"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "规则引擎也必须走工具层并留痕：{j}"
    );
    // 「找出没有海报的电影」被规则引擎路由到专用工具，而不是通用搜索 ——
    // 说明意图解析真的在按语义分流，不是一律丢给 search_media。
    assert_eq!(j["traces"][0]["tool"], "find_missing_poster");
    let _ = item;
}

#[tokio::test]
async fn rules_path_finds_the_posterless_movie() {
    let (state, tok, _item, _dir) = state_with_ai(false).await;
    let r = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/chat",
            json!({ "message": "哪些电影缺海报" }),
        ),
    )
    .await;
    let j = r.json();
    let answer = j["answer"].as_str().unwrap_or("");
    assert!(
        answer.contains("星际穿越"),
        "应该查到那部没海报的电影，实际回答：{answer}"
    );
}

#[tokio::test]
async fn unparsable_question_says_so_instead_of_pretending() {
    let (state, tok, _item, _dir) = state_with_ai(false).await;
    let r = send(
        &state,
        post_json(&tok, "/api/v1/ai/chat", json!({ "message": "嗯" })),
    )
    .await;
    let j = r.json();
    assert!(
        j["answer"].as_str().unwrap_or("").contains("没理解"),
        "听不懂就该直说：{j}"
    );
    assert!(j["traces"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn empty_message_is_rejected() {
    let (state, tok, _item, _dir) = state_with_ai(false).await;
    let r = send(
        &state,
        post_json(&tok, "/api/v1/ai/chat", json!({ "message": "   " })),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

// ─────────────────────────────────────────────────────────────
// 开关与隔离
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn disabled_ai_returns_503_and_leaves_core_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db = db().await;
    let _item = seed(&db, dir.path()).await;
    let tok = admin_token(&db).await;
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    cfg.ai.enabled = false;
    let state = AppState::new(cfg).with_db(db).with_ai();

    let r = send(
        &state,
        post_json(&tok, "/api/v1/ai/chat", json!({ "message": "找电影" })),
    )
    .await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.json()["error"]["code"], "AI_DISABLED");

    // 关键：AI 关掉不影响核心端点
    let core = send(&state, get(&tok, "/api/v1/media")).await;
    assert_eq!(core.status, StatusCode::OK, "核心端点不该受 AI 影响");
}

#[tokio::test]
async fn core_endpoints_still_work_while_ai_is_broken() {
    let (state, tok, _item, _dir) = state_with_ai(false).await;
    // AI 配了一个根本不存在的 provider，等于"坏了"
    let chat = send(
        &state,
        post_json(&tok, "/api/v1/ai/chat", json!({ "message": "找电影" })),
    )
    .await;
    assert_eq!(chat.status, StatusCode::OK);
    assert_eq!(chat.json()["degraded"], true);
    let core = send(&state, get(&tok, "/api/v1/media")).await;
    assert_eq!(core.status, StatusCode::OK);
}

#[tokio::test]
async fn ai_runtime_is_not_built_without_a_database() {
    // 没数据库时**不该装出运行时** —— 装出来也答不了任何问题。
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    cfg.ai.enabled = true;
    let state = AppState::new(cfg).with_ai();
    assert!(state.ai.is_none(), "没数据库时不该装出 AI 运行时");

    // 顺带记下这条**顺序**上的事实：没数据库就没有任何会话可被验证，
    // 于是请求在鉴权层就被拦成 401，永远到不了 DB_UNAVAILABLE 那个分支。
    // 这不是退化 —— 数据库缺失是防御性分支，真正的生产里服务根本起不来。
    let r = send(
        &state,
        Request::builder()
            .method("POST")
            .uri("/api/v1/ai/chat")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&json!({ "message": "找电影" })).unwrap(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

// ─────────────────────────────────────────────────────────────
// 工具清单
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn tool_list_hides_dangerous_tools_by_default() {
    let (state, tok, _item, _dir) = state_with_ai(false).await;
    let r = send(&state, get(&tok, "/api/v1/ai/tools")).await;
    assert_eq!(r.status, StatusCode::OK);
    let names: Vec<String> = r.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"search_media".to_string()), "{names:?}");
    assert!(
        !names.contains(&"scan_library".to_string()),
        "危险工具未开启时不该出现在清单里：{names:?}"
    );
}

#[tokio::test]
async fn tool_list_shows_dangerous_tools_when_allowed() {
    let (state, tok, _item, _dir) = state_with_ai(true).await;
    let r = send(&state, get(&tok, "/api/v1/ai/tools")).await;
    let names: Vec<String> = r.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"scan_library".to_string()), "{names:?}");
}

// ─────────────────────────────────────────────────────────────
// 确认式执行
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn dangerous_tool_stops_at_preview_until_confirmed() {
    let (state, tok, _item, _dir) = state_with_ai(true).await;
    let r = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/chat",
            json!({ "message": "扫描一下媒体库", "user_id": "u1", "role": "adult" }),
        ),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let j = r.json();
    let pending = j["pending"]
        .as_object()
        .expect("危险操作必须产出待确认工单");
    let token = pending["token"].as_str().unwrap().to_string();
    assert!(!token.is_empty());
    assert!(
        pending["summary"].as_str().unwrap().contains("扫描"),
        "预览必须说清要干什么：{pending:?}"
    );

    let c = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/confirm",
            json!({ "token": token, "user_id": "u1", "role": "adult" }),
        ),
    )
    .await;
    assert_eq!(
        c.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&c.body)
    );

    // 重放：令牌用过一次即销毁
    let replay = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/confirm",
            json!({ "token": token, "user_id": "u1", "role": "adult" }),
        ),
    )
    .await;
    assert_eq!(
        replay.status,
        StatusCode::CONFLICT,
        "重放的令牌必须被拒：{}",
        String::from_utf8_lossy(&replay.body)
    );
    assert_eq!(replay.json()["error"]["code"], "AI_TOKEN_REPLAYED");
}

#[tokio::test]
async fn another_user_cannot_confirm_someone_elses_token() {
    let (state, tok, _item, _dir) = state_with_ai(true).await;
    let r = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/chat",
            json!({ "message": "扫描一下媒体库", "user_id": "alice", "role": "adult" }),
        ),
    )
    .await;
    let token = r.json()["pending"]["token"].as_str().unwrap().to_string();

    let c = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/confirm",
            json!({ "token": token, "user_id": "bob", "role": "adult" }),
        ),
    )
    .await;
    assert_eq!(c.status, StatusCode::FORBIDDEN, "换人执行必须被拒");
    assert_eq!(c.json()["error"]["code"], "AI_TOKEN_MISMATCH");
}

#[tokio::test]
async fn unknown_token_is_rejected() {
    let (state, tok, _item, _dir) = state_with_ai(true).await;
    let c = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/confirm",
            json!({ "token": "不存在的令牌", "user_id": "u1" }),
        ),
    )
    .await;
    // 刻意给 403 而不是 404：返回 404 等于给了一个枚举有效令牌的神谕。
    assert_eq!(c.status, StatusCode::FORBIDDEN);
    assert_eq!(c.json()["error"]["code"], "AI_TOKEN_MISMATCH");
}

#[tokio::test]
async fn child_role_cannot_run_dangerous_tools() {
    let (state, tok, _item, _dir) = state_with_ai(true).await;
    let r = send(
        &state,
        post_json(
            &tok,
            "/api/v1/ai/chat",
            json!({ "message": "扫描一下媒体库", "user_id": "kid", "role": "child" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "儿童角色不能触发写操作");
    assert_eq!(r.json()["error"]["code"], "AI_DENIED");
}
