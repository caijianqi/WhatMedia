//! 剧集结构：series / season / episode。

use anyhow::Result;
use chrono::Utc;
use mosaic_core::{Id, Page, Season, Series};
use serde::Serialize;
use sqlx::QueryBuilder;
use uuid::Uuid;

use crate::db::Db;

#[derive(Debug, Clone, Serialize)]
pub struct SeriesSummary {
    pub id: Id,
    pub title: String,
    pub year: Option<i32>,
    pub season_count: i64,
    pub episode_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpisodeSummary {
    pub id: Id,
    pub item_id: Id,
    pub series_id: Id,
    pub series_title: String,
    pub season_number: i32,
    pub episode_number: i32,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct PageParams {
    pub page: u32,
    pub page_size: u32,
}

impl Default for PageParams {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 50,
        }
    }
}

impl PageParams {
    pub fn clamp(&self) -> (u32, u32) {
        (self.page.max(1), self.page_size.clamp(1, 200))
    }
    pub fn offset(&self) -> i64 {
        let (page, size) = self.clamp();
        ((page - 1) as i64) * size as i64
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EpisodeQuery {
    pub series_id: Option<Id>,
    pub season_id: Option<Id>,
    pub page: Option<u32>,
    pub page_size: Option<u32>,
}

impl EpisodeQuery {
    pub fn params(&self) -> PageParams {
        PageParams {
            page: self.page.unwrap_or(1),
            page_size: self.page_size.unwrap_or(50),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 写入：全部按自然键幂等
// ─────────────────────────────────────────────────────────────

/// 幂等写入剧集。
///
/// **为什么不能先 SELECT 再 INSERT**：扫描是并发的，同一季的两集会同时走到这里，
/// 两条 SELECT 都返回 None，于是创建出两条同名 series。
/// 唯一索引 + `ON CONFLICT DO NOTHING` 让并发插入收敛到同一行 —— 这是测出来的。
pub async fn upsert_series(db: &Db, title: &str, year: Option<i32>) -> Result<Series> {
    if let Some(s) = find_series(db, title, year).await? {
        return Ok(s);
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO series (id, title, original_title, year, overview) VALUES (?,?,NULL,?,NULL)
         ON CONFLICT DO NOTHING",
    )
    .bind(id.to_string())
    .bind(title)
    .bind(year)
    .execute(db.pool())
    .await?;

    find_series(db, title, year)
        .await?
        .ok_or_else(|| anyhow::anyhow!("写入后读不到剧集 {title}"))
}

pub async fn find_series(db: &Db, title: &str, year: Option<i32>) -> Result<Option<Series>> {
    let row: Option<(String, String, Option<i32>)> = sqlx::query_as(
        "SELECT id, title, year FROM series
          WHERE lower(title) = lower(?)
            AND (year IS ? OR (year IS NULL AND ? IS NULL))
          LIMIT 1",
    )
    .bind(title)
    .bind(year)
    .bind(year)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|(id, title, year)| Series {
        id: crate::rows::to_id(&id).unwrap_or_else(Uuid::new_v4),
        title,
        original_title: None,
        year,
        overview: None,
    }))
}

pub async fn upsert_season(db: &Db, series_id: Id, number: i32) -> Result<Season> {
    if let Some(s) = find_season(db, series_id, number).await? {
        return Ok(s);
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO season (id, series_id, season_number, title) VALUES (?,?,?,NULL)
         ON CONFLICT(series_id, season_number) DO NOTHING",
    )
    .bind(id.to_string())
    .bind(series_id.to_string())
    .bind(number)
    .execute(db.pool())
    .await?;

    find_season(db, series_id, number)
        .await?
        .ok_or_else(|| anyhow::anyhow!("写入后读不到第 {number} 季"))
}

pub async fn find_season(db: &Db, series_id: Id, number: i32) -> Result<Option<Season>> {
    let row: Option<(String, i32)> = sqlx::query_as(
        "SELECT id, season_number FROM season WHERE series_id = ? AND season_number = ? LIMIT 1",
    )
    .bind(series_id.to_string())
    .bind(number)
    .fetch_optional(db.pool())
    .await?;

    Ok(row.map(|(id, season_number)| Season {
        id: crate::rows::to_id(&id).unwrap_or_else(Uuid::new_v4),
        series_id,
        season_number,
        title: None,
    }))
}

/// 一季一集只对应一个条目；重复扫描同一集会走 UPDATE 分支。
pub async fn upsert_episode(
    db: &Db,
    season_id: Id,
    episode_number: i32,
    item_id: Id,
    title: Option<&str>,
) -> Result<Id> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO episode (id, season_id, episode_number, item_id, title) VALUES (?,?,?,?,?)
         ON CONFLICT(season_id, episode_number) DO UPDATE SET
            item_id = excluded.item_id,
            title = excluded.title",
    )
    .bind(id.to_string())
    .bind(season_id.to_string())
    .bind(episode_number)
    .bind(item_id.to_string())
    .bind(title)
    .execute(db.pool())
    .await?;

    let row: Option<(String,)> =
        sqlx::query_as("SELECT id FROM episode WHERE season_id = ? AND episode_number = ? LIMIT 1")
            .bind(season_id.to_string())
            .bind(episode_number)
            .fetch_optional(db.pool())
            .await?;

    Ok(row.and_then(|(s,)| crate::rows::to_id(&s)).unwrap_or(id))
}

// ─────────────────────────────────────────────────────────────
// 读取
// ─────────────────────────────────────────────────────────────

pub async fn list_series(db: &Db, params: PageParams) -> Result<Page<SeriesSummary>> {
    let (_, size) = params.clamp();

    let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM series")
        .fetch_one(db.pool())
        .await?;

    let rows = sqlx::query_as::<_, (String, String, Option<i32>, i64, i64)>(
        "SELECT s.id, s.title, s.year,
                (SELECT COUNT(*) FROM season se WHERE se.series_id = s.id),
                (SELECT COUNT(*) FROM season se JOIN episode e ON e.season_id = se.id
                  WHERE se.series_id = s.id)
           FROM series s
          ORDER BY s.title
          LIMIT ? OFFSET ?",
    )
    .bind(size as i64)
    .bind(params.offset())
    .fetch_all(db.pool())
    .await?;

    let data = rows
        .into_iter()
        .filter_map(|(id, title, year, season_count, episode_count)| {
            Some(SeriesSummary {
                id: crate::rows::to_id(&id)?,
                title,
                year,
                season_count,
                episode_count,
            })
        })
        .collect();

    Ok(Page::new(data, total.0, params.clamp().0, size))
}

pub async fn list_episodes(db: &Db, q: &EpisodeQuery) -> Result<Page<EpisodeSummary>> {
    let params = q.params();
    let (_, size) = params.clamp();

    let mut count = QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT COUNT(*) FROM episode e JOIN season se ON se.id = e.season_id WHERE 1=1",
    );
    push_episode_filters(&mut count, q);
    let total: (i64,) = count.build_query_as().fetch_one(db.pool()).await?;

