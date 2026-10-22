//! 收藏 / 通知 / 设置 / 存储统计。
//!
//! 与同目录其它模块一样：**这里是唯一允许出现 SQL 的地方。**

use anyhow::Result;
use serde::Serialize;
use sqlx::QueryBuilder;
use uuid::Uuid;
use whatmedia_core::Id;

use crate::db::Db;

// ─────────────────────────────────────────────────────────────
// 收藏
// ─────────────────────────────────────────────────────────────

/// 收藏列表里的一行。**带条目信息** —— 只返回 item_id 的话前端还得再拉一次详情。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FavoriteRow {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    pub kind: String,
    pub title: String,
    pub year: Option<i32>,
    pub added_at: String,
}

/// 加收藏。重复点是 `INSERT OR IGNORE`，**不是错误** ——
/// 用户双击一下就报 409 是最没必要的失败。
pub async fn add_favorite(db: &Db, user_id: Id, item_id: Id) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO favorite (user_id, item_id, created_at) VALUES (?,?,?)")
        .bind(user_id.to_string())
        .bind(item_id.to_string())
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(db.pool())
        .await?;
    Ok(())
}

/// 取消收藏。返回"是否真的删掉了一条"。
/// 用来区分 204（删了）与 404（本来就没收藏）—— 两者都不是错误，
/// 但前端要显示"已取消"还是"本来就没收藏"，得知道是哪种。
pub async fn remove_favorite(db: &Db, user_id: Id, item_id: Id) -> Result<bool> {
    let r = sqlx::query("DELETE FROM favorite WHERE user_id = ? AND item_id = ?")
        .bind(user_id.to_string())
        .bind(item_id.to_string())
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn is_favorited(db: &Db, user_id: Id, item_id: Id) -> Result<bool> {
    let row = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM favorite WHERE user_id = ? AND item_id = ?",
    )
    .bind(user_id.to_string())
    .bind(item_id.to_string())
    .fetch_one(db.pool())
    .await?;
    Ok(row > 0)
}

/// 这个条目被多少人收藏过。详情页显示。
pub async fn favorite_count_for_item(db: &Db, item_id: Id) -> Result<i64> {
    let n = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM favorite WHERE item_id = ?")
        .bind(item_id.to_string())
        .fetch_one(db.pool())
        .await?;
    Ok(n)
}

/// 我的收藏。按收藏时间倒序。
pub async fn list_favorites(
    db: &Db,
    user_id: Id,
    page: u32,
    page_size: u32,
) -> Result<(Vec<FavoriteRow>, i64)> {
    let page = page.max(1);
    let size = page_size.clamp(1, 200);
    let offset = ((page - 1) as i64) * size as i64;

    let total = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM favorite WHERE user_id = ?")
        .bind(user_id.to_string())
        .fetch_one(db.pool())
        .await?;

    let rows = sqlx::query_as::<_, (String, String, String, Option<i32>, String)>(
        "SELECT mi.id, mi.kind, mi.title, mi.year, f.created_at
           FROM favorite f
           JOIN media_item mi ON mi.id = f.item_id
          WHERE f.user_id = ?
          ORDER BY f.created_at DESC
          LIMIT ? OFFSET ?",
    )
    .bind(user_id.to_string())
    .bind(size as i64)
    .bind(offset)
    .fetch_all(db.pool())
    .await?;

    let items = rows
        .into_iter()
        .filter_map(|(id, kind, title, year, added_at)| {
            Some(FavoriteRow {
                item_id: crate::rows::to_id(&id)?,
                kind,
                title,
                year,
                added_at,
            })
        })
        .collect();
    Ok((items, total))
}

// ─────────────────────────────────────────────────────────────
// 通知
// ─────────────────────────────────────────────────────────────

/// 通知级别。只影响界面颜色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum NotifyLevel {
    Info,
    Warn,
    Error,
}

impl NotifyLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            NotifyLevel::Info => "info",
            NotifyLevel::Warn => "warn",
            NotifyLevel::Error => "error",
        }
    }
}

