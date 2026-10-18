//! 鉴权集成测试。
//!
//! 这里守的是**整套鉴权链路的因果关系**，单测覆盖不到，因为它们跨越了
//! HTTP → 中间件 → 数据库 → 到来加密 四层：
//!
//!   1. **默认关闭。** 任何没白名单的端点，不带凭据就是 401，
//!      而且状态码**必须先于**"数据库在不在"这种内部信息给出。
//!   2. **管理员资格只能被显式赋予。** bootstrap 只在零用户时开放，
//!      第二次调用必须是 403 —— 否则"装个首例"就成了永久后门。
//!   3. **儿童设备要过家长审批。** 这是全链路唯一一处"凭 BYPASS 能否绕过"的
//!      地方，且失败必须是**可见的**（返回 approval_id），不能静默。
//!   4. **签名 URL 能开整个 HLS 目录，但只开这一个 job。**
//!      跨 job 与过期都必须被拒。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use whatmedia_api::{auth::SESSION_COOKIE, build_router, AppState};
use whatmedia_config::AppConfig;
use whatmedia_core::Id;

mod common;
use common::{db, get, patch_json, post_json};

struct Reply {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn set_cookie(&self) -> String {
        self.headers
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }
}

async fn send(state: &AppState, req: Request<Body>) -> Reply {
    let res = build_router(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    Reply {
        status,
        headers,
        body,
    }
}

fn post_json_anon(uri: &str, v: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&v).unwrap()))
        .unwrap()
}

async fn state_with_db() -> AppState {
    let db = db().await;
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    AppState::new(cfg).with_db(db)
}

/// 走一遍真实的 bootstrap + login 通道。
async fn boot_with(username: &str, password: &str) -> (AppState, String) {
    let st = state_with_db().await;
    let r = send(
        &st,
        post_json_anon(
            "/api/v1/auth/bootstrap",
            json!({ "username": username, "password": password }),
        ),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );

    let r = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": username, "password": password, "device_name": "书房电脑", "device_kind": "desktop" }),
        ),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let token = r.json()["token"].as_str().unwrap().to_string();
    (st, token)
}

// ─────────────────────────────────────────────────────────────
// 默认拒绝
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn bootstrap_creates_the_first_admin_and_then_closes() {
    let st = state_with_db().await;
    let first = send(
        &st,
        post_json_anon(
            "/api/v1/auth/bootstrap",
            json!({ "username": "owner", "password": "long-enough-password" }),
        ),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED);
    assert!(first.json()["id"].is_string());

    // 第二次必须关起来 —— 这是"谁拥有这台服务器"的唯一入口。
    let second = send(
        &st,
        post_json_anon(
            "/api/v1/auth/bootstrap",
            json!({ "username": "attacker", "password": "long-enough-password" }),
        ),
    )
    .await;
    assert_eq!(second.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn health_probes_stay_anonymous() {
    // 容器探针不该因为鉴权挂掉而判定服务不健康。
    let st = state_with_db().await;
    for uri in ["/healthz", "/api/v1/health"] {
        let r = send(
            &st,
            Request::builder().uri(uri).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
async fn wrong_password_is_401_and_does_not_say_which_part() {
    let (_st, _tok) = boot_with("owner", "correct-horse-battery").await;
    // 重新建一个干净库，用错误口令登录。
    let st = state_with_db().await;
    send(
        &st,
        post_json_anon(
            "/api/v1/auth/bootstrap",
            json!({ "username": "owner", "password": "correct-horse-battery" }),
        ),
    )
    .await;

    let wrong = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": "owner", "password": "not-the-password" }),
        ),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.json()["error"]["code"], "INVALID_CREDENTIAL");

    // 不存在的用户名：同样的 401、同样的错误码。响应时间也刻意做了惯性处理。
    let missing = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": "nobody-here", "password": "not-the-password" }),
        ),
    )
    .await;
    assert_eq!(missing.status, StatusCode::UNAUTHORIZED);
    assert_eq!(missing.json()["error"]["code"], "INVALID_CREDENTIAL");
}

#[tokio::test]
async fn repeated_failures_lock_the_account() {
    let st = state_with_db().await;
    send(
        &st,
        post_json_anon(
            "/api/v1/auth/bootstrap",
            json!({ "username": "owner", "password": "correct-horse-battery" }),
        ),
    )
    .await;

    let threshold = AppConfig::default().security.lockout_threshold;
    // 前 threshold-1 次只是普通的 401；第 threshold 次触发锁定并返回 429。
    for i in 1..=threshold {
        let r = send(
            &st,
            post_json_anon(
                "/api/v1/auth/login",
                json!({ "username": "owner", "password": "wrong wrong wrong" }),
            ),
        )
        .await;
        let expected = if i >= threshold {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            StatusCode::UNAUTHORIZED
        };
        assert_eq!(r.status, expected, "第 {i} 次失败");
    }
    // 之后再试，即使口令**正确**也要被挡住 —— 锁定优先于凭据正确性。
    let locked = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": "owner", "password": "correct-horse-battery" }),
        ),
    )
    .await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(locked.json()["error"]["code"], "ACCOUNT_LOCKED");
}

