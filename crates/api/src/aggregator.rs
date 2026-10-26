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
    cluster_items, AggregatedVodItem, AggregatorEngine, ClusterInput, MacCmsProvider, VodRoute,
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
    client_headers: axum::http::HeaderMap,
) -> Response {
    if principal.is_child() {
        return error_response(
            StatusCode::FORBIDDEN,
            "CHILD_RESTRICTED",
            "儿童模式禁止拉取未分级三方在线流",
        );
    }

    let mut raw_url = query.url.trim().to_string();
    if !raw_url.starts_with("http://") && !raw_url.starts_with("https://") {
        let decoded = urldecode(&raw_url);
        if decoded.starts_with("http://") || decoded.starts_with("https://") {
            raw_url = decoded;
        }
    }

    let Ok(target_url) = reqwest::Url::parse(&raw_url) else {
        return bad_request("非法代理目标 URL");
    };

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default();

    // 伪装 Referer 与 Origin 防盗链检测
    let origin = format!(
        "{}://{}",
        target_url.scheme(),
        target_url.host_str().unwrap_or("")
    );
    let referer = format!("{}/", origin);

    let mut req = client
        .get(target_url.as_str())
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .header(reqwest::header::REFERER, referer)
        .header("Origin", origin)
        .header(reqwest::header::ACCEPT, "*/*");

    // 透传客户端的 Range 请求头，支持分片 Seek 和 byte-range 请求
    if let Some(range) = client_headers.get(axum::http::header::RANGE) {
        if let Ok(val) = range.to_str() {
            req = req.header(reqwest::header::RANGE, val);
        }
    }

    let resp = match req.send().await {
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

    let content_range = resp
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "UPSTREAM_READ_ERROR",
                format!("读取上游数据失败: {e}"),
            );
        }
    };

    // 智能探查 M3U8：URL 包含 .m3u8、或 Content-Type 匹配、或内容前缀包含 #EXTM3U
    let is_m3u8 = raw_url.contains(".m3u8")
        || content_type.contains("mpegurl")
        || content_type.contains("application/x-mpegurl")
        || bytes.starts_with(b"#EXTM3U");

    if is_m3u8 {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            let rewritten = rewrite_vod_m3u8(text, &target_url);
            return (
                status,
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        "application/vnd.apple.mpegurl",
                    ),
                    (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                    (axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, "*"),
                    (
                        axum::http::header::CACHE_CONTROL,
                        "no-cache, no-store, must-revalidate",
                    ),
                ],
                rewritten,
            )
                .into_response();
        }
    }

    let out_ct = if content_type.is_empty() {
        "video/MP2T"
    } else {
        &content_type
    };

    let mut response_headers = axum::http::HeaderMap::new();
    response_headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_str(out_ct)
            .unwrap_or(axum::http::HeaderValue::from_static("application/octet-stream")),
    );
    response_headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        axum::http::HeaderValue::from_static("*"),
    );
    response_headers.insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS,
        axum::http::HeaderValue::from_static("*"),
    );
    response_headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("public, max-age=86400"),
    );
    if let Some(cr) = content_range {
        if let Ok(v) = axum::http::HeaderValue::from_str(&cr) {
            response_headers.insert(axum::http::header::CONTENT_RANGE, v);
        }
    }

    (status, response_headers, bytes).into_response()
}

