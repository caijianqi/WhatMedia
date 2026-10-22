//! 多源影视聚合搜索与流媒体代理端点。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_stream::Stream;
use whatmedia_aggregator::{
    cluster_items, AggregatedVodItem, AggregatorEngine, ClusterInput, MacCmsProvider,
    VodSourceConfig,
};
use whatmedia_database::repo::{self, ItemQuery, ItemSummary};
use whatmedia_database::Db;

use crate::auth::Current;
use crate::{bad_request, error_response, internal, AppState};

/// 聚合搜索查询参数
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AggregateSearchParams {
    /// 搜索关键词
    pub q: String,
    /// 是否以 SSE 流式输出 (默认 false 为 JSON)
    #[serde(default)]
    pub sse: bool,
    #[serde(flatten)]
    pub page: ItemQuery,
}

/// 聚合搜索响应结构 (JSON 模式)
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AggregateSearchResponse {
    /// 本地媒体库命中条目
    pub local: whatmedia_core::Page<ItemSummary>,
    /// 云端多源聚合影视条目
    pub remote: Vec<AggregatedVodItem>,
}

/// 代理拉流查询参数
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VodProxyQuery {
    pub url: String,
    pub source: Option<String>,
}

/// 重新加载或初始化 AggregatorEngine
pub async fn reload_aggregator_engine(
    db: &Db,
    engine_lock: &Arc<RwLock<AggregatorEngine>>,
) -> anyhow::Result<()> {
    let active_sources = repo::list_active_vod_sources(db).await?;
    let mut new_engine = AggregatorEngine::new();

    for src in active_sources {
        let provider = Arc::new(MacCmsProvider::new(src));
        new_engine.register(provider);
    }

    let mut lock = engine_lock.write().await;
    *lock = new_engine;
    Ok(())
}

/// GET /api/v1/search/aggregate
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/search/aggregate",
        params(
            ("q" = String, Query, description = "搜索词"),
            ("sse" = Option<bool>, Query, description = "是否走 SSE 流式输出"),
        ),
        responses(
            (status = 200, description = "搜索结果", body = AggregateSearchResponse),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "media"
    )
)]
pub async fn search_aggregate(
    State(state): State<AppState>,
    Current(principal): Current,
    Query(params): Query<AggregateSearchParams>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let kw = params.q.trim().to_string();
    if kw.is_empty() {
        return Json(AggregateSearchResponse {
            local: whatmedia_core::Page::new(Vec::new(), 0, 1, 50),
            remote: Vec::new(),
        })
        .into_response();
    }

    let is_child = principal.is_child();

    if params.sse {
        // SSE 流式响应
        let stream = build_aggregate_sse_stream(state, db, kw, params.page, is_child);
        return Sse::new(stream)
            .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
            .into_response();
    }

    // 儿童模式下拦截并屏蔽云端未清洗聚合源
    if is_child {
        let local_res = repo::search_items(&db, &kw, &params.page).await;
        let local_page = match local_res {
            Ok(p) => p,
            Err(e) => return internal(e),
        };
        return Json(AggregateSearchResponse {
            local: local_page,
            remote: Vec::new(),
        })
        .into_response();
    }

    // JSON 统一响应
    let local_fut = repo::search_items(&db, &kw, &params.page);
    let remote_fut = async {
        let engine = state.aggregator.read().await;
        engine.search(&kw).await
    };

    let (local_res, remote_res) = tokio::join!(local_fut, remote_fut);

    let local_page = match local_res {
        Ok(p) => p,
        Err(e) => return internal(e),
    };

    Json(AggregateSearchResponse {
        local: local_page,
        remote: remote_res,
    })
    .into_response()
}

fn build_aggregate_sse_stream(
    state: AppState,
    db: Db,
    kw: String,
    page: ItemQuery,
    is_child: bool,
) -> impl Stream<Item = Result<Event, Infallible>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(32);

    tokio::spawn(async move {
        // 1. 本地媒体库检索 (极速响应)
        match repo::search_items(&db, &kw, &page).await {
            Ok(local_res) => {
                if let Ok(data) = serde_json::to_string(&local_res) {
                    let _ = tx
                        .send(Ok(Event::default().event("local").data(data)))
                        .await;
                }
            }
            Err(e) => {
                tracing::warn!("本地搜索失败: {e}");
            }
        }

        // 儿童模式安全门禁：不抓取任何远端三方未分级内容
        if is_child {
            let _ = tx
                .send(Ok(Event::default().event("done").data("{\"done\":true}")))
                .await;
            return;
        }

        // 2. 远端多源并发抓取
        let (source_tx, mut source_rx) = tokio::sync::mpsc::channel(16);
        let engine_clone = Arc::clone(&state.aggregator);

        tokio::spawn(async move {
            let engine = engine_clone.read().await;
            engine.search_stream(&kw, source_tx).await;
        });

        let mut all_cluster_inputs = Vec::new();

        while let Some(res) = source_rx.recv().await {
            // 发送单源进度通知
            let progress_data = serde_json::json!({
                "source_id": res.source_id,
                "source_name": res.source_name,
                "success": res.success,
                "count": res.items.len(),
            });
            let _ = tx
                .send(Ok(Event::default()
                    .event("remote_source")
                    .data(progress_data.to_string())))
                .await;

            for item in res.items {
                all_cluster_inputs.push(ClusterInput {
                    source_id: res.source_id.clone(),
                    source_name: res.source_name.clone(),
                    item,
                });
            }
        }

        // 3. 聚类去重后推送最终聚合影视列表
        let clustered = cluster_items(all_cluster_inputs);
        if let Ok(data) = serde_json::to_string(&clustered) {
            let _ = tx
                .send(Ok(Event::default().event("remote_cluster").data(data)))
                .await;
        }

        // 4. 发送完成事件
        let _ = tx
            .send(Ok(Event::default().event("done").data("{\"done\":true}")))
            .await;
    });

    tokio_stream::wrappers::ReceiverStream::new(rx)
}

