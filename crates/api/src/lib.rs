//! Axum 路由层 —— 系统唯一对外出口。
//!
//! 约定（见 DEVELOPMENT-PLAN.md §七）：
//!   · 前缀 `/api/v1`，全 JSON
//!   · 错误统一 `{ error: { code, message, trace_id } }`
//!   · 列表统一 `{ data: [...], page: { total, page, page_size } }`
//!
//! 未接数据库时（例如只做健康检查的容器探针），数据类端点返回 503 而不是 501，
//! 这样前端能区分"功能没做"和"后端没准备好"。

pub mod metadata;
pub mod playback;

#[cfg(feature = "openapi")]
pub mod openapi;

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use whatmedia_config::AppConfig;
use whatmedia_core::{Id, MediaKind};
use whatmedia_database::repo::tv::EpisodeQuery;
use whatmedia_database::repo::ItemQuery;
use whatmedia_database::Db;
use whatmedia_metadata::{ImageCache, Scraper};
use whatmedia_scanner::{ScanConfig, ScanControl, ScanProgress, Scanner};
use whatmedia_storage::LocalStorage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

// ─────────────────────────────────────────────────────────────
// 状态
// ─────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct ScanRegistry {
    inner: Arc<Mutex<HashMap<Uuid, ScanTaskView>>>,
}

#[derive(Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanTaskView {
    pub id: Uuid,
    pub library: String,
    pub progress: ScanProgress,
}

impl ScanRegistry {
    pub fn insert(&self, view: ScanTaskView) {
        self.inner.lock().unwrap().insert(view.id, view);
    }

    pub fn update(&self, id: Uuid, progress: ScanProgress) {
        if let Some(v) = self.inner.lock().unwrap().get_mut(&id) {
            v.progress = progress;
        }
    }

    pub fn list(&self) -> Vec<ScanTaskView> {
        self.inner.lock().unwrap().values().cloned().collect()
    }
}

/// 媒体库根 → 存储实例。
///
/// 同一个库根只建一次：每次播放都重新 canonicalize 一遍库根，
/// 在机械盘上是能感知到的开销。
#[derive(Clone, Default)]
pub struct StorageRegistry {
    inner: Arc<Mutex<HashMap<String, Arc<LocalStorage>>>>,
}

impl StorageRegistry {
    pub fn get(&self, root: &str) -> Option<Arc<LocalStorage>> {
        self.inner.lock().unwrap().get(root).cloned()
    }

    pub fn insert(&self, root: &str, storage: Arc<LocalStorage>) {
        self.inner.lock().unwrap().insert(root.to_string(), storage);
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub db: Option<Db>,
    pub ffprobe: Option<Arc<whatmedia_media::FfprobeRunner>>,
    /// 刮削器。没有配置任何 Provider 时是 None，元数据端点返回 501。
    pub scraper: Option<Arc<Scraper>>,
    pub images: Option<Arc<ImageCache>>,
    pub scans: ScanRegistry,
    pub scrapes: metadata::ScrapeRegistry,
    /// 播放与转码运行时（P3）。
    pub transcode: playback::TranscodeRuntime,
    /// 播放会话。需要数据库，所以启动阶段装好。
    pub sessions: Option<Arc<whatmedia_playback::SessionManager>>,
    pub storage: StorageRegistry,
    pub started_at: chrono::DateTime<Utc>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let hls_root = config.paths.cache_dir.join("hls");
        Self {
            config: Arc::new(config),
            db: None,
            ffprobe: None,
            scraper: None,
            images: None,
            scans: ScanRegistry::default(),
            scrapes: metadata::ScrapeRegistry::default(),
            transcode: playback::TranscodeRuntime::new(None, Vec::new(), hls_root),
            sessions: None,
            storage: StorageRegistry::default(),
            started_at: Utc::now(),
        }
    }

    pub fn with_db(mut self, db: Db) -> Self {
        self.db = Some(db);
        self
    }

    pub fn with_ffprobe(mut self, runner: Option<Arc<whatmedia_media::FfprobeRunner>>) -> Self {
        self.ffprobe = runner;
        self
    }