/// 一条待写入的通知。**还没有 id** —— id 在扇出时给每人生成一个。
#[derive(Debug, Clone)]
pub struct NewNotification {
    pub kind: &'static str,
    pub level: NotifyLevel,
    pub title: String,
    pub body: Option<String>,
    pub link: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotificationRow {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub kind: String,
    pub level: String,
    pub title: String,
    pub body: Option<String>,
    pub link: Option<String>,
    pub created_at: String,
    pub read_at: Option<String>,
}

/// 扇出给指定用户。
///
/// **每人一行，而不是一条 `user_id IS NULL` 的广播**：已读是每人一份的状态，
/// 广播行只有一个 `read_at` 可写，读的人会把别人的也标成已读。
/// 家庭场景用户数是几个量级，扇出的代价可以忽略。
///
/// 放在一个事务里：半途失败会留下"一半人收到通知"的状态，
/// 而那正是最难解释的一种。
pub async fn notify_users(db: &Db, user_ids: &[Id], n: &NewNotification) -> Result<usize> {
    if user_ids.is_empty() {
        return Ok(0);
    }
    let mut tx = db.pool().begin().await?;
    let now = chrono::Utc::now().to_rfc3339();
    for uid in user_ids {
        sqlx::query(
            "INSERT INTO notification (id, user_id, kind, level, title, body, link, created_at)
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(uid.to_string())
        .bind(n.kind)
        .bind(n.level.as_str())
        .bind(&n.title)
        .bind(&n.body)
        .bind(&n.link)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(user_ids.len())
}

/// 通知所有管理员。扫描/刮削这类系统事件走它。
pub async fn notify_admins(db: &Db, n: &NewNotification) -> Result<usize> {
    let ids = sqlx::query_scalar::<_, String>("SELECT id FROM user WHERE role = 'admin'")
        .fetch_all(db.pool())
        .await?;
    let ids: Vec<Id> = ids.iter().filter_map(|s| crate::rows::to_id(s)).collect();
    notify_users(db, &ids, n).await
}

/// 通知列表。`unread_only` 时只给未读的。
pub async fn list_notifications(
    db: &Db,
    user_id: Id,
    limit: u32,
    unread_only: bool,
) -> Result<Vec<NotificationRow>> {
    let limit = limit.clamp(1, 200);
    let mut b = QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT id, kind, level, title, body, link, created_at, read_at
           FROM notification
          WHERE user_id = ",
    );
    b.push_bind(user_id.to_string());
    if unread_only {
        b.push(" AND read_at IS NULL");
    }
    b.push(" ORDER BY created_at DESC LIMIT ")
        .push_bind(limit as i64);

    let rows = b
        .build_query_as::<(
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<String>,
        )>()
        .fetch_all(db.pool())
        .await?;

    Ok(rows
        .into_iter()
        .filter_map(
            |(id, kind, level, title, body, link, created_at, read_at)| {
                Some(NotificationRow {
                    id: crate::rows::to_id(&id)?,
                    kind,
                    level,
                    title,
                    body,
                    link,
                    created_at,
                    read_at,
                })
            },
        )
        .collect())
}

/// 未读数。顶栏小红点只查这一个数字，走 `idx_notification_unread`。
pub async fn count_unread(db: &Db, user_id: Id) -> Result<i64> {
    let n = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM notification WHERE user_id = ? AND read_at IS NULL",
    )
    .bind(user_id.to_string())
    .fetch_one(db.pool())
    .await?;
    Ok(n)
}

/// 标记已读。**带 user_id 条件**：不加的话任何人都能把别人的通知标成已读。
pub async fn mark_notification_read(db: &Db, user_id: Id, id: Id) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE notification SET read_at = ? WHERE id = ? AND user_id = ? AND read_at IS NULL",
    )
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(id.to_string())
    .bind(user_id.to_string())
    .execute(db.pool())
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn mark_all_read(db: &Db, user_id: Id) -> Result<u64> {
    let r =
        sqlx::query("UPDATE notification SET read_at = ? WHERE user_id = ? AND read_at IS NULL")
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(user_id.to_string())
            .execute(db.pool())
            .await?;
    Ok(r.rows_affected())
}

pub async fn delete_notification(db: &Db, user_id: Id, id: Id) -> Result<bool> {
    let r = sqlx::query("DELETE FROM notification WHERE id = ? AND user_id = ?")
        .bind(id.to_string())
        .bind(user_id.to_string())
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected() > 0)
}

/// 清掉过期通知。启动时顺手做一次 —— 通知表是唯一会自己长大的表。
pub async fn prune_notifications(db: &Db, keep_days: i64) -> Result<u64> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(keep_days)).to_rfc3339();
    let r = sqlx::query("DELETE FROM notification WHERE created_at < ?")
        .bind(cutoff)
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected())
}

