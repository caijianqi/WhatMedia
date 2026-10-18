//! 播放会话与转码任务读写。
//!
//! 两条不变量：
//!
//! 1. **续播点永远用 UPSERT，不用 INSERT。** 心跳每 5 秒一次，
//!    一次播放会产生几百条更新；用独立行会让"继续观看"列表被刷爆。
//! 2. **流式输出目录必须落库。** 见 0004 迁移注释：不落库就无法回收僵尸分片。

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::QueryBuilder;
use whatmedia_core::{
    Id, MediaFile, MediaKind, MediaStream, PlaybackSession, PlaybackStrategy, TranscodeJob,
    TranscodeState,
};

use crate::db::Db;
use crate::rows::{to_id, to_kind};

// ─────────────────────────────────────────────────────────────
// 播放会话
// ─────────────────────────────────────────────────────────────

/// 新建会话的参数。
#[derive(Debug, Clone)]
pub struct NewSession {
    pub item_id: Id,
    pub file_id: Id,
    pub strategy: PlaybackStrategy,
    pub user_id: Option<Id>,
    pub device_id: Option<String>,
    pub client_id: Option<String>,
    pub strategy_reason: Option<String>,
    /// 起始位置。续播时从上次的位置开始。
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
}

fn now_str() -> String {
    Utc::now().to_rfc3339()
}

fn parse_time(s: &str) -> DateTime<Utc> {
    crate::rows::to_time(s).unwrap_or_else(Utc::now)
}

fn to_strategy(s: &str) -> PlaybackStrategy {
    match s {
        "direct_play" => PlaybackStrategy::DirectPlay,
        "remux" => PlaybackStrategy::Remux,
        "transcode" => PlaybackStrategy::Transcode,
        _ => PlaybackStrategy::Unsupported,
    }
}

/// Row 里的 id 一定是合法 UUID；不合法说明数据被外部改动过，直接报错比静默丢行好。
fn parse_id(s: &str) -> Result<Id> {
    to_id(s).ok_or_else(|| anyhow::anyhow!("数据库里的 id 不是合法 UUID：{s}"))
}

fn from_strategy(s: PlaybackStrategy) -> &'static str {
    match s {
        PlaybackStrategy::DirectPlay => "direct_play",
        PlaybackStrategy::Remux => "remux",
        PlaybackStrategy::Transcode => "transcode",
        PlaybackStrategy::Unsupported => "unsupported",
    }
}

pub async fn create_session(db: &Db, new: NewSession) -> Result<PlaybackSession> {
    let id = Id::new_v4();
    let now = now_str();
    sqlx::query(
        "INSERT INTO playback_session (
            id, user_id, item_id, file_id, device_id, client_id, strategy, strategy_reason,
            position_ms, duration_ms, created_at, updated_at, stopped_at
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,NULL)",
    )
    .bind(id.to_string())
    .bind(new.user_id.map(|u| u.to_string()))
    .bind(new.item_id.to_string())
    .bind(new.file_id.to_string())
    .bind(&new.device_id)
    .bind(&new.client_id)
    .bind(from_strategy(new.strategy))
    .bind(&new.strategy_reason)
    .bind(new.position_ms)
    .bind(new.duration_ms)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await?;

    Ok(PlaybackSession {
        id,
        user_id: new.user_id,
        item_id: new.item_id,
        file_id: new.file_id,
        device_id: new.device_id,
        client_id: new.client_id,
        strategy: new.strategy,
        strategy_reason: new.strategy_reason,
        position_ms: new.position_ms,
        duration_ms: new.duration_ms,
        created_at: parse_time(&now),
        updated_at: parse_time(&now),
        stopped_at: None,
    })
}

