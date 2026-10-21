//! 鉴权的 HTTP 边界：登录、账户管理、设备审批、签名 URL 与请求过滤。
//!
//! ## 为什么 filter 放在中间件而不是每个 handler
//!
//! 每个 handler 里自己 `require()` 的方案看起来更"显式"，但它有一个结构性缺陷：
//! **新增端点的人会忘记写那一行**。而忘记的后果不是报错，是静默开放。
//! 中间件把默认状态从"开放"翻转成"关闭" —— 想要开放必须显式加进白名单，
//! 且白名单每一行都得写理由。
//!
//! ## 三条白名单
//!
//! | 类别       | 成员                                     | 理由                        |
//! | -------- | -------------------------------------- | ------------------------- |
//! | 公开      | `/healthz`、`/api/v1/health`            | 容器探针不该先登录                 |
//! | 首次安装   | `/api/v1/auth/bootstrap`               | 库里零用户时才可用，否则 403          |
//! | 签名可达   | `/api/v1/stream/*`、`/api/v1/hls/*`     | `<video>`/ExoPlayer 发不出自定义头 |
//! | 登录      | `/api/v1/auth/login`                   | 否则永远没人拿得到 token           |
//!
//! ## 儿童审批怎么走
//!
//! 儿童在一台没批准过的设备上登录 → `login` 返回 **403 APPROVAL_REQUIRED**
//! 并带上 `approval_id` → 前端把它显示成"已请求，请家长批准" →
//! 家长在 `/auth/approvals` 看到并放行 → 儿童重新登录成功。
//! 整个链路里**一次都没有绕过这道闸的地方**：想退回到"只要密码对就能进"，
//! 得先改服务端代码。

use axum::extract::{FromRequestParts, Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use whatmedia_auth::{
    derive_stream_key, dummy_verify, hash_password, hash_token, new_token, verify_password,
    Ability, AuthError, Principal, Role, SigningKey,
};
use whatmedia_core::Id;

use crate::{AppState, ErrorBody, ErrorDetail};

/// 会话 Cookie 名。刻意不用 `session`/`sid` 这种通用名：
/// 同一台机器上可能还有别的服务在跑，撞名会导致 Cookie 互相覆盖。
pub const SESSION_COOKIE: &str = "wm_session";

/// `/healthz` 之外，**发言人名单**。
fn is_public(path: &str) -> bool {
    if matches!(
        path,
        "/healthz" | "/api/v1/health" | "/api/v1/auth/bootstrap" | "/api/v1/auth/login"
    ) {
        return true;
    }
    // 媒体图片资源放行：前端原生 <img> 和 CSS background 无法携带 Authorization 头
    // 形如 /api/v1/items/{id}/images/{kind}
    if path.starts_with("/api/v1/items/") && path.contains("/images/") {
        return true;
    }
    // 远程图片代理端点放行
    if path.starts_with("/api/v1/metadata/image_proxy") {
        return true;
    }
    // 直播纯内存代理端点放行：前端原生流播放器拉取分片时不携带 Authorization 头
    if path.starts_with("/api/v1/live/proxy") {
        return true;
    }
    false
}

/// 接受"HTTP 头带不出凭据"的播放器用签名 URL 访问的路径。
///
/// `hls` 那一支必须留签名通道，因为 hls.js / ExoPlayer 只在 master.m3u8 上
/// 带 query，播放列表里的分片是**相对路径**。解决办法见 `derive_stream_key`：
/// 按 job 派生一把子钥匙，于是整个目录共用一张签名。
fn is_stream_resource(path: &str) -> Option<&'static str> {
    if path.starts_with("/api/v1/stream/") {
        Some("stream")
    } else if path.starts_with("/api/v1/hls/") {
        Some("hls")
    } else {
        None
    }
}

// ─────────────────────────────────────────────────────────────
// 中间件
// ─────────────────────────────────────────────────────────────

fn deny(e: AuthError) -> Response {
    let status = StatusCode::from_u16(e.status_code()).unwrap_or(StatusCode::UNAUTHORIZED);
    (
        status,
        Json(ErrorBody {
            error: ErrorDetail {
                code: e.code(),
                message: e.to_string(),
                trace_id: None,
            },
        }),
    )
        .into_response()
}

pub(crate) async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();

    // 快路径：探针与登录等公开端点直接放行。
    if is_public(&path) {
        return next.run(req).await;
    }

    if let Some(principal) = principal_from_request(&state, req.headers()).await {
        req.extensions_mut().insert(principal);
        return next.run(req).await;
    }

    // 流端点允许"凭据在 URL 里"：浏览器播放器与原生播放器都发不出自定义头。
    if let Some(kind) = is_stream_resource(&path) {
        let query = req.uri().query().unwrap_or_default();
        if let Some(scope) = stream_scope(kind, &path) {
            if let Ok(()) = verify_signed(&state, &scope.path, query) {
                return next.run(req).await;
            }
        }
        // Cookie / Bearer 没带上，签名又对不上 —— 一律按 401 处理。
        // 不区分"资源不存在""签名过期""签名被篡改"：这三种情况的差别
        // 对调用方没有任何用处，对攻击者却是不小的提示。
        return deny(AuthError::MissingCredential);
    }

    deny(AuthError::MissingCredential)
}

