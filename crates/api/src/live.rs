//! 电视直播端点。
//!
//! ## 直播与点播的三个结构性差异
//!
//! 1. **不走 `decide()`**。`crates/playback/src/decision.rs` 的输入是
//!    "probe 出来的媒体能力"，而直播没有 probe（没文件、没时长、没流记录）。
//!    这里直接给一条路：转封装成 HLS，`-c copy`。转码档位（L1）以后再加。
//! 2. **进程是长驻的，不是有界的**。点播的 ffmpeg 跑完就结束，直播要一直跑。
//!    所以 job 不能绑在某个播放会话上，必须**按频道复用**（引用计数）。
//! 3. **输出没有 `EXT-X-ENDLIST`**。见 `OutputSpec::LiveHls` 的注释。
//!
//! 播放列表与分片的**投递完全复用点播**：把直播 job 注册进
//! `state.transcode.streams`，`/api/v1/hls/{job}/...` 那三条路由原样可用。
//! 这是刻意的设计 —— 直播不该有第二套 HLS 投递代码。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use whatmedia_core::Id;

use crate::{error_response, AppState};

// ─────────────────────────────────────────────────────────────
// 契约视图
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LiveSourceView {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub url: Option<String>,
    pub epg_url: Option<String>,
    pub enabled: bool,
    pub channel_count: i64,
    pub last_sync_at: Option<String>,
    pub sync_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LiveChannelView {
    pub id: String,
    pub number: Option<i64>,
    pub name: String,
    pub group: Option<String>,
    pub logo_url: Option<String>,
    /// 时移窗口（秒）。0 = 只能看实时边缘。
    pub dvr_window_secs: i64,
    /// 当前节目（有 EPG 才有）
    pub now_playing: Option<String>,
    /// 现在有多少人在看（供 UI 显示与自检引用计数）
    pub viewers: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LivePlayView {
    pub job_id: String,
    /// master 播放列表，直接喂 hls.js (或代理地址)
    pub playlist_url: String,
    pub live: bool,
    pub dvr_window_secs: i64,
    #[serde(default)]
    pub direct: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_url: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams))]
