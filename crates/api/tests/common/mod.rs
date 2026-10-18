//! 集成测试公共装配。
//!
//! 鉴权上线之后，**每个**测试都要先有一个登录态。把这件事收在这里，
//! 是为了避免各测试文件各写一份"绕过鉴权的土办法" ——
//! 那种土办法一旦出现，测试测的就不是生产配置的那条路由了。
//!
//! 这里刻意**不打 bootstrap/login 的 HTTP 端点**：那会让鉴权测试与被鉴权
//! 测试的失败互相传染。鉴权自身的集成测试在 `auth_endpoints.rs` 里。

// 每个测试二进制只用到这里的一部分助手（只有 auth 用到 patch/delete）。
// 不加这一条，没用到的会被判 dead_code，而 `make lint` 是 -D warnings ——
// 于是"少用一个助手"就变成构建失败。
#![allow(dead_code)]

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use std::path::{Path, PathBuf};
use tower::ServiceExt;
use whatmedia_api::{build_router, AppState};
use whatmedia_auth::{hash_password, new_token};
use whatmedia_core::Id;
use whatmedia_database::repo::{insert_session, insert_user, NewAuthSession, NewUser};
use whatmedia_database::Db;

pub fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("migrations")
}

/// 迁移好的内存库。
pub async fn db() -> Db {
    let db = Db::open_in_memory().await.unwrap();
    db.migrate(&migrations_dir()).await.unwrap();
    db
}

/// 管理员的会话票据。
///
/// 默认口令刻意写死成常量而不是随机生成：这些是测试夹具，
/// 随机值只会在失败时多出一层不确定性。
pub async fn admin_token(db: &Db) -> String {
    let user = insert_user(
        db,
        NewUser {
            username: "admin",
            display_name: None,
            password_hash: &hash_password("password123").unwrap(),
            role: "admin",
            parent_id: None,
            max_rating: None,
        },
    )
    .await
    .unwrap();
    token_for(db, user).await
}

/// 普通成年成员的会话票据。用于验证"能力检查"确实在工作。
pub async fn member_token(db: &Db, username: &str) -> String {
    let user = insert_user(
        db,
        NewUser {
            username,
            display_name: None,
            password_hash: &hash_password("password123").unwrap(),
            role: "user",
            parent_id: None,
            max_rating: None,
        },
    )
    .await
    .unwrap();
    token_for(db, user).await
}

async fn token_for(db: &Db, user_id: Id) -> String {
    let (token, hash) = new_token();
    let expires = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    insert_session(
        db,
        NewAuthSession {
            user_id,
            device_id: None,
            token_hash: &hash,
            kind: "web",
            ip: None,
            user_agent: None,
            expires_at: &expires,
        },
    )
    .await
    .unwrap();
    token
}

// ───────────────── 请求构造 ─────────────────
//
// token 一律当第一个参数，且**必须显式传**。写成"默认带 / 默认不带"的
// 任一种形式，都会有一天被人忘在手上 —— 而忘掉的那个测试会安静地
// 一直在测一条不存在的路由。

pub fn get(token: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

pub fn post_json(token: &str, uri: &str, v: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&v).unwrap()))
        .unwrap()
}

pub fn patch_json(token: &str, uri: &str, v: Value) -> Request<Body> {
    Request::builder()
        .method("PATCH")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&v).unwrap()))
        .unwrap()
}

pub fn delete(token: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

use serde_json::Value;

// ─────────────────────────────────────────────────────────────
// 请求辅助
// ─────────────────────────────────────────────────────────────

/// 一次真实 HTTP 往返的结果。
///
/// 收在这里（而不是每个测试文件各写一份）是因为 `send` 要 `oneshot`
/// 一个真 router —— 那份装配一旦各写一遍，"某个文件忘了走鉴权中间件"
/// 这类偏差就会悄悄出现。
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// 取某个响应头。不存在时返回空串而不是 `None`：
    /// 断言"这个头没有"和"这个头是空的"在测试里是同一件事，
    /// 分开处理只会让断言多出一层无意义的 `unwrap`。
    pub fn header(&self, name: impl axum::http::header::AsHeaderName) -> String {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }
}

pub async fn send(state: &AppState, req: Request<Body>) -> Reply {
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