// ─────────────────────────────────────────────────────────────
// 设置
// ─────────────────────────────────────────────────────────────

/// 一条设置的当前值。**带来源**：界面要能显示"已被改动"还是"仍是默认"。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingRow {
    pub scope: String,
    pub key: String,
    pub value: String,
}

pub async fn list_server_settings(db: &Db) -> Result<Vec<SettingRow>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT key, value FROM setting WHERE scope = 'server' AND scope_id = 'default'",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(key, value)| SettingRow {
            scope: "server".into(),
            key,
            value,
        })
        .collect())
}

pub async fn set_server_setting(db: &Db, key: &str, value: &str, updated_by: Id) -> Result<()> {
    sqlx::query(
        "INSERT INTO setting (scope, scope_id, key, value, updated_at, updated_by)
         VALUES ('server','default',?,?,?,?)
         ON CONFLICT(scope, scope_id, key)
         DO UPDATE SET value = excluded.value,
                       updated_at = excluded.updated_at,
                       updated_by = excluded.updated_by",
    )
    .bind(key)
    .bind(value)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(updated_by.to_string())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 恢复默认 = 删掉覆盖行。返回是否真的删了一条。
pub async fn delete_server_setting(db: &Db, key: &str) -> Result<bool> {
    let r = sqlx::query(
        "DELETE FROM setting WHERE scope = 'server' AND scope_id = 'default' AND key = ?",
    )
    .bind(key)
    .execute(db.pool())
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn list_user_settings(db: &Db, user_id: Id) -> Result<Vec<SettingRow>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT key, value FROM setting WHERE scope = 'user' AND scope_id = ?",
    )
    .bind(user_id.to_string())
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .into_iter()
        .map(|(key, value)| SettingRow {
            scope: "user".into(),
            key,
            value,
        })
        .collect())
}

pub async fn set_user_setting(db: &Db, user_id: Id, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO setting (scope, scope_id, key, value, updated_at, updated_by)
         VALUES ('user',?,?,?,?,NULL)
         ON CONFLICT(scope, scope_id, key)
         DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(user_id.to_string())
    .bind(key)
    .bind(value)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn delete_user_setting(db: &Db, user_id: Id, key: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM setting WHERE scope = 'user' AND scope_id = ? AND key = ?")
        .bind(user_id.to_string())
        .bind(key)
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected() > 0)
}

// ─────────────────────────────────────────────────────────────
// 存储统计
// ─────────────────────────────────────────────────────────────

/// 一个媒体库的占用。
///
/// **全部来自数据库，不扫盘。** 扫一遍几个 TB 的目录要几分钟，
/// 而这只是设置页的一张卡片 —— 同时 `media_file` 里已经有准确的
/// `size_bytes`，扫盘是重复劳动且更慢。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LibraryStorage {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub name: String,
    pub kind: String,
    pub root_path: String,
    /// 库根目录现在还在不在。不在的话显示成红色 ——
    /// 这是"盘没挂载"最常见的表现，比任何报错都直观。
    pub reachable: bool,
    pub item_count: i64,
    pub file_count: i64,
    pub bytes: i64,
}

pub async fn library_storage(db: &Db) -> Result<Vec<LibraryStorage>> {
    let rows = sqlx::query_as::<_, (String, String, String, String, i64, i64, i64)>(
        "SELECT l.id, l.name, l.kind, l.root_path,
                (SELECT COUNT(*) FROM media_item mi WHERE mi.library_id = l.id),
                (SELECT COUNT(*) FROM media_file mf
                   JOIN media_version mv ON mv.id = mf.version_id
                   JOIN media_item mi2 ON mi2.id = mv.item_id
                  WHERE mi2.library_id = l.id),
                COALESCE((SELECT SUM(mf.size_bytes) FROM media_file mf
                   JOIN media_version mv ON mv.id = mf.version_id
                   JOIN media_item mi3 ON mi3.id = mv.item_id
                  WHERE mi3.library_id = l.id), 0)
           FROM library l
          ORDER BY l.name",
    )
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(
            |(id, name, kind, root_path, item_count, file_count, bytes)| {
                Some(LibraryStorage {
                    id: crate::rows::to_id(&id)?,
                    name,
                    kind,
                    // 可达性只能问文件系统，这一下是 stat 而不是遍历，很便宜。
                    reachable: std::path::Path::new(&root_path).exists(),
                    root_path,
                    item_count,
                    file_count,
                    bytes,
                })
            },
        )
        .collect())
}
