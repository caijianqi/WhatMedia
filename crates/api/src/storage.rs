//! 存储源账号管理与目录浏览 API。
//!
//! 支持通过标准 WebDAV 协议无缝挂载 AList / CD2 / 云盘网关（百度、夸克、迅雷、移动云盘等），
//! 并向前端目录选择器与媒体库创建流程提供统一的目录树浏览能力。

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;
use whatmedia_core::{StorageEntry, StorageProvider};
use whatmedia_database::repo::{
    delete_storage_account, get_storage_account, insert_storage_account, list_storage_accounts,
    update_storage_account, CreateStorageAccountInput, UpdateStorageAccountInput,
};
use whatmedia_storage::{WebDavAuth, WebDavConfig, WebDavStorage};

use crate::{bad_request, internal, not_found, AppState};

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateStorageRequest {
    pub name: String,
    /// 存储驱动类型：'webdav' | 'local' | 'alist'
    pub driver: String,
    /// 服务端端点，如 "http://127.0.0.1:5244/dav"
    pub endpoint: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_root_path")]
    pub root_path: String,
    #[serde(default = "default_true")]
    pub direct_stream: bool,
    #[serde(default)]
    pub extra_headers: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateStorageRequest {
    pub name: Option<String>,
    pub endpoint: Option<String>,
    pub auth_type: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub root_path: Option<String>,
    pub direct_stream: Option<bool>,
    pub extra_headers: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TestStorageRequest {
    pub driver: String,
    pub endpoint: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    pub username: Option<String>,
    pub password: Option<String>,
    #[serde(default = "default_root_path")]
    pub root_path: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TestStorageResponse {
    pub ok: bool,
    pub message: String,
    pub latency_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct BrowseQuery {
    #[serde(default)]
    pub path: Option<String>,
}

fn default_auth_type() -> String {
    "none".into()
}

fn default_root_path() -> String {
    "/".into()
}

fn default_true() -> bool {
    true
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/storages", get(list_storages).post(create_storage))
        .route("/api/v1/storages/test", post(test_storage))
        .route(
            "/api/v1/storages/{id}",
            get(get_storage)
                .put(update_storage)
                .delete(delete_storage_endpoint),
        )
        .route("/api/v1/storages/{id}/browse", get(browse_storage))
}

async fn list_storages(State(state): State<AppState>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    match list_storage_accounts(&db).await {
        Ok(list) => (StatusCode::OK, Json(list)).into_response(),
        Err(e) => internal(e),
    }
}

async fn create_storage(
    State(state): State<AppState>,
    Json(body): Json<CreateStorageRequest>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    if body.name.trim().is_empty() {
        return bad_request("存储源名称不能为空");
    }
    if body.endpoint.trim().is_empty() {
        return bad_request("存储源端点地址不能为空");
    }

    let input = CreateStorageAccountInput {
        name: body.name.trim().to_string(),
        driver: body.driver.trim().to_lowercase(),
        endpoint: body.endpoint.trim().to_string(),
        auth_type: body.auth_type.trim().to_lowercase(),
        username: body.username.filter(|s| !s.trim().is_empty()),
        password: body.password.filter(|s| !s.trim().is_empty()),
        root_path: body.root_path.trim().to_string(),
        direct_stream: body.direct_stream,
        extra_headers: body.extra_headers.unwrap_or_else(|| serde_json::json!({})),
    };

    match insert_storage_account(&db, input).await {
        Ok(item) => (StatusCode::CREATED, Json(item)).into_response(),
        Err(e) => internal(e),
    }
}

async fn get_storage(State(state): State<AppState>, AxumPath(id): AxumPath<String>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => return bad_request("无效的存储源 ID"),
    };

    match get_storage_account(&db, uuid).await {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => not_found("storage_account", id),
        Err(e) => internal(e),
    }
}

async fn update_storage(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<UpdateStorageRequest>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => return bad_request("无效的存储源 ID"),
    };

    let input = UpdateStorageAccountInput {
        name: body.name.map(|s| s.trim().to_string()),
        endpoint: body.endpoint.map(|s| s.trim().to_string()),
        auth_type: body.auth_type.map(|s| s.trim().to_lowercase()),
        username: body.username,
        password: body.password,
        root_path: body.root_path.map(|s| s.trim().to_string()),
        direct_stream: body.direct_stream,
        extra_headers: body.extra_headers,
    };

    match update_storage_account(&db, uuid, input).await {
        Ok(Some(item)) => (StatusCode::OK, Json(item)).into_response(),
        Ok(None) => not_found("storage_account", id),
        Err(e) => internal(e),
    }
}

async fn delete_storage_endpoint(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => return bad_request("无效的存储源 ID"),
    };

    match delete_storage_account(&db, uuid).await {
        Ok(true) => (StatusCode::OK, Json(serde_json::json!({ "ok": true }))).into_response(),
        Ok(false) => not_found("storage_account", id),
        Err(e) => internal(e),
    }
}

async fn test_storage(Json(body): Json<TestStorageRequest>) -> Response {
    let start = std::time::Instant::now();
    let auth = match body.auth_type.as_str() {
        "basic" => WebDavAuth::Basic {
            username: body.username.unwrap_or_default(),
            password: body.password.unwrap_or_default(),
        },
        "bearer" => WebDavAuth::Bearer {
            token: body.password.unwrap_or_default(),
        },
        _ => WebDavAuth::Anonymous,
    };

    let config = WebDavConfig {
        id: "test".into(),
        endpoint: body.endpoint,
        auth,
        root_path: body.root_path,
        direct_stream: true,
    };

    let storage = match WebDavStorage::new(config) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::OK,
                Json(TestStorageResponse {
                    ok: false,
                    message: format!("参数无效: {e}"),
                    latency_ms: None,
                }),
            )
                .into_response();
        }
    };

    match storage.list("/").await {
        Ok(entries) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            (
                StatusCode::OK,
                Json(TestStorageResponse {
                    ok: true,
                    message: format!("连接成功，根目录下发现 {} 个资源", entries.len()),
                    latency_ms: Some(latency_ms),
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::OK,
            Json(TestStorageResponse {
                ok: false,
                message: format!("连接测试失败: {e}"),
                latency_ms: Some(start.elapsed().as_millis() as u64),
            }),
        )
            .into_response(),
    }
}

/// 浏览存储目录树。支持 local 本地磁盘以及任意已配置的云盘/WebDAV 存储源。
async fn browse_storage(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Query(q): Query<BrowseQuery>,
) -> Response {
    let target_path = q.path.as_deref().unwrap_or("/").trim();

    if id == "local" {
        // 本地磁盘浏览
        let mut entries = Vec::new();
        let target_dir = if target_path.is_empty() || target_path == "/" {
            #[cfg(target_os = "windows")]
            {
                // Windows 下如果查询根路径，返回各个可用盘符
                for b in b'C'..=b'Z' {
                    let drive = format!("{}:\\", b as char);
                    if Path::new(&drive).exists() {
                        entries.push(StorageEntry {
                            path: drive.clone(),
                            name: format!("{}: 盘", b as char),
                            is_dir: true,
                            size: None,
                            modified_ns: None,
                        });
                    }
                }
                return (StatusCode::OK, Json(entries)).into_response();
            }
            #[cfg(not(target_os = "windows"))]
            {
                Path::new("/")
            }
        } else {
            Path::new(target_path)
        };

        let mut read_dir = match tokio::fs::read_dir(target_dir).await {
            Ok(rd) => rd,
            Err(e) => return bad_request(format!("无法读取本地目录: {e}")),
        };

        while let Ok(Some(entry)) = read_dir.next_entry().await {
            let path_buf = entry.path();
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            let metadata = entry.metadata().await.ok();
            let size = metadata.as_ref().map(|m| m.len() as i64);
            let modified_ns = metadata
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64);

            entries.push(StorageEntry {
                path: path_buf.to_string_lossy().to_string(),
                name: entry.file_name().to_string_lossy().to_string(),
                is_dir,
                size,
                modified_ns,
            });
        }

        // 目录排在前面
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
        return (StatusCode::OK, Json(entries)).into_response();
    }

    // 远程存储源（WebDAV / 云盘中转）
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let uuid = match Uuid::parse_str(&id) {
        Ok(u) => u,
        Err(_) => return bad_request("无效的存储源 ID"),
    };

    let account = match get_storage_account(&db, uuid).await {
        Ok(Some(acc)) => acc,
        Ok(None) => return not_found("storage_account", id),
        Err(e) => return internal(e),
    };

    let auth = match account.auth_type.as_str() {
        "basic" => WebDavAuth::Basic {
            username: account.username.unwrap_or_default(),
            password: account.password.unwrap_or_default(),
        },
        "bearer" => WebDavAuth::Bearer {
            token: account.password.unwrap_or_default(),
        },
        _ => WebDavAuth::Anonymous,
    };

    let config = WebDavConfig {
        id: account.id.to_string(),
        endpoint: account.endpoint,
        auth,
        root_path: account.root_path,
        direct_stream: account.direct_stream,
    };

    let storage = match WebDavStorage::new(config) {
        Ok(s) => s,
        Err(e) => return internal(e),
    };

    match storage.list(target_path).await {
        Ok(mut entries) => {
            entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
            (StatusCode::OK, Json(entries)).into_response()
        }
        Err(e) => internal(e),
    }
}
