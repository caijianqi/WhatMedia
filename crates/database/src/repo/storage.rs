//! 存储源账号 CRUD。

use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;
use whatmedia_core::{Id, StorageAccount};

use crate::db::Db;
use crate::rows::StorageAccountRow;

#[derive(Debug, Clone)]
pub struct CreateStorageAccountInput {
    pub name: String,
    pub driver: String,
    pub endpoint: String,
    pub auth_type: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub root_path: String,
    pub direct_stream: bool,
    pub extra_headers: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct UpdateStorageAccountInput {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub auth_type: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub root_path: Option<String>,
    pub direct_stream: Option<bool>,
    pub extra_headers: Option<serde_json::Value>,
}

pub async fn insert_storage_account(
    db: &Db,
    input: CreateStorageAccountInput,
) -> Result<StorageAccount> {
    let id = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    let headers_json = serde_json::to_string(&input.extra_headers)?;

    sqlx::query(
        "INSERT INTO storage_account (id, name, driver, endpoint, auth_type, username, password, root_path, direct_stream, extra_headers, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(&input.name)
    .bind(&input.driver)
    .bind(&input.endpoint)
    .bind(&input.auth_type)
    .bind(&input.username)
    .bind(&input.password)
    .bind(&input.root_path)
    .bind(if input.direct_stream { 1 } else { 0 })
    .bind(headers_json)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await?;

    get_storage_account(db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("写入后未查询到存储源账号 {id}"))
}

pub async fn list_storage_accounts(db: &Db) -> Result<Vec<StorageAccount>> {
    let rows = sqlx::query_as::<_, StorageAccountRow>(
        "SELECT * FROM storage_account ORDER BY created_at ASC",
    )
    .fetch_all(db.pool())
    .await?;

    Ok(rows.into_iter().filter_map(|r| r.into_domain()).collect())
}

pub async fn get_storage_account(db: &Db, id: Id) -> Result<Option<StorageAccount>> {
    let row = sqlx::query_as::<_, StorageAccountRow>(
        "SELECT * FROM storage_account WHERE id = ?",
    )
    .bind(id.to_string())
    .fetch_optional(db.pool())
    .await?;

    Ok(row.and_then(|r| r.into_domain()))
}

pub async fn update_storage_account(
    db: &Db,
    id: Id,
    input: UpdateStorageAccountInput,
) -> Result<Option<StorageAccount>> {
    let existing = match get_storage_account(db, id).await? {
        Some(e) => e,
        None => return Ok(None),
    };

    let name = input.name.unwrap_or(existing.name);
    let endpoint = input.endpoint.unwrap_or(existing.endpoint);
    let auth_type = input.auth_type.unwrap_or(existing.auth_type);
    let username = input.username.or(existing.username);
    let password = input.password.or(existing.password);
    let root_path = input.root_path.unwrap_or(existing.root_path);
    let direct_stream = input.direct_stream.unwrap_or(existing.direct_stream);
    let extra_headers = input.extra_headers.unwrap_or(existing.extra_headers);
    let headers_json = serde_json::to_string(&extra_headers)?;
    let now = Utc::now().to_rfc3339();

    sqlx::query(
        "UPDATE storage_account
         SET name = ?, endpoint = ?, auth_type = ?, username = ?, password = ?, root_path = ?, direct_stream = ?, extra_headers = ?, updated_at = ?
         WHERE id = ?",
    )
    .bind(name)
    .bind(endpoint)
    .bind(auth_type)
    .bind(username)
    .bind(password)
    .bind(root_path)
    .bind(if direct_stream { 1 } else { 0 })
    .bind(headers_json)
    .bind(&now)
    .bind(id.to_string())
    .execute(db.pool())
    .await?;

    get_storage_account(db, id).await
}

pub async fn delete_storage_account(db: &Db, id: Id) -> Result<bool> {
    let res = sqlx::query("DELETE FROM storage_account WHERE id = ?")
        .bind(id.to_string())
        .execute(db.pool())
        .await?;

    Ok(res.rows_affected() > 0)
}
