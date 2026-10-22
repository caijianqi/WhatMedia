//! 元数据写入与刮削队列的 SQL 层。
//!
//! 和 `library` 一样，所有 id 都用确定性 UUIDv5 —— 类型、人物都是共享实体，
//! 两部片共用一个"动作"类型时必须是同一行。

use anyhow::Result;
use chrono::Utc;
use sqlx::Row;

use whatmedia_core::{deterministic_id, Id, MetadataCredit};

use crate::Db;

// ─────────────────────────────────────────────────────────────
// 类型
// ─────────────────────────────────────────────────────────────

/// 覆盖式设置类型。
///
/// 先删后插而不是增量 diff：类型最多十来个，diff 省下的写入不值当复杂度，
/// 而"刮错了改回来"这个场景里 diff 很容易漏删。
pub async fn set_genres(db: &Db, item_id: Id, genres: &[String]) -> Result<()> {
    sqlx::query("DELETE FROM item_genre WHERE item_id = ?")
        .bind(item_id.to_string())
        .execute(db.pool())
        .await?;

    for name in genres {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let gid = deterministic_id("genre", &[&name.to_lowercase()]);
        sqlx::query("INSERT INTO genre (id, name) VALUES (?, ?) ON CONFLICT DO NOTHING")
            .bind(gid.to_string())
            .bind(name)
            .execute(db.pool())
            .await?;
        sqlx::query(
            "INSERT INTO item_genre (item_id, genre_id) VALUES (?, ?) ON CONFLICT DO NOTHING",
        )
        .bind(item_id.to_string())
        .bind(gid.to_string())
        .execute(db.pool())
        .await?;
    }
    Ok(())
}

pub async fn genres_of(db: &Db, item_id: Id) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT g.name FROM item_genre ig JOIN genre g ON g.id = ig.genre_id
         WHERE ig.item_id = ? ORDER BY g.name",
    )
    .bind(item_id.to_string())
    .fetch_all(db.pool())
    .await?;
    Ok(rows.iter().map(|r| r.get::<String, _>("name")).collect())
}

// ─────────────────────────────────────────────────────────────
// 演职员
// ─────────────────────────────────────────────────────────────

