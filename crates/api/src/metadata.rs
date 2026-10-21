//! 元数据相关端点。
//!
//! 三条设计约束：
//!   · 刮削是异步的，POST 立即返回 202，进度靠 `/scrape/stats` 轮询
//!   · 人工确认入口必须存在 —— 自动刮削永远有认不准的时候
//!   · 图片取不到要返回占位图，不能 404（海报墙会变成一片破图）

use axum::{
    extract::{Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use whatmedia_core::Id;
use whatmedia_metadata::{
    placeholder_svg, HttpTransport, ImageCache, Outcome, ScrapeConfig, ScrapeStats, Scraper,
};

use crate::{bad_request, error_response, internal, AppState};

// ─────────────────────────────────────────────────────────────
// 刮削运行状态
// ─────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct ScrapeRegistry {
    inner: Arc<Mutex<std::collections::HashMap<Uuid, ScrapeRunView>>>,
}

#[derive(Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScrapeRunView {
    pub id: Uuid,
    pub library_id: Option<String>,
    pub running: bool,
    pub stats: Option<ScrapeStats>,
}

impl ScrapeRegistry {
    fn insert(&self, v: ScrapeRunView) {
        self.inner.lock().unwrap().insert(v.id, v);
    }

    fn finish(&self, id: Uuid, stats: ScrapeStats) {
        if let Some(v) = self.inner.lock().unwrap().get_mut(&id) {
            v.running = false;
            v.stats = Some(stats);
        }
    }

    pub fn list(&self) -> Vec<ScrapeRunView> {
        self.inner.lock().unwrap().values().cloned().collect()
    }
}