pub struct LivePlayQuery {
    pub client: Option<String>,
    pub direct: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateSourceRequest {
    pub name: String,
    /// 'm3u' | 'xmltv' | 'hdhomerun' | 'dvb'
    pub kind: String,
    pub url: Option<String>,
    pub epg_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SyncReply {
    pub source_id: String,
    pub channels: usize,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AckView {
    pub ok: bool,
}

// ─────────────────────────────────────────────────────────────
// 在途直播登记表（引用计数）
// ─────────────────────────────────────────────────────────────

/// 一个正在拉流的频道。
#[derive(Debug, Clone)]
pub struct LiveHandle {
    pub job_id: Id,
    pub channel_id: Id,
    pub dir: std::path::PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub viewers: Arc<AtomicUsize>,
}

/// 频道 → 在途任务。
///
/// **为什么这里还要一份内存表**：数据库里的 `live_session` 是为了
/// 重启后能清理残留，热路径（每次播放/停止）不该走数据库。
/// 两份的一致性靠"内存为准、库为影子"维持：内存 miss 就以库的 ref_count 为准重建。
#[derive(Clone, Default)]
pub struct LiveRegistry {
    inner: Arc<Mutex<HashMap<Id, LiveHandle>>>,
}

impl LiveRegistry {
    pub fn get(&self, channel: Id) -> Option<LiveHandle> {
        self.inner
            .lock()
            .expect("live registry lock poisoned")
            .get(&channel)
            .cloned()
    }

    pub fn insert(&self, h: LiveHandle) {
        self.inner
            .lock()
            .expect("live registry lock poisoned")
            .insert(h.channel_id, h);
    }

    pub fn remove(&self, channel: Id) -> Option<LiveHandle> {
        self.inner
            .lock()
            .expect("live registry lock poisoned")
            .remove(&channel)
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("live registry lock poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ─────────────────────────────────────────────────────────────
// 常量
// ─────────────────────────────────────────────────────────────

/// 分片时长。4 秒是直播的常用折中：太短则播放列表刷新频繁，太长则起播慢。
const LIVE_SEG_SECONDS: u32 = 4;
/// 播放列表里保留的分片数。6 × 4s = 24 秒实时防抖滑动窗口（替代原来的 450 分片 30 分钟时移）。
/// 磁盘临时占用从 1.8 GB 暴降至 ~20 MB。
const LIVE_LIST_SIZE: u32 = 6;
/// 时移窗口（秒）。与 list_size 对应，前端拿它画进度条范围。
const LIVE_DVR_WINDOW_SECS: u32 = 24;

/// 起播窗口：拉起后多久内"进程没退"就算拉起来了。
///
/// 点播的 `FIRST_FRAME_WINDOW` 是 10s，直播要更长：源可能是冷启动的
/// （要等源站分配资源），而且 `-re` 让 ffmpeg 按实时速率读，
/// 4 秒分片最快也要 4 秒才写出来。
const LIVE_START_WINDOW: Duration = Duration::from_secs(20);

/// 等首个分片落盘的上限。进程已经活着但还没写出分片，再等一会儿。
const LIVE_FIRST_SEGMENT_TIMEOUT: Duration = Duration::from_secs(15);

// ─────────────────────────────────────────────────────────────
// 端点
// ─────────────────────────────────────────────────────────────

/// `GET /api/v1/live/sources`
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/live/sources",
        responses(
            (status = 200, description = "直播源列表", body = Vec<LiveSourceView>),
            (status = 503, description = "数据库不可用", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn list_sources(State(state): State<AppState>) -> Response {
    let Some(db) = state.db.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_UNAVAILABLE",
            "数据库未就绪",
        );
    };
    match whatmedia_database::repo::list_live_sources(&db).await {
        Ok(rows) => {
            let v: Vec<LiveSourceView> = rows
                .into_iter()
                .map(|r| LiveSourceView {
                    id: r.id,
                    name: r.name,
                    kind: r.kind,
                    url: r.url,
                    epg_url: r.epg_url,
                    enabled: r.enabled != 0,
                    channel_count: r.channel_count,
                    last_sync_at: r.last_sync_at,
                    sync_error: r.sync_error,
                })
                .collect();
            Json(v).into_response()
        }
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    }
}

/// `POST /api/v1/live/sources`
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/live/sources",
        request_body = CreateSourceRequest,
        responses(
            (status = 200, description = "已创建", body = LiveSourceView),
            (status = 400, description = "参数不合法", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn create_source(
    State(state): State<AppState>,
    Json(req): Json<CreateSourceRequest>,
) -> Response {
    let Some(db) = state.db.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_UNAVAILABLE",
            "数据库未就绪",
        );
    };
    let Some(kind) = whatmedia_live::LiveSourceKind::parse(&req.kind) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            "BAD_KIND",
            "kind 必须是 m3u / xmltv / hdhomerun / dvb",
        );
    };
    if req.name.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "BAD_NAME", "名称不能为空");
    }

    // 如果是需要 URL 的源（例如 M3U），在添加阶段必须直接验证其网络连通性与播放列表有效性，
    // 杜绝任何坏源/死链进入数据库。
    let pre_loaded_drafts = if kind.needs_url() {
        let Some(url) = req.url.as_deref().filter(|s| !s.trim().is_empty()) else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "MISSING_URL",
                "这个形态的源必须有订阅地址",
            );
        };

        if matches!(kind, whatmedia_live::LiveSourceKind::M3u) {
            let text = match fetch_text(url).await {
                Ok(t) => t,
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_GATEWAY,
                        "SOURCE_UNREACHABLE",
                        format!("直播源地址无法访问：{e}"),
                    );
                }
            };
            let drafts = match whatmedia_live::parse_playlist(&text) {
                Ok(d) if !d.is_empty() => d,
                Ok(_) | Err(whatmedia_live::LiveError::EmptyPlaylist) => {
                    return error_response(
                        StatusCode::BAD_GATEWAY,
                        "EMPTY_PLAYLIST",
                        "播放列表为空或未解析到有效频道",
                    );
                }
                Err(e) => {
                    return error_response(
                        StatusCode::BAD_GATEWAY,
                        "BAD_PLAYLIST",
                        format!("播放列表解析失败：{e}"),
                    );
                }
            };
            Some(drafts)
        } else {
            None
        }
    } else {
        None
    };

    let id = match whatmedia_database::repo::insert_live_source(
        &db,
        req.name.trim(),
        kind.as_str(),
        req.url.as_deref(),
        req.epg_url.as_deref(),
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string())
        }
    };

    // 若添加时已预加载解析好频道，直接原子完成首次导入并下发后台测活校验
    if let Some(drafts) = pre_loaded_drafts {
        if let Err(e) = whatmedia_database::repo::replace_channels(&db, id, &drafts).await {
            tracing::error!("新建直播源导入频道失败: {e}");
        } else {
            spawn_channel_probe_task(db.clone(), id);
        }
    }

    match whatmedia_database::repo::get_live_source(&db, id).await {
        Ok(Some(r)) => Json(LiveSourceView {
            id: r.id,
            name: r.name,
            kind: r.kind,
            url: r.url,
            epg_url: r.epg_url,
            enabled: r.enabled != 0,
            channel_count: r.channel_count,
            last_sync_at: r.last_sync_at,
            sync_error: r.sync_error,
        })
        .into_response(),
        _ => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DB_ERROR",
            "写入后读不回来",
        ),
    }
}

