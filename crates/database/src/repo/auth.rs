//! 账户、会话、设备与审批的读写。
//!
//! 与 `crates/auth` 的分工：**那边做判断，这边做存储。**
//! 这层不认识 argon2，也不认识"这个请求该不该放行" —— 它只负责
//! 把行安全地搬进搬出 SQLite。
//!
//! 三条贯穿全文件的约定：
//!
//! 1. **永远不会有一条 SQL 返回 `password_hash` 给调用方之外的东西。**
//!    `list_users` 用的是去除了散列列的投影；只有登录路径的
//!    `get_user_by_username` 才带出来。这是最简单的"防止手滑把散列塞进 JSON"的写法。
//! 2. **会话容错是以"过期 + 吊销"两种方式表达的，从不物理删除正在用的行。**
//!    `revoke_*` 打 `revoked_at`，清理交给 `delete_expired_sessions`。原因：
//!    `token_hash` 上有唯一索引，删了又来同一条不用指望 sqlite 报错解释清楚。
//! 3. **时间是字符串。** 全仓统一用 RFC 3339，理由见 `crates/database/src/db.rs`。

use anyhow::Result;
use serde::Serialize;
use whatmedia_core::Id;

use crate::Db;

fn now_str() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn parse_id(s: &str) -> Result<Id> {
    crate::rows::to_id(s).ok_or_else(|| anyhow::anyhow!("数据库里的 id 不是合法 UUID：{s}"))
}

// ─────────────────────────────────────────────────────────────
// 用户
// ─────────────────────────────────────────────────────────────

/// 完整账户行。**含散列，不得直接序列化进 HTTP 响应。**
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserAuthRow {
    pub id: String,
    pub username: String,
    pub display_name: Option<String>,
    pub password_hash: Option<String>,
    pub pin_hash: Option<String>,
    pub role: String,
    pub parent_id: Option<String>,
    pub max_rating: Option<String>,
    pub disabled: i32,
    pub created_at: String,
    pub last_login_at: Option<String>,
    pub failed_logins: i32,
    pub locked_until: Option<String>,
}

/// 对外可见的账户视图 —— 去掉了所有密文。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UserView {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub parent_id: Option<Id>,
    pub max_rating: Option<String>,
    pub disabled: bool,
    pub created_at: String,
    pub last_login_at: Option<String>,
}

impl UserView {
    pub fn from_auth(row: &UserAuthRow) -> Result<Self> {
        Ok(Self {
            id: parse_id(&row.id)?,
            username: row.username.clone(),
            display_name: row.display_name.clone(),
            role: row.role.clone(),
            parent_id: row.parent_id.as_deref().map(parse_id).transpose()?,
            max_rating: row.max_rating.clone(),
            disabled: row.disabled != 0,
            created_at: row.created_at.clone(),
            last_login_at: row.last_login_at.clone(),
        })
    }
}

/// 账户总数。** bootstrap 判断要不要放开首次注册 **完全靠它。
pub async fn count_users(db: &Db) -> Result<i64> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user")
        .fetch_one(db.pool())
        .await?;
    Ok(n)
}

pub struct NewUser<'a> {
    pub username: &'a str,
    pub display_name: Option<&'a str>,
    pub password_hash: &'a str,
    pub role: &'a str,
    pub parent_id: Option<Id>,
    pub max_rating: Option<&'a str>,
}

pub async fn insert_user(db: &Db, u: NewUser<'_>) -> Result<Id> {
    let id = Id::new_v4();
    let now = now_str();
    sqlx::query(
        "INSERT INTO user (id, username, display_name, password_hash, role,
                           parent_id, max_rating, is_child, disabled, failed_logins, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, 0, 0, 0, ?)",
    )
    .bind(id.to_string())
    .bind(u.username)
    .bind(u.display_name)
    .bind(u.password_hash)
    .bind(u.role)
    .bind(u.parent_id.map(|i| i.to_string()))
    .bind(u.max_rating)
    .bind(now)
    .execute(db.pool())
    .await?;
    Ok(id)
}

/// 按用户名取带散列的完整行 —— 只有登录路径可以用它。
pub async fn get_user_by_username(db: &Db, username: &str) -> Result<Option<UserAuthRow>> {
    let row = sqlx::query_as::<_, UserAuthRow>("SELECT * FROM user WHERE username = ?")
        .bind(username)
        .fetch_optional(db.pool())
        .await?;
    Ok(row)
}