// ─────────────────────────────────────────────────────────────
// 请求 / 响应
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScrapeRequest {
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub library_id: Option<Id>,
    /// 只重跑失败项。
    #[serde(default)]
    pub retry_failed: bool,
    /// 强制全量重新刮削所有条目（无论是否已完成），用于凭据变更或重新刷新图片与演职员。
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScrapeAccepted {
    pub run_id: Uuid,
    pub queued: u64,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct IdentifyRequest {
    pub provider: String,
    pub provider_id: String,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LockRequest {
    /// 要锁定或解锁的字段名。
    pub fields: Vec<String>,
    #[serde(default = "default_true")]
    pub locked: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LockResult {
    pub locked_fields: Vec<String>,
    /// 被拒绝的字段名（不在白名单里）。UI 应当提示用户。
    pub rejected: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CandidatesQuery {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    pub title: Option<String>,
    pub year: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StatsQuery {
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub library_id: Option<Id>,
}

// ─────────────────────────────────────────────────────────────
// 处理器
// ─────────────────────────────────────────────────────────────

/// 启动一轮刮削。
///
/// 1,000 部片要跑几十分钟，绝不能让 HTTP 请求挂在那儿等。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/scrape",
        request_body = ScrapeRequest,
        responses(
            (status = 202, description = "已受理", body = ScrapeAccepted),
            (status = 501, description = "未配置元数据 Provider", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn start_scrape(
    State(state): State<AppState>,
    Json(body): Json<ScrapeRequest>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let scraper = match &state.scraper {
        Some(s) => s.clone(),
        None => {
            return error_response(
                StatusCode::NOT_IMPLEMENTED,
                "NO_METADATA_PROVIDER",
                "未配置任何元数据 Provider（需要 TMDB API Key 或启用 Bangumi）",
            )
        }
    };

    let library_id = body.library_id;
    let queued = if body.force {
        match scraper.requeue_all(library_id).await {
            Ok(n) => n,
            Err(e) => return internal(e),
        }
    } else {
        match library_id {
            Some(l) if body.retry_failed => match scraper.requeue_failed(l).await {
                Ok(n) => n,
                Err(e) => return internal(e),
            },
            Some(l) => match scraper.enqueue(l).await {
                Ok(n) => n,
                Err(e) => return internal(e),
            },
            None => {
                let mut total = 0u64;
                if let Ok(libs) = whatmedia_database::repo::list_libraries(&db).await {
                    for lib in libs {
                        if let Ok(n) = if body.retry_failed {
                            scraper.requeue_failed(lib.id).await
                        } else {
                            scraper.enqueue(lib.id).await
                        } {
                            total += n;
                        }
                    }
                }
                total
            }
        }
    };

    // 先把崩溃遗留的 running 捞回来，再开跑
    if let Err(e) = scraper.reap_running().await {
        tracing::warn!("回收 running 任务失败：{e:#}");
    }

    let run_id = Uuid::new_v4();
    state.scrapes.insert(ScrapeRunView {
        id: run_id,
        library_id: library_id.map(|i| i.to_string()),
        running: true,
        stats: None,
    });

    let registry = state.scrapes.clone();
    let notify_db = state.db.clone();
    tokio::spawn(async move {
        let result = scraper.run(library_id, Default::default()).await;
        match result {
            Ok(stats) => {
                // 刮削跑几分钟是常态，跑完时人早不在页面上了 —— 结果要靠通知送出去。
                // 有待确认项时升级成 warn：那意味着这批匹配里可能有错的需要人看一眼。
                if let Some(db) = &notify_db {
                    let n = whatmedia_database::repo::NewNotification {
                        kind: "scrape_finished",
                        level: if stats.failed > 0 {
                            whatmedia_database::repo::NotifyLevel::Error
                        } else if stats.review > 0 {
                            whatmedia_database::repo::NotifyLevel::Warn
                        } else {
                            whatmedia_database::repo::NotifyLevel::Info
                        },
                        title: "刮削完成".to_string(),
                        body: Some(format!(
                            "匹配 {} 个，待确认 {} 个，失败 {} 个，抓图 {} 张。",
                            stats.matched, stats.review, stats.failed, stats.images
                        )),
                        link: None,
                    };
                    if let Err(e) = whatmedia_database::repo::notify_admins(db, &n).await {
                        tracing::warn!("写刮削完成通知失败：{e:#}");
                    }
                }
                registry.finish(run_id, stats);
            }
            Err(e) => {
                tracing::error!("刮削失败：{e:#}");
                registry.finish(run_id, ScrapeStats::default());
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(ScrapeAccepted { run_id, queued }),
    )
        .into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/scrape/runs",
        responses(
            (status = 200, description = "成功", body = Vec<ScrapeRunView>),
        ),
        tag = "metadata"
    )
)]
pub async fn scrape_runs(State(state): State<AppState>) -> Response {
    Json(state.scrapes.list()).into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/scrape/stats",
        params(
        ("library_id" = Option<String>, Query, description = "不传则统计全部"),
        ),
        responses(
            (status = 200, description = "成功", body = whatmedia_database::repo::JobStats),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn scrape_stats(State(state): State<AppState>, Query(q): Query<StatsQuery>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::metadata::job_stats(&db, q.library_id).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => internal(e),
    }
}

/// 待人工确认列表。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/metadata/review",
        params(
        ("library_id" = Option<String>, Query),
        ),
        responses(
            (status = 200, description = "待人工确认", body = Vec<ReviewItem>),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn review_queue(State(state): State<AppState>, Query(q): Query<StatsQuery>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::metadata::list_review(&db, q.library_id, 200).await {
        Ok(list) => {
            let view: Vec<ReviewItem> = list
                .into_iter()
                .map(|t| ReviewItem {
                    item_id: t.id,
                    kind: t.kind,
                    title: t.title,
                    match_title: t.match_title,
                    year: t.year,
                })
                .collect();
            Json(view).into_response()
        }
        Err(e) => internal(e),
    }
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReviewItem {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    pub kind: String,
    pub title: String,
    /// 实际用于搜索的标题。UI 要显示它 —— 用户常常不知道"改了名就搜不到了"。
    pub match_title: String,
    pub year: Option<i32>,
}

/// 候选列表。人工确认界面靠它。
/// 只有一个状态字段的确认信封。与 `AckView` 的区别是语义：
/// 这个表示"事情做完了"，那个表示"收到了"。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StatusView {
    pub status: String,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/metadata/candidates",
        params(
        ("item_id" = String, Query),
        ("title" = Option<String>, Query, description = "覆盖标题"),
        ("year" = Option<i32>, Query),
        ),
        responses(
            (status = 200, description = "候选列表", body = Vec<whatmedia_metadata::CandidateView>),
            (status = 404, description = "条目不存在", body = crate::ErrorBody),
            (status = 501, description = "未配置元数据 Provider", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn candidates(
    State(state): State<AppState>,
    Query(q): Query<CandidatesQuery>,
) -> Response {
    let scraper = match &state.scraper {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::NOT_IMPLEMENTED,
                "NO_METADATA_PROVIDER",
                "未配置任何元数据 Provider",
            )
        }
    };
    match scraper.candidates(q.item_id, q.title.clone(), q.year).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => match e.to_string() {
            m if m.contains("条目不存在") => {
                error_response(StatusCode::NOT_FOUND, "NOT_FOUND", m)
            }
            m => internal(m),
        },
    }
}

/// 人工确认：直接采用指定 Provider 的指定 id。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/items/{id}/identify",
        params(
        ("id" = String, Path, description = "条目 id"),
        ),
        request_body = IdentifyRequest,
        responses(
            (status = 200, description = "成功", body = StatusView),
            (status = 404, description = "条目不存在", body = crate::ErrorBody),
            (status = 501, description = "未配置元数据 Provider", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn identify(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Id>,
    Json(body): Json<IdentifyRequest>,
) -> Response {
    let scraper = match &state.scraper {
        Some(s) => s,
        None => {
            return error_response(
                StatusCode::NOT_IMPLEMENTED,
                "NO_METADATA_PROVIDER",
                "未配置任何元数据 Provider",
            )
        }
    };
    match scraper
        .identify(id, &body.provider, &body.provider_id)
        .await
    {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response(),
        Err(e) => match e.to_string() {
            m if m.contains("条目不存在") => {
                error_response(StatusCode::NOT_FOUND, "NOT_FOUND", m)
            }
            m if m.contains("未配置") => bad_request(m),
            m => internal(m),
        },
    }
}

/// 锁定 / 解锁字段。
///
/// 拒绝白名单外的字段，而不是静默忽略 —— 否则用户以为锁上了，其实没有。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/items/{id}/locks",
        params(
        ("id" = String, Path, description = "条目 id"),
        ),
        request_body = LockRequest,
        responses(
            (status = 200, description = "成功", body = LockResult),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn set_locks(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Id>,
    Json(body): Json<LockRequest>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let current = match whatmedia_database::repo::metadata::locked_fields_of_item(&db, id).await {
        Ok(s) => s,
        Err(e) => return internal(e),
    };
    let mut locked = whatmedia_metadata::parse_locked(&current);
    let mut rejected = Vec::new();

    for f in &body.fields {
        if !whatmedia_metadata::set_locked(&mut locked, f, body.locked) {
            rejected.push(f.clone());
        }
    }

    let json = whatmedia_metadata::serialize_locked(&locked);
    if let Err(e) = whatmedia_database::repo::metadata::update_item_meta(
        &db,
        id,
        &whatmedia_database::repo::metadata::ItemMetaUpdate {
            locked_fields: Some(json),
            ..Default::default()
        },
    )
    .await
    {
        return internal(e);
    }

    let mut out: Vec<String> = locked.into_iter().collect();
    out.sort();
    Json(LockResult {
        locked_fields: out,
        rejected,
    })
    .into_response()
}

/// 某条目的图片清单。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/items/{id}/images",
        params(
        ("id" = String, Path, description = "条目 id"),
        ),
        responses(
            (status = 200, description = "成功", body = Vec<whatmedia_database::repo::ItemImage>),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn item_images(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::image::list_for_item(&db, id).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => internal(e),
    }
}

/// 取一张图。缓存里有就直出，没有就给占位图 —— 绝不 404。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/items/{id}/images/{kind}",
        params(
        ("id" = String, Path, description = "条目 id"),
        ("kind" = String, Path, description = "poster | backdrop | ..."),
        ),
        responses(
            (status = 200, description = "图片二进制；缓存缺失时给占位图，绝不 404", content_type = "image/*", body = Vec<u8>),
        ),
        tag = "metadata"
    )
)]
pub async fn item_image(
    State(state): State<AppState>,
    AxumPath((id, kind)): AxumPath<(Id, String)>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let images = match whatmedia_database::repo::image::list_for_item(&db, id).await {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    let wanted = images.iter().find(|i| i.kind == kind);

    // ① 已缓存：直出文件
    if let Some(p) = wanted.and_then(|i| i.cached_path.as_ref()) {
        if let Ok(bytes) = tokio::fs::read(p).await {
            let ct = content_type_of(p);
            return (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, ct),
                    (header::CACHE_CONTROL, "public, max-age=86400"),
                ],
                bytes,
            )
                .into_response();
        }
    }

    // ② 有 URL 且有缓存：按需下载一次再直出
    if let (Some(img), Some(cache)) = (wanted, &state.images) {
        if let Some(url) = &img.url {
            if let Some(p) = download_into(cache, url, &kind).await {
                let path_str = p.to_string_lossy().to_string();
                let db_clone = db.clone();
                let url_clone = url.clone();
                let p_clone = path_str.clone();
                tokio::spawn(async move {
                    let _ = sqlx::query("UPDATE image SET cached_path = ? WHERE url = ?")
                        .bind(&p_clone)
                        .bind(&url_clone)
                        .execute(db_clone.pool())
                        .await;
                });

                if let Ok(bytes) = tokio::fs::read(&p).await {
                    return (
                        StatusCode::OK,
                        [
                            (header::CONTENT_TYPE, content_type_of(&path_str)),
                            (header::CACHE_CONTROL, "public, max-age=86400"),
                        ],
                        bytes,
                    )
                        .into_response();
                }
            }
        }
    }

    // ③ 兜底：占位图。海报墙不能出现破图。
    let title = whatmedia_database::repo::get_item(&db, id)
        .await
        .ok()
        .flatten()
        .map(|d| d.item.title)
        .unwrap_or_default();
    let clean_title = title.replace('\u{FFFD}', "");
    let clean_title = if clean_title.trim().is_empty() {
        "WhatMedia".to_string()
    } else {
        clean_title
    };

    let svg = if kind == "backdrop" {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="720" viewBox="0 0 1280 720">
  <defs>
    <radialGradient id="bg" cx="75%" cy="25%" r="75%">
      <stop offset="0%" stop-color="#1e293b" stop-opacity="0.6"/>
      <stop offset="50%" stop-color="#0f172a" stop-opacity="0.8"/>
      <stop offset="100%" stop-color="#080d1a" stop-opacity="1"/>
    </radialGradient>
  </defs>
  <rect width="1280" height="720" fill="url(#bg)"/>
</svg>"##
        )
    } else {
        placeholder_svg(&clean_title, 300, 450)
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml")],
        svg,
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImageProxyQuery {
    pub url: String,
}

/// 远程图片直连代理端点，避免浏览器直接请求外网 CDN（如 TMDB image.tmdb.org）被拦截或超时。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/metadata/image_proxy",
        params(
            ("url" = String, Query, description = "远程图片绝对 URL"),
        ),
        responses(
            (status = 200, description = "图片流", content_type = "image/*", body = Vec<u8>),
        ),
        tag = "metadata"
    )
)]
pub async fn image_proxy(
    State(state): State<AppState>,
    Query(q): Query<ImageProxyQuery>,
) -> Response {
    let url = q.url.trim();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return (StatusCode::BAD_REQUEST, "Invalid URL").into_response();
    }

    if let Some(cache) = &state.images {
        if let Some(p) = download_into(cache, url, "poster").await {
            let path_str = p.to_string_lossy().to_string();
            if let Ok(bytes) = tokio::fs::read(&p).await {
                return (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, content_type_of(&path_str)),
                        (header::CACHE_CONTROL, "public, max-age=86400"),
                    ],
                    bytes,
                )
                    .into_response();
            }
        }
    }

    let svg = placeholder_svg("WhatMedia", 300, 450);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        svg,
    )
        .into_response()
}

async fn download_into(cache: &ImageCache, url: &str, kind: &str) -> Option<std::path::PathBuf> {
    let transport = whatmedia_metadata::ReqwestTransport::new();
    let key = ImageCache::key_for(url, kind);
    if let Some(p) = cache.get(&key).await {
        return Some(p);
    }
    let res = transport.get(url, None).await.ok()?;
    if res.body.is_empty() {
        return None;
    }
    let ext = extension_of(url);
    cache.put(kind, url, kind, ext, &res.body).await.ok()
}

fn extension_of(url: &str) -> &'static str {
    let lower = url.to_lowercase();
    let p = lower.split('?').next().unwrap_or(&lower);
    match p.rsplit('.').next().unwrap_or("") {
        "png" => "png",
        "webp" => "webp",
        "gif" => "gif",
        _ => "jpg",
    }
}

fn content_type_of(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        _ => "image/jpeg",
    }
}

/// 缓存统计。用于验证 DoD 的"命中率 ≥ 90%"。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/metadata/cache/stats",
        responses(
            (status = 200, description = "成功", body = whatmedia_database::repo::image::CacheStats),
            (status = 501, description = "图片缓存未启用", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn cache_stats(State(state): State<AppState>) -> Response {
    match &state.images {
        Some(c) => match c.stats().await {
            Ok(s) => Json(s).into_response(),
            Err(e) => internal(e),
        },
        None => error_response(
            StatusCode::NOT_IMPLEMENTED,
            "NO_IMAGE_CACHE",
            "图片缓存未启用",
        ),
    }
}

/// 手动触发一次缓存清理。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/metadata/cache/evict",
        responses(
            (status = 200, description = "清理结果", body = whatmedia_metadata::EvictReport),
            (status = 501, description = "图片缓存未启用", body = crate::ErrorBody),
        ),
        tag = "metadata"
    )
)]
pub async fn cache_evict(State(state): State<AppState>) -> Response {
    match &state.images {
        Some(c) => match c.evict().await {
            Ok(r) => Json(r).into_response(),
            Err(e) => internal(e),
        },
        None => error_response(
            StatusCode::NOT_IMPLEMENTED,
            "NO_IMAGE_CACHE",
            "图片缓存未启用",
        ),
    }
}

/// 判定结果字符串转枚举，便于日志与统计。
pub fn outcome_label(o: Outcome) -> &'static str {
    match o {
        Outcome::Matched => "matched",
        Outcome::Review => "review",
        Outcome::Skipped => "skipped",
        Outcome::Failed => "failed",
    }
}