    pub fn with_scraper(mut self, scraper: Option<Arc<Scraper>>) -> Self {
        self.scraper = scraper;
        self
    }

    pub fn with_images(mut self, cache: Option<Arc<ImageCache>>) -> Self {
        self.images = cache;
        self
    }

    /// 装配播放运行时：ffmpeg 句柄 + 硬件能力 + 会话管理器。
    pub fn with_playback(
        mut self,
        ffmpeg: Option<Arc<whatmedia_transcoder::FfmpegRunner>>,
        profiles: Vec<whatmedia_core::HardwareProfile>,
    ) -> Self {
        let hls_root = self.config.paths.cache_dir.join("hls");
        self.transcode = playback::TranscodeRuntime::new(ffmpeg, profiles, hls_root);
        if let Some(db) = self.db.clone() {
            self.sessions = Some(Arc::new(whatmedia_playback::SessionManager::new(db)));
        }
        self
    }

    /// 取某个媒体库根对应的存储；首次访问时创建并缓存。
    pub fn storage_for(&self, root: &str) -> Result<Arc<LocalStorage>, Box<Response>> {
        if let Some(s) = self.storage.get(root) {
            return Ok(s);
        }
        let s = match LocalStorage::new("local", std::path::PathBuf::from(root)) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                return Err(Box::new(bad_request(format!(
                    "媒体库根不可访问 {root}：{e}"
                ))))
            }
        };
        self.storage.insert(root, s.clone());
        Ok(s)
    }

    /// 取数据库；没有就返回 503。
    ///
    /// 错误类型是 `Box<Response>` 而不是 `Response`：`Response` 超过 128 字节，
    /// 直接放进 `Result` 会让每个调用点都背着这么大的返回值（clippy::result_large_err）。
    fn db_or_503(&self) -> Result<Db, Box<Response>> {
        self.db
            .clone()
            .ok_or_else(|| Box::new(unavailable("数据库尚未就绪")))
    }
}

/// 统一错误信封。所有 4xx/5xx 都必须用它，不得裸返回字符串。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ErrorDetail {
    pub code: &'static str,
    pub message: String,
    pub trace_id: Option<String>,
}