#[tokio::test]
async fn login_sets_an_http_only_cookie_for_the_browser() {
    let st = state_with_db().await;
    send(
        &st,
        post_json_anon(
            "/api/v1/auth/bootstrap",
            json!({ "username": "owner", "password": "correct-horse-battery" }),
        ),
    )
    .await;
    let r = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": "owner", "password": "correct-horse-battery" }),
        ),
    )
    .await;
    let cookie = r.set_cookie();
    assert!(
        cookie.starts_with(&format!("{SESSION_COOKIE}=")),
        "{cookie}"
    );
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Lax"), "{cookie}");
}

#[tokio::test]
async fn cookie_authenticates_without_a_bearer_header() {
    // `<video src>` 只会带 Cookie，不会带 Authorization —— 这条通道必须通。
    let (_st, _tok) = boot_with("owner", "correct-horse-battery").await;
    let st = state_with_db().await;
    send(
        &st,
        post_json_anon(
            "/api/v1/auth/bootstrap",
            json!({ "username": "owner", "password": "correct-horse-battery" }),
        ),
    )
    .await;
    let r = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": "owner", "password": "correct-horse-battery" }),
        ),
    )
    .await;
    let cookie = r.set_cookie().split(';').next().unwrap().to_string();

    let r = send(
        &st,
        Request::builder()
            .uri("/api/v1/media")
            .header(header::COOKIE, cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

// ─────────────────────────────────────────────────────────────
// 角色与能力
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn members_cannot_manage_accounts_but_can_browse() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;

    let created = send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/users",
            json!({ "username": "mom", "password": "member-password-1", "role": "user" }),
        ),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);

    let (_st2, member) = {
        let r = send(
            &st,
            post_json_anon(
                "/api/v1/auth/login",
                json!({ "username": "mom", "password": "member-password-1" }),
            ),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        (st.clone(), r.json()["token"].as_str().unwrap().to_string())
    };

    // 浏览没问题。
    assert_eq!(
        send(&st, get(&member, "/api/v1/media")).await.status,
        StatusCode::OK
    );
    // 但管人是 403。
    let forbidden = send(&st, get(&member, "/api/v1/auth/users")).await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert_eq!(forbidden.json()["error"]["code"], "FORBIDDEN");
}