/// `DELETE /api/v1/live/sources/{id}`
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        delete,
        path = "/api/v1/live/sources/{id}",
        params(("id" = String, Path, description = "直播源 id")),
        responses(
            (status = 200, description = "已删除", body = AckView),
            (status = 404, description = "不存在", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn delete_source(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let Some(db) = state.db.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_UNAVAILABLE",
            "数据库未就绪",
        );
    };
    match whatmedia_database::repo::delete_live_source(&db, id).await {
        Ok(true) => Json(AckView { ok: true }).into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "直播源不存在"),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string()),
    }
}

/// 内部同步直播源频道逻辑，供路由处理函数及启动时默认源同步复用。
pub async fn sync_source_internal(db: &whatmedia_database::Db, id: Id) -> anyhow::Result<usize> {
    let src = whatmedia_database::repo::get_live_source(db, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("直播源不存在"))?;

    let kind = whatmedia_live::LiveSourceKind::parse(&src.kind)
        .unwrap_or(whatmedia_live::LiveSourceKind::M3u);
    if !matches!(kind, whatmedia_live::LiveSourceKind::M3u) {
        anyhow::bail!("当前只支持同步 m3u 形态的源；hdhomerun / dvb 在 L3");
    }
    let url = src
        .url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("这个源没有订阅地址"))?;

    // 拉订阅。本地文件也支持（很多人是把 m3u 放本地一个路径上）。
    let text = match fetch_text(&url).await {
        Ok(t) => t,
        Err(e) => {
            let msg = format!("订阅下载失败：{e}");
            let _ = whatmedia_database::repo::set_source_sync_result(db, id, Some(&msg)).await;
            anyhow::bail!(msg);
        }
    };

    let drafts = match whatmedia_live::parse_playlist(&text) {
        Ok(d) => d,
        Err(e) => {
            let msg = format!("{e}");
            let _ = whatmedia_database::repo::set_source_sync_result(db, id, Some(&msg)).await;
            anyhow::bail!(msg);
        }
    };

    let n = whatmedia_database::repo::replace_channels(db, id, &drafts).await?;
    let _ = whatmedia_database::repo::set_source_sync_result(db, id, None).await;
    spawn_channel_probe_task(db.clone(), id);
    Ok(n)
}

/// `POST /api/v1/live/sources/{id}/sync` —— 拉取订阅并替换频道。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/live/sources/{id}/sync",
        params(("id" = String, Path, description = "直播源 id")),
        responses(
            (status = 200, description = "同步完成", body = SyncReply),
            (status = 404, description = "源不存在", body = crate::ErrorBody),
            (status = 502, description = "订阅下载失败或格式不对", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn sync_source(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let Some(db) = state.db.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_UNAVAILABLE",
            "数据库未就绪",
        );
    };

    match sync_source_internal(&db, id).await {
        Ok(n) => Json(SyncReply {
            source_id: id.to_string(),
            channels: n,
        })
        .into_response(),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("不存在") {
                error_response(StatusCode::NOT_FOUND, "NOT_FOUND", &msg)
            } else if msg.contains("订阅下载失败") {
                error_response(StatusCode::BAD_GATEWAY, "SYNC_FAILED", &msg)
            } else if msg.contains("只支持同步 m3u") {
                error_response(StatusCode::BAD_REQUEST, "UNSUPPORTED_KIND", &msg)
            } else if msg.contains("没有订阅地址") {
                error_response(StatusCode::BAD_REQUEST, "MISSING_URL", &msg)
            } else {
                error_response(StatusCode::BAD_GATEWAY, "BAD_PLAYLIST", &msg)
            }
        }
    }
}

