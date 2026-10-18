//! Axum 路由层 —— 系统唯一对外出口。
//!
//! 约定（见 开发计划 §七）：
//!   · 前缀 `/api/v1`，全 JSON
//!   · 错误统一 `{ error: { code, message, trace_id } }`
//!   · 列表统一 `{ data: [...], page: { total, page, page_size } }`
//!
//! 未接数据库时（例如只做健康检查的容器探针），数据类端点返回 503 而不是 501，
//! 这样前端能区分"功能没做"和"后端没准备好"。

pub mod ai;
pub mod auth;
pub mod live;
pub mod metadata;
pub mod playback;
pub mod social;

#[cfg(feature = "openapi")]
pub mod openapi;

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tower_http::trace::TraceLayer;
use uuid::Uuid;
use whatmedia_config::AppConfig;
use whatmedia_core::{Id, MediaKind};
use whatmedia_database::repo::tv::EpisodeQuery;
use whatmedia_database::repo::ItemQuery;
use whatmedia_database::Db;
use whatmedia_metadata::{ImageCache, Scraper};
use whatmedia_scanner::{ScanConfig, ScanControl, ScanProgress, Scanner};
use whatmedia_storage::LocalStorage;

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
    /// 签名 URL 的根密钥（多用户鉴权）。启动时生成或从配置加载，
    /// 之后只读 —— 放进 `Arc` 是为了让 `AppState` 保持廉价克隆。
    pub signing: Arc<whatmedia_auth::SigningKey>,
    /// AI 运行时（P7）。`None` 表示数据库没就绪 —— 此时 `/ai/*` 返回
    /// `DB_UNAVAILABLE`，而 `/api/v1/media` 等核心端点照常工作。
    pub ai: Option<Arc<whatmedia_ai::AiRuntime>>,
    /// 在途直播任务（P8）。与 `transcode.streams` 分开：
    /// 那张表按 job 索引，直播要的是**按频道**索引以便复用。
    pub live: live::LiveRegistry,
    /// 启动时被拒掉的设置覆盖项（`apply_setting_overrides` 的返回值）。
    ///
    /// 正常是空的。非空意味着库里存着不合法的值 —— 必须能通过
    /// `GET /api/v1/settings` 看到，否则用户永远不知道自己那次改动没生效。
    pub rejected_settings: Arc<Vec<String>>,
    pub started_at: chrono::DateTime<Utc>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        let hls_root = config.paths.cache_dir.join("hls");
        // 密钥要在 config 被搬进 Arc 之前读出来。
        let signing = auth::init_signing_key(&config.security);
        Self {
            config: Arc::new(config),
            db: None,
            ffprobe: None,
            scraper: None,
            images: None,
            scans: ScanRegistry::default(),
            scrapes: metadata::ScrapeRegistry::default(),
            transcode: playback::TranscodeRuntime::new(
                None,
                Vec::new(),
                hls_root,
                whatmedia_transcoder::AccelContext::default(),
            ),
            sessions: None,
            storage: StorageRegistry::default(),
            ai: None,
            signing,
            live: live::LiveRegistry::default(),
            rejected_settings: Arc::new(Vec::new()),
            started_at: Utc::now(),
        }
    }

    pub fn with_db(mut self, db: Db) -> Self {
        self.db = Some(db);
        self
    }

    /// 记下启动时被拒掉的设置覆盖项。见字段注释：它们必须能被界面看到。
    pub fn with_rejected_settings(mut self, rejected: Vec<String>) -> Self {
        self.rejected_settings = Arc::new(rejected);
        self
    }

    /// 被拒掉的覆盖项。给 `GET /api/v1/settings` 用。
    pub fn rejected_settings(&self) -> Vec<String> {
        self.rejected_settings.as_ref().clone()
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
        accel_ctx: whatmedia_transcoder::AccelContext,
    ) -> Self {
        let hls_root = self.config.paths.cache_dir.join("hls");
        self.transcode = playback::TranscodeRuntime::new(ffmpeg, profiles, hls_root, accel_ctx);
        if let Some(db) = self.db.clone() {
            self.sessions = Some(Arc::new(whatmedia_playback::SessionManager::new(db)));
        }
        self
    }

    /// 装配 AI 运行时（P7）。**必须在 `with_db` 之后调用**：`DbHost` 没有
    /// 数据库就没有意义。
    ///
    /// 没配模型也照装 —— 走规则引擎；只有"AI 开关没开"才让端点返回 503。
    /// 这条是刻意的：AI 是可选能力，它挂了不该让整个服务起不来。
    pub fn with_ai(mut self) -> Self {
        let actions = ai::AppActions::new(&self).map(|a| Arc::new(a) as Arc<dyn ai::AiActions>);
        let runtime = actions.and_then(|a| ai::build_ai_runtime(&self, a));
        self.ai = runtime;
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

/// 各类型的条目数。首页的分类 tab 用它决定渲染哪几个 tab。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/media/kinds",
        responses(
            (status = 200, description = "成功", body = Vec<whatmedia_database::repo::KindCount>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn media_kinds(State(state): State<AppState>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::kind_counts(&db).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => internal(e),
    }
}

/// 演职员表。
///
/// 拆成 `cast` / `crew` 两组而不是让前端自己按 role 分组：分组规则只有一处
/// 定义才不会漂移（"actor 是演员、其余都是幕后"这条规则写在服务端）。
/// role 的取值来自 `whatmedia_core::provider::MetadataCredit`，存的是小写。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreditsView {
    /// 演员。已按 `sort_order` 排好，前端通常只显示前 12 个。
    pub cast: Vec<whatmedia_database::repo::CreditRow>,
    /// 导演 / 编剧等幕后。同样按 sort_order。
    pub crew: Vec<whatmedia_database::repo::CreditRow>,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/items/{id}/credits",
        params(
        ("id" = String, Path, description = "条目 id"),
        ),
        responses(
            (status = 200, description = "成功", body = CreditsView),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn item_credits(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::metadata::credits_of(&db, id).await {
        // 没刮削过的条目返回两个空数组，而不是 404：
        // "条目存在但没有演职员"与"条目不存在"在详情页上是两种不同的空状态，
        // 而前端区分它们没有收益，一律渲染成"还没有演职员信息"即可。
        Ok(rows) => {
            let (cast, crew) = rows.into_iter().partition(|c| c.role == "actor");
            Json(CreditsView { cast, crew }).into_response()
        }
        Err(e) => internal(e),
    }
}

/// 条目所属剧集的端点。
///
/// **为什么是 `/items/{id}/series` 而不是让前端先猜剧集 id**：
/// 扫描出来的是"一集一个 `media_item`"，前端手里只有条目 id，
/// 拿不到 series_id。让服务端反查一次，比让前端拿 `/series` 翻页去比对标题可靠。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/items/{id}/series",
        params(
        ("id" = String, Path, description = "条目 id"),
        ),
        responses(
            (status = 200, description = "成功", body = Option<whatmedia_database::repo::SeriesRef>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn item_series(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::series_of_item(&db, id).await {
        // 电影 / 音乐这类条目不属于任何剧，返回 null 而不是 404：
        // 前端要的是"要不要渲染季列表"这个判断，null 就是答案，
        // 404 会把它变成一个需要 try/catch 的错误分支。
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/series/{id}/seasons",
        params(
        ("id" = String, Path, description = "剧集 id"),
        ),
        responses(
            (status = 200, description = "成功", body = Vec<whatmedia_database::repo::SeasonSummary>),
            (status = 503, description = "数据库未就绪", body = ErrorBody),
        ),
        tag = "media"
    )
)]
async fn series_seasons(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::list_seasons(&db, id).await {
        Ok(rows) => Json(rows).into_response(),
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
    // 通知要写库，所以把 db 也带进这个后台任务。
    let notify_db = state.db.clone();
    tokio::spawn(async move {
        let ctrl = ScanControl::new();
        let _stats = scanner.scan(&lib, ctrl, Some(tx)).await;
        while let Ok(p) = rx.recv().await {
            registry.update(task_id, p.clone());
            if p.finished {
                // 扫描是后台跑的，跑完时用户多半已经离开设置页了。
                // 结果只留在 `/api/v1/tasks` 里的话，没人会去看 —— 所以发一条通知。
                if let Some(db) = &notify_db {
                    let n = scan_notification(&lib.name, &p);
                    if let Err(e) = whatmedia_database::repo::notify_admins(db, &n).await {
                        tracing::warn!("写扫描完成通知失败：{e:#}");
                    }
                }
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

/// 把一次扫描结果变成一条通知。
///
/// 有失败项时升级成 `warn`：全部成功还弹一条"警告"会让人学会忽略通知，
/// 而"扫了 300 个文件、3 个失败"才是真的需要看一眼的那种。
fn scan_notification(
    library: &str,
    p: &whatmedia_scanner::ScanProgress,
) -> whatmedia_database::repo::NewNotification {
    use whatmedia_database::repo::{NewNotification, NotifyLevel};
    let title = format!("「{library}」扫描完成");
    let body = format!(
        "新增 {} 个条目，更新 {} 个，跳过 {} 个，失败 {} 个。",
        p.added, p.updated, p.skipped, p.failed
    );
    NewNotification {
        kind: "scan_finished",
        level: if p.failed > 0 {
            NotifyLevel::Warn
        } else {
            NotifyLevel::Info
        },
        title,
        body: Some(body),
        link: None,
    }
}

// ─────────────────────────────────────────────────────────────
// 路由
// ─────────────────────────────────────────────────────────────

/// 路由之外公开的部分。**每一行都要有理由**，理由写在注释里。
///
/// 这里刻意不用"给整个 router 加中间件再逐条豁免"的写法 —— 那种写法里，
/// 遗漏的代价是某个端点悄悄裸奔；而这种写法的代价是某个端点 401，
/// 后者会立刻被人发现。
fn public_routes(state: AppState) -> Router {
    Router::new()
        // 容器 HEALTHCHECK 与 CI 冒烟；/docker-compose 的健康探针不该先 login。
        .route("/healthz", get(healthz))
        .route("/api/v1/health", get(healthz))
        // 首次安装：库里零账户时只开放这一条，创建完管理员立即失效。
        .route("/api/v1/auth/bootstrap", post(auth::bootstrap))
        // 登录：没人能带着 token 来登录。
        .route("/api/v1/auth/login", post(auth::login))
        .with_state(state)
}

/// 需要登录才能访问的部分。
///
/// `/api/v1/stream/*` 与 `/api/v1/hls/*` 额外接受签名 URL（见 `auth::require_auth`），
/// 因为 `<video>`、hls.js、ExoPlayer 都发不出自定义请求头。
fn guarded_routes(state: AppState) -> Router {
    Router::new()
        // ── 账户与会话 ──
        .route("/api/v1/auth/me", get(auth::me))
        .route("/api/v1/auth/logout", post(auth::logout))
        .route("/api/v1/auth/logout-all", post(auth::logout_all))
        .route("/api/v1/auth/signed-url", post(auth::signed_url))
        // ── 账户管理（admin）──
        .route(
            "/api/v1/auth/users",
            get(auth::list_accounts).post(auth::create_account),
        )
        .route(
            "/api/v1/auth/users/{id}",
            axum::routing::patch(auth::update_account).delete(auth::delete_account),
        )
        // ── 设备与家长审批 ──
        .route("/api/v1/auth/devices", get(auth::list_devices))
        .route("/api/v1/auth/devices/{id}", delete(auth::revoke_device))
        .route("/api/v1/auth/approvals", get(auth::list_approvals))
        .route("/api/v1/auth/approvals/{id}", post(auth::decide))
        // 媒体库
        .route(
            "/api/v1/libraries",
            get(list_libraries).post(create_library),
        )
        // 媒体资产
        .route("/api/v1/media", get(list_media))
        // 分类 tab 的数据源：只返回"真的有内容"的类型。
        .route("/api/v1/media/kinds", get(media_kinds))
        .route("/api/v1/movies", get(list_movies))
        .route("/api/v1/series", get(list_series))
        .route("/api/v1/episodes", get(list_episodes))
        .route("/api/v1/items/{id}", get(get_item))
        // 详情页的演职员与季列表（飞牛风详情页）。两个都是"附属数据"：
        // 主详情不带它们，是为了让海报墙那类列表页不至于为每一行都多查两张表。
        .route("/api/v1/items/{id}/credits", get(item_credits))
        .route("/api/v1/items/{id}/series", get(item_series))
        .route("/api/v1/series/{id}/seasons", get(series_seasons))
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
        // AI 助手（P7）。危险操作分两步：chat 出工单，confirm 才执行。
        .route("/api/v1/ai/chat", post(ai::chat))
        .route("/api/v1/ai/tools", get(ai::list_tools))
        .route("/api/v1/ai/confirm", post(ai::confirm))
        // 电视直播（P8）。播放列表与分片的投递复用上面那三条 /hls/{job} 路由，
        // 直播只多出"源管理 + 频道列表 + 起/停拉流"。
        //
        // 刻意不用 merge 把路由藏在 live.rs 里：契约覆盖率守卫是扫本文件的
        // 路由注册字面量的，藏到别的模块就扫不到，等于绕过了
        // "新路由必须进契约"这条门禁。
        .route(
            "/api/v1/live/sources",
            get(live::list_sources).post(live::create_source),
        )
        .route("/api/v1/live/sources/{id}", delete(live::delete_source))
        .route("/api/v1/live/sources/{id}/sync", post(live::sync_source))
        .route("/api/v1/live/channels", get(live::list_channels))
        .route("/api/v1/live/channels/{id}/play", post(live::play_channel))
        .route("/api/v1/live/channels/{id}/stop", post(live::stop_channel))
        // ── 收藏 ──────────────────────────────────────────────
        .route("/api/v1/me/favorites", get(social::list_my_favorites))
        .route("/api/v1/items/{id}/favorite", get(social::get_favorite))
        .route("/api/v1/items/{id}/favorite", put(social::set_favorite))
        // ── 通知 ──────────────────────────────────────────────
        .route("/api/v1/notifications", get(social::list_notifications))
        .route("/api/v1/notifications/read-all", post(social::read_all_notifications))
        .route("/api/v1/notifications/{id}/read", post(social::read_notification))
        .route("/api/v1/notifications/{id}", delete(social::delete_notification))
        // ── 设置 ──────────────────────────────────────────────
        .route("/api/v1/settings", get(social::get_settings))
        .route("/api/v1/settings", put(social::put_setting))
        .route("/api/v1/settings/{key}", delete(social::reset_setting))
        // ── 存储统计 ──────────────────────────────────────────
        .route("/api/v1/system/storage", get(social::storage_stats))
        // 鉴权中间件放在with_state之前 —— 它需要自己持有 State，
        // 于是这条 router 里此后新增的路由**默认被保护**。
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ))
        .with_state(state)
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .merge(public_routes(state.clone()))
        .merge(guarded_routes(state))
        .layer(TraceLayer::new_for_http())
        .fallback(fallback)
}

/// 供集成测试使用：无需绑定端口即可发起请求。
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;
    use whatmedia_core::Page;

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

    #[allow(dead_code)]
    async fn get_json_authed(uri: &str, token: &str) -> (StatusCode, serde_json::Value) {
        let res = test_router()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
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
    async fn handlers_report_db_unavailable_when_state_has_no_db() {
        // 没接数据库的处理函数必须是 503，不能是 200 空列表 ——
        // 前端据此区分"功能没做"与"后端没准备好"。
        //
        // 这里**直接调处理函数**而不是打 HTTP：走 HTTP 会先被鉴权中间件
        // 拦成 401，而"没接数据库"这种状态在生产里根本进不到那一步 ——
        // 它是防御性分支，测它就得绕过中间件。
        let state = AppState::new(AppConfig::default());
        let responses = vec![
            list_libraries(State(state.clone())).await,
            list_media(State(state.clone()), Query(ItemQuery::default())).await,
            list_movies(State(state.clone()), Query(ItemQuery::default())).await,
        ];
        for res in responses {
            assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    #[tokio::test]
    async fn unauthenticated_calls_get_401_and_learn_nothing() {
        // 这条守卫的是顺序：没凭据就是 401，**绝不能**因为数据库没接就
        // 先返回 503 —— 那等于告诉陌生人"这里有个活着的 WhatMedia"。
        for uri in [
            "/api/v1/libraries",
            "/api/v1/media",
            "/api/v1/items/00000000-0000-0000-0000-000000000000",
            "/api/v1/ai/chat",
            "/api/v1/live/channels",
        ] {
            let (status, _) = get_json(uri).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{uri}");
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