/// 从配置构造 Scraper。Provider 为空时返回 None —— 此时元数据端点返回 501。
pub fn build_scraper(
    db: whatmedia_database::Db,
    cfg: &whatmedia_config::AppConfig,
) -> Option<Arc<Scraper>> {
    let chain = whatmedia_metadata::ProviderChainConfig {
        tmdb_api_key: cfg.metadata.tmdb_api_key.clone(),
        tmdb_bearer: cfg.metadata.tmdb_bearer.clone(),
        tmdb_base_url: cfg.metadata.tmdb_base_url.clone(),
        bangumi_enabled: cfg.metadata.bangumi_enabled,
        language: cfg.metadata.language.clone(),
        requests_per_second: cfg.metadata.requests_per_second,
    };
    let providers = whatmedia_metadata::build_providers(&chain);
    if providers.is_empty() {
        return None;
    }
    Some(Arc::new(Scraper::new(
        db,
        providers,
        ScrapeConfig {
            concurrency: cfg.metadata.concurrency,
            auto_accept: cfg.metadata.auto_accept,
            fetch_images: cfg.metadata.fetch_images,
            ..Default::default()
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_mapping() {
        assert_eq!(content_type_of("/a/b.jpg"), "image/jpeg");
        assert_eq!(content_type_of("/a/b.PNG"), "image/png");
        assert_eq!(content_type_of("/a/b.webp"), "image/webp");
        assert_eq!(content_type_of("/a/b"), "image/jpeg");
    }

    #[test]
    fn outcome_labels_are_stable() {
        assert_eq!(outcome_label(Outcome::Matched), "matched");
        assert_eq!(outcome_label(Outcome::Review), "review");
    }
}
