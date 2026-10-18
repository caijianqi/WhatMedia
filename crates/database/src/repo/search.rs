//! 全文检索。
//!
//! **中文方案（Spike S4 结论落地）**：SQLite FTS5 + `trigram` 分词器。
//! FTS5 没有内置中文分词器，trigram 把每个字切成三字重叠片段，
//! 于是"流浪地球"可以被"地球"命中 —— 代价是索引体积约 3 倍、短词无法命中。
//!
//! 短词（< 3 字符）trigram 无解，一律降级到 LIKE。
//! 超过 5 万条后评估切换 Tantivy（§75）。

use anyhow::Result;
use sqlx::QueryBuilder;
use whatmedia_core::Page;

use crate::db::Db;
use crate::repo::{page_of, ItemQuery, ItemSummary};
use crate::rows::ItemRow;

pub async fn search_items(db: &Db, term: &str, q: &ItemQuery) -> Result<Page<ItemSummary>> {
    let term = term.trim();
    if term.is_empty() {
        return Ok(page_of(Vec::new(), 0, q));
    }

    // 三字以下 trigram 无解，直接走 LIKE。
    let page = if char_count(term) < 3 {
        like_search(db, term, q).await?
    } else {
        match fts_search(db, term, q).await {
            Ok(p) => p,
            // FTS 语法错误 / 索引异常都不能让搜索接口 500，静默降级。
            Err(e) => {
                tracing::warn!("FTS 检索失败，降级到 LIKE：{e}");
                like_search(db, term, q).await?
            }
        }
    };
    Ok(page)
}

async fn fts_search(db: &Db, term: &str, q: &ItemQuery) -> Result<Page<ItemSummary>> {
    let pattern = fts_phrase(term);
    let (_, size) = q.clamp();

    let total: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM media_item
          WHERE rowid IN (SELECT rowid FROM media_item_fts WHERE media_item_fts MATCH ?)",
    )
    .bind(&pattern)
    .fetch_one(db.pool())
    .await?;

    let rows = sqlx::query_as::<_, ItemRow>(
        "SELECT * FROM media_item
          WHERE rowid IN (SELECT rowid FROM media_item_fts WHERE media_item_fts MATCH ?)
          ORDER BY title
          LIMIT ? OFFSET ?",
    )
    .bind(&pattern)
    .bind(size as i64)
    .bind(q.offset())
    .fetch_all(db.pool())
    .await?;

    Ok(page_of(items_from(rows), total.0, q))
}

async fn like_search(db: &Db, term: &str, q: &ItemQuery) -> Result<Page<ItemSummary>> {
    let like = format!("%{term}%");
    let (_, size) = q.clamp();

    let mut count =
        QueryBuilder::<sqlx::Sqlite>::new("SELECT COUNT(*) FROM media_item WHERE title LIKE ");
    count.push_bind(&like).push(" OR original_title LIKE ");
    count.push_bind(&like);
    let total: (i64,) = count.build_query_as().fetch_one(db.pool()).await?;

    let mut b = QueryBuilder::<sqlx::Sqlite>::new("SELECT * FROM media_item WHERE title LIKE ");
    b.push_bind(&like).push(" OR original_title LIKE ");
    b.push_bind(&like).push(" ORDER BY title LIMIT ");
    b.push_bind(size as i64).push(" OFFSET ");
    b.push_bind(q.offset());

    let rows = b.build_query_as::<ItemRow>().fetch_all(db.pool()).await?;
    Ok(page_of(items_from(rows), total.0, q))
}

fn items_from(rows: Vec<ItemRow>) -> Vec<ItemSummary> {
    rows.into_iter()
        .filter_map(|r| r.into_domain())
        .map(|i| ItemSummary::from_row(&i))
        .collect()
}

/// 把用户输入包成 FTS5 短语字面量：`"` 需转义为 `""`。
///
/// 不做转义的话，用户输入里的 `-`、`*`、`OR` 会被当成 FTS 查询语法，
/// 轻则结果不对，重则整个查询报错。
fn fts_phrase(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

fn char_count(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phrase_escapes_quotes() {
        assert_eq!(fts_phrase(r#"a"b"#), "\"a\"\"b\"");
    }

    #[test]
    fn short_terms_are_detected() {
        assert_eq!(char_count("地球"), 2);
        assert_eq!(char_count("流浪地球"), 4);
    }
}
