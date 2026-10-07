//! 媒体库 CRUD。

use anyhow::Result;
use chrono::Utc;
use mosaic_core::{Id, Library, MediaKind};
use uuid::Uuid;

use crate::db::Db;
use crate::rows::{from_kind, LibraryRow};

pub async fn insert_library(
    db: &Db,
    name: &str,
    kind: MediaKind,
    storage_provider: &str,
    root_path: &str,
    metadata_providers: &[String],
) -> Result<Library> {
    let id = Uuid::new_v4();
    let providers = serde_json::to_string(metadata_providers)?;

    sqlx::query(
        "INSERT INTO library (id, name, kind, storage_provider, root_path, scan_policy, metadata_providers, created_at)
         VALUES (?, ?, ?, ?, ?, 'incremental', ?, ?)",
    )
    .bind(id.to_string())
    .bind(name)
    .bind(from_kind(kind))
    .bind(storage_provider)
    .bind(root_path)
    .bind(providers)
    .bind(Utc::now().to_rfc3339())
    .execute(db.pool())
    .await?;

    get_library(db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("写入后读不到媒体库 {id}"))
}

pub async fn list_libraries(db: &Db) -> Result<Vec<Library>> {
    let rows = sqlx::query_as::<_, LibraryRow>("SELECT * FROM library ORDER BY created_at")
        .fetch_all(db.pool())
        .await?;
    Ok(rows.into_iter().filter_map(|r| r.into_domain()).collect())
}

pub async fn get_library(db: &Db, id: Id) -> Result<Option<Library>> {
    let row = sqlx::query_as::<_, LibraryRow>("SELECT * FROM library WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(db.pool())
        .await?;
    Ok(row.and_then(|r| r.into_domain()))
}

/// 按 (存储, 根路径) 查找。扫描命令重复执行时靠它保持幂等，不产生重复库。
pub async fn find_library_by_root(
    db: &Db,
    storage_provider: &str,
    root_path: &str,
) -> Result<Option<Library>> {
    let row = sqlx::query_as::<_, LibraryRow>(
        "SELECT * FROM library WHERE storage_provider = ? AND root_path = ? LIMIT 1",
    )
    .bind(storage_provider)
    .bind(root_path)
    .fetch_optional(db.pool())
    .await?;
    Ok(row.and_then(|r| r.into_domain()))
}