    let mut b = QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT e.id, e.item_id, e.episode_number, e.title,
                se.season_number, sr.id, sr.title
           FROM episode e
           JOIN season se ON se.id = e.season_id
           JOIN series sr ON sr.id = se.series_id
          WHERE 1=1",
    );
    push_episode_filters(&mut b, q);
    b.push(" ORDER BY sr.title, se.season_number, e.episode_number LIMIT ");
    b.push_bind(size as i64);
    b.push(" OFFSET ");
    b.push_bind(params.offset());

    let rows = b
        .build_query_as::<(String, String, i32, Option<String>, i32, String, String)>()
        .fetch_all(db.pool())
        .await?;

    let data = rows
        .into_iter()
        .filter_map(
            |(id, item_id, episode_number, title, season_number, series_id, series_title)| {
                Some(EpisodeSummary {
                    id: crate::rows::to_id(&id)?,
                    item_id: crate::rows::to_id(&item_id)?,
                    series_id: crate::rows::to_id(&series_id)?,
                    series_title,
                    season_number,
                    episode_number,
                    title,
                })
            },
        )
        .collect();

    Ok(Page::new(data, total.0, params.clamp().0, size))
}

fn push_episode_filters(b: &mut QueryBuilder<sqlx::Sqlite>, q: &EpisodeQuery) {
    if let Some(id) = q.season_id {
        b.push(" AND e.season_id = ").push_bind(id.to_string());
    }
    if let Some(id) = q.series_id {
        b.push(" AND se.series_id = ").push_bind(id.to_string());
    }
}

/// 供 CLI 显示扫描结果的时间戳用。
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}