pub async fn set_credits(
    db: &Db,
    item_id: Id,
    provider: &str,
    credits: &[MetadataCredit],
) -> Result<()> {
    sqlx::query("DELETE FROM credit WHERE item_id = ?")
        .bind(item_id.to_string())
        .execute(db.pool())
        .await?;

    for c in credits {
        let name = c.name.trim();
        if name.is_empty() {
            continue;
        }
        // 同名同职业 = 同一个人。跨 Provider 的 id 不同，所以不能用 provider_id 做键。
        let pid = deterministic_id("person", &[&name.to_lowercase()]);
        sqlx::query(
            "INSERT INTO person (id, name, provider, provider_id) VALUES (?,?,?,?)
             ON CONFLICT DO NOTHING",
        )
        .bind(pid.to_string())
        .bind(name)
        .bind(provider)
        .bind(&c.provider_id)
        .execute(db.pool())
        .await?;

        sqlx::query(
            "INSERT INTO credit (person_id, item_id, role, character, sort_order)
             VALUES (?,?,?,?,?) ON CONFLICT DO NOTHING",
        )
        .bind(pid.to_string())
        .bind(item_id.to_string())
        .bind(&c.role)
        .bind(&c.character)
        .bind(c.sort_order)
        .execute(db.pool())
        .await?;
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreditRow {
    pub person: String,
    pub role: String,
    pub character: Option<String>,
    pub sort_order: i32,
}

pub async fn credits_of(db: &Db, item_id: Id) -> Result<Vec<CreditRow>> {
    let rows = sqlx::query(
        "SELECT p.name AS person, c.role, c.character, c.sort_order
         FROM credit c JOIN person p ON p.id = c.person_id
         WHERE c.item_id = ? ORDER BY c.sort_order",
    )
    .bind(item_id.to_string())
    .fetch_all(db.pool())
    .await?;
    if !rows.is_empty() {
        return Ok(rows
            .iter()
            .map(|r| CreditRow {
                person: r.get("person"),
                role: r.get("role"),
                character: r.get("character"),
                sort_order: r.get("sort_order"),
            })
            .collect());
    }

    // 若当前条目（如电视剧单集）本身无独立演职员，向上回溯同剧集的演职员
    let parent_rows = sqlx::query(
        "SELECT p.name AS person, c.role, c.character, c.sort_order
         FROM credit c
         JOIN person p ON p.id = c.person_id
         WHERE c.item_id IN (
             SELECT ep2.item_id FROM episode ep2
             WHERE ep2.season_id IN (
                 SELECT se.id FROM season se
                 WHERE se.series_id = (
                     SELECT se2.series_id FROM episode ep
                     JOIN season se2 ON se2.id = ep.season_id
                     WHERE ep.item_id = ?
                 )
             )
         )
         GROUP BY p.name, c.role
         ORDER BY c.sort_order",
    )
    .bind(item_id.to_string())
    .fetch_all(db.pool())
    .await?;

    Ok(parent_rows
        .iter()
        .map(|r| CreditRow {
            person: r.get("person"),
            role: r.get("role"),
            character: r.get("character"),
            sort_order: r.get("sort_order"),
        })
        .collect())
}

// ─────────────────────────────────────────────────────────────
// item / series 元数据写入
// ─────────────────────────────────────────────────────────────

/// 要写入 `media_item` 的元数据字段。`None` = 不动这一列。
#[derive(Debug, Clone, Default)]
pub struct ItemMetaUpdate {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub tagline: Option<String>,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    pub runtime_ms: Option<i64>,
    pub locked_fields: Option<String>,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
}

/// 只更新非 None 的列。
///
/// **绝不写全列 UPDATE**：刮削拿到的记录总是残缺的（没有 tagline、没有时长…），
/// 全列覆盖会把上次刮到的好数据清成 NULL。
pub async fn update_item_meta(db: &Db, item_id: Id, u: &ItemMetaUpdate) -> Result<()> {
    sqlx::query(
        "UPDATE media_item SET
            title          = ifnull(?, title),
            original_title = ifnull(?, original_title),
            sort_title     = ifnull(?, sort_title),
            year           = ifnull(?, year),
            tagline        = ifnull(?, tagline),
            overview       = ifnull(?, overview),
            rating         = ifnull(?, rating),
            runtime_ms     = ifnull(?, runtime_ms),
            locked_fields  = ifnull(?, locked_fields),
            provider       = ifnull(?, provider),
            provider_id    = ifnull(?, provider_id)
         WHERE id = ?",
    )
    .bind(&u.title)
    .bind(&u.original_title)
    .bind(&u.sort_title)
    .bind(u.year)
    .bind(&u.tagline)
    .bind(&u.overview)
    .bind(u.rating)
    .bind(u.runtime_ms)
    .bind(&u.locked_fields)
    .bind(&u.provider)
    .bind(&u.provider_id)
    .bind(item_id.to_string())
    .execute(db.pool())
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct SeriesMetaUpdate {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub locked_fields: Option<String>,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
}

pub async fn update_series_meta(db: &Db, series_id: Id, u: &SeriesMetaUpdate) -> Result<()> {
    sqlx::query(
        "UPDATE series SET
            title          = ifnull(?, title),
            original_title = ifnull(?, original_title),
            year           = ifnull(?, year),
            overview       = ifnull(?, overview),
            locked_fields  = ifnull(?, locked_fields),
            provider       = ifnull(?, provider),
            provider_id    = ifnull(?, provider_id)
         WHERE id = ?",
    )
    .bind(&u.title)
    .bind(&u.original_title)
    .bind(u.year)
    .bind(&u.overview)
    .bind(&u.locked_fields)
    .bind(&u.provider)
    .bind(&u.provider_id)
    .bind(series_id.to_string())
    .execute(db.pool())
    .await?;
    Ok(())
}

/// 读回当前值与锁定集合。合并逻辑在 `whatmedia-metadata`，这里只负责搬运。
pub async fn locked_fields_of_item(db: &Db, item_id: Id) -> Result<String> {
    let row = sqlx::query("SELECT locked_fields FROM media_item WHERE id = ?")
        .bind(item_id.to_string())
        .fetch_optional(db.pool())
        .await?;
    Ok(row
        .map(|r| r.get::<String, _>("locked_fields"))
        .unwrap_or_else(|| "[]".into()))
}

// ─────────────────────────────────────────────────────────────
// 刮削队列
// ─────────────────────────────────────────────────────────────

/// 把一个库里所有 item 排进队列。已完成的不会被重置 —— 这正是断点续跑的前提。
pub async fn enqueue_library(db: &Db, library_id: Id) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let r = sqlx::query(
        "INSERT INTO metadata_job (item_id, library_id, kind, status, attempts, updated_at)
         SELECT i.id, i.library_id, i.kind, 'pending', 0, ?
         FROM media_item i
         WHERE i.library_id = ?
           AND NOT EXISTS (SELECT 1 FROM metadata_job j WHERE j.item_id = i.id)",
    )
    .bind(&now)
    .bind(library_id.to_string())
    .execute(db.pool())
    .await?;
    Ok(r.rows_affected())
}

/// 只重置失败的任务。用户点"重试失败项"时用它。
pub async fn requeue_failed(db: &Db, library_id: Id) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let r = sqlx::query(
        "UPDATE metadata_job SET status = 'pending', attempts = 0, last_error = NULL, updated_at = ?
         WHERE library_id = ? AND status = 'failed'",
    )
    .bind(&now)
    .bind(library_id.to_string())
    .execute(db.pool())
    .await?;
    Ok(r.rows_affected())
}

/// 强制全量重新入队。无论先前状态是 done、failed、skipped 还是 pending，
/// 全部重置为 pending 并补齐未入队的条目，用于凭据变更或重新刷新图片与演职员。
pub async fn requeue_all(db: &Db, library_id: Option<Id>) -> Result<u64> {
    let now = Utc::now().to_rfc3339();
    let updated = match library_id {
        Some(l) => {
            sqlx::query(
                "UPDATE metadata_job SET status = 'pending', attempts = 0, last_error = NULL, updated_at = ?
                 WHERE library_id = ?",
            )
            .bind(&now)
            .bind(l.to_string())
            .execute(db.pool())
            .await?
            .rows_affected()
        }
        None => {
            sqlx::query(
                "UPDATE metadata_job SET status = 'pending', attempts = 0, last_error = NULL, updated_at = ?",
            )
            .bind(&now)
            .execute(db.pool())
            .await?
            .rows_affected()
        }
    };

    let inserted = match library_id {
        Some(l) => sqlx::query(
            "INSERT INTO metadata_job (item_id, library_id, kind, status, attempts, updated_at)
                 SELECT i.id, i.library_id, i.kind, 'pending', 0, ?
                 FROM media_item i
                 WHERE i.library_id = ?
                   AND NOT EXISTS (SELECT 1 FROM metadata_job j WHERE j.item_id = i.id)",
        )
        .bind(&now)
        .bind(l.to_string())
        .execute(db.pool())
        .await?
        .rows_affected(),
        None => sqlx::query(
            "INSERT INTO metadata_job (item_id, library_id, kind, status, attempts, updated_at)
                 SELECT i.id, i.library_id, i.kind, 'pending', 0, ?
                 FROM media_item i
                 WHERE NOT EXISTS (SELECT 1 FROM metadata_job j WHERE j.item_id = i.id)",
        )
        .bind(&now)
        .execute(db.pool())
        .await?
        .rows_affected(),
    };

    Ok(updated + inserted)
}

/// 查询所属剧集的所有单集 item_id
pub async fn series_item_ids(db: &Db, series_id: Id) -> Result<Vec<Id>> {
    let rows = sqlx::query(
        "SELECT e.item_id
         FROM episode e
         JOIN season se ON se.id = e.season_id
         WHERE se.series_id = ?",
    )
    .bind(series_id.to_string())
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .iter()
        .filter_map(|r| r.get::<String, _>("item_id").parse().ok())
        .collect())
}

