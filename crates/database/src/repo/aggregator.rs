//! 视频采集源与搜索缓存仓储。

use anyhow::Result;
use chrono::Utc;
use whatmedia_aggregator::VodSourceConfig;

use crate::db::Db;
use crate::rows::VodSourceRow;

/// 列出所有采集源 (按 weight 升序，数字越小优先级越高)
pub async fn list_vod_sources(db: &Db) -> Result<Vec<VodSourceConfig>> {
    let rows = sqlx::query_as::<_, VodSourceRow>(
        "SELECT * FROM vod_remote_source ORDER BY weight ASC, name ASC",
    )
    .fetch_all(db.pool())
    .await?;

    Ok(rows.into_iter().map(|r| r.into_config()).collect())
}

/// 列出所有启用的采集源
pub async fn list_active_vod_sources(db: &Db) -> Result<Vec<VodSourceConfig>> {
    let rows = sqlx::query_as::<_, VodSourceRow>(
        "SELECT * FROM vod_remote_source WHERE is_active = 1 ORDER BY weight ASC, name ASC",
    )
    .fetch_all(db.pool())
    .await?;

    Ok(rows.into_iter().map(|r| r.into_config()).collect())
}

/// 根据 ID 获取单个采集源
pub async fn get_vod_source(db: &Db, id: &str) -> Result<Option<VodSourceConfig>> {
    let row = sqlx::query_as::<_, VodSourceRow>(
        "SELECT * FROM vod_remote_source WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|r| r.into_config()))
}

/// 插入或更新采集源
pub async fn upsert_vod_source(db: &Db, config: &VodSourceConfig) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO vod_remote_source (id, name, api_url, is_active, weight, timeout_ms, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            api_url = excluded.api_url,
            is_active = excluded.is_active,
            weight = excluded.weight,
            timeout_ms = excluded.timeout_ms,
            updated_at = excluded.updated_at",
    )
    .bind(&config.id)
    .bind(&config.name)
    .bind(&config.api_url)
    .bind(if config.is_active { 1 } else { 0 })
    .bind(config.weight as i64)
    .bind(config.timeout_ms as i64)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await?;

    Ok(())
}

/// 删除采集源
pub async fn delete_vod_source(db: &Db, id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM vod_remote_source WHERE id = ?")
        .bind(id)
        .execute(db.pool())
        .await?;

    Ok(res.rows_affected() > 0)
}

/// 读取搜索缓存
pub async fn get_search_cache(db: &Db, query_hash: &str, now_ts: i64) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT response_json FROM vod_search_cache WHERE query_hash = ? AND expires_at > ?",
    )
    .bind(query_hash)
    .bind(now_ts)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|r| r.0))
}

/// 写入搜索缓存
pub async fn set_search_cache(
    db: &Db,
    query_hash: &str,
    response_json: &str,
    expires_at: i64,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO vod_search_cache (query_hash, response_json, created_at, expires_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(query_hash) DO UPDATE SET
            response_json = excluded.response_json,
            expires_at = excluded.expires_at",
    )
    .bind(query_hash)
    .bind(response_json)
    .bind(&now)
    .bind(expires_at)
    .execute(db.pool())
    .await?;

    Ok(())
}

/// 清理过期缓存
pub async fn cleanup_expired_cache(db: &Db, now_ts: i64) -> Result<u64> {
    let res = sqlx::query("DELETE FROM vod_search_cache WHERE expires_at <= ?")
        .bind(now_ts)
        .execute(db.pool())
        .await?;

    Ok(res.rows_affected())
}
