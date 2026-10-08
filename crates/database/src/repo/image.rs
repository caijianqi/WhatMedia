//! 图片缓存的 SQL 层。
//!
//! 正文在磁盘上，这里只管 LRU 元数据。分开的原因：
//! 缓存策略（谁该被删）属于 `mosaic-metadata`，而"怎么查"属于本 crate。

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::Row;

use mosaic_core::Id;

use crate::Db;

#[derive(Debug, Clone)]
pub struct ImageCacheRow {
    pub key: String,
    pub kind: String,
    pub url: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub size_bytes: i64,
    pub path: String,
    pub created_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
    pub hits: i64,
}

pub async fn get(db: &Db, key: &str) -> Result<Option<ImageCacheRow>> {
    let row = sqlx::query(
        "SELECT key, kind, url, width, height, size_bytes, path, created_at, accessed_at, hits
         FROM image_cache WHERE key = ?",
    )
    .bind(key)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.as_ref().map(decode))
}

fn decode(row: &sqlx::sqlite::SqliteRow) -> ImageCacheRow {
    ImageCacheRow {
        key: row.get("key"),
        kind: row.get("kind"),
        url: row.get("url"),
        width: row.get("width"),
        height: row.get("height"),
        size_bytes: row.get("size_bytes"),
        path: row.get("path"),
        created_at: parse_dt(&row.get::<String, _>("created_at")),
        accessed_at: parse_dt(&row.get::<String, _>("accessed_at")),
        hits: row.get("hits"),
    }
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// 命中计数 +1 并刷新访问时间。这是 LRU 的全部依据。
pub async fn touch(db: &Db, key: &str) -> Result<()> {
    sqlx::query("UPDATE image_cache SET hits = hits + 1, accessed_at = ? WHERE key = ?")
        .bind(Utc::now().to_rfc3339())
        .bind(key)
        .execute(db.pool())
        .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn put(
    db: &Db,
    key: &str,
    kind: &str,
    url: &str,
    width: Option<i32>,
    height: Option<i32>,
    size_bytes: i64,
    path: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO image_cache
           (key, kind, url, width, height, size_bytes, path, created_at, accessed_at, hits)
         VALUES (?,?,?,?,?,?,?,?,?,1)
         ON CONFLICT(key) DO UPDATE SET
           accessed_at = excluded.accessed_at,
           hits = image_cache.hits + 1,
           path = excluded.path",
    )
    .bind(key)
    .bind(kind)
    .bind(url)
    .bind(width)
    .bind(height)
    .bind(size_bytes)
    .bind(path)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn delete(db: &Db, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM image_cache WHERE key = ?")
        .bind(key)
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn delete_many(db: &Db, keys: &[String]) -> Result<u64> {
    let mut n = 0u64;
    for k in keys {
        n += sqlx::query("DELETE FROM image_cache WHERE key = ?")
            .bind(k)
            .execute(db.pool())
            .await?
            .rows_affected();
    }
    Ok(n)
}

/// 全部条目，按清理优先级排好序：先删优先级低的，同优先级先删最久未访问的。
///
/// `kind_priority` 由调用方给出（数字越大越先被清理）。
/// SQL 里没法直接排，所以在 Rust 里排序 —— 条目量级是几万，内存里排完全够。
pub async fn all_for_eviction(db: &Db) -> Result<Vec<ImageCacheRow>> {
    let rows = sqlx::query(
        "SELECT key, kind, url, width, height, size_bytes, path, created_at, accessed_at, hits
         FROM image_cache",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(decode).collect())
}

pub async fn stats(db: &Db) -> Result<CacheStats> {
    let row = sqlx::query(
        "SELECT count(*) AS n, ifnull(sum(size_bytes),0) AS bytes,
                ifnull(sum(hits),0) AS hits FROM image_cache",
    )
    .fetch_one(db.pool())
    .await?;
    Ok(CacheStats {
        entries: row.get::<i64, _>("n") as u64,
        bytes: row.get::<i64, _>("bytes") as u64,
        hits: row.get::<i64, _>("hits") as u64,
    })
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct CacheStats {
    pub entries: u64,
    pub bytes: u64,
    pub hits: u64,
}

// ─────────────────────────────────────────────────────────────
// image 表（item ↔ 图片的绑定关系）
// ─────────────────────────────────────────────────────────────

/// 绑定一张图到 item 的请求。
///
/// 用结构体而不是 9 个位置参数：参数一多，
/// `bind(db, id, "poster", None, None, None, None, None, None)` 这种调用
/// 没人能一眼看出哪个 None 是哪个字段。
#[derive(Debug, Clone, Default)]
pub struct ImageBind<'a> {
    pub item_id: Id,
    pub kind: &'a str,
    pub url: Option<&'a str>,
    pub cache_key: Option<&'a str>,
    pub provider: Option<&'a str>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub cached_path: Option<&'a str>,
}

/// 绑定一张图到 item。重刮是 UPDATE 不是 INSERT 堆积（依赖 uq_image_item_kind）。
pub async fn bind(db: &Db, req: &ImageBind<'_>) -> Result<()> {
    let id = mosaic_core::deterministic_id("image", &[&req.item_id.to_string(), req.kind]);
    sqlx::query(
        "INSERT INTO image (id, item_id, kind, url, width, height, cached_path, provider, cache_key)
         VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(item_id, kind) DO UPDATE SET
            url = excluded.url,
            width = excluded.width,
            height = excluded.height,
            cached_path = excluded.cached_path,
            provider = excluded.provider,
            cache_key = excluded.cache_key",
    )
    .bind(id.to_string())
    .bind(req.item_id.to_string())
    .bind(req.kind)
    .bind(req.url)
    .bind(req.width)
    .bind(req.height)
    .bind(req.cached_path)
    .bind(req.provider)
    .bind(req.cache_key)
    .execute(db.pool())
    .await?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ItemImage {
    pub kind: String,
    pub url: Option<String>,
    pub cached_path: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

pub async fn list_for_item(db: &Db, item_id: Id) -> Result<Vec<ItemImage>> {
    let rows = sqlx::query(
        "SELECT kind, url, cached_path, width, height FROM image WHERE item_id = ? ORDER BY kind",
    )
    .bind(item_id.to_string())
    .fetch_all(db.pool())
    .await?;
    Ok(rows
        .iter()
        .map(|r| ItemImage {
            kind: r.get("kind"),
            url: r.get("url"),
            cached_path: r.get("cached_path"),
            width: r.get("width"),
            height: r.get("height"),
        })
        .collect())
}

/// 仅供测试：把某条缓存记录的访问时间改成指定值，用于验证过期清理。
pub async fn set_accessed_for_test(db: &Db, key: &str, when: &str) -> Result<()> {
    sqlx::query("UPDATE image_cache SET accessed_at = ? WHERE key = ?")
        .bind(when)
        .bind(key)
        .execute(db.pool())
        .await?;
    Ok(())
}