/// 从流路径里取出用于派生签名钥匙的 scope。
///
/// `/api/v1/hls/{job}/master.m3u8` 的 scope 是 `{job}`；
/// `/api/v1/stream/{id}` 的 scope 是 `{id}`。两处拿到的是同一个 job 目录。
struct StreamScope {
    path: String,
}

fn stream_scope(kind: &str, path: &str) -> Option<StreamScope> {
    let rest = path
        .strip_prefix("/api/v1/hls/")
        .or_else(|| path.strip_prefix("/api/v1/stream/"))?;
    let first = rest.split('/').next()?;
    if first.is_empty() {
        return None;
    }
    Some(StreamScope {
        path: format!("/api/v1/{kind}/{first}"),
    })
}

fn verify_signed(state: &AppState, resource: &str, query: &str) -> Result<(), AuthError> {
    let mut exp: Option<i64> = None;
    let mut sig: Option<&str> = None;
    for kv in query.split('&') {
        match kv.split_once('=') {
            Some(("exp", v)) => exp = v.parse().ok(),
            Some(("sig", v)) => sig = Some(v),
            _ => {}
        }
    }
    let (Some(exp), Some(sig)) = (exp, sig) else {
        return Err(AuthError::MissingCredential);
    };
    // 同一 job 目录里的所有文件共一把派生钥匙：给 master.m3u8 签的名，
    // 里面的 index.m3u8 与 seg-N.ts 也应当认。
    let key = derive_stream_key(&state.signing, stream_kind(resource), job_scope(resource));
    key.verify(resource, exp, sig, Utc::now().timestamp())
}

/// resource → 派生钥匙用的 kind：`hls` 或 `stream`。
fn stream_kind(resource: &str) -> &str {
    if resource.starts_with("/api/v1/hls/") {
        "hls"
    } else {
        "stream"
    }
}

/// resource → 派生钥匙用的 scope。`/api/v1/hls/<job>` 取 `<job>`。
fn job_scope(resource: &str) -> &str {
    resource
        .strip_prefix("/api/v1/hls/")
        .or_else(|| resource.strip_prefix("/api/v1/stream/"))
        .and_then(|r| r.split('/').next())
        .unwrap_or(resource)
}

/// 从 headers 里解析 Principal：Bearer → Cookie → query fallback。
async fn principal_from_request(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Option<Principal> {
    let db = state.db.as_ref()?;

    let token = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(whatmedia_auth::bearer_value)
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get(COOKIE)
                .and_then(|v| v.to_str().ok())
                .and_then(|c| whatmedia_auth::cookie_value(c, SESSION_COOKIE))
                .map(|s| s.to_string())
        })?;

    let hash = hash_token(&token);
    let row = whatmedia_database::repo::get_session_by_hash(db, &hash)
        .await
        .ok()
        .flatten()?;

    // 吊销 / 过期 / 账户停用 —— 全部同样看待，输出是一致的 401。
    if row.revoked_at.is_some() || row.user_disabled != 0 {
        return None;
    }
    if chrono::DateTime::parse_from_rfc3339(&row.expires_at)
        .map(|t| t.timestamp() <= Utc::now().timestamp())
        .unwrap_or(true)
    {
        return None;
    }
    if let Some(lock) = &row.locked_until {
        if chrono::DateTime::parse_from_rfc3339(lock)
            .map(|t| t.timestamp() > Utc::now().timestamp())
            .unwrap_or(false)
        {
            return None;
        }
    }

    Some(Principal {
        user_id: row.user_id.parse().ok()?,
        username: row.username,
        display_name: row.display_name,
        role: Role::parse(&row.role)?,
        device_id: row.device_id.as_deref().and_then(|s| s.parse().ok()),
        session_id: row.id.parse().ok()?,
    })
}

// ─────────────────────────────────────────────────────────────
// 提取器
// ─────────────────────────────────────────────────────────────

/// 已认证身份。从中间件塞进来的 extension 里取。
///
/// 取不到就是 401 —— **不存在"可选的当前用户"**。需要"没登录也能看"的端点
/// 应当自成一个 open router，而不是让 handler 收到 `Option<Principal>` 去瞎猜。
pub struct Current(pub Principal);

impl<S> FromRequestParts<S> for Current
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Principal>()
            .cloned()
            .map(Current)
            .ok_or_else(|| deny(AuthError::MissingCredential))
    }
}

impl Current {
    pub fn require(&self, a: Ability) -> Result<(), Box<Response>> {
        if self.0.can(a) {
            Ok(())
        } else {
            Err(Box::new(deny(AuthError::Forbidden)))
        }
    }
}