#[tokio::test]
async fn admin_cannot_disable_or_delete_themselves() {
    // 这条守卫的是"最后一个管理员把自己锁死在系统外面"。
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let me = send(&st, get(&admin, "/api/v1/auth/me")).await;
    let id = me.json()["id"].as_str().unwrap().to_string();

    let del = send(
        &st,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/auth/users/{id}"))
            .header("authorization", format!("Bearer {admin}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(del.status, StatusCode::BAD_REQUEST);

    let patch = send(
        &st,
        patch_json(
            &admin,
            &format!("/api/v1/auth/users/{id}"),
            json!({ "disabled": true }),
        ),
    )
    .await;
    assert_eq!(patch.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn disabling_an_account_kicks_its_sessions() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/users",
            json!({ "username": "guest", "password": "guest-password-1", "role": "user" }),
        ),
    )
    .await;
    let r = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": "guest", "password": "guest-password-1" }),
        ),
    )
    .await;
    let guest = r.json()["token"].as_str().unwrap().to_string();
    assert_eq!(
        send(&st, get(&guest, "/api/v1/media")).await.status,
        StatusCode::OK
    );

    let me = send(&st, get(&guest, "/api/v1/auth/me")).await;
    let gid = me.json()["id"].as_str().unwrap().to_string();
    let patch = send(
        &st,
        patch_json(
            &admin,
            &format!("/api/v1/auth/users/{gid}"),
            json!({ "disabled": true }),
        ),
    )
    .await;
    assert_eq!(patch.status, StatusCode::OK);

    // 在线会话必须立刻失效，而不是等它过期。
    assert_eq!(
        send(&st, get(&guest, "/api/v1/media")).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn user_listing_never_exposes_password_hashes() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let r = send(&st, get(&admin, "/api/v1/auth/users")).await;
    assert_eq!(r.status, StatusCode::OK);
    let flat = String::from_utf8_lossy(&r.body).to_string();
    assert!(!flat.contains("password"), "{flat}");
    assert!(!flat.contains("$argon2id$"), "{flat}");
    assert!(flat.contains("admin"), "{flat}");
}

// ─────────────────────────────────────────────────────────────
// 儿童设备审批
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn child_device_needs_parental_approval_before_login() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let created = send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/users",
            json!({
                "username": "kid",
                "password": "kid-password-1",
                "role": "child",
                "max_rating": "PG",
            }),
        ),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED);

    // 第一次登录：设备被登记成**未批准**，必须被挡回并给出审批编号。
    let attempt = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({
                "username": "kid",
                "password": "kid-password-1",
                "device_name": "客厅电视",
                "device_kind": "tv",
            }),
        ),
    )
    .await;
    assert_eq!(attempt.status, StatusCode::FORBIDDEN);
    assert_eq!(attempt.json()["error"]["code"], "APPROVAL_REQUIRED");
    let approval_id = attempt.json()["error"]["approval_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!approval_id.is_empty());

    // 家长看到这条待办。
    let list = send(&st, get(&admin, "/api/v1/auth/approvals")).await;
    assert_eq!(list.status, StatusCode::OK);
    let pending = list.json().as_array().unwrap().clone();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"].as_str().unwrap(), approval_id);

    // 批准 —— 连带把这台设备放行。
    let ok = send(
        &st,
        post_json(
            &admin,
            &format!("/api/v1/auth/approvals/{approval_id}"),
            json!({ "approved": true, "note": "周末可以看" }),
        ),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);

    // 再登录就通了。
    let retry = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({
                "username": "kid",
                "password": "kid-password-1",
                "device_name": "客厅电视",
                "device_kind": "tv",
            }),
        ),
    )
    .await;
    assert_eq!(
        retry.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&retry.body)
    );

    // 同一个请求不能被批第二次。
    let again = send(
        &st,
        post_json(
            &admin,
            &format!("/api/v1/auth/approvals/{approval_id}"),
            json!({ "approved": true }),
        ),
    )
    .await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn denying_leaves_the_device_blocked() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/users",
            json!({ "username": "kid2", "password": "kid-password-1", "role": "child" }),
        ),
    )
    .await;
    let attempt = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({
                "username": "kid2",
                "password": "kid-password-1",
                "device_name": "卧室平板",
                "device_kind": "mobile",
            }),
        ),
    )
    .await;
    let aid = attempt.json()["error"]["approval_id"]
        .as_str()
        .unwrap()
        .to_string();

    let denied = send(
        &st,
        post_json(
            &admin,
            &format!("/api/v1/auth/approvals/{aid}"),
            json!({ "approved": false }),
        ),
    )
    .await;
    assert_eq!(denied.status, StatusCode::OK);

    // 拒绝之后会因为已经不是 pending 而重新登记一条新请求，
    // 但**关键**的是：它依旧拿不到会话。
    let retry = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({
                "username": "kid2",
                "password": "kid-password-1",
                "device_name": "卧室平板",
                "device_kind": "mobile",
            }),
        ),
    )
    .await;
    assert_eq!(retry.status, StatusCode::FORBIDDEN);
}