fn rewrite_tag_uri(line: &str, base_url: &reqwest::Url) -> String {
    let uri_pattern = "URI=\"";
    if let Some(start) = line.find(uri_pattern) {
        let after = &line[start + uri_pattern.len()..];
        if let Some(end) = after.find('"') {
            let orig_uri = &after[..end];
            if let Ok(abs_url) = base_url.join(orig_uri) {
                let proxy_uri = encode_vod_proxy_url(abs_url.as_str());
                return format!(
                    "{}{}{}\"{}",
                    &line[..start],
                    uri_pattern,
                    proxy_uri,
                    &after[end + 1..]
                );
            }
        }
    } else if let Some(start) = line.find("URI='") {
        let after = &line[start + 5..];
        if let Some(end) = after.find('\'') {
            let orig_uri = &after[..end];
            if let Ok(abs_url) = base_url.join(orig_uri) {
                let proxy_uri = encode_vod_proxy_url(abs_url.as_str());
                return format!(
                    "{}URI='{}'{}",
                    &line[..start],
                    proxy_uri,
                    &after[end + 1..]
                );
            }
        }
    }
    line.to_string()
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
            // 拦截并代理重写所有带有 URI="..." 的标签（如 #EXT-X-KEY, #EXT-X-MAP, #EXT-X-MEDIA 等）
            if trimmed.contains("URI=") {
                let rewritten_tag = rewrite_tag_uri(line, base_url);
                out.push_str(&rewritten_tag);
                out.push('\n');
                continue;
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

pub(crate) fn urlencoding(input: &str) -> String {
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

pub(crate) fn urldecode(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let mut chars = input.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                let s = [h1, h2];
                if let Ok(hex_str) = std::str::from_utf8(&s) {
                    if let Ok(byte) = u8::from_str_radix(hex_str, 16) {
                        bytes.push(byte);
                        continue;
                    }
                }
                bytes.push(b'%');
                bytes.push(h1);
                bytes.push(h2);
            } else {
                bytes.push(b'%');
                if let Some(h1) = h1 {
                    bytes.push(h1);
                }
            }
        } else if b == b'+' {
            bytes.push(b' ');
        } else {
            bytes.push(b);
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|_| input.to_string())
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

    (
        StatusCode::OK,
        Json(serde_json::json!({ "imported": count })),
    )
        .into_response()
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

/// 入库请求参数
#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportVodRequest {
    pub library_id: Option<uuid::Uuid>,
    pub item: AggregatedVodItem,
    /// 指定入库的线路源 ID 列表；若为空，则入库全部可用线路
    #[serde(default)]
    pub selected_source_ids: Option<Vec<String>>,
    /// 指定首选/默认线路源 ID
    #[serde(default)]
    pub default_source_id: Option<String>,
}

/// 入库响应结果
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ImportVodResponse {
    pub item_id: uuid::Uuid,
    pub library_id: uuid::Uuid,
    pub title: String,
    pub is_new: bool,
}

/// 查询是否已入库
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CheckImportedQuery {
    pub title: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CheckImportedResponse {
    pub imported: bool,
    pub item_id: Option<uuid::Uuid>,
}

pub async fn import_vod_to_library(
    State(state): State<AppState>,
    Current(principal): Current,
    Json(payload): Json<ImportVodRequest>,
) -> Response {
    if principal.is_child() {
        return error_response(
            StatusCode::FORBIDDEN,
            "CHILD_RESTRICTED",
            "儿童模式禁止添加云端条目到媒体库",
        );
    }

    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let item = &payload.item;
    let title = item.title.trim();
    if title.is_empty() {
        return bad_request("片名不能为空");
    }

    // 1. 查找或创建媒体库
    let libraries = match repo::list_libraries(&db).await {
        Ok(l) => l,
        Err(e) => return internal(e),
    };

    let library = if let Some(lid) = payload.library_id {
        libraries.into_iter().find(|l| l.id == lid)
    } else {
        libraries.into_iter().next()
    };

    let library = match library {
        Some(l) => l,
        None => match repo::insert_library(
            &db,
            "云端聚合",
            whatmedia_core::MediaKind::Mixed,
            "cloud_vod",
            "vod://cloud",
            &["tmdb".to_string()],
        )
        .await
        {
            Ok(new_lib) => new_lib,
            Err(e) => return internal(e),
        },
    };

    // 2. 检查是否已经入库
    let existing_check: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        "SELECT id FROM media_item WHERE provider = 'vod_aggregate' AND title = ? LIMIT 1",
    )
    .bind(title)
    .fetch_optional(db.pool())
    .await;

    if let Ok(Some((id_str,))) = existing_check {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id_str) {
            return Json(ImportVodResponse {
                item_id: uuid,
                library_id: library.id,
                title: title.to_string(),
                is_new: false,
            })
            .into_response();
        }
    }

    // 3. 构造 MediaItem 并写入
    let item_id = uuid::Uuid::new_v4();
    let type_name_lower = item.type_name.as_deref().unwrap_or("").to_lowercase();
    let is_anime = type_name_lower.contains("动漫")
        || type_name_lower.contains("动画")
        || type_name_lower.contains("番剧")
        || type_name_lower.contains("二次元");
    let is_variety = type_name_lower.contains("综艺");
    let is_doc = type_name_lower.contains("纪录") || type_name_lower.contains("记录");
    let is_series = item.routes.iter().any(|r| r.episodes.len() > 1)
        || type_name_lower.contains("剧")
        || type_name_lower.contains("电视");

    let kind = if is_anime {
        whatmedia_core::MediaKind::Anime
    } else if is_variety {
        whatmedia_core::MediaKind::Variety
    } else if is_doc {
        whatmedia_core::MediaKind::Documentary
    } else if is_series {
        whatmedia_core::MediaKind::Series
    } else {
        whatmedia_core::MediaKind::Movie
    };

    let media_item = whatmedia_core::MediaItem {
        id: item_id,
        library_id: library.id,
        kind,
        title: title.to_string(),
        original_title: None,
        sort_title: Some(title.to_string()),
        year: item.year.map(|y| y as i32),
        overview: item.content.clone(),
        runtime_ms: None,
        rating: None,
        locked_fields: vec![],
        provider: Some("vod_aggregate".to_string()),
        provider_id: Some(item.id.clone()),
        added_at: chrono::Utc::now(),
    };

    if let Err(e) = repo::upsert_item(&db, &media_item).await {
        return internal(e);
    }

    // 确定入库的线路列表与首选默认线路
    let target_routes: Vec<&VodRoute> = if let Some(ref selected) = payload.selected_source_ids {
        if !selected.is_empty() {
            let filtered: Vec<&VodRoute> = item
                .routes
                .iter()
                .filter(|r| selected.contains(&r.source_id))
                .collect();
            if filtered.is_empty() {
                item.routes.iter().collect()
            } else {
                filtered
            }
        } else {
            item.routes.iter().collect()
        }
    } else {
        item.routes.iter().collect()
    };

    let default_source = payload
        .default_source_id
        .as_deref()
        .filter(|s| target_routes.iter().any(|r| r.source_id == *s))
        .unwrap_or_else(|| {
            // 优先选择含有直链 m3u8 的可用线路，避免网页解析页成为默认线路
            target_routes
                .iter()
                .find(|r| r.episodes.iter().any(|ep| ep.play_url.contains(".m3u8")))
                .or_else(|| target_routes.first())
                .map(|r| r.source_id.as_str())
                .unwrap_or("")
        });

    let primary_route = target_routes
        .iter()
        .find(|r| r.source_id == default_source)
        .or_else(|| {
            target_routes
                .iter()
                .find(|r| r.episodes.iter().any(|ep| ep.play_url.contains(".m3u8")))
        })
        .or_else(|| target_routes.first())
        .copied();

    // 若为剧集/动漫/综艺，同步建立 series、season、episode 体系，使详情页能选集、搜索能搜到剧名
    if is_anime || is_series || is_variety {
        if let Ok(s) = repo::tv::upsert_series(&db, title, item.year.map(|y| y as i32)).await {
            if let Ok(season) = repo::tv::upsert_season(&db, s.id, 1).await {
                if let Some(route) = primary_route {
                    for episode in &route.episodes {
                        let _ = repo::tv::upsert_episode(
                            &db,
                            season.id,
                            episode.index as i32,
                            item_id,
                            Some(&episode.name),
                        )
                        .await;
                    }
                }
            }
        }
    }

    // 4. 写入封面绑定
    if let Some(poster_url) = &item.poster {
        let _ = repo::image::bind(
            &db,
            &repo::image::ImageBind {
                item_id,
                kind: "poster",
                url: Some(poster_url.as_str()),
                cache_key: None,
                provider: Some("vod_aggregate"),
                width: None,
                height: None,
                cached_path: None,
            },
        )
        .await;
    }

    // 5. 写入版本与虚拟文件
    for (idx, route) in target_routes.iter().enumerate() {
        let is_default = if !default_source.is_empty() {
            route.source_id == default_source
        } else {
            idx == 0
        };
        let version_id = uuid::Uuid::new_v4();
        let version = whatmedia_core::MediaVersion {
            id: version_id,
            item_id,
            name: route.source_name.clone(),
            source: Some(route.source_id.clone()),
            resolution: Some("1080P".to_string()),
            video_codec: Some(whatmedia_core::VideoCodec::H264),
            hdr_type: whatmedia_core::HdrType::Sdr,
            is_default,
        };
        if let Err(e) = repo::upsert_version(&db, &version).await {
            tracing::warn!("写入 VOD 版本失败: {e}");
            continue;
        }

        for episode in &route.episodes {
            let file_id = uuid::Uuid::new_v4();
            let file = whatmedia_core::MediaFile {
                id: file_id,
                version_id,
                storage_provider: "cloud_vod".to_string(),
                path: episode.play_url.clone(),
                container: Some(whatmedia_core::Container::Other("m3u8".to_string())),
                size_bytes: 0,
                duration_ms: None,
                bitrate: None,
                file_fingerprint: format!("{}_{}_{}", route.source_id, item.id, episode.index),
                probe_json: None,
                is_available: true,
            };
            let _ = repo::upsert_file(&db, &file).await;

            // 写入默认的视频流和音频流，防止播放决策报 NoVideoTrack (文件里没有视频轨)
            let default_streams = vec![
                whatmedia_core::MediaStream {
                    id: uuid::Uuid::new_v4(),
                    file_id,
                    stream_type: whatmedia_core::StreamType::Video,
                    index: 0,
                    codec: Some("h264".to_string()),
                    language: None,
                    title: Some("1080P".to_string()),
                    is_default: true,
                    is_forced: false,
                    width: Some(1920),
                    height: Some(1080),
                    bit_depth: Some(8),
                    hdr_type: whatmedia_core::HdrType::Sdr,
                    channels: None,
                    bitrate: None,
                },
                whatmedia_core::MediaStream {
                    id: uuid::Uuid::new_v4(),
                    file_id,
                    stream_type: whatmedia_core::StreamType::Audio,
                    index: 1,
                    codec: Some("aac".to_string()),
                    language: None,
                    title: Some("立体声".to_string()),
                    is_default: true,
                    is_forced: false,
                    width: None,
                    height: None,
                    bit_depth: None,
                    hdr_type: whatmedia_core::HdrType::Sdr,
                    channels: Some(2),
                    bitrate: None,
                },
            ];
            let _ = repo::replace_streams(&db, file_id, &default_streams).await;
        }
    }

    Json(ImportVodResponse {
        item_id,
        library_id: library.id,
        title: title.to_string(),
        is_new: true,
    })
    .into_response()
}