#[derive(Debug, Clone)]
pub struct JobRow {
    pub item_id: Id,
    pub library_id: Id,
    pub kind: String,
    pub status: String,
    pub attempts: i64,
}

/// 取出一批待处理任务，并立刻标记为 `running`。
///
/// 标记和取出必须在**同一条语句**里完成，否则两个 worker 会拿到同一批。
/// SQLite 没有 `UPDATE ... RETURNING` 的并发保证（取决于版本与编译选项），
/// 所以这里用"先更新后查询"两步，靠 status 过滤保证不重复。
pub async fn claim_jobs(db: &Db, library_id: Option<Id>, limit: u32) -> Result<Vec<JobRow>> {
    let now = Utc::now().to_rfc3339();
    let ids: Vec<(String, String, String, i64)> = match library_id {
        Some(l) => {
            sqlx::query_as(
                "SELECT item_id, library_id, kind, attempts FROM metadata_job
                 WHERE status = 'pending' AND library_id = ? ORDER BY updated_at LIMIT ?",
            )
            .bind(l.to_string())
            .bind(limit as i64)
            .fetch_all(db.pool())
            .await?
        }
        None => {
            sqlx::query_as(
                "SELECT item_id, library_id, kind, attempts FROM metadata_job
                 WHERE status = 'pending' ORDER BY updated_at LIMIT ?",
            )
            .bind(limit as i64)
            .fetch_all(db.pool())
            .await?
        }
    };

    for (id, _, _, _) in &ids {
        sqlx::query("UPDATE metadata_job SET status = 'running', updated_at = ? WHERE item_id = ?")
            .bind(&now)
            .bind(id)
            .execute(db.pool())
            .await?;
    }

    Ok(ids
        .into_iter()
        .map(|(item_id, library_id, kind, attempts)| JobRow {
            item_id: to_id(&item_id),
            library_id: to_id(&library_id),
            kind,
            status: "running".into(),
            attempts,
        })
        .collect())
}