// ─────────────────────────────────────────────────────────────
// 签名 URL
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn signed_stream_url_is_accepted_without_any_credential() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let job = Id::new_v4();

    let r = send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/signed-url",
            json!({ "resource": format!("/api/v1/hls/{job}/master.m3u8") }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let url = r.json()["url"].as_str().unwrap().to_string();
    assert!(url.contains("exp="), "{url}");
    assert!(url.contains("sig="), "{url}");

    // 匿名请求 URL：路由要把它放行到 handler（这里 handler 会因 job 不存在给 404，
    // 而不是 401）。差别很重要 —— 401 说明签名链路没起作用。
    let anon = send(
        &st,
        Request::builder().uri(&url).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(anon.status, StatusCode::NOT_FOUND, "签名应当先通过鉴权");

    // 同一 job 的其它分片用同一张签名。
    let seg = url.replace("/master.m3u8", "/seg-3.ts");
    let seg_res = send(
        &st,
        Request::builder().uri(&seg).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(seg_res.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn tampered_signature_is_rejected() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let job = Id::new_v4();
    let r = send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/signed-url",
            json!({ "resource": format!("/api/v1/stream/{job}") }),
        ),
    )
    .await;
    let url = r.json()["url"].as_str().unwrap().to_string();

    let broken = url.replace("sig=", "sig=AAAA");
    let res = send(
        &st,
        Request::builder().uri(&broken).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn expired_signature_is_rejected() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let job = Id::new_v4();
    let r = send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/signed-url",
            json!({ "resource": format!("/api/v1/stream/{job}") }),
        ),
    )
    .await;
    let url = r.json()["url"].as_str().unwrap().to_string();
    // 把 exp 往前改：签名本身当然就不匹配了 —— 这正是"改 exp"这个动作的价值。
    let start = url.find("exp=").unwrap() + 4;
    let end = url[start..].find('&').unwrap() + start;
    let past = format!("{}1{}", &url[..start], &url[end..]);
    let res = send(
        &st,
        Request::builder().uri(&past).body(Body::empty()).unwrap(),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn signed_url_only_covers_stream_paths() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let r = send(
        &st,
        post_json(
            &admin,
            "/api/v1/auth/signed-url",
            json!({ "resource": "/api/v1/media" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

// ─────────────────────────────────────────────────────────────
// 登出
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn logout_revokes_only_the_current_session() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let second = send(
        &st,
        post_json_anon(
            "/api/v1/auth/login",
            json!({ "username": "owner", "password": "correct-horse-battery", "device_name": "笔记本" }),
        ),
    )
    .await;
    let other = second.json()["token"].as_str().unwrap().to_string();

    let out = send(&st, post_json(&admin, "/api/v1/auth/logout", json!({}))).await;
    assert_eq!(out.status, StatusCode::OK);

    assert_eq!(
        send(&st, get(&admin, "/api/v1/media")).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&st, get(&other, "/api/v1/media")).await.status,
        StatusCode::OK
    );

    // 全部登出：一台不留。
    let all = send(&st, post_json(&other, "/api/v1/auth/logout-all", json!({}))).await;
    assert_eq!(all.status, StatusCode::OK);
    assert_eq!(
        send(&st, get(&other, "/api/v1/media")).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn devices_and_sessions_listing_never_returns_the_token_hash() {
    let (st, admin) = boot_with("owner", "correct-horse-battery").await;
    let r = send(&st, get(&admin, "/api/v1/auth/devices")).await;
    assert_eq!(r.status, StatusCode::OK);
    let flat = String::from_utf8_lossy(&r.body).to_string();
    assert!(!flat.contains("token_hash"), "{flat}");
    assert!(flat.contains("书房电脑"), "{flat}");
}