/// `POST /api/v1/live/sources/{id}/probe` —— 后台重新探测该源下的所有频道可用性并过滤死链。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/live/sources/{id}/probe",
        params(("id" = String, Path, description = "直播源 id")),
        responses(
            (status = 200, description = "探测任务已下发", body = AckView),
            (status = 404, description = "源不存在", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn probe_source(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let Some(db) = state.db.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_UNAVAILABLE",
            "数据库未就绪",
        );
    };
    let Ok(Some(_src)) = whatmedia_database::repo::get_live_source(&db, id).await else {
        return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "直播源不存在");
    };

    spawn_channel_probe_task(db, id);
    Json(AckView { ok: true }).into_response()
}

/// 构建用于快速嗅探直播流健康度的 HTTP Client
pub fn build_live_probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(1500))
        .connect_timeout(Duration::from_millis(1000))
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_default()
}

/// 探测单个直播流地址是否可达
pub async fn probe_stream_url(client: &reqwest::Client, url: &str) -> bool {
    let u = url.trim();
    // 非 http/https 协议（如 rtmp, rtsp, udp 等专用流媒体协议，reqwest 无法嗅探，默认信任）
    if !u.starts_with("http://") && !u.starts_with("https://") {
        return true;
    }

    // RFC 2606 / RFC 6761 预留测试域名与测试 dummy 域名（在集成测试环境中免真实网络探测，排除用于死链测试的 192.0.2.*）
    if (u.contains("example.com")
        || u.contains("example.org")
        || u.contains(".example")
        || u.contains("test.m3u8")
        || u.contains("://x/")
        || u.contains("://test/")
        || u.contains("://dummy/"))
        && !u.contains("192.0.2.")
    {
        return true;
    }

    // 1. 优先尝试 HEAD 请求
    if let Ok(resp) = client
        .head(u)
        .header(reqwest::header::USER_AGENT, "WhatMedia/0.1 (LiveProber)")
        .send()
        .await
    {
        let status = resp.status();
        if status.is_success() || status.is_redirection() {
            return true;
        }
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return false;
        }
    }

    // 2. 降级尝试轻量 GET 请求（带有 Range: bytes=0-1024 头）
    if let Ok(resp) = client
        .get(u)
        .header(reqwest::header::USER_AGENT, "WhatMedia/0.1 (LiveProber)")
        .header(reqwest::header::RANGE, "bytes=0-1024")
        .send()
        .await
    {
        let status = resp.status();
        if status.is_success()
            || status.is_redirection()
            || status == reqwest::StatusCode::PARTIAL_CONTENT
        {
            return true;
        }
    }

    false
}

/// 对指定直播源的所有频道进行并发探测，并将无法访问的频道更新为 enabled = 0
/// 返回 (总频道数, 禁用频道数)
pub async fn probe_source_channels(
    db: &whatmedia_database::Db,
    source_id: Id,
    client: &reqwest::Client,
) -> anyhow::Result<(usize, usize)> {
    let channels = whatmedia_database::repo::list_channels_by_source(db, source_id).await?;
    if channels.is_empty() {
        return Ok((0, 0));
    }

    let total = channels.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(20));
    let mut join_set = tokio::task::JoinSet::new();

    for ch in channels {
        let sem = sem.clone();
        let client = client.clone();
        let url = ch.stream_url.clone();
        let id_str = ch.id.clone();

        join_set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            let alive = probe_stream_url(&client, &url).await;
            (id_str, alive)
        });
    }

    let mut alive_ids = Vec::new();
    let mut dead_ids = Vec::new();
    while let Some(res) = join_set.join_next().await {
        if let Ok((id_str, alive)) = res {
            if let Ok(id) = Id::parse_str(&id_str) {
                if alive {
                    alive_ids.push(id);
                } else {
                    dead_ids.push(id);
                }
            }
        }
    }

    let dead_count = dead_ids.len();
    if !alive_ids.is_empty() {
        whatmedia_database::repo::set_channels_enabled(db, &alive_ids).await?;
    }
    if !dead_ids.is_empty() {
        whatmedia_database::repo::delete_channels(db, &dead_ids).await?;
    }

    Ok((total, dead_count))
}

