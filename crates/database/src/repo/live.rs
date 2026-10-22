//! 直播的读写。**全仓库只有 repo 目录允许出现 SQL**，直播也不例外。
//!
//! 与点播最大的不同是 `live_session` 表：它记"哪个频道现在由哪个进程在拉"，
//! 带引用计数。为什么必须落库而不是只放内存 —— 进程崩了、容器重启了，
//! 没人认领的 ffmpeg 会一直挂着拉流。

use anyhow::Result;
use serde::{Deserialize, Serialize};
use whatmedia_core::Id;

// ───────────────── 源 ─────────────────

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LiveSourceRow {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: Option<String>,
    pub epg_url: Option<String>,
    pub enabled: i32,
    pub number_offset: i64,
    pub last_sync_at: Option<String>,
    pub sync_error: Option<String>,
    pub created_at: Option<String>,
    pub channel_count: i64,
}

pub async fn insert_live_source(
    db: &crate::Db,
    name: &str,
    kind: &str,
    url: Option<&str>,
    epg_url: Option<&str>,
) -> Result<Id> {
    let id = Id::new_v4();
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO live_source (id, name, kind, url, epg_url, enabled, number_offset, created_at)
         VALUES (?, ?, ?, ?, ?, 1, 0, ?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(kind)
    .bind(url)
    .bind(epg_url)
    .bind(now)
    .execute(db.pool())
    .await?;
    Ok(id)
}

pub async fn list_live_sources(db: &crate::Db) -> Result<Vec<LiveSourceRow>> {
    let rows = sqlx::query_as::<_, LiveSourceRow>(
        "SELECT s.*, (SELECT COUNT(*) FROM live_channel c WHERE c.source_id = s.id) AS channel_count
         FROM live_source s ORDER BY s.created_at",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

pub async fn get_live_source(db: &crate::Db, id: Id) -> Result<Option<LiveSourceRow>> {
    let row = sqlx::query_as::<_, LiveSourceRow>(
        "SELECT s.*, (SELECT COUNT(*) FROM live_channel c WHERE c.source_id = s.id) AS channel_count
         FROM live_source s WHERE s.id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(db.pool())
    .await?;
    Ok(row)
}

pub async fn delete_live_source(db: &crate::Db, id: Id) -> Result<bool> {
    // 频道与节目靠 ON DELETE CASCADE 一起走。
    let r = sqlx::query("DELETE FROM live_source WHERE id = ?")
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(r.rows_affected() > 0)
}

/// 记一次同步的结果。成功的错误位是 NULL，失败则写原因 ——
/// 前端要能显示"上次同步失败：连接超时"，而不是永远显示"从未同步"。
pub async fn set_source_sync_result(db: &crate::Db, id: Id, error: Option<&str>) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE live_source SET last_sync_at = ?, sync_error = ? WHERE id = ?")
        .bind(now)
        .bind(error)
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

// ───────────────── 频道 ─────────────────

#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LiveChannelRow {
    pub id: String,
    pub source_id: String,
    pub number: Option<i64>,
    pub name: String,
    pub logo_url: Option<String>,
    pub group_title: Option<String>,
    pub stream_url: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub dvr_window_secs: i64,
    pub enabled: i32,
    pub epg_channel_id: Option<String>,
    pub sort_order: i64,
}

/// 用一批草稿**替换**某个源的全部频道。
///
/// 为什么是替换而不是 upsert：订阅是权威来源，删掉的频道就该消失。
/// 用 tvg-id + stream_url 做 upsert 会留下"订阅里已经没有的僵尸频道"。
pub async fn replace_channels(
    db: &crate::Db,
    source_id: Id,
    drafts: &[whatmedia_live::ChannelDraft],
) -> Result<usize> {
    let mut tx = db.pool().begin().await?;
    sqlx::query("DELETE FROM live_channel WHERE source_id = ?")
        .bind(source_id.to_string())
        .execute(&mut *tx)
        .await?;

    for (i, d) in drafts.iter().enumerate() {
        let id = Id::new_v4();
        // 频道号：草稿里写了就用，没写就按 源偏移 + 顺序 + 1 发。
        let number = d.number.unwrap_or(i as i64 + 1);
        sqlx::query(
            "INSERT INTO live_channel
             (id, source_id, number, name, logo_url, group_title, stream_url,
              dvr_window_secs, enabled, epg_channel_id, sort_order)
             VALUES (?, ?, ?, ?, ?, ?, ?, 0, 1, ?, ?)",
        )
        .bind(id.to_string())
        .bind(source_id.to_string())
        .bind(number)
        .bind(&d.name)
        .bind(&d.logo_url)
        .bind(&d.group_title)
        .bind(&d.stream_url)
        .bind(&d.epg_channel_id)
        .bind(d.sort_order)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(drafts.len())
}

pub async fn list_live_channels(db: &crate::Db) -> Result<Vec<LiveChannelRow>> {
    let rows = sqlx::query_as::<_, LiveChannelRow>(
        "SELECT * FROM live_channel WHERE enabled = 1
         ORDER BY number IS NULL, number, sort_order",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

pub async fn get_live_channel(db: &crate::Db, id: Id) -> Result<Option<LiveChannelRow>> {
    let row = sqlx::query_as::<_, LiveChannelRow>("SELECT * FROM live_channel WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    Ok(row)
}

pub async fn list_channels_by_source(db: &crate::Db, source_id: Id) -> Result<Vec<LiveChannelRow>> {
    let rows = sqlx::query_as::<_, LiveChannelRow>(
        "SELECT * FROM live_channel WHERE source_id = ? ORDER BY number IS NULL, number, sort_order",
    )
    .bind(source_id.to_string())
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

pub async fn set_channel_enabled(db: &crate::Db, id: Id, enabled: bool) -> Result<()> {
    sqlx::query("UPDATE live_channel SET enabled = ? WHERE id = ?")
        .bind(if enabled { 1 } else { 0 })
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_channels_disabled(db: &crate::Db, ids: &[Id]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut tx = db.pool().begin().await?;
    for id in ids {
        sqlx::query("UPDATE live_channel SET enabled = 0 WHERE id = ?")
            .bind(id.to_string())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

// ───────────────── 节目单 ─────────────────

/// 替换一个频道的节目单。EPG 每次抓取都是全量覆盖。
pub async fn replace_programs(
    db: &crate::Db,
    channel_id: Id,
    programs: &[whatmedia_live::ProgramDraft],
) -> Result<usize> {
    let mut tx = db.pool().begin().await?;
    sqlx::query("DELETE FROM live_program WHERE channel_id = ?")
        .bind(channel_id.to_string())
        .execute(&mut *tx)
        .await?;
    for p in programs {
        sqlx::query(
            "INSERT INTO live_program (id, channel_id, start_at, end_at, title, subtitle, description)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Id::new_v4().to_string())
        .bind(channel_id.to_string())
        .bind(&p.start_at)
        .bind(&p.end_at)
        .bind(&p.title)
        .bind(&p.subtitle)
        .bind(&p.description)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(programs.len())
}

/// 当前在播的节目标题。
///
/// 用 `start_at <= now AND end_at > now`。注意是**半开区间**：
/// 两个节目首尾相接时（19:00-19:30 / 19:30-20:00），用闭区间会在整点命中两条。
pub async fn current_program(db: &crate::Db, channel_id: Id, now: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT title FROM live_program
         WHERE channel_id = ? AND start_at <= ? AND end_at > ?
         ORDER BY start_at LIMIT 1",
    )
    .bind(channel_id.to_string())
    .bind(now)
    .bind(now)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.map(|r| r.0))
}

/// 接下来 N 条节目。
pub async fn upcoming_programs(
    db: &crate::Db,
    channel_id: Id,
    now: &str,
    limit: i64,
) -> Result<Vec<ProgramRow>> {
    let rows = sqlx::query_as::<_, ProgramRow>(
        "SELECT * FROM live_program WHERE channel_id = ? AND end_at > ?
         ORDER BY start_at LIMIT ?",
    )
    .bind(channel_id.to_string())
    .bind(now)
    .bind(limit)
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}

#[derive(Debug, Clone, sqlx::FromRow, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProgramRow {
    pub id: String,
    pub channel_id: String,
    pub start_at: String,
    pub end_at: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
}

/// 按 `tvg-id` 找频道。EPG 导入时用它把节目对到频道上。
pub async fn find_channel_by_epg_id(
    db: &crate::Db,
    epg_channel_id: &str,
) -> Result<Option<LiveChannelRow>> {
    let row = sqlx::query_as::<_, LiveChannelRow>(
        "SELECT * FROM live_channel WHERE epg_channel_id = ? LIMIT 1",
    )
    .bind(epg_channel_id)
    .fetch_optional(db.pool())
    .await?;
    Ok(row)
}

// ───────────────── 在途会话（引用计数） ─────────────────

/// 记一个频道正在被拉流。返回当前引用计数。
///
/// 为什么加引用计数：两个人看同一个频道，现状会起两个 ffmpeg、
/// 拉两路流 —— 带宽翻倍，源站还可能因为并发限制把两路都掐了。
pub async fn bump_live_session(
    db: &crate::Db,
    channel_id: Id,
    job_id: Id,
    dir: &str,
) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO live_session (channel_id, job_id, dir, ref_count, started_at, last_access_at)
         VALUES (?, ?, ?, 1, ?, ?)
         ON CONFLICT(channel_id) DO UPDATE SET
           ref_count = ref_count + 1,
           last_access_at = excluded.last_access_at",
    )
    .bind(channel_id.to_string())
    .bind(job_id.to_string())
    .bind(dir)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await?;
    live_ref_count(db, channel_id).await
}

/// 释放一个引用。返回**减完之后**的计数；归零由调用方负责停进程。
pub async fn release_live_session(db: &crate::Db, channel_id: Id) -> Result<i64> {
    sqlx::query("UPDATE live_session SET ref_count = MAX(ref_count - 1, 0) WHERE channel_id = ?")
        .bind(channel_id.to_string())
        .execute(db.pool())
        .await?;
    live_ref_count(db, channel_id).await
}

pub async fn live_ref_count(db: &crate::Db, channel_id: Id) -> Result<i64> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT ref_count FROM live_session WHERE channel_id = ?")
            .bind(channel_id.to_string())
            .fetch_optional(db.pool())
            .await?;
    Ok(row.map(|r| r.0).unwrap_or(0))
}

pub async fn delete_live_session(db: &crate::Db, channel_id: Id) -> Result<()> {
    sqlx::query("DELETE FROM live_session WHERE channel_id = ?")
        .bind(channel_id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

/// 引用计数归零但记录还在的行 —— 回收器用它清理残留目录。
pub async fn list_dead_live_sessions(db: &crate::Db) -> Result<Vec<(String, String)>> {
    let rows = sqlx::query_as::<_, (String, String)>(
        "SELECT channel_id, dir FROM live_session WHERE ref_count <= 0",
    )
    .fetch_all(db.pool())
    .await?;
    Ok(rows)
}