pub async fn get_session(db: &Db, id: Id) -> Result<Option<PlaybackSession>> {
    let row = sqlx::query_as::<_, SessionRow>(
        "SELECT id, user_id, item_id, file_id, device_id, client_id, strategy, strategy_reason,
                position_ms, duration_ms, created_at, updated_at, stopped_at
           FROM playback_session WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(db.pool())
    .await?;
    Ok(row.and_then(|r| r.into_domain()))
}

/// 心跳。位置回退（倒退超过 5 秒）时不覆盖 —— 客户端 seek 抖动不该抹掉真实进度。
pub async fn heartbeat(
    db: &Db,
    id: Id,
    position_ms: i64,
    duration_ms: Option<i64>,
) -> Result<bool> {
    let r = sqlx::query(
        "UPDATE playback_session
            SET position_ms = ?,
                duration_ms = COALESCE(?, duration_ms),
                updated_at  = ?
          WHERE id = ?
            AND ? >= position_ms - 5000",
    )
    .bind(position_ms)
    .bind(duration_ms)
    .bind(now_str())
    .bind(id.to_string())
    .bind(position_ms)
    .execute(db.pool())
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 兜底心跳：强制写入，忽略回退保护。停止播放时用。
pub async fn force_position(db: &Db, id: Id, position_ms: i64) -> Result<()> {
    sqlx::query("UPDATE playback_session SET position_ms = ?, updated_at = ? WHERE id = ?")
        .bind(position_ms)
        .bind(now_str())
        .bind(id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn stop_session(db: &Db, id: Id, position_ms: Option<i64>) -> Result<()> {
    sqlx::query(
        "UPDATE playback_session
            SET position_ms = COALESCE(?, position_ms), stopped_at = ?, updated_at = ?
          WHERE id = ?",
    )
    .bind(position_ms)
    .bind(now_str())
    .bind(now_str())
    .bind(id.to_string())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 续播点视图。"继续观看"页面只需要这些字段。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ResumePoint {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub session_id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub file_id: Id,
    pub title: String,
    /// 条目类型。海报卡片要按它决定跳详情页还是剧集页 ——
    /// 少了它，"继续观看"里每一张卡片都只能当电影处理。
    pub kind: MediaKind,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub strategy: String,
    pub updated_at: String,
}

/// 最近观看。同一部片只取最新一条，已看完（进度 > 95%）的不进列表。
pub async fn resume_points(db: &Db, user_id: Option<Id>, limit: u32) -> Result<Vec<ResumePoint>> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            i64,
            Option<i64>,
            String,
            String,
        ),
    >(
        "SELECT s.id, s.item_id, s.file_id, i.title, i.kind, s.position_ms, s.duration_ms,
                s.strategy, s.updated_at
           FROM playback_session s
           JOIN media_item i ON i.id = s.item_id
          WHERE s.stopped_at IS NULL
            AND (? IS NULL OR s.user_id = ?)
            AND s.position_ms > 30000
            AND (s.duration_ms IS NULL OR s.position_ms < s.duration_ms * 95 / 100)
          ORDER BY s.updated_at DESC
          LIMIT ?",
    )
    .bind(user_id.map(|u| u.to_string()))
    .bind(user_id.map(|u| u.to_string()))
    .bind(limit as i64)
    .fetch_all(db.pool())
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    let mut seen = std::collections::HashSet::new();
    for (sid, iid, fid, title, kind, pos, dur, strategy, updated) in rows {
        let (Some(sid), Some(iid), Some(fid)) = (to_id(&sid), to_id(&iid), to_id(&fid)) else {
            continue;
        };
        // 同一部片只保留最新的一条（上面已按时间倒序）。
        if !seen.insert(iid) {
            continue;
        }
        out.push(ResumePoint {
            session_id: sid,
            item_id: iid,
            file_id: fid,
            title,
            kind: to_kind(&kind),
            position_ms: pos,
            duration_ms: dur,
            strategy,
            updated_at: updated,
        });
    }
    Ok(out)
}

/// 播放一个文件所需的全部落库信息：文件本体 + 所属媒体库根 + 所属条目。
///
/// 合成一条 SQL 而不是分三次查：播放是"用户正在等"的路径（§107），
/// 每多一次往返都是首帧时间的开销。
#[derive(Debug, Clone)]
pub struct FileTarget {
    pub file: MediaFile,
    pub library_root: String,
    pub item_id: Id,
    pub title: String,
}

pub async fn file_target(db: &Db, file_id: Id) -> Result<Option<FileTarget>> {
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            Option<String>,
            i64,
            Option<i64>,
            Option<i64>,
            String,
            i32,
            String,
            String,
        ),
    >(
        "SELECT mf.id, mf.version_id, mf.storage_provider, mf.path, mf.container,
                mf.size_bytes, mf.duration_ms, mf.bitrate,
                mf.file_fingerprint, mf.is_available,
                li.root_path, mi.id AS item_id
           FROM media_file mf
           JOIN media_version mv ON mv.id = mf.version_id
           JOIN media_item   mi ON mi.id = mv.item_id
           JOIN library      li ON li.id = mi.library_id
          WHERE mf.id = ?",
    )
    .bind(file_id.to_string())
    .fetch_optional(db.pool())
    .await?;

    let Some((
        id,
        version_id,
        provider,
        path,
        container,
        size,
        duration,
        bitrate,
        fingerprint,
        avail,
        root,
        item_id,
    )) = row
    else {
        return Ok(None);
    };

    Ok(Some(FileTarget {
        file: MediaFile {
            id: parse_id(&id)?,
            version_id: parse_id(&version_id)?,
            storage_provider: provider,
            path,
            container: container.as_deref().map(crate::rows::to_container),
            size_bytes: size,
            duration_ms: duration,
            bitrate,
            file_fingerprint: fingerprint,
            probe_json: None,
            is_available: avail != 0,
        },
        library_root: root,
        item_id: parse_id(&item_id)?,
        title: String::new(),
    }))
}

/// 某个条目下的所有文件（含流）。详情页与播放前的版本选择用。
pub async fn files_for_item(db: &Db, item_id: Id) -> Result<Vec<(MediaFile, Vec<MediaStream>)>> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            Option<String>,
            i64,
            Option<i64>,
            Option<i64>,
            String,
            i32,
        ),
    >(
        "SELECT mf.id, mf.version_id, mf.storage_provider, mf.path, mf.container,
                mf.size_bytes, mf.duration_ms, mf.bitrate, mf.file_fingerprint, mf.is_available
           FROM media_file mf
           JOIN media_version mv ON mv.id = mf.version_id
          WHERE mv.item_id = ?
          ORDER BY mf.path",
    )
    .bind(item_id.to_string())
    .fetch_all(db.pool())
    .await?;

    let mut out = Vec::new();
    for (id, version_id, provider, path, container, size, duration, bitrate, fingerprint, avail) in
        rows
    {
        let Some(file_id) = to_id(&id) else { continue };
        let streams = sqlx::query_as::<_, crate::rows::StreamRow>(
            "SELECT * FROM media_stream WHERE file_id = ? ORDER BY idx",
        )
        .bind(file_id.to_string())
        .fetch_all(db.pool())
        .await?
        .into_iter()
        .filter_map(|r| r.into_domain())
        .collect();
        out.push((
            MediaFile {
                id: file_id,
                version_id: to_id(&version_id).unwrap_or_else(Id::nil),
                storage_provider: provider,
                path,
                container: container.as_deref().map(crate::rows::to_container),
                size_bytes: size,
                duration_ms: duration,
                bitrate,
                file_fingerprint: fingerprint,
                probe_json: None,
                is_available: avail != 0,
            },
            streams,
        ));
    }
    Ok(out)
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct SessionRow {
    id: String,
    user_id: Option<String>,
    item_id: String,
    file_id: String,
    device_id: Option<String>,
    client_id: Option<String>,
    strategy: String,
    strategy_reason: Option<String>,
    position_ms: i64,
    duration_ms: Option<i64>,
    created_at: String,
    updated_at: String,
    stopped_at: Option<String>,
}

impl SessionRow {
    fn into_domain(self) -> Option<PlaybackSession> {
        Some(PlaybackSession {
            id: to_id(&self.id)?,
            user_id: self.user_id.as_deref().and_then(to_id),
            item_id: to_id(&self.item_id)?,
            file_id: to_id(&self.file_id)?,
            device_id: self.device_id,
            client_id: self.client_id,
            strategy: to_strategy(&self.strategy),
            strategy_reason: self.strategy_reason,
            position_ms: self.position_ms,
            duration_ms: self.duration_ms,
            created_at: parse_time(&self.created_at),
            updated_at: parse_time(&self.updated_at),
            stopped_at: self.stopped_at.as_deref().map(parse_time),
        })
    }
}

// ─────────────────────────────────────────────────────────────
// 转码任务
// ─────────────────────────────────────────────────────────────

fn to_state(s: &str) -> TranscodeState {
    match s {
        "pending" => TranscodeState::Pending,
        "probing" => TranscodeState::Probing,
        "running" => TranscodeState::Running,
        "paused" => TranscodeState::Paused,
        "succeeded" => TranscodeState::Succeeded,
        "failed" => TranscodeState::Failed,
        _ => TranscodeState::Cancelled,
    }
}

fn from_state(s: TranscodeState) -> &'static str {
    match s {
        TranscodeState::Pending => "pending",
        TranscodeState::Probing => "probing",
        TranscodeState::Running => "running",
        TranscodeState::Paused => "paused",
        TranscodeState::Succeeded => "succeeded",
        TranscodeState::Failed => "failed",
        TranscodeState::Cancelled => "cancelled",
    }
}