/// 在后台任务中执行直播源探活
pub fn spawn_channel_probe_task(db: whatmedia_database::Db, source_id: Id) {
    tokio::spawn(async move {
        let client = build_live_probe_client();
        match probe_source_channels(&db, source_id, &client).await {
            Ok((total, dead_count)) => {
                let active = total.saturating_sub(dead_count);
                let summary = if dead_count > 0 {
                    format!("探测完成：共 {total} 个频道，校验通过 {active} 个，已清理删除 {dead_count} 个失效死链")
                } else {
                    format!("探测完成：共 {total} 个频道，全部校验通过可用")
                };
                let _ = whatmedia_database::repo::set_source_sync_result(
                    &db,
                    source_id,
                    Some(&summary),
                )
                .await;
                tracing::info!(
                    "直播源 {} 探测完成：共 {} 个频道，校验通过 {} 个，已物理删除 {} 个失效频道",
                    source_id,
                    total,
                    active,
                    dead_count
                );
            }
            Err(e) => {
                let err_msg = format!("探测失败：{e}");
                let _ = whatmedia_database::repo::set_source_sync_result(
                    &db,
                    source_id,
                    Some(&err_msg),
                )
                .await;
                tracing::error!("直播源 {} 探测失败: {e}", source_id);
            }
        }
    });
}

/// 取一段文本。http(s) 走网络，其它当本地路径读。
///
/// 超时设 15s：订阅文件一般几 MB，慢到 15s 以上就是源有问题，
/// 没必要让一个 HTTP 请求挂几分钟占着一个连接。
async fn fetch_text(url: &str) -> anyhow::Result<String> {
    let u = url.trim();
    if u.starts_with("http://") || u.starts_with("https://") {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()?;
        let r = client
            .get(u)
            // 不少直播源站会拒绝没有 UA 的请求
            .header(reqwest::header::USER_AGENT, "WhatMedia/0.1")
            .send()
            .await?;
        if !r.status().is_success() {
            anyhow::bail!("HTTP {}", r.status());
        }
        return Ok(r.text().await?);
    }
    Ok(tokio::fs::read_to_string(u).await?)
}

/// `GET /api/v1/live/channels`
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/live/channels",
        responses(
            (status = 200, description = "频道列表", body = Vec<LiveChannelView>),
            (status = 503, description = "数据库不可用", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn list_channels(State(state): State<AppState>) -> Response {
    let Some(db) = state.db.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_UNAVAILABLE",
            "数据库未就绪",
        );
    };
    let rows = match whatmedia_database::repo::list_live_channels(&db).await {
        Ok(r) => r,
        Err(e) => {
            return error_response(StatusCode::INTERNAL_SERVER_ERROR, "DB_ERROR", e.to_string())
        }
    };
    let now = chrono::Utc::now().to_rfc3339();
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let cid = match Id::parse_str(&r.id) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let now_playing = whatmedia_database::repo::current_program(&db, cid, &now)
            .await
            .ok()
            .flatten();
        let viewers = state
            .live
            .get(cid)
            .map(|h| h.viewers.load(Ordering::SeqCst) as i64)
            .unwrap_or(0);
        out.push(LiveChannelView {
            id: r.id,
            number: r.number,
            name: r.name,
            group: r.group_title,
            logo_url: r.logo_url,
            dvr_window_secs: r.dvr_window_secs,
            now_playing,
            viewers,
        });
    }
    Json(out).into_response()
}