/// 500。后端错误一律走它，绝不把内部错误细节直接暴露成别的码。
pub fn internal(e: impl std::fmt::Display) -> Response {
    error_response(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", e.to_string())
}

pub fn bad_request(msg: impl Into<String>) -> Response {
    error_response(StatusCode::BAD_REQUEST, "BAD_REQUEST", msg)
}

pub fn unavailable(msg: impl Into<String>) -> Response {
    error_response(StatusCode::SERVICE_UNAVAILABLE, "DB_UNAVAILABLE", msg)
}

pub fn error_response(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
) -> Response {
    let body = ErrorBody {
        error: ErrorDetail {
            code,
            message: message.into(),
            trace_id: None,
        },
    };
    (status, Json(body)).into_response()
}

// ─────────────────────────────────────────────────────────────
// 处理器
// ─────────────────────────────────────────────────────────────

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/health",
        responses(
            (status = 200, description = "健康检查报告。`/healthz` 是同一个处理器的别名，供容器 HEALTHCHECK 使用", body = whatmedia_diagnostics::HealthReport),
        ),
        tag = "system"
    )
)]
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let report = whatmedia_diagnostics::collect(env!("CARGO_PKG_VERSION"), state.started_at);
    (StatusCode::OK, Json(report))
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/libraries",
        responses(
            (status = 200, description = "媒体库列表", body = Vec<whatmedia_core::Library>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn list_libraries(State(state): State<AppState>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::list_libraries(&db).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateLibrary {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub kind: Option<MediaKind>,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/libraries",
        request_body = CreateLibrary,
        responses(
            (status = 201, description = "已创建", body = whatmedia_core::Library),
            (status = 400, description = "目录不存在", body = ErrorBody),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn create_library(
    State(state): State<AppState>,
    Json(body): Json<CreateLibrary>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    if !std::path::Path::new(&body.path).is_dir() {
        return bad_request(format!("目录不存在：{}", body.path));
    }
    match whatmedia_database::repo::insert_library(
        &db,
        &body.name,
        body.kind.unwrap_or(MediaKind::Mixed),
        "local",
        &body.path,
        &[],
    )
    .await
    {
        Ok(lib) => (StatusCode::CREATED, Json(lib)).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/media",
        params(
        ("library_id" = Option<String>, Query, description = "按媒体库过滤"),
        ("kind" = Option<String>, Query, description = "按类型过滤"),
        ("year" = Option<i32>, Query, description = "按年份过滤"),
        ("sort" = Option<String>, Query, description = "added_at | title | year | rating"),
        ("order" = Option<String>, Query, description = "desc | asc"),
        ("page" = Option<u32>, Query, description = "从 1 开始"),
        ("page_size" = Option<u32>, Query, description = "上限 200"),
        ),
        responses(
            (status = 200, description = "成功", body = whatmedia_core::Page<whatmedia_database::repo::ItemSummary>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn list_media(State(state): State<AppState>, Query(q): Query<ItemQuery>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::list_items(&db, &q).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/movies",
        params(
        ("library_id" = Option<String>, Query, description = "按媒体库过滤"),
        ("kind" = Option<String>, Query, description = "按类型过滤"),
        ("year" = Option<i32>, Query, description = "按年份过滤"),
        ("sort" = Option<String>, Query, description = "added_at | title | year | rating"),
        ("order" = Option<String>, Query, description = "desc | asc"),
        ("page" = Option<u32>, Query, description = "从 1 开始"),
        ("page_size" = Option<u32>, Query, description = "上限 200"),
        ),
        responses(
            (status = 200, description = "成功", body = whatmedia_core::Page<whatmedia_database::repo::ItemSummary>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn list_movies(State(state): State<AppState>, Query(q): Query<ItemQuery>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let mut q = q;
    q.kind = Some(MediaKind::Movie);
    match whatmedia_database::repo::list_items(&db, &q).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Paging {
    #[serde(default = "one")]
    pub page: u32,
    #[serde(default = "fifty")]
    pub page_size: u32,
}

fn one() -> u32 {
    1
}
fn fifty() -> u32 {
    50
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/series",
        params(
        ("page" = Option<u32>, Query),
        ("page_size" = Option<u32>, Query),
        ),
        responses(
            (status = 200, description = "成功", body = whatmedia_core::Page<whatmedia_database::repo::SeriesSummary>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn list_series(State(state): State<AppState>, Query(p): Query<Paging>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let params = whatmedia_database::repo::tv::PageParams {
        page: p.page,
        page_size: p.page_size,
    };
    match whatmedia_database::repo::list_series(&db, params).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EpisodeParams {
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub series_id: Option<Id>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub season_id: Option<Id>,
    #[serde(default = "one")]
    pub page: u32,
    #[serde(default = "fifty")]
    pub page_size: u32,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/episodes",
        params(
        ("series_id" = Option<String>, Query, description = "按剧集过滤"),
        ("season_id" = Option<String>, Query, description = "按季过滤"),
        ("page" = Option<u32>, Query),
        ("page_size" = Option<u32>, Query),
        ),
        responses(
            (status = 200, description = "成功", body = whatmedia_core::Page<whatmedia_database::repo::EpisodeSummary>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn list_episodes(State(state): State<AppState>, Query(p): Query<EpisodeParams>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let q = EpisodeQuery {
        series_id: p.series_id,
        season_id: p.season_id,
        page: Some(p.page),
        page_size: Some(p.page_size),
    };
    match whatmedia_database::repo::list_episodes(&db, &q).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/items/{id}",
        params(
        ("id" = String, Path, description = "条目 id"),
        ),
        responses(
            (status = 200, description = "成功", body = whatmedia_database::repo::ItemDetail),
            (status = 404, description = "条目不存在", body = ErrorBody),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn get_item(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::get_item(&db, id).await {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "条目不存在"),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SearchParams {
    pub q: String,
    #[serde(flatten)]
    pub page: ItemQuery,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/search",
        params(
        ("q" = String, Query, description = "搜索词"),
        ("library_id" = Option<String>, Query, description = "按媒体库过滤"),
        ("kind" = Option<String>, Query, description = "按类型过滤"),
        ("year" = Option<i32>, Query, description = "按年份过滤"),
        ("sort" = Option<String>, Query, description = "added_at | title | year | rating"),
        ("order" = Option<String>, Query, description = "desc | asc"),
        ("page" = Option<u32>, Query, description = "从 1 开始"),
        ("page_size" = Option<u32>, Query, description = "上限 200"),
        ),
        responses(
            (status = 200, description = "成功", body = whatmedia_core::Page<whatmedia_database::repo::ItemSummary>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn search(State(state): State<AppState>, Query(p): Query<SearchParams>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::search_items(&db, &p.q, &p.page).await {
        Ok(page) => Json(page).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/tasks",
        responses(
            (status = 200, description = "在途任务", body = Vec<ScanTaskView>),
        ),
        tag = "system"
    )
)]
async fn list_tasks(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.scans.list())
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanRequest {
    pub path: String,
    pub name: Option<String>,
    #[serde(default)]
    pub probe: bool,
    #[serde(default = "four")]
    pub concurrency: usize,
}

fn four() -> usize {
    4
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanAccepted {
    pub task_id: Uuid,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub library_id: Id,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/scan",
        request_body = ScanRequest,
        responses(
            (status = 202, description = "已受理，异步执行", body = ScanAccepted),
            (status = 400, description = "路径非法", body = ErrorBody),
        ),
        tag = "system"
    )
)]
async fn start_scan(State(state): State<AppState>, Json(body): Json<ScanRequest>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    if !std::path::Path::new(&body.path).is_dir() {
        return bad_request(format!("目录不存在：{}", body.path));
    }

    let name = body.name.clone().unwrap_or_else(|| {
        std::path::Path::new(&body.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "媒体库".into())
    });

    let root = match std::path::Path::new(&body.path).canonicalize() {
        Ok(p) => whatmedia_storage::clean_path(&p),
        Err(e) => return bad_request(format!("目录不可访问 {}：{e}", body.path)),
    };

    let library =
        match whatmedia_database::repo::find_library_by_root(&db, "local", &root.to_string_lossy())
            .await
        {
            Ok(Some(l)) => l,
            Ok(None) => {
                match whatmedia_database::repo::insert_library(
                    &db,
                    &name,
                    MediaKind::Mixed,
                    "local",
                    &body.path,
                    &[],
                )
                .await
                {
                    Ok(l) => l,
                    Err(e) => return internal(e),
                }
            }
            Err(e) => return internal(e),
        };

    let storage = match LocalStorage::new("local", &root) {
        Ok(s) => Arc::new(s),
        Err(e) => return bad_request(e.to_string()),
    };

    let cfg = ScanConfig {
        concurrency: body.concurrency.max(1),
        incremental: true,
        probe: body.probe && state.ffprobe.is_some(),
        ffprobe: state.ffprobe.clone(),
        ..Default::default()
    };
    let scanner = Arc::new(Scanner::new(storage, db, cfg));

    let task_id = Uuid::new_v4();
    let (tx, mut rx) = tokio::sync::broadcast::channel::<ScanProgress>(64);
    state.scans.insert(ScanTaskView {
        id: task_id,
        library: library.name.clone(),
        progress: ScanProgress::default(),
    });

    let registry = state.scans.clone();
    let lib = library.clone();
    tokio::spawn(async move {
        let ctrl = ScanControl::new();
        let _stats = scanner.scan(&lib, ctrl, Some(tx)).await;
        while let Ok(p) = rx.recv().await {
            registry.update(task_id, p.clone());
            if p.finished {
                break;
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(ScanAccepted {
            task_id,
            library_id: library.id,
        }),
    )
        .into_response()
}

async fn fallback() -> Response {
    error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "资源不存在")
}

// ─────────────────────────────────────────────────────────────
// 路由
// ─────────────────────────────────────────────────────────────

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // 健康检查：容器 HEALTHCHECK 与 CI 冒烟都打这个
        .route("/healthz", get(healthz))
        .route("/api/v1/health", get(healthz))
        // 媒体库
        .route(
            "/api/v1/libraries",
            get(list_libraries).post(create_library),
        )
        // 媒体资产
        .route("/api/v1/media", get(list_media))
        .route("/api/v1/movies", get(list_movies))
        .route("/api/v1/series", get(list_series))
        .route("/api/v1/episodes", get(list_episodes))
        .route("/api/v1/items/{id}", get(get_item))
        // 检索
        .route("/api/v1/search", get(search))
        // 任务与扫描
        .route("/api/v1/tasks", get(list_tasks))
        .route("/api/v1/scan", post(start_scan))
        // 元数据与图片（P2）
        .route("/api/v1/scrape", post(metadata::start_scrape))
        .route("/api/v1/scrape/stats", get(metadata::scrape_stats))
        .route("/api/v1/scrape/runs", get(metadata::scrape_runs))
        .route("/api/v1/metadata/review", get(metadata::review_queue))
        .route("/api/v1/metadata/candidates", get(metadata::candidates))
        .route("/api/v1/metadata/cache/stats", get(metadata::cache_stats))
        .route("/api/v1/metadata/cache/evict", post(metadata::cache_evict))
        .route("/api/v1/items/{id}/identify", post(metadata::identify))
        .route("/api/v1/items/{id}/locks", post(metadata::set_locks))
        .route("/api/v1/items/{id}/images", get(metadata::item_images))
        .route(
            "/api/v1/items/{id}/images/{kind}",
            get(metadata::item_image),
        )
        // 播放（P3）
        .route("/api/v1/playback/decide", get(playback::decide_playback))
        .route(
            "/api/v1/playback/sessions",
            post(playback::create_playback_session),
        )
        .route(
            "/api/v1/playback/sessions/{id}/heartbeat",
            post(playback::session_heartbeat),
        )
        .route(
            "/api/v1/playback/sessions/{id}/stop",
            post(playback::session_stop),
        )
        .route("/api/v1/playback/resume", get(playback::resume_list))
        .route("/api/v1/playback/streams", get(playback::list_streams))
        .route("/api/v1/hardware/profile", get(playback::hardware_profile))
        .route(
            "/api/v1/items/{id}/subtitles",
            get(playback::item_subtitles),
        )
        .route(
            "/api/v1/subtitles/external",
            post(playback::external_subtitle),
        )
        .route("/api/v1/stream/{id}", get(playback::stream_direct))
        .route("/api/v1/stream/{id}/remux", get(playback::stream_remux))
        .route("/api/v1/hls/{job}/master.m3u8", get(playback::hls_master))
        .route("/api/v1/hls/{job}/stop", post(playback::hls_stop))
        .route("/api/v1/hls/{job}/{name}", get(playback::hls_file))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .fallback(fallback)
}

/// 供集成测试使用：无需绑定端口即可发起请求。
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use whatmedia_core::Page;
    use serde_json::json;
    use tower::ServiceExt;

    fn test_router() -> Router {
        let mut cfg = AppConfig::default();
        cfg.server.port = 0;
        build_router(AppState::new(cfg))
    }

    async fn get_json(uri: &str) -> (StatusCode, serde_json::Value) {
        let res = test_router()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!(null));
        (status, v)
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let res = test_router()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_route_returns_error_envelope() {
        let res = test_router()
            .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn data_endpoints_report_db_unavailable_without_db() {
        // 没接数据库时必须是 503，不能是 200 空列表 —— 前端据此判断后端没准备好。
        for uri in [
            "/api/v1/libraries",
            "/api/v1/media",
            "/api/v1/movies",
            "/api/v1/series",
            "/api/v1/episodes",
            "/api/v1/search?q=x",
        ] {
            let (status, body) = get_json(uri).await;
            assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{uri}");
            assert_eq!(body["error"]["code"], "DB_UNAVAILABLE");
        }
    }

    #[test]
    fn error_body_shape_is_stable() {
        let body = ErrorBody {
            error: ErrorDetail {
                code: "X",
                message: "y".into(),
                trace_id: None,
            },
        };
        assert_eq!(json!(body)["error"]["code"], "X");
    }

    #[test]
    fn page_envelope_is_uniform() {
        let p: Page<u32> = Page::new(vec![1, 2], 10, 1, 50);
        assert_eq!(json!(p)["page"]["total"], 10);
        assert_eq!(json!(p)["page"]["page_size"], 50);
    }
}