#[derive(Debug, Clone)]
pub struct NewTranscodeJob {
    pub session_id: Id,
    pub file_id: Option<Id>,
    pub args: Vec<String>,
    pub hw_accel: Option<String>,
    pub output_kind: Option<String>,
    pub output_dir: Option<String>,
}

pub async fn insert_transcode_job(db: &Db, new: NewTranscodeJob) -> Result<Id> {
    let id = Id::new_v4();
    let now = now_str();
    sqlx::query(
        "INSERT INTO transcode_job (
            id, session_id, file_id, args, hw_accel, state, progress,
            output_kind, output_dir, pid, created_at, started_at, finished_at, error
         ) VALUES (?,?,?,?,?,'pending',0,?,?,NULL,?,NULL,NULL,NULL)",
    )
    .bind(id.to_string())
    .bind(new.session_id.to_string())
    .bind(new.file_id.map(|f| f.to_string()))
    .bind(serde_json::to_string(&new.args)?)
    .bind(&new.hw_accel)
    .bind(&new.output_kind)
    .bind(&new.output_dir)
    .bind(&now)
    .execute(db.pool())
    .await?;
    Ok(id)
}

pub async fn set_transcode_state(
    db: &Db,
    id: Id,
    state: TranscodeState,
    progress: Option<f32>,
    error: Option<String>,
    pid: Option<i32>,
) -> Result<()> {
    let mut q = QueryBuilder::<sqlx::Sqlite>::new("UPDATE transcode_job SET state = ");
    q.push_bind(from_state(state));
    if let Some(p) = progress {
        q.push(", progress = ").push_bind(p);
    }
    if let Some(e) = error {
        q.push(", error = ").push_bind(e);
    }
    if let Some(p) = pid {
        q.push(", pid = ").push_bind(p);
    }
    if matches!(state, TranscodeState::Running) {
        q.push(", started_at = ").push_bind(now_str());
    }
    if matches!(
        state,
        TranscodeState::Succeeded | TranscodeState::Failed | TranscodeState::Cancelled
    ) {
        q.push(", finished_at = ").push_bind(now_str());
    }
    q.push(" WHERE id = ").push_bind(id.to_string());
    q.build().execute(db.pool()).await?;
    Ok(())
}