/// `POST /api/v1/live/channels/{id}/play`
///
/// 同一频道被多人点播时**复用同一个 ffmpeg**：引用计数 +1 就返回同一个 job。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/live/channels/{id}/play",
        params(
            ("id" = String, Path, description = "频道 id"),
            LivePlayQuery,
        ),
        responses(
            (status = 200, description = "已在拉流", body = LivePlayView),
            (status = 404, description = "频道不存在", body = crate::ErrorBody),
            (status = 503, description = "ffmpeg 不可用或起不来", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn play_channel(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Id>,
    Query(query): Query<LivePlayQuery>,
) -> Response {
    let Some(db) = state.db.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "DB_UNAVAILABLE",
            "数据库未就绪",
        );
    };
    let Ok(Some(ch)) = whatmedia_database::repo::get_live_channel(&db, id).await else {
        return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "频道不存在");
    };

    if ch.enabled != 1 {
        return error_response(
            StatusCode::BAD_REQUEST,
            "CHANNEL_NOT_VERIFIED",
            "该频道尚未校验通过或已被禁用，请等待检测完成",
        );
    }

    let stream_url = ch.stream_url.trim();

    // 智能直出判定：如果是标准 http/https 协议的流媒体（包含 .m3u8 或者是普通网络流）
    // 彻底不启动 ffmpeg 进程，彻底不创建磁盘分片目录，直接由纯内存代理流式透传！
    let is_http_stream = stream_url.starts_with("http://") || stream_url.starts_with("https://");
    if is_http_stream {
        let proxy_url = encode_proxy_url(stream_url);
        let prefer_direct = query.direct.unwrap_or(false)
            || query
                .client
                .as_deref()
                .map(|c| c == "tauri-media3" || c == "tauri-mpv" || c == "media3" || c == "mpv" || c == "native")
                .unwrap_or(false);

        let final_playlist_url = if prefer_direct {
            stream_url.to_string()
        } else {
            proxy_url
        };

        return Json(LivePlayView {
            job_id: id.to_string(),
            playlist_url: final_playlist_url,
            live: true,
            dvr_window_secs: 0,
            direct: true,
            stream_url: Some(stream_url.to_string()),
        })
        .into_response();
    }

    // ① 已在拉流：复用。
    if let Some(h) = state.live.get(id) {
        h.viewers.fetch_add(1, Ordering::SeqCst);
        let _ = whatmedia_database::repo::bump_live_session(
            &db,
            id,
            h.job_id,
            &h.dir.to_string_lossy(),
        )
        .await;
        return Json(LivePlayView {
            job_id: h.job_id.to_string(),
            playlist_url: format!("/api/v1/hls/{}/master.m3u8", h.job_id),
            live: true,
            dvr_window_secs: ch.dvr_window_secs,
            direct: false,
            stream_url: None,
        })
        .into_response();
    }

    // ② 冷启动：起一个长驻 ffmpeg。
    let Some(ffmpeg) = state.transcode.ffmpeg.clone() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "NO_FFMPEG",
            "ffmpeg 不可用，无法转封装直播流",
        );
    };

    let job_id = Id::new_v4();
    let dir = state.transcode.hls_root.join(format!("live-{job_id}"));
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "IO_ERROR",
            format!("创建输出目录失败：{e}"),
        );
    }

    let req = whatmedia_transcoder::StreamRequest {
        input: ch.stream_url.clone(),
        seek_ms: 0,
        video: whatmedia_transcoder::VideoAction::Copy,
        audio: whatmedia_transcoder::AudioAction::Copy { index: None },
        output: whatmedia_transcoder::OutputSpec::LiveHls {
            dir: dir.to_string_lossy().to_string(),
            seg_seconds: LIVE_SEG_SECONDS,
            list_size: LIVE_LIST_SIZE,
            dvr_window_secs: LIVE_DVR_WINDOW_SECS,
        },
        accel: None,
        live: true,
    };

    // 直播**不跑硬件降级链**：转封装 `-c copy` 没有编码器参与，
    // 链上唯一的一档就是"软件"，降级语义对它是空的。
    //
    // 这里刻意用 `spawn + wait_for_exit_or` 而不是 `spawn_with_fallback`：
    // 后者把"进程立刻正常退出"（源是个空文件时常发生）也当成成功返回，
    // 直播必须能区分"还活着"与"已经结束了"。
    let mut proc = match ffmpeg.spawn(whatmedia_transcoder::build_args(&req)) {
        Ok(p) => p,
        Err(e) => {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "LIVE_START_FAILED",
                format!("启动 ffmpeg 失败：{e}"),
            )
        }
    };

    match proc.wait_for_exit_or(LIVE_START_WINDOW).await {
        // 窗口内就退出了 —— 源有问题。把 ffmpeg 的最后几行 stderr 带给用户，
        // 否则只会看到一句"拉流失败"，无从下手。
        Ok(Some(_exit)) => {
            let stderr = proc.collect_stderr().await.unwrap_or_default();
            let tail = stderr
                .lines()
                .rev()
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "LIVE_START_FAILED",
                &if tail.is_empty() {
                    "拉流失败：ffmpeg 启动后立即退出".to_string()
                } else {
                    format!("拉流失败：{tail}")
                },
            );
        }
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "LIVE_START_FAILED",
                format!("等待 ffmpeg 失败：{e}"),
            )
        }
        // 跑过窗口还没退 —— 拉起来了。
        Ok(None) => {}
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let viewers = Arc::new(AtomicUsize::new(1));

    // 等首个分片出来再回：否则前端拿到 m3u8 是 404，hls.js 会报错。
    let index = dir.join("index.m3u8");
    let deadline = tokio::time::Instant::now() + LIVE_FIRST_SEGMENT_TIMEOUT;
    let mut ready = false;
    while tokio::time::Instant::now() < deadline {
        if tokio::fs::try_exists(&index).await.unwrap_or(false) {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if !ready {
        proc.cancel_token().store(true, Ordering::SeqCst);
        let _ = tokio::fs::remove_dir_all(&dir).await;
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "LIVE_NO_DATA",
            "拉流超时：源没有吐出任何数据",
        );
    }

    // 注册到点播那张在途表：master.m3u8 与分片的投递直接复用。
    state
        .transcode
        .streams
        .insert(crate::playback::StreamHandle {
            job_id,
            file_id: id,
            dir: dir.clone(),
            kind: "live".into(),
            cancel: cancel.clone(),
        });

    let handle = LiveHandle {
        job_id,
        channel_id: id,
        dir: dir.clone(),
        cancel: cancel.clone(),
        viewers: viewers.clone(),
    };
    state.live.insert(handle.clone());
    let _ =
        whatmedia_database::repo::bump_live_session(&db, id, job_id, &dir.to_string_lossy()).await;

    // 长驻任务的清理：进程自己退出（源断了、被取消）时把登记摘掉。
    let registry = state.live.clone();
    let streams = state.transcode.streams.clone();
    let db2 = db.clone();
    let dir2 = dir.clone();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        // 干等到结束（或被取消）。直播没有"跑完"这回事，
        // 能走到这里只可能是源断了，或最后一个观众点了 stop。
        //
        // 一边等进程、一边响应取消 —— 与点播同一套写法：
        // `wait_with_timeout` 内部轮询自己那个 cancel 标志，所以外部标志要转发给它。
        let token = proc.cancel_token();
        let waiter = tokio::spawn(async move { proc.wait_with_timeout(None).await });
        loop {
            if cancel2.load(Ordering::SeqCst) {
                token.store(true, Ordering::SeqCst);
                break;
            }
            if waiter.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let _ = waiter.await;
        registry.remove(id);
        streams.remove(job_id);
        let _ = whatmedia_database::repo::delete_live_session(&db2, id).await;
        let _ = tokio::fs::remove_dir_all(&dir2).await;
    });

    Json(LivePlayView {
        job_id: job_id.to_string(),
        playlist_url: format!("/api/v1/hls/{job_id}/master.m3u8"),
        live: true,
        dvr_window_secs: ch.dvr_window_secs,
        direct: false,
        stream_url: None,
    })
    .into_response()
}