/// GET /api/v1/proxy/vod_stream
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/proxy/vod_stream",
        params(
            ("url" = String, Query, description = "目标 M3U8 或切片 URL"),
            ("source" = Option<String>, Query, description = "源标识"),
        ),
        responses(
            (status = 200, description = "重写或代理后的流数据"),
            (status = 400, description = "非法 URL", body = crate::ErrorBody),
            (status = 502, description = "上游请求失败", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn proxy_vod_stream(
    Current(principal): Current,
    Query(query): Query<VodProxyQuery>,
) -> Response {
    if principal.is_child() {
        return error_response(
            StatusCode::FORBIDDEN,
            "CHILD_RESTRICTED",
            "儿童模式禁止拉取未分级三方在线流",
        );
    }

    let raw_url = query.url.trim();
    let Ok(target_url) = reqwest::Url::parse(raw_url) else {
        return bad_request("非法代理目标 URL");
    };

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default();

    let resp = match client
        .get(target_url.as_str())
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "UPSTREAM_FAILED",
                format!("请求上游资源失败: {e}"),
            );
        }
    };

    let status = resp.status();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let is_m3u8 = raw_url.contains(".m3u8")
        || content_type.contains("mpegurl")
        || content_type.contains("application/x-mpegurl");

    if is_m3u8 {
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "UPSTREAM_READ_ERROR",
                    format!("读取上游播放列表失败: {e}"),
                );
            }
        };

        let rewritten = rewrite_vod_m3u8(&text, &target_url);
        return (
            status,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/vnd.apple.mpegurl",
                ),
                (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                (
                    axum::http::header::CACHE_CONTROL,
                    "no-cache, no-store, must-revalidate",
                ),
            ],
            rewritten,
        )
            .into_response();
    }

    let out_ct = if content_type.is_empty() {
        "video/MP2T"
    } else {
        &content_type
    };

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "UPSTREAM_READ_ERROR",
                format!("读取上游分片数据失败: {e}"),
            );
        }
    };

    (
        status,
        [
            (axum::http::header::CONTENT_TYPE, out_ct),
            (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (axum::http::header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response()
}

fn rewrite_vod_m3u8(text: &str, base_url: &reqwest::Url) -> String {
    let mut out = String::with_capacity(text.len() + 1024);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        if trimmed.starts_with('#') {
            if trimmed.starts_with("#EXT-X-MAP:") && trimmed.contains("URI=\"") {
                if let Some(start) = trimmed.find("URI=\"") {
                    let after = &trimmed[start + 5..];
                    if let Some(end) = after.find('"') {
                        let orig_uri = &after[..end];
                        if let Ok(abs_url) = base_url.join(orig_uri) {
                            let proxy_uri = encode_vod_proxy_url(abs_url.as_str());
                            let new_line = format!(
                                "{}URI=\"{}\"{}",
                                &trimmed[..start],
                                proxy_uri,
                                &after[end + 1..]
                            );
                            out.push_str(&new_line);
                            out.push('\n');
                            continue;
                        }
                    }
                }
            }
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let abs_url = match base_url.join(trimmed) {
            Ok(u) => u,
            Err(_) => {
                out.push_str(line);
                out.push('\n');
                continue;
            }
        };

        out.push_str(&encode_vod_proxy_url(abs_url.as_str()));
        out.push('\n');
    }
    out
}

fn encode_vod_proxy_url(url: &str) -> String {
    format!("/api/v1/proxy/vod_stream?url={}", urlencoding(url))
}

fn urlencoding(input: &str) -> String {
    let mut result = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(b as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", b));
            }
        }
    }
    result
}

// ─────────────────────────────────────────────────────────────
// 采集源配置管理 (Settings)
// ─────────────────────────────────────────────────────────────

/// GET /api/v1/settings/vod_sources
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/settings/vod_sources",
        responses(
            (status = 200, description = "采集源列表", body = Vec<whatmedia_aggregator::VodSourceConfig>),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn list_vod_sources(
    State(state): State<AppState>,
    Current(principal): Current,
) -> Response {
    if !principal.can(whatmedia_auth::Ability::ManageSettings) {
        return error_response(StatusCode::FORBIDDEN, "FORBIDDEN", "无权管理采集源配置");
    }

    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    match repo::list_vod_sources(&db).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => internal(e),
    }
}

/// POST /api/v1/settings/vod_sources (支持单个、批量或 MoonTV config.json 格式)
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/settings/vod_sources",
        request_body = serde_json::Value,
        responses(
            (status = 200, description = "导入结果"),
            (status = 400, description = "配置不合法", body = crate::ErrorBody),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn import_vod_sources(
    State(state): State<AppState>,
    Current(principal): Current,
    Json(payload): Json<serde_json::Value>,
) -> Response {
    if !principal.can(whatmedia_auth::Ability::ManageSettings) {
        return error_response(StatusCode::FORBIDDEN, "FORBIDDEN", "无权管理采集源配置");
    }

    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let sources = parse_sources_payload(payload);
    if sources.is_empty() {
        return bad_request("未解析到有效的采集源配置");
    }

    let count = sources.len();
    for src in sources {
        if let Err(e) = repo::upsert_vod_source(&db, &src).await {
            return internal(format!("写入采集源失败: {e}"));
        }
    }

    // 重新加载 AggregatorEngine
    if let Err(e) = reload_aggregator_engine(&db, &state.aggregator).await {
        tracing::warn!("重载 AggregatorEngine 失败: {e}");
    }

    (StatusCode::OK, Json(serde_json::json!({ "imported": count }))).into_response()
}

/// DELETE /api/v1/settings/vod_sources/{id}
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        delete,
        path = "/api/v1/settings/vod_sources/{id}",
        params(
            ("id" = String, Path, description = "采集源标识"),
        ),
        responses(
            (status = 204, description = "删除成功"),
            (status = 404, description = "采集源不存在", body = crate::ErrorBody),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn delete_vod_source(
    State(state): State<AppState>,
    Current(principal): Current,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !principal.can(whatmedia_auth::Ability::ManageSettings) {
        return error_response(StatusCode::FORBIDDEN, "FORBIDDEN", "无权管理采集源配置");
    }

    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    match repo::delete_vod_source(&db, &id).await {
        Ok(true) => {
            let _ = reload_aggregator_engine(&db, &state.aggregator).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "采集源不存在"),
        Err(e) => internal(e),
    }
}

/// PUT /api/v1/settings/vod_sources/{id}/toggle
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        put,
        path = "/api/v1/settings/vod_sources/{id}/toggle",
        params(
            ("id" = String, Path, description = "采集源标识"),
        ),
        responses(
            (status = 200, description = "更新后状态", body = whatmedia_aggregator::VodSourceConfig),
            (status = 404, description = "采集源不存在", body = crate::ErrorBody),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn toggle_vod_source(
    State(state): State<AppState>,
    Current(principal): Current,
    AxumPath(id): AxumPath<String>,
) -> Response {
    if !principal.can(whatmedia_auth::Ability::ManageSettings) {
        return error_response(StatusCode::FORBIDDEN, "FORBIDDEN", "无权管理采集源配置");
    }
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let src = match repo::get_vod_source(&db, &id).await {
        Ok(Some(s)) => s,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "采集源不存在"),
        Err(e) => return internal(e),
    };

    let mut updated = src;
    updated.is_active = !updated.is_active;

    if let Err(e) = repo::upsert_vod_source(&db, &updated).await {
        return internal(e);
    }

    let _ = reload_aggregator_engine(&db, &state.aggregator).await;
    Json(updated).into_response()
}

/// 解析用户提交的各种 JSON 格式（单条、多条数组、或 MoonTV config.json 格式）
fn parse_sources_payload(payload: serde_json::Value) -> Vec<VodSourceConfig> {
    let mut results = Vec::new();

    // 格式 1: 单个对象
    if let Ok(single) = serde_json::from_value::<VodSourceConfig>(payload.clone()) {
        return vec![single];
    }

    // 格式 2: 数组
    if let Ok(list) = serde_json::from_value::<Vec<VodSourceConfig>>(payload.clone()) {
        return list;
    }

    // 格式 3: MoonTV config.json 格式：{ "api_site": { "api_1": { "name": "...", "api": "...", "active": true } } }
    if let Some(api_site) = payload.get("api_site").and_then(|v| v.as_object()) {
        for (key, val) in api_site {
            let name = val.get("name").and_then(|v| v.as_str()).unwrap_or(key);
            let api = val.get("api").and_then(|v| v.as_str()).unwrap_or("");
            let active = val.get("active").and_then(|v| v.as_bool()).unwrap_or(true);

            if !api.is_empty() {
                results.push(VodSourceConfig {
                    id: key.clone(),
                    name: name.to_string(),
                    api_url: api.to_string(),
                    is_active: active,
                    weight: 100,
                    timeout_ms: 2500,
                });
            }
        }
    }

    results
}