fn to_id(s: &str) -> Id {
    uuid::Uuid::parse_str(s).unwrap_or_else(|_| deterministic_id("id", &[s]))
}

pub async fn finish_job(
    db: &Db,
    item_id: Id,
    status: &str,
    provider: Option<&str>,
    provider_id: Option<&str>,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE metadata_job SET status = ?, attempts = attempts + 1, provider = ?,
            provider_id = ?, last_error = ?, updated_at = ?
         WHERE item_id = ?",
    )
    .bind(status)
    .bind(provider)
    .bind(provider_id)
    .bind(error)
    .bind(Utc::now().to_rfc3339())
    .bind(item_id.to_string())
    .execute(db.pool())
    .await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JobStats {
    pub pending: u64,
    pub running: u64,
    pub done: u64,
    pub failed: u64,
    pub review: u64,
    pub skipped: u64,
}

pub async fn job_stats(db: &Db, library_id: Option<Id>) -> Result<JobStats> {
    let rows = match library_id {
        Some(l) => sqlx::query(
            "SELECT status, count(*) AS n FROM metadata_job WHERE library_id = ? GROUP BY status",
        )
        .bind(l.to_string())
        .fetch_all(db.pool())
        .await?,
        None => {
            sqlx::query("SELECT status, count(*) AS n FROM metadata_job GROUP BY status")
                .fetch_all(db.pool())
                .await?
        }
    };
    let mut s = JobStats::default();
    for r in rows {
        let n = r.get::<i64, _>("n") as u64;
        match r.get::<String, _>("status").as_str() {
            "pending" => s.pending = n,
            "running" => s.running = n,
            "done" => s.done = n,
            "failed" => s.failed = n,
            "review" => s.review = n,
            "skipped" => s.skipped = n,
            _ => {}
        }
    }
    Ok(s)
}