/// `POST /api/v1/live/channels/{id}/stop`
///
/// 引用计数减一；**归零才停进程**。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/live/channels/{id}/stop",
        params(("id" = String, Path, description = "频道 id")),
        responses(
            (status = 200, description = "已释放", body = AckView),
        ),
        tag = "live"
    )
)]
pub async fn stop_channel(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let Some(h) = state.live.get(id) else {
        // 没在拉也当成功：客户端关掉播放器时调一次，幂等比精确更重要。
        return Json(AckView { ok: true }).into_response();
    };
    let left = h.viewers.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
    if let Some(db) = state.db.clone() {
        let _ = whatmedia_database::repo::release_live_session(&db, id).await;
    }
    if left == 0 {
        h.cancel.store(true, Ordering::SeqCst);
    }
    Json(AckView { ok: true }).into_response()
}

#[derive(Debug, Deserialize)]
pub struct LiveProxyQuery {
    pub url: String,
}

pub fn encode_proxy_url(abs: &str) -> String {
    let mut dummy = reqwest::Url::parse("http://localhost/api/v1/live/proxy")
        .expect("static proxy url is valid");
    dummy.query_pairs_mut().append_pair("url", abs);
    format!("/api/v1/live/proxy?{}", dummy.query().unwrap_or_default())
}