pub async fn get_transcode_job(db: &Db, id: Id) -> Result<Option<TranscodeJob>> {
    let row = sqlx::query_as::<_, JobRow>(
        "SELECT id, session_id, file_id, args, hw_accel, state, progress,
                output_kind, output_dir, pid, created_at, started_at, finished_at, error
           FROM transcode_job WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(db.pool())
    .await?;
    Ok(row.and_then(|r| r.into_domain()))
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct JobRow {
    id: String,
    session_id: String,
    file_id: Option<String>,
    args: String,
    hw_accel: Option<String>,
    state: String,
    progress: f32,
    output_kind: Option<String>,
    output_dir: Option<String>,
    pid: Option<i32>,
    created_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    error: Option<String>,
}

impl JobRow {
    fn into_domain(self) -> Option<TranscodeJob> {
        Some(TranscodeJob {
            id: to_id(&self.id)?,
            session_id: to_id(&self.session_id)?,
            file_id: self.file_id.as_deref().and_then(to_id),
            args: serde_json::from_str(&self.args).unwrap_or_default(),
            hw_accel: self.hw_accel,
            state: to_state(&self.state),
            progress: self.progress,
            output_kind: self.output_kind,
            output_dir: self.output_dir,
            pid: self.pid,
            created_at: parse_time(&self.created_at),
            started_at: self.started_at.as_deref().map(parse_time),
            finished_at: self.finished_at.as_deref().map(parse_time),
            error: self.error,
        })
    }
}

// ─────────────────────────────────────────────────────────────
// 流式输出登记（僵尸分片回收的依据）
// ─────────────────────────────────────────────────────────────

/// 流式输出的输出目录与生命周期状态。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StreamSession {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub job_id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub file_id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub session_id: Option<Id>,
    pub kind: String,
    pub strategy: String,
    pub output_dir: String,
    pub args: Vec<String>,
    pub state: String,
    pub created_at: String,
    pub last_access_at: String,
}

#[derive(Debug, Clone)]
pub struct NewStreamSession {
    pub job_id: Id,
    pub file_id: Id,
    pub session_id: Option<Id>,
    pub kind: String,
    pub strategy: String,
    pub output_dir: String,
    pub args: Vec<String>,
}

pub async fn insert_stream_session(db: &Db, new: NewStreamSession) -> Result<()> {
    let now = now_str();
    sqlx::query(
        "INSERT INTO transcode_session (
            job_id, file_id, session_id, kind, strategy, output_dir, args,
            state, created_at, last_access_at
         ) VALUES (?,?,?,?,?,?,?,'running',?,?)
         ON CONFLICT(job_id) DO UPDATE SET
            output_dir = excluded.output_dir,
            args = excluded.args,
            state = 'running',
            last_access_at = excluded.last_access_at",
    )
    .bind(new.job_id.to_string())
    .bind(new.file_id.to_string())
    .bind(new.session_id.map(|s| s.to_string()))
    .bind(&new.kind)
    .bind(&new.strategy)
    .bind(&new.output_dir)
    .bind(serde_json::to_string(&new.args)?)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await?;
    Ok(())
}