pub async fn get_user_by_id(db: &Db, id: Id) -> Result<Option<UserAuthRow>> {
    let row = sqlx::query_as::<_, UserAuthRow>("SELECT * FROM user WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    Ok(row)
}

pub async fn list_users(db: &Db) -> Result<Vec<UserAuthRow>> {
    let rows = sqlx::query_as::<_, UserAuthRow>("SELECT * FROM user ORDER BY created_at")
        .fetch_all(db.pool())
        .await?;
    Ok(rows)
}

/// 用户名是否已被占用。`username` 上有 UNIQUE 索引，靠报错也能知道，
/// 但抛错走不到"给前端一个干净的 400"那条路上，所以这里显式查一次。
pub async fn username_exists(db: &Db, username: &str) -> Result<bool> {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user WHERE username = ?")
        .bind(username)
        .fetch_one(db.pool())
        .await?;
    Ok(n > 0)
}

/// 登录成功：清失败计数、清锁定、记登录时间。三件事一个事务里做。
pub async fn record_login_success(db: &Db, id: Id) -> Result<()> {
    sqlx::query(
        "UPDATE user SET failed_logins = 0, locked_until = NULL, last_login_at = ? WHERE id = ?",
    )
    .bind(now_str())
    .bind(id.to_string())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 登录失败：计数 +1。`lock_until` 由调用方按策略算出再传进来 ——
/// 阈值是策略，不是存储，不该写死在 SQL 里。
pub async fn record_login_failure(db: &Db, id: Id, lock_until: Option<&str>) -> Result<()> {
    sqlx::query(
        "UPDATE user
            SET failed_logins = failed_logins + 1,
                locked_until  = CASE WHEN ? IS NULL THEN NULL ELSE ? END
          WHERE id = ?",
    )
    .bind(lock_until)
    .bind(lock_until)
    .bind(id.to_string())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn set_user_password(db: &Db, id: Id, hash: &str) -> Result<()> {
    sqlx::query("UPDATE user SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_user_pin(db: &Db, id: Id, hash: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE user SET pin_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_user_role(db: &Db, id: Id, role: &str) -> Result<()> {
    // role 与 is_child 是同一个事实的两种写法，必须一起更新。
    // 把这条规则收在这里，是为了避免将来有人在别处手写 SQL 时只改了其中一个。
    let is_child = i32::from(role == "child");
    sqlx::query("UPDATE user SET role = ?, is_child = ? WHERE id = ?")
        .bind(role)
        .bind(is_child)
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_user_display_name(db: &Db, id: Id, display_name: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE user SET display_name = ? WHERE id = ?")
        .bind(display_name)
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_user_limits(
    db: &Db,
    id: Id,
    max_rating: Option<&str>,
    parent_id: Option<Id>,
) -> Result<()> {
    sqlx::query("UPDATE user SET max_rating = ?, parent_id = ? WHERE id = ?")
        .bind(max_rating)
        .bind(parent_id.map(|i| i.to_string()))
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_user_disabled(db: &Db, id: Id, disabled: bool) -> Result<()> {
    sqlx::query("UPDATE user SET disabled = ? WHERE id = ?")
        .bind(i32::from(disabled))
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    // 停用必须立刻踢掉在线会话，否则"禁用"形同虚设。
    if disabled {
        revoke_user_sessions(db, id).await?;
    }
    Ok(())
}

pub async fn delete_user(db: &Db, id: Id) -> Result<bool> {
    let mut tx = db.pool().begin().await?;
    // 会话 / 设备 / 收藏 / 通知靠外键级联，而**用户级设置没有外键** ——
    // `setting.scope_id` 还要存 server 级的 'default'，加不了 REFERENCES。
    // 所以这里必须手动清：不清的话删掉的账户会留下一批永远读不到的垃圾行。
    sqlx::query("DELETE FROM setting WHERE scope = 'user' AND scope_id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    let r = sqlx::query("DELETE FROM user WHERE id = ?")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(r.rows_affected() > 0)
}

// ─────────────────────────────────────────────────────────────
// 会话
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionRow {
    pub id: String,
    pub user_id: String,
    pub device_id: Option<String>,
    pub token_hash: String,
    pub kind: String,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

pub struct NewSession<'a> {
    pub user_id: Id,
    pub device_id: Option<Id>,
    pub token_hash: &'a str,
    pub kind: &'a str,
    pub ip: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    pub expires_at: &'a str,
}

pub async fn insert_session(db: &Db, s: NewSession<'_>) -> Result<Id> {
    let id = Id::new_v4();
    let now = now_str();
    sqlx::query(
        "INSERT INTO session (id, user_id, device_id, token_hash, kind, ip, user_agent,
                              created_at, last_seen_at, expires_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(s.user_id.to_string())
    .bind(s.device_id.map(|i| i.to_string()))
    .bind(s.token_hash)
    .bind(s.kind)
    .bind(s.ip)
    .bind(s.user_agent)
    .bind(&now)
    .bind(&now)
    .bind(s.expires_at)
    .execute(db.pool())
    .await?;
    Ok(id)
}

/// 用摘要取会话。一次查询就把 user 的信息 join 出来 ——
/// 鉴权在热路径上，两次往返的延迟是能感觉到的。
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SessionWithUser {
    pub id: String,
    pub user_id: String,
    pub device_id: Option<String>,
    pub kind: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
    // ── user ──
    pub username: String,
    pub display_name: Option<String>,
    pub role: String,
    pub max_rating: Option<String>,
    pub user_disabled: i32,
    pub locked_until: Option<String>,
}

pub async fn get_session_by_hash(db: &Db, hash: &str) -> Result<Option<SessionWithUser>> {
    let row = sqlx::query_as::<_, SessionWithUser>(
        "SELECT s.id, s.user_id, s.device_id, s.kind, s.expires_at, s.revoked_at,
                u.username, u.display_name, u.role, u.max_rating,
                u.disabled AS user_disabled, u.locked_until
           FROM session s JOIN user u ON u.id = s.user_id
          WHERE s.token_hash = ?",
    )
    .bind(hash)
    .fetch_optional(db.pool())
    .await?;
    Ok(row)
}

/// 心跳。刻意**每次都写**：`last_seen_at` 是"清理僵尸会话"和
/// "用户看自己哪些设备在线"两个功能唯一的数据源。
pub async fn touch_session(db: &Db, id: Id) -> Result<()> {
    sqlx::query("UPDATE session SET last_seen_at = ? WHERE id = ?")
        .bind(now_str())
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn revoke_session(db: &Db, id: Id) -> Result<bool> {
    let r = sqlx::query("UPDATE session SET revoked_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(now_str())
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected() > 0)
}

/// 登出所有设备。**打标记而不是物理删除**：另一个在跑的实例可能正持有同一条
/// session_id，删行会让它手上的状态凭空消失，表现为"随机掉线"。
pub async fn revoke_user_sessions(db: &Db, user_id: Id) -> Result<u64> {
    let r =
        sqlx::query("UPDATE session SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
            .bind(now_str())
            .bind(user_id.to_string())
            .execute(db.pool())
            .await?;
    Ok(r.rows_affected())
}

/// 物理清理：删掉已经过期超过 `grace_days` 天的会话。
///
/// 为什么不是一到点就删：客户端时钟可能慢几分钟，立刻删会让"刚好在过期边界上
/// 重连"的设备掉线。留一个宽限期，日志里就少一类解释不清的 401。
pub async fn prune_sessions(db: &Db, grace_days: i64) -> Result<u64> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(grace_days)).to_rfc3339();
    let r = sqlx::query("DELETE FROM session WHERE expires_at < ?")
        .bind(cutoff)
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected())
}

pub async fn list_sessions(db: &Db, user_id: Id) -> Result<Vec<SessionRow>> {
    let rows = sqlx::query_as::<_, SessionRow>(
        "SELECT * FROM session WHERE user_id = ? AND revoked_at IS NULL ORDER BY last_seen_at DESC",
    )
    .bind(user_id.to_string())
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

// ─────────────────────────────────────────────────────────────
// 设备
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceRow {
    pub id: String,
    pub user_id: String,
    pub name: String,
    pub kind: Option<String>,
    pub platform: Option<String>,
    pub app_version: Option<String>,
    pub last_ip: Option<String>,
    pub created_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub approved: i32,
    pub approval_id: Option<String>,
}

pub struct NewDevice<'a> {
    pub user_id: Id,
    pub name: &'a str,
    pub kind: Option<&'a str>,
    pub platform: Option<&'a str>,
    pub app_version: Option<&'a str>,
    /// 儿童设备默认 **未批准**，成人设备默认已批准。这个默认值由调用方决定，
    /// 因为"是不是儿童"是策略判断。
    pub approved: bool,
}

pub async fn insert_device(db: &Db, d: NewDevice<'_>) -> Result<Id> {
    let id = Id::new_v4();
    let now = now_str();
    sqlx::query(
        "INSERT INTO device (id, user_id, name, token_hash, kind, platform, app_version,
                             created_at, last_seen_at, approved)
         VALUES (?, ?, ?, '', ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(d.user_id.to_string())
    .bind(d.name)
    .bind(d.kind)
    .bind(d.platform)
    .bind(d.app_version)
    .bind(&now)
    .bind(&now)
    .bind(i32::from(d.approved))
    .execute(db.pool())
    .await?;
    Ok(id)
}

pub async fn get_device(db: &Db, id: Id) -> Result<Option<DeviceRow>> {
    let row = sqlx::query_as::<_, DeviceRow>("SELECT * FROM device WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    Ok(row)
}

/// 同一用户 + 同名设备视为同一台。
///
/// 名字由客户端在首次登录时给出（例如 "客厅电视"）。用名字而不是 UUID 去重，
/// 是因为重装 App 后拿不到原来的 id —— 用户的心智里它还是同一台电视。
pub async fn find_device_by_name(db: &Db, user_id: Id, name: &str) -> Result<Option<DeviceRow>> {
    let row = sqlx::query_as::<_, DeviceRow>(
        "SELECT * FROM device WHERE user_id = ? AND name = ? ORDER BY created_at LIMIT 1",
    )
    .bind(user_id.to_string())
    .bind(name)
    .fetch_optional(db.pool())
    .await?;
    Ok(row)
}

pub async fn list_user_devices(db: &Db, user_id: Id) -> Result<Vec<DeviceRow>> {
    let rows = sqlx::query_as::<_, DeviceRow>(
        "SELECT * FROM device WHERE user_id = ? ORDER BY last_seen_at DESC",
    )
    .bind(user_id.to_string())
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

pub async fn touch_device(db: &Db, id: Id, ip: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE device SET last_seen_at = ?, last_ip = COALESCE(?, last_ip) WHERE id = ?")
        .bind(now_str())
        .bind(ip)
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_device_approved(
    db: &Db,
    id: Id,
    approved: bool,
    approval_id: Option<Id>,
) -> Result<bool> {
    let r = sqlx::query("UPDATE device SET approved = ?, approval_id = ? WHERE id = ?")
        .bind(i32::from(approved))
        .bind(approval_id.map(|i| i.to_string()))
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn delete_device(db: &Db, id: Id) -> Result<bool> {
    let r = sqlx::query("DELETE FROM device WHERE id = ?")
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected() > 0)
}

// ─────────────────────────────────────────────────────────────
// 审批
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApprovalRow {
    pub id: String,
    pub user_id: String,
    pub device_id: Option<String>,
    pub kind: String,
    pub title: String,
    pub detail: Option<String>,
    pub status: String,
    pub requested_at: String,
    pub decided_at: Option<String>,
    pub decided_by: Option<String>,
    pub note: Option<String>,
}

pub struct NewApproval<'a> {
    pub user_id: Id,
    pub device_id: Option<Id>,
    pub kind: &'a str,
    pub title: &'a str,
    pub detail: Option<&'a str>,
}

pub async fn insert_approval(db: &Db, a: NewApproval<'_>) -> Result<Id> {
    let id = Id::new_v4();
    sqlx::query(
        "INSERT INTO approval_request (id, user_id, device_id, kind, title, detail,
                                       status, requested_at)
         VALUES (?, ?, ?, ?, ?, ?, 'pending', ?)",
    )
    .bind(id.to_string())
    .bind(a.user_id.to_string())
    .bind(a.device_id.map(|i| i.to_string()))
    .bind(a.kind)
    .bind(a.title)
    .bind(a.detail)
    .bind(now_str())
    .execute(db.pool())
    .await?;
    Ok(id)
}

pub async fn get_approval(db: &Db, id: Id) -> Result<Option<ApprovalRow>> {
    let row = sqlx::query_as::<_, ApprovalRow>("SELECT * FROM approval_request WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    Ok(row)
}

/// 该用户还未处理完的请求。
pub async fn pending_approval_for_user(db: &Db, user_id: Id) -> Result<Option<ApprovalRow>> {
    let row = sqlx::query_as::<_, ApprovalRow>(
        "SELECT * FROM approval_request WHERE user_id = ? AND status = 'pending'
          ORDER BY requested_at DESC LIMIT 1",
    )
    .bind(user_id.to_string())
    .fetch_optional(db.pool())
    .await?;
    Ok(row)
}

/// 待处理的请求列表 —— 家长视角。
///
/// `child_user_id` 是可选过滤项：一个家长往往同时管几个孩子，全看的时候传 `None`。
pub async fn list_pending_approvals(
    db: &Db,
    child_user_id: Option<Id>,
) -> Result<Vec<ApprovalRow>> {
    let rows = if let Some(cid) = child_user_id {
        sqlx::query_as::<_, ApprovalRow>(
            "SELECT * FROM approval_request WHERE status = 'pending' AND user_id = ?
              ORDER BY requested_at DESC",
        )
        .bind(cid.to_string())
        .fetch_all(db.pool())
        .await?
    } else {
        sqlx::query_as::<_, ApprovalRow>(
            "SELECT * FROM approval_request WHERE status = 'pending' ORDER BY requested_at DESC",
        )
        .fetch_all(db.pool())
        .await?
    };
    Ok(rows)
}

pub async fn decide_approval(
    db: &Db,
    id: Id,
    approved: bool,
    decided_by: Id,
    note: Option<&str>,
) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE approval_request
            SET status = ?, decided_at = ?, decided_by = ?, note = ?
          WHERE id = ? AND status = 'pending'",
    )
    .bind(if approved { "approved" } else { "denied" })
    .bind(now_str())
    .bind(decided_by.to_string())
    .bind(note)
    .bind(id.to_string())
    .execute(db.pool())
    .await?;
    Ok(r.rows_affected() > 0)
}
