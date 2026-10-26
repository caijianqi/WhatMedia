//! 全文检索。
//!
//! **中文方案（Spike S4 结论落地）**：SQLite FTS5 + `trigram` 分词器。
//! FTS5 没有内置中文分词器，trigram 把每个字切成三字重叠片段，
//! 于是"流浪地球"可以被"地球"命中 —— 代价是索引体积约 3 倍、短词无法命中。
//!
//! 短词（< 3 字符）trigram 无解，一律降级到 LIKE。
//! 超过 5 万条后评估切换 Tantivy（§75）。

use anyhow::Result;
use whatmedia_core::Page;

use crate::db::Db;
use crate::repo::{page_of, ItemQuery, ItemSummary};

#[derive(Debug, sqlx::FromRow)]
struct SearchItemRow {
    id: Option<String>,
    kind: String,
    title: String,
    original_title: Option<String>,
    year: Option<i32>,
    rating: Option<f64>,
    runtime_ms: Option<i64>,
    added_at: String,
    episode_count: Option<i64>,
}

impl SearchItemRow {
    fn into_summary(self) -> Option<ItemSummary> {
        let id = crate::rows::to_id(&self.id?)?;
        let kind = crate::rows::to_kind(&self.kind);
        Some(ItemSummary {
            id,
            kind,
            title: self.title,
            original_title: self.original_title,
            year: self.year,
            rating: self.rating,
            runtime_ms: self.runtime_ms,
            added_at: self.added_at,
            episode_count: self.episode_count,
        })
    }
}

pub async fn search_items(db: &Db, term: &str, q: &ItemQuery) -> Result<Page<ItemSummary>> {
    let term = term.trim();
    if term.is_empty() {
        return Ok(page_of(Vec::new(), 0, q));
    }

    let like = format!("%{term}%");
    let (_, size) = q.clamp();

    // 联合查询：未归属剧集的散片（电影/单片） + 剧集（支持按剧集标题、别名及单集名匹配）
    let count_sql = "SELECT COUNT(*) FROM (
        SELECT m.id FROM media_item m
        WHERE NOT EXISTS (SELECT 1 FROM episode e WHERE e.item_id = m.id)
          AND (m.title LIKE ? OR m.original_title LIKE ?)
        UNION ALL
        SELECT (SELECT e.item_id FROM episode e JOIN season se ON se.id = e.season_id WHERE se.series_id = s.id ORDER BY se.season_number ASC, e.episode_number ASC LIMIT 1) AS id
        FROM series s
        WHERE (s.title LIKE ? OR s.original_title LIKE ?)
           OR EXISTS (
               SELECT 1 FROM season se
               JOIN episode e ON e.season_id = se.id
               JOIN media_item m ON m.id = e.item_id
               WHERE se.series_id = s.id AND (m.title LIKE ? OR m.original_title LIKE ?)
           )
    ) WHERE id IS NOT NULL";

    let total: (i64,) = sqlx::query_as(count_sql)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .fetch_one(db.pool())
        .await?;

    let data_sql = "SELECT id, kind, title, original_title, year, rating, runtime_ms, added_at, episode_count FROM (
        SELECT 
            m.id AS id,
            m.kind AS kind,
            m.title AS title,
            m.original_title AS original_title,
            m.year AS year,
            m.rating AS rating,
            m.runtime_ms AS runtime_ms,
            m.added_at AS added_at,
            NULL AS episode_count
        FROM media_item m
        WHERE NOT EXISTS (SELECT 1 FROM episode e WHERE e.item_id = m.id)
          AND (m.title LIKE ? OR m.original_title LIKE ?)

        UNION ALL

        SELECT 
            (SELECT e.item_id 
             FROM episode e 
             JOIN season se ON se.id = e.season_id 
             JOIN media_item m ON m.id = e.item_id
             WHERE se.series_id = s.id 
             ORDER BY se.season_number ASC, e.episode_number ASC 
             LIMIT 1) AS id,
            COALESCE(
                (SELECT m.kind FROM episode e 
                 JOIN season se ON se.id = e.season_id 
                 JOIN media_item m ON m.id = e.item_id 
                 WHERE se.series_id = s.id AND m.kind IN ('anime', 'documentary', 'variety') 
                 LIMIT 1),
                'series'
            ) AS kind,
            s.title AS title,
            s.original_title AS original_title,
            s.year AS year,
            NULL AS rating,
            NULL AS runtime_ms,
            COALESCE(
                (SELECT m.added_at 
                 FROM episode e 
                 JOIN season se ON se.id = e.season_id 
                 JOIN media_item m ON m.id = e.item_id
                 WHERE se.series_id = s.id 
                 ORDER BY m.added_at DESC 
                 LIMIT 1),
                '1970-01-01T00:00:00Z'
            ) AS added_at,
            (SELECT COUNT(*) 
             FROM episode e 
             JOIN season se ON se.id = e.season_id 
             WHERE se.series_id = s.id) AS episode_count
        FROM series s
        WHERE (s.title LIKE ? OR s.original_title LIKE ?)
           OR EXISTS (
               SELECT 1 FROM season se
               JOIN episode e ON e.season_id = se.id
               JOIN media_item m ON m.id = e.item_id
               WHERE se.series_id = s.id AND (m.title LIKE ? OR m.original_title LIKE ?)
           )
    ) WHERE id IS NOT NULL
    ORDER BY title
    LIMIT ? OFFSET ?";

    let rows = sqlx::query_as::<_, SearchItemRow>(data_sql)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(&like)
        .bind(size as i64)
        .bind(q.offset())
        .fetch_all(db.pool())
        .await?;

    let items = rows.into_iter().filter_map(|r| r.into_summary()).collect();
    Ok(page_of(items, total.0, q))
}

#[allow(dead_code)]
fn char_count(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_terms_are_detected() {
        assert_eq!(char_count("凡人"), 2);
        assert_eq!(char_count("凡人修仙传"), 5);
    }
}