/// 把一个"某个能力不够"的结构化拒绝翻译成 HTTP 响应。
///
/// 只回 `FORBIDDEN`，**不回**被拒绝的能力名。告诉调用方"你缺 ManageUsers"
/// 本身没问题（他本来就知道自己的角色），但把这套信息喂进去 Extra 又没有收益。
fn guard(p: &Principal, a: Ability) -> Result<(), Box<Response>> {
    p.require(a)
        .map_err(|_| Box::new(deny(AuthError::Forbidden)))
}

fn db_or_503(state: &AppState) -> Result<whatmedia_database::Db, Box<Response>> {
    state
        .db
        .clone()
        .ok_or_else(|| Box::new(crate::unavailable("数据库尚未就绪")))
}

// ─────────────────────────────────────────────────────────────
// 视图
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginResult {
    /// 会话票据。**只在这一次响应里出现**：服务端存的是它的 SHA-256。
    pub token: String,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub session_id: Id,
    pub expires_at: String,
    pub user: UserOut,
    /// 儿童账户的设备等待审批时非空。
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub approval_id: Option<Id>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UserOut {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub max_rating: Option<String>,
    pub disabled: bool,
}

impl UserOut {
    fn from(row: &whatmedia_database::repo::UserAuthRow) -> Result<Self, String> {
        Ok(Self {
            id: row.id.parse().map_err(|_| "账户 id 非法".to_string())?,
            username: row.username.clone(),
            display_name: row.display_name.clone(),
            role: row.role.clone(),
            max_rating: row.max_rating.clone(),
            disabled: row.disabled != 0,
        })
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LoginInput {
    pub username: String,
    pub password: String,
    /// 设备名。首次登录时用它登记设备；同一台重复登录会复用同一条记录。
    pub device_name: Option<String>,
    /// web | desktop | mobile | tv。默认 web。
    pub device_kind: Option<String>,
    pub platform: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BootstrapInput {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateUserInput {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
    /// admin | user | child
    #[serde(default = "default_role")]
    pub role: String,
    pub max_rating: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub parent_id: Option<Id>,
}

fn default_role() -> String {
    "user".into()
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateUserInput {
    pub role: Option<String>,
    pub max_rating: Option<String>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub parent_id: Option<Id>,
    pub disabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DecideInput {
    pub approved: bool,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignedUrlInput {
    /// 需要签名的路径，必须是 `/api/v1/stream/...` 或 `/api/v1/hls/...`。
    pub resource: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SignedUrlOut {
    pub url: String,
    pub expires_at: String,
}

// ─────────────────────────────────────────────────────────────
// 端点：首次安装与登录
// ─────────────────────────────────────────────────────────────

/// 创建第一个账户（管理员）。**库里已有任何账户时返回 403。**
///
/// 这条规则保护的是"谁是这个服务器的所有者"：只在零用户状态下开放，
/// 一旦有人占了坑，后面的人必须被邀请。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/auth/bootstrap",
        request_body = BootstrapInput,
        responses(
            (status = 201, description = "管理员已创建"),
            (status = 400, description = "口令不合策略", body = ErrorBody),
            (status = 403, description = "库里已有账户，首次安装通道已关闭", body = ErrorBody),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn bootstrap(
    State(state): State<AppState>,
    Json(body): Json<BootstrapInput>,
) -> Response {
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let Ok(n) = whatmedia_database::repo::count_users(&db).await else {
        return crate::internal("读取账户数失败");
    };
    if n > 0 {
        return deny(AuthError::Forbidden);
    }
    let Ok(hash) = hash_password(&body.password) else {
        return crate::bad_request("口令不合策略：至少 8 个字符");
    };
    if let Ok(true) = whatmedia_database::repo::username_exists(&db, &body.username).await {
        return deny(AuthError::UsernameTaken);
    }
    let Ok(id) = whatmedia_database::repo::insert_user(
        &db,
        whatmedia_database::repo::NewUser {
            username: &body.username,
            display_name: body.display_name.as_deref(),
            password_hash: &hash,
            role: Role::Admin.as_str(),
            parent_id: None,
            max_rating: None,
        },
    )
    .await
    else {
        return crate::internal("创建管理员失败");
    };
    tracing::info!(username = %body.username, "已创建首个管理员账户");
    (
        StatusCode::CREATED,
        Json(serde_json::json!({"status": "ok", "id": id.to_string()})),
    )
        .into_response()
}

fn session_expiry(state: &AppState, kind: &str) -> chrono::DateTime<Utc> {
    let cfg = &state.config.security;
    match kind {
        "web" => Utc::now() + Duration::hours(cfg.web_session_hours as i64),
        _ => Utc::now() + Duration::days(cfg.device_session_days as i64),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/auth/login",
        request_body = LoginInput,
        responses(
            (status = 200, description = "登录成功", body = LoginResult),
            (status = 401, description = "用户名或口令不对", body = ErrorBody),
            (status = 403, description = "账户已停用 / 设备等待家长批准", body = ErrorBody),
            (status = 429, description = "连续失败过多，账户已临时锁定", body = ErrorBody),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn login(State(state): State<AppState>, Json(body): Json<LoginInput>) -> Response {
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let Some(user) = whatmedia_database::repo::get_user_by_username(&db, &body.username)
        .await
        .ok()
        .flatten()
    else {
        // 用户不存在也要花掉一次 argon2 的时间，否则响应时间本身
        // 就成了"这个用户名存不存在"的预言机。
        dummy_verify();
        return deny(AuthError::InvalidCredential);
    };

    let user_id = match user.id.parse::<Id>() {
        Ok(i) => i,
        Err(_) => return crate::internal("账户 id 损坏"),
    };

    let Some(hash) = user.password_hash.clone() else {
        return deny(AuthError::InvalidCredential);
    };
    let Ok(ok) = verify_password(&body.password, &hash) else {
        return crate::internal("口令摘要损坏");
    };
    if !ok {
        let failed = user.failed_logins + 1;
        let cfg = &state.config.security;
        let lock = if failed >= cfg.lockout_threshold {
            Some((Utc::now() + Duration::minutes(cfg.lockout_minutes)).to_rfc3339())
        } else {
            None
        };
        let _ = whatmedia_database::repo::record_login_failure(&db, user_id, lock.as_deref()).await;
        if lock.is_some() {
            tracing::warn!(username = %body.username, "登录失败次数过多，账户已锁定");
            return deny(AuthError::AccountLocked);
        }
        return deny(AuthError::InvalidCredential);
    }

    if user.disabled != 0 {
        return deny(AuthError::AccountDisabled);
    }
    if let Some(until) = &user.locked_until {
        let locked = chrono::DateTime::parse_from_rfc3339(until)
            .map(|t| t.timestamp() > Utc::now().timestamp())
            .unwrap_or(false);
        if locked {
            return deny(AuthError::AccountLocked);
        }
    }

    let role = match Role::parse(&user.role) {
        Some(r) => r,
        None => {
            tracing::error!(username = %user.username, role = %user.role, "账户 role 无法识别，拒绝登录");
            return deny(AuthError::InvalidCredential);
        }
    };

    // 设备登记。名字缺失时用 UA 兜底 —— 没有名字的设备在"退出所有设备"里
    // 会显示成一片空白，用户就没法判断该退哪一台。
    let device_name = body
        .device_name
        .clone()
        .unwrap_or_else(|| "未知设备".into());
    let kind = device_kind(&body);
    let device_id = match find_or_register_device(&db, user_id, &device_name, &body, role).await {
        Ok((id, _approved)) => id,
        Err(r) => return *r,
    };

    // 儿童 + 未批准设备 → 生成审批请求，不签发会话。
    if role == Role::Child {
        let approved = device_approved(&db, device_id).await.unwrap_or(true);
        if !approved {
            let Some(existing) = whatmedia_database::repo::pending_approval_for_user(&db, user_id)
                .await
                .ok()
                .flatten()
            else {
                let Ok(ap) = whatmedia_database::repo::insert_approval(
                    &db,
                    whatmedia_database::repo::NewApproval {
                        user_id,
                        device_id: Some(device_id),
                        kind: "device",
                        title: &format!("{device_name} 请求登录"),
                        detail: Some(user.username.as_str()),
                    },
                )
                .await
                else {
                    return crate::internal("创建审批请求失败");
                };
                return approval_required(&ap.to_string());
            };
            return approval_required(&existing.id);
        }
    }

    let (token, token_hash) = new_token();
    let expires = session_expiry(&state, kind);
    let Ok(session_id) = whatmedia_database::repo::insert_session(
        &db,
        whatmedia_database::repo::NewAuthSession {
            user_id,
            device_id: Some(device_id),
            token_hash: &token_hash,
            kind,
            ip: None,
            user_agent: None,
            expires_at: &expires.to_rfc3339(),
        },
    )
    .await
    else {
        return crate::internal("创建会话失败");
    };
    let _ = whatmedia_database::repo::record_login_success(&db, user_id).await;
    let _ = whatmedia_database::repo::touch_device(&db, device_id, None).await;

    let user_out = match UserOut::from(&user) {
        Ok(u) => u,
        Err(e) => return crate::internal(e),
    };

    let result = LoginResult {
        token: token.clone(),
        session_id,
        expires_at: expires.to_rfc3339(),
        user: user_out,
        approval_id: None,
    };

    // 浏览器那条路径同时也种 Cookie：`<video src>` 发出的请求会自动带上它，
    // SPA 的 fetch 则走 Authorization。两条都给，是因为两种通道在同一页面里并存。
    let cookie = format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        (expires - Utc::now()).num_seconds().max(1)
    );
    (StatusCode::OK, [(SET_COOKIE, cookie)], Json(result)).into_response()
}

fn approval_required(id: &str) -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({
            "error": {
                "code": "APPROVAL_REQUIRED",
                "message": "该设备尚未获得家长批准",
                "trace_id": null,
                "approval_id": id,
            }
        })),
    )
        .into_response()
}

async fn device_approved(db: &whatmedia_database::Db, device_id: Id) -> Result<bool, ()> {
    Ok(whatmedia_database::repo::get_device(db, device_id)
        .await
        .ok()
        .flatten()
        .map(|d| d.approved != 0)
        .unwrap_or(true))
}

/// 找同名设备；没有就登记一台。
///
/// 儿童的新设备**默认未批准**，成人设备默认已批准 —— 这是家长控制的唯一入口，
/// 一旦这里给错了默认值，"儿童账户"就只是一个改名而已。
async fn find_or_register_device(
    db: &whatmedia_database::Db,
    user_id: Id,
    name: &str,
    body: &LoginInput,
    role: Role,
) -> Result<(Id, bool), Box<Response>> {
    if let Ok(Some(d)) = whatmedia_database::repo::find_device_by_name(db, user_id, name).await {
        let id = match d.id.parse::<Id>() {
            Ok(i) => i,
            Err(_) => return Err(Box::new(crate::internal("设备 id 损坏"))),
        };
        return Ok((id, d.approved != 0));
    }
    let approved = role != Role::Child;
    let Ok(id) = whatmedia_database::repo::insert_device(
        db,
        whatmedia_database::repo::NewDevice {
            user_id,
            name,
            kind: Some(device_kind(body)),
            platform: body.platform.as_deref(),
            app_version: None,
            approved,
        },
    )
    .await
    else {
        return Err(Box::new(crate::internal("登记设备失败")));
    };
    Ok((id, approved))
}

fn device_kind(body: &LoginInput) -> &str {
    match body.device_kind.as_deref() {
        Some("desktop") | Some("mobile") | Some("tv") => {
            // 借用 device_kind 的静态分支让我们少一处字符串爆破。
            match body.device_kind.as_deref().unwrap() {
                "desktop" => "desktop",
                "mobile" => "mobile",
                _ => "tv",
            }
        }
        _ => "web",
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/auth/logout",
        responses(
            (status = 200, description = "当前会话已吊销"),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 404, description = "会话不存在", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn logout(State(state): State<AppState>, Current(p): Current) -> Response {
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::revoke_session(&db, p.session_id).await {
        Ok(true) => {
            let cookie = format!("{SESSION_COOKIE}=; Path=/; Max-Age=0");
            (
                StatusCode::OK,
                [(SET_COOKIE, cookie)],
                Json(serde_json::json!({"status":"ok"})),
            )
                .into_response()
        }
        Ok(false) => crate::error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "会话不存在"),
        Err(e) => crate::internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/auth/logout-all",
        responses(
            (status = 200, description = "该账户的全部会话已吊销"),
            (status = 401, description = "未认证", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn logout_all(State(state): State<AppState>, Current(p): Current) -> Response {
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::revoke_user_sessions(&db, p.user_id).await {
        Ok(n) => {
            tracing::info!(用户 = %p.username, devices = n, "已登出全部会话");
            Json(serde_json::json!({"status": "ok", "revoked": n})).into_response()
        }
        Err(e) => crate::internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/auth/me",
        responses(
            (status = 200, description = "当前身份", body = UserOut),
            (status = 401, description = "未认证", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn me(Current(p): Current) -> impl IntoResponse {
    Json(UserOut {
        id: p.user_id,
        username: p.username,
        display_name: p.display_name,
        role: p.role.as_str().to_string(),
        // max_rating 不在 Principal 里：它是闸门策略需要的，不是身份的一部分。
        max_rating: None,
        disabled: false,
    })
}

// ─────────────────────────────────────────────────────────────
// 端点：账户管理
// ─────────────────────────────────────────────────────────────

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/auth/users",
        responses(
            (status = 200, description = "账户清单（不含任何密文）", body = Vec<UserOut>),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 403, description = "仅管理员", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn list_accounts(State(state): State<AppState>, Current(p): Current) -> Response {
    if let Err(r) = guard(&p, Ability::ManageUsers) {
        return *r;
    }
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let Ok(rows) = whatmedia_database::repo::list_users(&db).await else {
        return crate::internal("读取账户失败");
    };
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        match UserOut::from(r) {
            Ok(u) => out.push(u),
            Err(e) => return crate::internal(e),
        }
    }
    Json(out).into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/auth/users",
        request_body = CreateUserInput,
        responses(
            (status = 201, description = "已创建"),
            (status = 400, description = "角色未知或口令不合策略", body = ErrorBody),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 403, description = "仅管理员", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn create_account(
    State(state): State<AppState>,
    Current(p): Current,
    Json(body): Json<CreateUserInput>,
) -> Response {
    if let Err(r) = guard(&p, Ability::ManageUsers) {
        return *r;
    }
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    // 角色合法性必须在这里查：写进库一个认不出的 role，
    // 那个人下次登录会被 role 解析失败挡在门外，而且看不见原因。
    let Some(role) = Role::parse(&body.role) else {
        return crate::bad_request(format!(
            "未知角色：{}（可选 admin | user | child）",
            body.role
        ));
    };
    let Ok(hash) = hash_password(&body.password) else {
        return crate::bad_request("口令不合策略：至少 8 个字符");
    };
    if let Ok(true) = whatmedia_database::repo::username_exists(&db, &body.username).await {
        return deny(AuthError::UsernameTaken);
    }
    match whatmedia_database::repo::insert_user(
        &db,
        whatmedia_database::repo::NewUser {
            username: &body.username,
            display_name: body.display_name.as_deref(),
            password_hash: &hash,
            role: role.as_str(),
            parent_id: body.parent_id,
            max_rating: body.max_rating.as_deref(),
        },
    )
    .await
    {
        Ok(id) => {
            tracing::info!(username = %body.username, role = role.as_str(), by = %p.username, "已创建账户");
            (
                StatusCode::CREATED,
                Json(serde_json::json!({"id": id.to_string()})),
            )
                .into_response()
        }
        Err(e) => crate::internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        patch,
        path = "/api/v1/auth/users/{id}",
        params(("id" = String, Path, description = "账户 id")),
        request_body = UpdateUserInput,
        responses(
            (status = 200, description = "已更新"),
            (status = 400, description = "参数非法（例如停用自己）", body = ErrorBody),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 403, description = "仅管理员", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn update_account(
    State(state): State<AppState>,
    Current(p): Current,
    axum::extract::Path(id): axum::extract::Path<Id>,
    Json(body): Json<UpdateUserInput>,
) -> Response {
    if let Err(r) = guard(&p, Ability::ManageUsers) {
        return *r;
    }
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    if let Some(role) = &body.role {
        let Some(role) = Role::parse(role) else {
            return crate::bad_request(format!("未知角色：{role}"));
        };
        if let Err(e) = whatmedia_database::repo::set_user_role(&db, id, role.as_str()).await {
            return crate::internal(e);
        }
    }
    if body.max_rating.is_some() || body.parent_id.is_some() {
        if let Err(e) = whatmedia_database::repo::set_user_limits(
            &db,
            id,
            body.max_rating.as_deref(),
            body.parent_id,
        )
        .await
        {
            return crate::internal(e);
        }
    }
    if let Some(disabled) = body.disabled {
        // 不允许把自己停用 —— 这会让"最后一个管理员"把自己锁死在系统外面。
        if disabled && id == p.user_id {
            return crate::bad_request("不能停用当前登录的账户");
        }
        if let Err(e) = whatmedia_database::repo::set_user_disabled(&db, id, disabled).await {
            return crate::internal(e);
        }
    }
    Json(serde_json::json!({"status": "ok"})).into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        delete,
        path = "/api/v1/auth/users/{id}",
        params(("id" = String, Path, description = "账户 id")),
        responses(
            (status = 200, description = "已删除（会话与设备级联）"),
            (status = 400, description = "不能删除自己", body = ErrorBody),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 403, description = "仅管理员", body = ErrorBody),
            (status = 404, description = "账户不存在", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn delete_account(
    State(state): State<AppState>,
    Current(p): Current,
    axum::extract::Path(id): axum::extract::Path<Id>,
) -> Response {
    if let Err(r) = guard(&p, Ability::ManageUsers) {
        return *r;
    }
    if id == p.user_id {
        return crate::bad_request("不能删除当前登录的账户");
    }
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::delete_user(&db, id).await {
        Ok(true) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Ok(false) => crate::error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "账户不存在"),
        Err(e) => crate::internal(e),
    }
}

// ─────────────────────────────────────────────────────────────
// 端点：设备与审批
// ─────────────────────────────────────────────────────────────

/// 当前账户的设备清单 + 在线会话。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/auth/devices",
        responses(
            (status = 200, description = "自己的设备与在线会话（不含 token_hash）"),
            (status = 401, description = "未认证", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn list_devices(State(state): State<AppState>, Current(p): Current) -> Response {
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let Ok(devices) = whatmedia_database::repo::list_user_devices(&db, p.user_id).await else {
        return crate::internal("读取设备失败");
    };
    let Ok(sessions) = whatmedia_database::repo::list_sessions(&db, p.user_id).await else {
        return crate::internal("读取会话失败");
    };
    // 刻意不给前端 `token_hash`：它出现在响应里就等于出现在浏览器历史、
    // 代理日志、错误上报里。这里只需要"哪台设备在线"。
    let sessions: Vec<_> = sessions
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "kind": s.kind,
                "created_at": s.created_at,
                "last_seen_at": s.last_seen_at,
                "expires_at": s.expires_at,
            })
        })
        .collect();
    Json(serde_json::json!({ "devices": devices, "sessions": sessions })).into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        delete,
        path = "/api/v1/auth/devices/{id}",
        params(("id" = String, Path, description = "设备 id")),
        responses(
            (status = 200, description = "设备已移除"),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 404, description = "设备不存在或不属于当前账户", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn revoke_device(
    State(state): State<AppState>,
    Current(p): Current,
    axum::extract::Path(id): axum::extract::Path<Id>,
) -> Response {
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    // 先确认这台设备属于当前用户 —— 否则就是一个"替别人下线"的接口。
    let Ok(devices) = whatmedia_database::repo::list_user_devices(&db, p.user_id).await else {
        return crate::internal("读取设备失败");
    };
    if !devices.iter().any(|d| d.id == id.to_string()) {
        return crate::error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "设备不存在");
    }
    match whatmedia_database::repo::delete_device(&db, id).await {
        Ok(true) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Ok(false) => crate::error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "设备不存在"),
        Err(e) => crate::internal(e),
    }
}

/// 待处理的儿童请求。**任何成人都能看到全部**：只有一位家长的
/// 家庭里不能因为"这不是我的孩子"就永远没人批得动。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/auth/approvals",
        responses(
            (status = 200, description = "待处理的儿童请求", body = Vec<whatmedia_database::repo::ApprovalRow>),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 403, description = "仅成人", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn list_approvals(State(state): State<AppState>, Current(p): Current) -> Response {
    if let Err(r) = guard(&p, Ability::ApproveRequests) {
        return *r;
    }
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::list_pending_approvals(&db, None).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => crate::internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/auth/approvals/{id}",
        params(("id" = String, Path, description = "审批请求 id")),
        request_body = DecideInput,
        responses(
            (status = 200, description = "已审批；批准时连带放行设备"),
            (status = 400, description = "该请求已被处理", body = ErrorBody),
            (status = 401, description = "未认证", body = ErrorBody),
            (status = 403, description = "仅成人", body = ErrorBody),
            (status = 404, description = "请求不存在", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn decide(
    State(state): State<AppState>,
    Current(p): Current,
    axum::extract::Path(id): axum::extract::Path<Id>,
    Json(body): Json<DecideInput>,
) -> Response {
    if let Err(r) = guard(&p, Ability::ApproveRequests) {
        return *r;
    }
    let db = match db_or_503(&state) {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let Some(ap) = whatmedia_database::repo::get_approval(&db, id)
        .await
        .ok()
        .flatten()
    else {
        return crate::error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "审批请求不存在");
    };
    match whatmedia_database::repo::decide_approval(
        &db,
        id,
        body.approved,
        p.user_id,
        body.note.as_deref(),
    )
    .await
    {
        Ok(true) => {
            // 批准就把设备一起放行 —— 否则儿童还是登不进来，
            // 而家长看到"已批准"会以为是 bug。
            let device_id = ap.device_id.as_deref().and_then(|s| s.parse::<Id>().ok());
            if let Some(did) = device_id {
                let _ = whatmedia_database::repo::set_device_approved(
                    &db,
                    did,
                    body.approved,
                    Some(id),
                )
                .await;
            }
            Json(serde_json::json!({"status": "ok"})).into_response()
        }
        Ok(false) => crate::bad_request("该请求已被处理"),
        Err(e) => crate::internal(e),
    }
}

// ─────────────────────────────────────────────────────────────
// 端点：签名 URL
// ─────────────────────────────────────────────────────────────

/// 给播放资源签一张短命 URL。
///
/// 必须登录后才签发。**不能直接把 `/hls/{job}` 那个 URL 换成永久签名** ——
/// 那样就等于签了一张能重放的长啥都不限的通行证。这里签的是：
///   · 派生钥匙（按 job scope）
///   · 数小时过期
///   · 只能开这一个 job 目录
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/auth/signed-url",
        request_body = SignedUrlInput,
        responses(
            (status = 200, description = "带 exp/sig 的短命 URL", body = SignedUrlOut),
            (status = 400, description = "资源不是播放路径", body = ErrorBody),
            (status = 401, description = "未认证", body = ErrorBody),
        ),
        tag = "auth"
    )
)]
pub async fn signed_url(
    State(state): State<AppState>,
    Current(_p): Current,
    Json(body): Json<SignedUrlInput>,
) -> Response {
    let Some(kind) = is_stream_resource(&body.resource) else {
        return crate::bad_request("只能给 /api/v1/stream/* 与 /api/v1/hls/* 签名");
    };
    let Some(scope) = stream_scope(kind, &body.resource) else {
        return crate::bad_request("资源路径缺少 job/资源 id");
    };
    let ttl = state.config.security.signed_url_hours.max(1) as i64;
    let exp = Utc::now().timestamp() + ttl * 3600;
    let key = derive_stream_key(
        &state.signing,
        stream_kind(&scope.path),
        job_scope(&scope.path),
    );
    let sig = key.sign(&scope.path, exp);
    let sep = if body.resource.contains('?') {
        '&'
    } else {
        '?'
    };
    let url = format!("{}{}exp={}&sig={}", body.resource, sep, exp, sig);
    Json(SignedUrlOut {
        url,
        expires_at: chrono::DateTime::from_timestamp(exp, 0)
            .map(|t| t.to_rfc3339())
            .unwrap_or_default(),
    })
    .into_response()
}

/// 首次启动时的引导：库里没有任何账户时创建一个 dev 账户。
///
/// **只在显式给了环境变量时做。** 不生成随机密码打到日志里：
/// 那种密码会被 grep 出来、会被截图、会被当成"反正今天是 mk"而忘掉。
/// 生产部署应当走 `/api/v1/auth/bootstrap` 的首次安装向导。
pub async fn bootstrap_dev_user(
    db: &whatmedia_database::Db,
) -> anyhow::Result<Option<(String, Id)>> {
    if whatmedia_database::repo::count_users(db).await? > 0 {
        return Ok(None);
    }
    let Ok(username) = std::env::var("WHATMEDIA_BOOTSTRAP_USER") else {
        return Ok(None);
    };
    let Ok(password) = std::env::var("WHATMEDIA_BOOTSTRAP_PASSWORD") else {
        anyhow::bail!("设了 WHATMEDIA_BOOTSTRAP_USER 但没设 WHATMEDIA_BOOTSTRAP_PASSWORD");
    };
    let hash = hash_password(&password)?;
    let id = whatmedia_database::repo::insert_user(
        db,
        whatmedia_database::repo::NewUser {
            username: &username,
            display_name: None,
            password_hash: &hash,
            role: Role::Admin.as_str(),
            parent_id: None,
            max_rating: None,
        },
    )
    .await?;
    Ok(Some((username, id)))
}

/// 生成签名密钥。
///
/// 配了就用配的（多实例共享），没配就每个进程随机一个 ——
/// 重启后所有在途的签名 URL 失效，这在自托管场景里是想要的行为，
/// 因为它顺手把"上次崩溃前发出去的那批 URL"收回来了。
pub fn init_signing_key(sec: &whatmedia_config::SecurityConfig) -> Arc<SigningKey> {
    let key = match sec.signing_key.as_deref().and_then(SigningKey::from_base64) {
        Some(k) => {
            tracing::info!("已加载配置的签名密钥（多实例共享）");
            k
        }
        None => {
            if sec.signing_key.is_some() {
                tracing::warn!(
                    "security.signing_key 格式不对（需要 32 字节的 base64url），本次随机生成"
                );
            }
            SigningKey::generate()
        }
    };
    Arc::new(key)
}

/// 过期 / 被吊销会话的物理清理。
pub async fn prune_sessions(db: &whatmedia_database::Db, grace_days: i64) {
    match whatmedia_database::repo::prune_sessions(db, grace_days).await {
        Ok(0) => {}
        Ok(n) => tracing::info!("已清理 {n} 条过期会话"),
        Err(e) => tracing::warn!("清理过期会话失败：{e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_cover_the_expected_paths() {
        assert!(is_public("/healthz"));
        assert!(is_public("/api/v1/health"));
        assert!(is_public("/api/v1/auth/login"));
        assert!(is_public("/api/v1/auth/bootstrap"));
        // 数据端点一个都不在白名单里 —— 这是整个文件的意义所在。
        for p in [
            "/api/v1/media",
            "/api/v1/libraries",
            "/api/v1/scan",
            "/api/v1/ai/chat",
            "/api/v1/live/channels",
        ] {
            assert!(!is_public(p), "{p} 不该公开");
        }
    }

    #[test]
    fn stream_routes_are_recognised() {
        assert_eq!(is_stream_resource("/api/v1/stream/abc"), Some("stream"));
        assert_eq!(
            is_stream_resource("/api/v1/hls/job/master.m3u8"),
            Some("hls")
        );
        assert_eq!(is_stream_resource("/api/v1/media"), None);
    }

    #[test]
    fn scope_derives_the_same_key_for_a_whole_directory() {
        // master.m3u8 与任意分片必须落到同一个派生 scope 上。
        let a = stream_scope("hls", "/api/v1/hls/job-1/master.m3u8").unwrap();
        let b = stream_scope("hls", "/api/v1/hls/job-1/seg-3.ts").unwrap();
        assert_eq!(a.path, b.path);
        assert_eq!(a.path, "/api/v1/hls/job-1");

        let c = stream_scope("stream", "/api/v1/stream/file-9").unwrap();
        assert_eq!(c.path, "/api/v1/stream/file-9");
        assert!(stream_scope("hls", "/api/v1/hls/").is_none());
    }

    #[test]
    fn device_kind_is_whitelisted() {
        let mk = |s: &str| LoginInput {
            username: "u".into(),
            password: "password123".into(),
            device_name: None,
            device_kind: Some(s.into()),
            platform: None,
        };
        assert_eq!(device_kind(&mk("desktop")), "desktop");
        assert_eq!(device_kind(&mk("tv")), "tv");
        assert_eq!(device_kind(&mk("mobile")), "mobile");
        // 认不出的值一律落回 web，而不是原样写进库。
        assert_eq!(device_kind(&mk("toaster")), "web");
        assert_eq!(device_kind(&mk("")), "web");
    }

    #[test]
    fn role_strings_used_by_the_api_are_parseable() {
        for r in ["admin", "user", "child"] {
            assert!(Role::parse(r).is_some(), "{r}");
        }
    }
}