pub async fn get_stream_session(db: &Db, job_id: Id) -> Result<Option<StreamSession>> {
    let row = sqlx::query_as::<_, StreamRow>(
        "SELECT job_id, file_id, session_id, kind, strategy, output_dir, args,
                state, created_at, last_access_at
           FROM transcode_session WHERE job_id = ?",
    )
    .bind(job_id.to_string())
    .fetch_optional(db.pool())
    .await?;
    Ok(row.map(|r| r.into_view()))
}

/// 刷新访问时间。播放中每来一个分片请求都会打到这里，
/// 所以回收器可以按"多久没被访问"来判断僵尸。
pub async fn touch_stream_session(db: &Db, job_id: Id) -> Result<()> {
    sqlx::query("UPDATE transcode_session SET last_access_at = ? WHERE job_id = ?")
        .bind(now_str())
        .bind(job_id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

pub async fn set_stream_state(db: &Db, job_id: Id, state: &str) -> Result<()> {
    sqlx::query("UPDATE transcode_session SET state = ?, last_access_at = ? WHERE job_id = ?")
        .bind(state)
        .bind(now_str())
        .bind(job_id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

/// 列出超过 `max_idle_secs` 秒没被访问的流式输出。回收器据此删目录。
pub async fn list_idle_stream_sessions(db: &Db, max_idle_secs: i64) -> Result<Vec<StreamSession>> {
    let cutoff = (Utc::now() - chrono::Duration::seconds(max_idle_secs)).to_rfc3339();
    let rows = sqlx::query_as::<_, StreamRow>(
        "SELECT job_id, file_id, session_id, kind, strategy, output_dir, args,
                state, created_at, last_access_at
           FROM transcode_session
          WHERE last_access_at < ?
          ORDER BY last_access_at ASC
          LIMIT 200",
    )
    .bind(cutoff)
    .fetch_all(db.pool())
    .await?;
    Ok(rows.into_iter().map(|r| r.into_view()).collect())
}

pub async fn delete_stream_session(db: &Db, job_id: Id) -> Result<()> {
    sqlx::query("DELETE FROM transcode_session WHERE job_id = ?")
        .bind(job_id.to_string())
        .execute(db.pool())
        .await?;
    Ok(())
}

#[derive(Debug, Clone, sqlx::FromRow)]
struct StreamRow {
    job_id: String,
    file_id: String,
    session_id: Option<String>,
    kind: String,
    strategy: String,
    output_dir: String,
    args: String,
    state: String,
    created_at: String,
    last_access_at: String,
}

impl StreamRow {
    fn into_view(self) -> StreamSession {
        StreamSession {
            job_id: to_id(&self.job_id).unwrap_or_else(Id::nil),
            file_id: to_id(&self.file_id).unwrap_or_else(Id::nil),
            session_id: self.session_id.as_deref().and_then(to_id),
            kind: self.kind,
            strategy: self.strategy,
            output_dir: self.output_dir,
            args: serde_json::from_str(&self.args).unwrap_or_default(),
            state: self.state,
            created_at: self.created_at,
            last_access_at: self.last_access_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Db;

    async fn db() -> Db {
        let d = Db::open_in_memory().await.unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("migrations");
        d.migrate(&dir).await.unwrap();
        d
    }

    async fn seed_item(d: &Db) -> (Id, Id) {
        // 直接插入最小可用的 library/item/version/file，避免依赖 P1 的入库逻辑。
        let library = Id::new_v4();
        sqlx::query(
            "INSERT INTO library (id, name, kind, storage_provider, root_path, created_at)
             VALUES (?,'lib','mixed','local','/media','2026-01-01T00:00:00Z')",
        )
        .bind(library.to_string())
        .execute(d.pool())
        .await
        .unwrap();
        let item = Id::new_v4();
        let version = Id::new_v4();
        let file = Id::new_v4();
        sqlx::query(
            "INSERT INTO media_item (id, library_id, kind, title, locked_fields, added_at, match_title)
             VALUES (?,?,'movie','Test','[]','2026-01-01T00:00:00Z','Test')",
        )
        .bind(item.to_string())
        .bind(library.to_string())
        .execute(d.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO media_version (id, item_id, name, hdr_type, is_default)
             VALUES (?,?,'v','sdr',1)",
        )
        .bind(version.to_string())
        .bind(item.to_string())
        .execute(d.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO media_file (id, version_id, storage_provider, path, size_bytes, file_fingerprint, is_available)
             VALUES (?,?,'local','/x.mkv',1,'f',1)",
        )
        .bind(file.to_string())
        .bind(version.to_string())
        .execute(d.pool())
        .await
        .unwrap();
        (item, file)
    }

    #[tokio::test]
    async fn session_roundtrip_and_resume() {
        let d = db().await;
        let (item, file) = seed_item(&d).await;

        let s = create_session(
            &d,
            NewSession {
                item_id: item,
                file_id: file,
                strategy: PlaybackStrategy::DirectPlay,
                user_id: None,
                device_id: Some("dev-1".into()),
                client_id: Some("web-chrome".into()),
                strategy_reason: Some("全部兼容".into()),
                position_ms: 0,
                duration_ms: Some(7_200_000),
            },
        )
        .await
        .unwrap();

        assert!(heartbeat(&d, s.id, 600_000, None).await.unwrap());
        let got = get_session(&d, s.id).await.unwrap().unwrap();
        assert_eq!(got.position_ms, 600_000);
        assert_eq!(got.client_id.as_deref(), Some("web-chrome"));
        assert_eq!(got.strategy, PlaybackStrategy::DirectPlay);

        // 心跳回退保护：倒退 1 分钟不该被接受（600s - 60s = 540s > 5s 容差）。
        assert!(!heartbeat(&d, s.id, 540_000, None).await.unwrap());
        assert_eq!(
            get_session(&d, s.id).await.unwrap().unwrap().position_ms,
            600_000
        );

        // 进度小于 30 秒的不进"继续观看"。
        let pts = resume_points(&d, None, 20).await.unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].title, "Test");

        // 看完（>95%）后不再出现。
        force_position(&d, s.id, 7_100_000).await.unwrap();
        assert!(resume_points(&d, None, 20).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stop_marks_stopped_at() {
        let d = db().await;
        let (item, file) = seed_item(&d).await;
        let s = create_session(
            &d,
            NewSession {
                item_id: item,
                file_id: file,
                strategy: PlaybackStrategy::Transcode,
                user_id: None,
                device_id: None,
                client_id: None,
                strategy_reason: None,
                position_ms: 0,
                duration_ms: None,
            },
        )
        .await
        .unwrap();
        stop_session(&d, s.id, Some(1234)).await.unwrap();
        let got = get_session(&d, s.id).await.unwrap().unwrap();
        assert!(got.stopped_at.is_some());
        assert_eq!(got.position_ms, 1234);
        // 已停止的会话不该出现在续播列表。
        assert!(resume_points(&d, None, 20).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn transcode_job_lifecycle() {
        let d = db().await;
        let (item, file) = seed_item(&d).await;
        let s = create_session(
            &d,
            NewSession {
                item_id: item,
                file_id: file,
                strategy: PlaybackStrategy::Transcode,
                user_id: None,
                device_id: None,
                client_id: None,
                strategy_reason: None,
                position_ms: 0,
                duration_ms: None,
            },
        )
        .await
        .unwrap();

        let job = insert_transcode_job(
            &d,
            NewTranscodeJob {
                session_id: s.id,
                file_id: Some(file),
                args: vec!["-i".into(), "x.mkv".into()],
                hw_accel: Some("vaapi".into()),
                output_kind: Some("hls".into()),
                output_dir: Some("/cache/hls/1".into()),
            },
        )
        .await
        .unwrap();

        set_transcode_state(&d, job, TranscodeState::Running, None, None, Some(4321))
            .await
            .unwrap();
        let j = get_transcode_job(&d, job).await.unwrap().unwrap();
        assert_eq!(j.state, TranscodeState::Running);
        assert_eq!(j.pid, Some(4321));
        assert!(j.started_at.is_some());
        assert_eq!(j.args.len(), 2);

        set_transcode_state(
            &d,
            job,
            TranscodeState::Failed,
            Some(0.4),
            Some("ffmpeg 退出码 1".into()),
            None,
        )
        .await
        .unwrap();
        let j = get_transcode_job(&d, job).await.unwrap().unwrap();
        assert_eq!(j.state, TranscodeState::Failed);
        assert!(j.finished_at.is_some());
        assert_eq!(j.error.as_deref(), Some("ffmpeg 退出码 1"));
    }

    #[tokio::test]
    async fn stream_session_registry_and_reap() {
        let d = db().await;
        let (_, file) = seed_item(&d).await;
        let job = Id::new_v4();

        insert_stream_session(
            &d,
            NewStreamSession {
                job_id: job,
                file_id: file,
                session_id: None,
                kind: "hls".into(),
                strategy: "transcode".into(),
                output_dir: "/cache/hls/x".into(),
                args: vec!["-f".into(), "hls".into()],
            },
        )
        .await
        .unwrap();

        let s = get_stream_session(&d, job).await.unwrap().unwrap();
        assert_eq!(s.state, "running");
        assert_eq!(s.output_dir, "/cache/hls/x");

        // 刚访问过，不该被回收。
        assert!(list_idle_stream_sessions(&d, 600).await.unwrap().is_empty());
        // 阈值设为 -1 秒（cutoff 在未来）→ 一定命中。
        assert_eq!(list_idle_stream_sessions(&d, -1).await.unwrap().len(), 1);

        set_stream_state(&d, job, "cancelled").await.unwrap();
        delete_stream_session(&d, job).await.unwrap();
        assert!(get_stream_session(&d, job).await.unwrap().is_none());
    }
}