pub async fn check_vod_imported(
    State(state): State<AppState>,
    Query(query): Query<CheckImportedQuery>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let title = query.title.trim();
    if title.is_empty() {
        return Json(CheckImportedResponse {
            imported: false,
            item_id: None,
        })
        .into_response();
    }

    let existing: Result<Option<(String,)>, sqlx::Error> = sqlx::query_as(
        "SELECT id FROM media_item WHERE provider = 'vod_aggregate' AND title = ? LIMIT 1",
    )
    .bind(title)
    .fetch_optional(db.pool())
    .await;

    match existing {
        Ok(Some((id_str,))) => {
            let uuid = uuid::Uuid::parse_str(&id_str).ok();
            Json(CheckImportedResponse {
                imported: true,
                item_id: uuid,
            })
            .into_response()
        }
        _ => Json(CheckImportedResponse {
            imported: false,
            item_id: None,
        })
        .into_response(),
    }
}

pub async fn get_hot_vod_items(
    State(state): State<AppState>,
    Current(principal): Current,
) -> Response {
    if principal.is_child() {
        return Json(Vec::<AggregatedVodItem>::new()).into_response();
    }
    let engine = state.aggregator.read().await;
    let items = engine.search("").await;
    Json(items).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rewrite_vod_m3u8_with_encryption_and_tags() {
        let base_url = reqwest::Url::parse("https://cdn.example.com/hls/index.m3u8").unwrap();
        let m3u8_content = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:10
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-KEY:METHOD=AES-128,URI="enc.key",IV=0x1234
#EXT-X-MAP:URI="init.mp4"
#EXTINF:10.0,
seg-001.ts
#EXTINF:10.0,
https://cdn.example.com/hls/seg-002.ts
#EXT-X-ENDLIST"#;

        let rewritten = rewrite_vod_m3u8(m3u8_content, &base_url);
        
        // 验证 AES-128 KEY 是否被重写为代理地址
        assert!(rewritten.contains("#EXT-X-KEY:METHOD=AES-128,URI=\"/api/v1/proxy/vod_stream?url=https%3A%2F%2Fcdn.example.com%2Fhls%2Fenc.key\",IV=0x1234"));
        // 验证 EXT-X-MAP 是否被重写
        assert!(rewritten.contains("#EXT-X-MAP:URI=\"/api/v1/proxy/vod_stream?url=https%3A%2F%2Fcdn.example.com%2Fhls%2Finit.mp4\""));
        // 验证相对切片
        assert!(rewritten.contains("/api/v1/proxy/vod_stream?url=https%3A%2F%2Fcdn.example.com%2Fhls%2Fseg-001.ts"));
        // 验证绝对切片
        assert!(rewritten.contains("/api/v1/proxy/vod_stream?url=https%3A%2F%2Fcdn.example.com%2Fhls%2Fseg-002.ts"));
    }

    #[test]
    fn test_urldecode_and_double_encoding() {
        let single = "https%3A%2F%2Fexample.com%2Fplay.m3u8";
        assert_eq!(urldecode(single), "https://example.com/play.m3u8");

        let double = "https%253A%252F%252Fexample.com%252Fplay.m3u8";
        let step1 = urldecode(double);
        assert_eq!(step1, "https%3A%2F%2Fexample.com%2Fplay.m3u8");
        assert_eq!(urldecode(&step1), "https://example.com/play.m3u8");
    }
}