/// 刮削一个 item 需要的最小上下文。
///
/// 剧集要带上 series_id / season / episode：拿不到这些就没法调 `get_episode`，
/// 而每集只刮剧集级信息会让"第 3 集 - 某某"这种集名永远缺着。
#[derive(Debug, Clone)]
pub struct ScrapeTarget {
    pub id: Id,
    pub library_id: Id,
    pub kind: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub locked_fields: String,
    /// 搜索用标题（文件名派生），用户改名不影响它。
    pub match_title: String,
    pub series_id: Option<Id>,
    pub series_title: Option<String>,
    pub series_match_title: Option<String>,
    /// 剧集在 Provider 侧的 id。单集元信息要靠它去取。
    pub series_provider_id: Option<String>,
    pub season_number: Option<i32>,
    pub episode_number: Option<i32>,
    /// 本地 NFO 的原始 XML。有它就走 NFO 优先链。
    pub nfo_json: Option<String>,
}

pub async fn scrape_target(db: &Db, item_id: Id) -> Result<Option<ScrapeTarget>> {
    let row = sqlx::query(
        "SELECT i.id, i.library_id, i.kind, i.title, i.original_title, i.year, i.locked_fields,
                ifnull(i.match_title, i.title) AS match_title,
                s.id AS series_id, s.title AS series_title, s.provider_id AS series_provider_id,
                ifnull(s.match_title, s.title) AS series_match_title,
                se.season_number, e.episode_number
         FROM media_item i
         LEFT JOIN episode e  ON e.item_id   = i.id
         LEFT JOIN season  se ON se.id       = e.season_id
         LEFT JOIN series s  ON s.id         = se.series_id
         WHERE i.id = ?",
    )
    .bind(item_id.to_string())
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|r| ScrapeTarget {
        id: item_id,
        library_id: to_id(&r.get::<String, _>("library_id")),
        kind: r.get("kind"),
        title: r.get("title"),
        original_title: r.get("original_title"),
        year: r.get("year"),
        locked_fields: r.get("locked_fields"),
        match_title: r.get("match_title"),
        series_id: r
            .get::<Option<String>, _>("series_id")
            .as_deref()
            .map(to_id),
        series_title: r.get("series_title"),
        series_match_title: r.get("series_match_title"),
        series_provider_id: r.get("series_provider_id"),
        season_number: r.get("season_number"),
        episode_number: r.get("episode_number"),
        nfo_json: None,
    }))
}

/// 列出需要人工确认的条目（status = 'review'）。
pub async fn list_review(db: &Db, library_id: Option<Id>, limit: u32) -> Result<Vec<ScrapeTarget>> {
    let rows = match library_id {
        Some(l) => {
            sqlx::query(
                "SELECT j.item_id FROM metadata_job j
             WHERE j.status = 'review' AND j.library_id = ? ORDER BY j.updated_at LIMIT ?",
            )
            .bind(l.to_string())
            .bind(limit as i64)
            .fetch_all(db.pool())
            .await?
        }
        None => sqlx::query(
            "SELECT item_id FROM metadata_job WHERE status = 'review' ORDER BY updated_at LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(db.pool())
        .await?,
    };

    let mut out = Vec::new();
    for r in rows {
        let id = to_id(&r.get::<String, _>("item_id"));
        if let Some(t) = scrape_target(db, id).await? {
            out.push(t);
        }
    }
    Ok(out)
}

pub async fn locked_fields_of_series(db: &Db, series_id: Id) -> Result<String> {
    let row = sqlx::query("SELECT locked_fields FROM series WHERE id = ?")
        .bind(series_id.to_string())
        .fetch_optional(db.pool())
        .await?;
    Ok(row
        .map(|r| r.get::<String, _>("locked_fields"))
        .unwrap_or_else(|| "[]".into()))
}

/// 把卡在 `running` 的任务放回 `pending`。
///
/// 进程崩溃时任务会永久留在 running，用户再点"重新刮削"也轮不到它们。
/// 启动刮削前先调一次，代价只是一条 UPDATE。
pub async fn reap_running(db: &Db, now: &str) -> Result<u64> {
    let r = sqlx::query(
        "UPDATE metadata_job SET status = 'pending', updated_at = ? WHERE status = 'running'",
    )
    .bind(now)
    .execute(db.pool())
    .await?;
    Ok(r.rows_affected())
}