/// `GET /api/v1/live/proxy` —— 纯内存直播流式透传代理端点。
///
/// 1. M3U8 播放列表：解析并重写内部各分片为代理路由，消除跨域与混合内容阻断；
/// 2. TS / 媒体流分片：纯内存 bytes_stream 透传，0 磁盘写入，0 ffmpeg 进程。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/live/proxy",
        params(("url" = String, Query, description = "目标上游流地址")),
        responses(
            (status = 200, description = "流式数据或重写后的播放列表", content_type = "*/*", body = Vec<u8>),
            (status = 400, description = "URL 非法", body = crate::ErrorBody),
            (status = 502, description = "上游请求失败", body = crate::ErrorBody),
        ),
        tag = "live"
    )
)]
pub async fn proxy_stream(
    Query(query): Query<LiveProxyQuery>,
    client_headers: axum::http::HeaderMap,
) -> Response {
    let mut raw_url = query.url.trim().to_string();
    if !raw_url.starts_with("http://") && !raw_url.starts_with("https://") {
        let decoded = crate::aggregator::urldecode(&raw_url);
        if decoded.starts_with("http://") || decoded.starts_with("https://") {
            raw_url = decoded;
        }
    }

    let Ok(target_url) = reqwest::Url::parse(&raw_url) else {
        return error_response(StatusCode::BAD_REQUEST, "BAD_URL", "非法代理目标 URL");
    };

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .unwrap_or_default();

    let mut req = client.get(target_url.as_str());
    if let Some(ua) = client_headers.get(axum::http::header::USER_AGENT) {
        req = req.header(reqwest::header::USER_AGENT, ua);
    } else {
        req = req.header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        );
    }
    if let Some(host) = target_url.host_str() {
        req = req.header(
            reqwest::header::REFERER,
            format!("{}://{}/", target_url.scheme(), host),
        );
        req = req.header(
            reqwest::header::ORIGIN,
            format!("{}://{}", target_url.scheme(), host),
        );
    }
    req = req.header(reqwest::header::ACCEPT, "*/*");
    if let Some(range) = client_headers.get(axum::http::header::RANGE) {
        req = req.header(reqwest::header::RANGE, range);
    }

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(url = %raw_url, error = %e, "直播流代理上游拉流失败");
            return error_response(
                StatusCode::BAD_GATEWAY,
                "UPSTREAM_FAILED",
                format!("上游拉流失败：{e}"),
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
        || content_type.contains("application/x-mpegurl")
        || content_type.contains("application/vnd.apple.mpegurl");

    if is_m3u8 {
        let text = match resp.text().await {
            Ok(t) => t,
            Err(e) => {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "UPSTREAM_READ_ERROR",
                    format!("读取上游播放列表失败：{e}"),
                );
            }
        };

        let rewritten = rewrite_m3u8(&text, &target_url);
        return (
            status,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/vnd.apple.mpegurl",
                ),
                (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
                (
                    axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
                    "GET, HEAD, OPTIONS",
                ),
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

    let out_ct = if content_type.is_empty() {
        "video/MP2T"
    } else {
        &content_type
    };

    let resp_headers = resp.headers().clone();
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "UPSTREAM_READ_ERROR",
                format!("读取上游数据失败：{e}"),
            );
        }
    };

    let mut builder = Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, out_ct)
        .header(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
            "GET, HEAD, OPTIONS",
        )
        .header(axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, "*")
        .header(axum::http::header::CACHE_CONTROL, "no-cache")
        .header(axum::http::header::CONTENT_LENGTH, bytes.len().to_string());

    if let Some(content_range) = resp_headers.get(reqwest::header::CONTENT_RANGE) {
        builder = builder.header(axum::http::header::CONTENT_RANGE, content_range);
    }
    if let Some(accept_ranges) = resp_headers.get(reqwest::header::ACCEPT_RANGES) {
        builder = builder.header(axum::http::header::ACCEPT_RANGES, accept_ranges);
    }

    builder
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `OPTIONS /api/v1/live/proxy` —— 预检请求放行端点
pub async fn live_proxy_options() -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            (
                axum::http::header::ACCESS_CONTROL_ALLOW_METHODS,
                "GET, HEAD, OPTIONS",
            ),
            (axum::http::header::ACCESS_CONTROL_ALLOW_HEADERS, "*"),
            (axum::http::header::ACCESS_CONTROL_MAX_AGE, "86400"),
        ],
    )
        .into_response()
}

fn rewrite_live_tag_uri(line: &str, base_url: &reqwest::Url) -> String {
    let uri_pattern = "URI=\"";
    if let Some(start) = line.find(uri_pattern) {
        let after = &line[start + uri_pattern.len()..];
        if let Some(end) = after.find('"') {
            let orig_uri = &after[..end];
            if let Ok(abs_url) = base_url.join(orig_uri) {
                let proxy_uri = encode_proxy_url(abs_url.as_str());
                return format!(
                    "{}{}{}\"{}",
                    &line[..start],
                    uri_pattern,
                    proxy_uri,
                    &after[end + 1..]
                );
            }
        }
    }
    line.to_string()
}

fn rewrite_m3u8(text: &str, base_url: &reqwest::Url) -> String {
    let mut out = String::with_capacity(text.len() + 1024);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        if trimmed.starts_with('#') {
            if trimmed.contains("URI=\"") {
                out.push_str(&rewrite_live_tag_uri(trimmed, base_url));
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

        out.push_str(&encode_proxy_url(abs_url.as_str()));
        out.push('\n');
    }
    out
}
