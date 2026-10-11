//! 播放相关 HTTP 端点。
//!
//! 三条约定：
//!   1. **Direct Play 必须支持 Range。** 客户端 Seek 全靠它，
//!      返回 200 全量会让播放器把整个文件下载完才能拖进度条。
//!   2. **没有 ffmpeg 时不能报 500。** 播不了要返回能解释的 501 + 错误码，
//!      前端据此提示"请安装 ffmpeg"，而不是弹一个网络错误。
//!   3. **所有路径都过 StorageProvider。** HTTP 层绝不自己拼文件系统路径。

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use whatmedia_core::{
    ByteRange, Container, HardwareProfile, Id, MediaStream, PlaybackStrategy, StorageProvider,
    TaskPriority,
};
use whatmedia_database::repo::{FileTarget, StreamSession};
use whatmedia_playback::{
    decide, ClientCapability, DecisionInput, MediaCapability, PlaybackDecision, ServerCapability,
};
use whatmedia_subtitle::{discover_sidecars, ExternalSubtitle};
use whatmedia_transcoder::{
    build_args, master_playlist, FfmpegRunner, OutputSpec, StreamRequest, TempDirGuard,
    TranscodePool,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::{bad_request, error_response, internal, unavailable, AppState};

// ─────────────────────────────────────────────────────────────
// 在途流式输出登记
// ─────────────────────────────────────────────────────────────

/// 一个正在（或曾经）往磁盘写分片的流式任务。
#[derive(Debug, Clone)]
pub struct StreamHandle {
    pub job_id: Id,
    pub file_id: Id,
    pub dir: std::path::PathBuf,
    pub kind: String,
    pub cancel: Arc<AtomicBool>,
}

/// 在途流式输出表。
///
/// 同时存在于内存与数据库：内存供热路径查，数据库供**重启后清理残留目录**。
#[derive(Clone, Default)]
pub struct StreamRegistry {
    inner: Arc<Mutex<HashMap<Id, StreamHandle>>>,
}

impl StreamRegistry {
    pub fn insert(&self, h: StreamHandle) {
        self.inner.lock().unwrap().insert(h.job_id, h);
    }

    pub fn get(&self, id: Id) -> Option<StreamHandle> {
        self.inner.lock().unwrap().get(&id).cloned()
    }

    pub fn remove(&self, id: Id) -> Option<StreamHandle> {
        self.inner.lock().unwrap().remove(&id)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 停止并移除。返回被停止的句柄。
    pub fn stop(&self, id: Id) -> Option<StreamHandle> {
        let h = self.remove(id)?;
        h.cancel.store(true, Ordering::SeqCst);
        Some(h)
    }
}

/// 服务端转码能力的句柄集合。放进 AppState 供所有播放端点共用。
#[derive(Clone)]
pub struct TranscodeRuntime {
    pub ffmpeg: Option<Arc<FfmpegRunner>>,
    pub profiles: Vec<HardwareProfile>,
    pub pool: Arc<TranscodePool>,
    pub streams: StreamRegistry,
    pub active: Arc<AtomicUsize>,
    pub hls_root: std::path::PathBuf,
}

impl TranscodeRuntime {
    pub fn new(
        ffmpeg: Option<Arc<FfmpegRunner>>,
        profiles: Vec<HardwareProfile>,
        hls_root: std::path::PathBuf,
    ) -> Self {
        let capacity = profiles
            .iter()
            .map(|p| p.max_concurrent_sessions as usize)
            .max()
            .unwrap_or(1)
            .max(1);
        Self {
            ffmpeg,
            profiles,
            pool: Arc::new(TranscodePool::new(capacity)),
            streams: StreamRegistry::default(),
            active: Arc::new(AtomicUsize::new(0)),
            hls_root,
        }
    }

    /// 当前服务端能力快照。
    pub fn server_capability(&self) -> ServerCapability {
        ServerCapability {
            ffmpeg_available: self.ffmpeg.is_some(),
            encoders: self.profiles.clone(),
            active_transcodes: self.active.load(Ordering::SeqCst),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 决策
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DecideParams {
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub item_id: Option<Id>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub file_id: Option<Id>,
    /// 客户端能力指纹：`web` / `tauri-mpv` / `android` / `apple` / `minimal`。
    #[serde(default = "default_client")]
    pub client: String,
    pub network_kbps: Option<u32>,
    #[serde(default)]
    pub start_ms: i64,
}

fn default_client() -> String {
    "web".to_string()
}

/// 决策结果的可序列化视图。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DecideView {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub file_id: Id,
    pub strategy: PlaybackStrategy,
    pub reason: String,
    pub reasons: Vec<ReasonView>,
    pub client: String,
    pub audio_index: Option<i32>,
    pub subtitles: Vec<SubtitleView>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReasonView {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SubtitleView {
    pub track_id: String,
    pub language: Option<String>,
    pub mode: String,
    pub url: Option<String>,
}

/// `GET /api/v1/playback/decide`
///
/// 只做决策，不产生副作用（不建会话、不启动 ffmpeg）。
/// 前端拿它做"这个片在这台设备上会怎么播"的预览。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/playback/decide",
        params(
        ("item_id" = Option<String>, Query, description = "与 file_id 至少给一个"),
        ("file_id" = Option<String>, Query),
        ("client" = Option<String>, Query, description = "web | tauri-mpv | android | apple | minimal"),
        ("network_kbps" = Option<u32>, Query, description = "实测带宽"),
        ("start_ms" = Option<i64>, Query, description = "续播起点"),
        ),
        responses(
            (status = 200, description = "决策结果（无副作用）", body = DecideView),
            (status = 404, description = "条目或文件不存在", body = crate::ErrorBody),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn decide_playback(
    State(state): State<AppState>,
    Query(q): Query<DecideParams>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let target = match resolve_file(&db, q.item_id, q.file_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "找不到可播放的文件")
        }
        Err(e) => return internal(e),
    };

    let client = ClientCapability::by_id(&q.client);
    let media = match build_media(&state, &db, &target).await {
        Ok(m) => m,
        Err(e) => return internal(e),
    };

    let d = decide(DecisionInput {
        media: &media,
        client: &client,
        server: &state.transcode.server_capability(),
        network_kbps: q.network_kbps,
        start_ms: q.start_ms,
        hls_dir: Some(
            hls_dir_for(&state, target.file.id)
                .to_string_lossy()
                .to_string(),
        ),
        seg_seconds: 6,
    });

    Json(decide_view(target.item_id, target.file.id, &q.client, &d)).into_response()
}

fn decide_view(item_id: Id, file_id: Id, client: &str, d: &PlaybackDecision) -> DecideView {
    DecideView {
        item_id,
        file_id,
        strategy: d.strategy,
        reason: d.summary(),
        reasons: d
            .reasons
            .iter()
            .map(|r| ReasonView {
                code: r.code().to_string(),
                message: r.message(),
            })
            .collect(),
        client: client.to_string(),
        audio_index: d.audio_index,
        subtitles: d
            .subtitle_plans
            .iter()
            .map(|p| SubtitleView {
                track_id: p.track_id.clone(),
                language: p.language.clone(),
                mode: format!("{:?}", p.mode).to_ascii_lowercase(),
                url: None,
            })
            .collect(),
    }
}

// ─────────────────────────────────────────────────────────────
// 会话
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateSessionBody {
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub item_id: Option<Id>,
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub file_id: Option<Id>,
    #[serde(default = "default_client")]
    pub client: String,
    pub device_id: Option<String>,
    pub network_kbps: Option<u32>,
    #[serde(default)]
    pub start_ms: i64,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SessionView {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub session_id: Id,
    pub strategy: PlaybackStrategy,
    pub reason: String,
    pub reasons: Vec<ReasonView>,
    /// Direct Play 用这个。
    pub url: Option<String>,
    /// Remux / 转码用这个。
    pub stream_url: Option<String>,
    /// HLS 用这个。
    pub hls_url: Option<String>,
    pub audio_index: Option<i32>,
    pub subtitles: Vec<SubtitleView>,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/playback/sessions",
        request_body = CreateSessionBody,
        responses(
            (status = 200, description = "会话；必要时已顺带启动转码", body = SessionView),
            (status = 404, description = "条目或文件不存在", body = crate::ErrorBody),
            (status = 501, description = "需要转码但服务端没有 ffmpeg", body = crate::ErrorBody),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn create_playback_session(
    State(state): State<AppState>,
    Json(body): Json<CreateSessionBody>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    let target = match resolve_file(&db, body.item_id, body.file_id).await {
        Ok(Some(t)) => t,
        Ok(None) => {
            return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "找不到可播放的文件")
        }
        Err(e) => return internal(e),
    };

    let client = ClientCapability::by_id(&body.client);
    let media = match build_media(&state, &db, &target).await {
        Ok(m) => m,
        Err(e) => return internal(e),
    };

    let hls_dir = hls_dir_for(&state, target.file.id);
    let mut d = decide(DecisionInput {
        media: &media,
        client: &client,
        server: &state.transcode.server_capability(),
        network_kbps: body.network_kbps,
        start_ms: body.start_ms,
        hls_dir: Some(hls_dir.to_string_lossy().to_string()),
        seg_seconds: 6,
    });

    if d.strategy == PlaybackStrategy::Unsupported {
        return error_response(
            StatusCode::NOT_ACCEPTABLE,
            "PLAYBACK_UNSUPPORTED",
            d.summary(),
        );
    }
    if d.strategy != PlaybackStrategy::DirectPlay && state.transcode.ffmpeg.is_none() {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            "FFMPEG_UNAVAILABLE",
            "需要转码或重新封装，但服务端没有可用的 ffmpeg",
        );
    }

    // 建会话（续播点的载体）。
    let sessions = match state.sessions.clone() {
        Some(s) => s,
        None => return unavailable("播放会话未初始化"),
    };
    let session = match sessions
        .start(whatmedia_playback::StartRequest {
            item_id: target.item_id,
            file_id: target.file.id,
            strategy: d.strategy,
            client_id: Some(body.client.clone()),
            device_id: body.device_id.clone(),
            reason: Some(d.summary()),
            position_ms: body.start_ms,
            duration_ms: target.file.duration_ms,
            user_id: None,
        })
        .await
    {
        Ok(s) => s,
        Err(e) => return internal(e),
    };

    // 需要 ffmpeg 时把真实路径填进请求并启动。
    let mut hls_url = None;
    let mut stream_url = None;
    if let Some(req) = d.stream.as_mut() {
        req.input = target.file.path.clone();
        match &req.output {
            OutputSpec::Hls { .. } => {
                let job = match start_hls(&state, &target, req.clone(), session.id).await {
                    Ok(j) => j,
                    Err(e) => return internal(e),
                };
                hls_url = Some(format!("/api/v1/hls/{job}/master.m3u8"));
            }
            _ => {
                stream_url = Some(format!("/api/v1/stream/{}/remux", target.file.id));
            }
        }
    }

    let url = if d.strategy == PlaybackStrategy::DirectPlay {
        Some(format!("/api/v1/stream/{}", target.file.id))
    } else {
        None
    };

    let subtitles = d
        .subtitle_plans
        .iter()
        .map(|p| SubtitleView {
            track_id: p.track_id.clone(),
            language: p.language.clone(),
            mode: format!("{:?}", p.mode).to_ascii_lowercase(),
            url: subtitle_url(target.file.id, &p.track_id),
        })
        .collect();

    (
        StatusCode::CREATED,
        Json(SessionView {
            session_id: session.id,
            strategy: d.strategy,
            reason: d.summary(),
            reasons: d
                .reasons
                .iter()
                .map(|r| ReasonView {
                    code: r.code().to_string(),
                    message: r.message(),
                })
                .collect(),
            url,
            stream_url,
            hls_url,
            audio_index: d.audio_index,
            subtitles,
        }),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HeartbeatBody {
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/playback/sessions/{id}/heartbeat",
        params(
        ("id" = String, Path, description = "会话 id"),
        ),
        request_body = Option<HeartbeatBody>,
        responses(
            (status = 200, description = "成功", body = AckView),
            (status = 404, description = "会话不存在", body = crate::ErrorBody),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn session_heartbeat(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Id>,
    Json(body): Json<HeartbeatBody>,
) -> Response {
    let sessions = match state.sessions.clone() {
        Some(s) => s,
        None => return unavailable("播放会话未初始化"),
    };
    match sessions
        .heartbeat(id, body.position_ms, body.duration_ms)
        .await
    {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/playback/sessions/{id}/stop",
        params(
        ("id" = String, Path, description = "会话 id"),
        ),
        request_body = Option<HeartbeatBody>,
        responses(
            (status = 200, description = "成功", body = AckView),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn session_stop(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<Id>,
    Json(body): Json<Option<HeartbeatBody>>,
) -> Response {
    let sessions = match state.sessions.clone() {
        Some(s) => s,
        None => return unavailable("播放会话未初始化"),
    };
    let pos = body.map(|b| b.position_ms);
    match sessions.stop(id, pos).await {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ResumeParams {
    #[serde(default = "resume_limit")]
    pub limit: u32,
}

fn resume_limit() -> u32 {
    20
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/playback/resume",
        params(
        ("limit" = Option<u32>, Query, description = "默认 20"),
        ),
        responses(
            (status = 200, description = "续播点", body = Vec<whatmedia_database::repo::ResumePoint>),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn resume_list(State(state): State<AppState>, Query(q): Query<ResumeParams>) -> Response {
    let sessions = match state.sessions.clone() {
        Some(s) => s,
        None => return unavailable("播放会话未初始化"),
    };
    match sessions.resume(None, q.limit.clamp(1, 100)).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => internal(e),
    }
}

// ─────────────────────────────────────────────────────────────
// 直出与流式
// ─────────────────────────────────────────────────────────────

/// `GET /api/v1/stream/{file_id}` —— Direct Play，支持 HTTP Range。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/stream/{id}",
        params(
        ("id" = String, Path, description = "文件 id"),
        ),
        responses(
            (status = 200, description = "原始文件，支持 HTTP Range", content_type = "video/*", body = Vec<u8>),
            (status = 206, description = "Range 区间", content_type = "video/*", body = Vec<u8>),
            (status = 404, description = "文件不存在", body = crate::ErrorBody),
            (status = 416, description = "Range 越界", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn stream_direct(
    State(state): State<AppState>,
    AxumPath(file_id): AxumPath<Id>,
    headers: HeaderMap,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let target = match resolve_file(&db, None, Some(file_id)).await {
        Ok(Some(t)) => t,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "文件不存在"),
        Err(e) => return internal(e),
    };

    let storage = match state.storage_for(&target.library_root) {
        Ok(s) => s,
        Err(r) => return *r,
    };

    let total = target.file.size_bytes.max(0) as u64;
    let resolved = match storage.resolve_path(&target.file.path) {
        Ok(p) => p,
        Err(e) => return bad_request(e.to_string()),
    };
    // 以磁盘上的实际大小为准：数据库里的 size 可能是上次扫描时记下的旧值。
    let total = std::fs::metadata(&resolved)
        .map(|m| m.len())
        .unwrap_or(total);

    let ctype = content_type(target.file.container.as_ref());

    let range = match parse_range_header(
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        total,
    ) {
        Ok(r) => r,
        Err(()) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            )
                .into_response()
        }
    };

    let (reader, len) = match storage
        .open_range(&target.file.path, range.unwrap_or(ByteRange::new(0, None)))
        .await
    {
        Ok(v) => v,
        Err(e) => return internal(e),
    };

    let stream = ReaderStream::new(reader);
    let body = Body::from_stream(stream);

    match range {
        Some(r) => {
            let end = r
                .end
                .unwrap_or(total.saturating_sub(1))
                .min(total.saturating_sub(1));
            (
                StatusCode::PARTIAL_CONTENT,
                [
                    (header::CONTENT_TYPE, ctype),
                    (header::CONTENT_LENGTH, len.to_string()),
                    (header::ACCEPT_RANGES, "bytes".to_string()),
                    (
                        header::CONTENT_RANGE,
                        format!("bytes {}-{}/{}", r.start, end, total),
                    ),
                ],
                body,
            )
                .into_response()
        }
        None => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, ctype),
                (header::CONTENT_LENGTH, total.to_string()),
                (header::ACCEPT_RANGES, "bytes".to_string()),
            ],
            body,
        )
            .into_response(),
    }
}

/// `GET /api/v1/stream/{file_id}/remux` —— remux 流式输出（MPEG-TS）。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/stream/{id}/remux",
        params(
        ("id" = String, Path, description = "文件 id"),
        ("client" = Option<String>, Query),
        ("start_ms" = Option<i64>, Query),
        ("network_kbps" = Option<u32>, Query),
        ),
        responses(
            (status = 200, description = "重封装后的流（不重编码）", content_type = "video/*", body = Vec<u8>),
            (status = 404, description = "文件不存在", body = crate::ErrorBody),
            (status = 501, description = "服务端没有 ffmpeg", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn stream_remux(
    State(state): State<AppState>,
    AxumPath(file_id): AxumPath<Id>,
    Query(q): Query<RemuxParams>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let target = match resolve_file(&db, None, Some(file_id)).await {
        Ok(Some(t)) => t,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "文件不存在"),
        Err(e) => return internal(e),
    };

    let Some(ffmpeg) = state.transcode.ffmpeg.clone() else {
        return error_response(
            StatusCode::NOT_IMPLEMENTED,
            "FFMPEG_UNAVAILABLE",
            "服务端没有可用的 ffmpeg，无法流式输出",
        );
    };

    let client = ClientCapability::by_id(&q.client);
    let media = match build_media(&state, &db, &target).await {
        Ok(m) => m,
        Err(e) => return internal(e),
    };
    let d = decide(DecisionInput {
        media: &media,
        client: &client,
        server: &state.transcode.server_capability(),
        network_kbps: q.network_kbps,
        start_ms: q.start_ms,
        hls_dir: None,
        seg_seconds: 6,
    });

    let mut req = match d.stream.clone() {
        Some(r) => r,
        None => {
            // 决策说可以直出，但客户端还是来要流式 —— 按 remux 兜底。
            StreamRequest {
                input: target.file.path.clone(),
                seek_ms: q.start_ms,
                video: whatmedia_transcoder::VideoAction::Copy,
                audio: whatmedia_transcoder::AudioAction::Copy { index: None },
                burn: None,
                output: OutputSpec::PipeTs,
                accel: None,
            }
        }
    };
    req.input = target.file.path.clone();
    req.output = OutputSpec::PipeTs;

    // 播放是 Critical：用户正在等。
    let _slot = state.transcode.pool.acquire(TaskPriority::Critical).await;
    state.transcode.active.fetch_add(1, Ordering::SeqCst);

    let mut proc = match ffmpeg.spawn(build_args(&req)) {
        Ok(p) => p,
        Err(e) => {
            state.transcode.active.fetch_sub(1, Ordering::SeqCst);
            return internal(e);
        }
    };
    let Some(stdout) = proc.take_stdout() else {
        state.transcode.active.fetch_sub(1, Ordering::SeqCst);
        return internal("ffmpeg 没有可用的 stdout");
    };

    // ffmpeg 结束或客户端断开后要把 active 减回去。
    let active = state.transcode.active.clone();
    tokio::spawn(async move {
        let _ = proc.wait_with_timeout(None).await;
        active.fetch_sub(1, Ordering::SeqCst);
    });

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "video/mp2t".to_string()),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        Body::from_stream(ReaderStream::new(stdout)),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RemuxParams {
    #[serde(default = "default_client")]
    pub client: String,
    #[serde(default)]
    pub start_ms: i64,
    pub network_kbps: Option<u32>,
}

// ─────────────────────────────────────────────────────────────
// HLS
// ─────────────────────────────────────────────────────────────

/// 启动一个 HLS 输出任务。目录由 `TempDirGuard` 持有，
/// ffmpeg 进程一结束（正常完成、失败、被取消）目录就被删掉。
async fn start_hls(
    state: &AppState,
    target: &FileTarget,
    req: StreamRequest,
    session_id: Id,
) -> anyhow::Result<Id> {
    let Some(ffmpeg) = state.transcode.ffmpeg.clone() else {
        return Err(anyhow::anyhow!("ffmpeg 不可用"));
    };
    let job_id = Id::new_v4();
    let dir = state.transcode.hls_root.join(job_id.to_string());

    let guard = TempDirGuard::create(&dir)?;
    let args = build_args(&req);
    let cancel = Arc::new(AtomicBool::new(false));

    if let Some(db) = state.db.clone() {
        let _ = whatmedia_database::repo::insert_stream_session(
            &db,
            whatmedia_database::repo::NewStreamSession {
                job_id,
                file_id: target.file.id,
                session_id: Some(session_id),
                kind: "hls".into(),
                strategy: format!("{:?}", PlaybackStrategy::Transcode).to_ascii_lowercase(),
                output_dir: dir.to_string_lossy().to_string(),
                args: args.clone(),
            },
        )
        .await;
    }

    state.transcode.streams.insert(StreamHandle {
        job_id,
        file_id: target.file.id,
        dir: dir.clone(),
        kind: "hls".into(),
        cancel: cancel.clone(),
    });

    let registry = state.transcode.streams.clone();
    let active = state.transcode.active.clone();
    let db = state.db.clone();
    let pool = state.transcode.pool.clone();

    tokio::spawn(async move {
        // TempDirGuard 进任务：进程结束（含被取消）立即清理分片。
        let _guard = guard;
        let _slot = pool.acquire(TaskPriority::Critical).await;
        active.fetch_add(1, Ordering::SeqCst);

        let mut proc = match ffmpeg.spawn(args) {
            Ok(p) => p,
            Err(_) => {
                active.fetch_sub(1, Ordering::SeqCst);
                if let Some(db) = db.as_ref() {
                    let _ = whatmedia_database::repo::set_stream_state(db, job_id, "failed").await;
                }
                registry.remove(job_id);
                return;
            }
        };
        let token = proc.cancel_token();

        // 一边等进程，一边响应取消。
        let waiter = tokio::spawn(async move { proc.wait_with_timeout(None).await });
        loop {
            if cancel.load(Ordering::SeqCst) {
                token.store(true, Ordering::SeqCst);
                break;
            }
            if waiter.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let exit = waiter
            .await
            .unwrap_or(Ok(whatmedia_transcoder::Exit::Cancelled));

        active.fetch_sub(1, Ordering::SeqCst);
        if let Some(db) = db.as_ref() {
            let st = match exit {
                Ok(whatmedia_transcoder::Exit::Completed) => "finished",
                Ok(whatmedia_transcoder::Exit::Cancelled) => "cancelled",
                _ => "failed",
            };
            let _ = whatmedia_database::repo::set_stream_state(db, job_id, st).await;
        }
        registry.remove(job_id);
    });

    Ok(job_id)
}

/// `GET /api/v1/hls/{job}/master.m3u8`
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/hls/{job}/master.m3u8",
        params(
        ("job" = String, Path, description = "转码任务 id"),
        ),
        responses(
            (status = 200, description = "HLS 主播放列表", content_type = "application/vnd.apple.mpegurl", body = String),
            (status = 404, description = "转码任务不存在", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn hls_master(State(state): State<AppState>, AxumPath(job): AxumPath<Id>) -> Response {
    let Some(h) = state.transcode.streams.get(job) else {
        return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "HLS 任务不存在或已结束");
    };
    if let Some(db) = state.db.clone() {
        let _ = whatmedia_database::repo::touch_stream_session(&db, job).await;
    }

    // 单码率也必须有 master：将来加 ABR 时客户端不用改代码。
    let bandwidth = 8_000_000u64;
    let body = master_playlist("index.m3u8", bandwidth, None, None, "avc1.640029,mp4a.40.2");
    let _ = &h;
    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.apple.mpegurl".to_string(),
            ),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        body,
    )
        .into_response()
}

/// `GET /api/v1/hls/{job}/index.m3u8` 与分片。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/hls/{job}/{name}",
        params(
        ("job" = String, Path, description = "转码任务 id"),
        ("name" = String, Path, description = "分片文件名"),
        ),
        responses(
            (status = 200, description = "分片或媒体播放列表", content_type = "video/mp2t", body = Vec<u8>),
            (status = 404, description = "分片不存在", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn hls_file(
    State(state): State<AppState>,
    AxumPath((job, name)): AxumPath<(Id, String)>,
) -> Response {
    let Some(h) = state.transcode.streams.get(job) else {
        return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "HLS 任务不存在或已结束");
    };
    if let Some(db) = state.db.clone() {
        let _ = whatmedia_database::repo::touch_stream_session(&db, job).await;
    }

    // 只允许 m3u8 / ts，且不允许路径穿越。
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return bad_request("非法文件名");
    }
    let ext = std::path::Path::new(&name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let ctype = match ext {
        "m3u8" => "application/vnd.apple.mpegurl",
        "ts" => "video/mp2t",
        _ => return bad_request("不支持的文件类型"),
    };

    let path = h.dir.join(&name);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, ctype.to_string()),
                (header::CACHE_CONTROL, "no-cache".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => {
            // ffmpeg 还没写出来。hls.js 会重试，这里返回 404 是协议内的行为。
            error_response(StatusCode::NOT_FOUND, "NOT_READY", "分片尚未生成")
        }
    }
}

/// `POST /api/v1/hls/{job}/stop` —— 停止并清理。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/hls/{job}/stop",
        params(
        ("job" = String, Path, description = "转码任务 id"),
        ),
        responses(
            (status = 200, description = "成功", body = AckView),
            (status = 404, description = "任务不存在", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn hls_stop(State(state): State<AppState>, AxumPath(job): AxumPath<Id>) -> Response {
    let Some(h) = state.transcode.streams.stop(job) else {
        return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "HLS 任务不存在");
    };
    if let Some(db) = state.db.clone() {
        let _ = whatmedia_database::repo::set_stream_state(&db, job, "cancelled").await;
        let _ = whatmedia_database::repo::delete_stream_session(&db, job).await;
    }
    Json(serde_json::json!({ "ok": true, "job": h.job_id })).into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/playback/streams",
        responses(
            (status = 200, description = "在途流式输出", body = Vec<whatmedia_database::repo::StreamSession>),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn list_streams(State(state): State<AppState>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    match whatmedia_database::repo::list_idle_stream_sessions(&db, -1).await {
        Ok(v) => {
            let views: Vec<StreamSession> = v;
            Json(views).into_response()
        }
        Err(e) => internal(e),
    }
}

// ─────────────────────────────────────────────────────────────
// 字幕
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SubtitleItemView {
    pub track_id: String,
    pub codec: String,
    pub language: Option<String>,
    pub forced: bool,
    pub name: Option<String>,
    pub url: Option<String>,
}

/// `GET /api/v1/items/{id}/subtitles`
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/items/{id}/subtitles",
        params(
        ("id" = String, Path, description = "条目 id"),
        ),
        responses(
            (status = 200, description = "内嵌 + 外挂字幕", body = Vec<SubtitleItemView>),
            (status = 404, description = "条目下没有文件", body = crate::ErrorBody),
            (status = 503, description = "数据库未就绪", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn item_subtitles(State(state): State<AppState>, AxumPath(id): AxumPath<Id>) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };
    let files = match whatmedia_database::repo::files_for_item(&db, id).await {
        Ok(f) => f,
        Err(e) => return internal(e),
    };
    let Some((file, streams)) = files.into_iter().next() else {
        return error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "条目下没有文件");
    };

    let external = match external_for(&state, &db, &file.path).await {
        Ok(v) => v,
        Err(e) => return internal(e),
    };

    let mut out: Vec<SubtitleItemView> = Vec::new();
    for s in streams
        .iter()
        .filter(|s| matches!(s.stream_type, whatmedia_core::StreamType::Subtitle))
    {
        out.push(SubtitleItemView {
            track_id: format!("internal:{}", s.index),
            codec: s.codec.clone().unwrap_or_default(),
            language: s.language.clone(),
            forced: s.is_forced,
            name: s.title.clone(),
            url: subtitle_url(file.id, &format!("internal:{}", s.index)),
        });
    }
    for e in external {
        out.push(SubtitleItemView {
            track_id: format!("external:{}", e.path),
            codec: format!("{:?}", e.codec).to_ascii_lowercase(),
            language: e.language.clone(),
            forced: e.forced,
            name: Some(e.name.clone()),
            url: None,
        });
    }
    Json(out).into_response()
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SubtitleConvertBody {
    pub path: String,
    #[serde(default = "default_to")]
    pub to: String,
}

fn default_to() -> String {
    "vtt".to_string()
}

/// `POST /api/v1/subtitles/external` —— 转换并输出外挂字幕。
///
/// 路径走 POST + JSON 而不是 GET 参数：字幕路径含冒号与反斜杠，
/// 塞在 URL 里既难编码又容易被网关改写。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/subtitles/external",
        request_body = SubtitleConvertBody,
        responses(
            (status = 200, description = "转换后的字幕（默认 text/vtt）", content_type = "text/vtt", body = String),
            (status = 404, description = "字幕不存在或不在库内", body = crate::ErrorBody),
        ),
        tag = "playback"
    )
)]
pub async fn external_subtitle(
    State(state): State<AppState>,
    Json(body): Json<SubtitleConvertBody>,
) -> Response {
    let db = match state.db_or_503() {
        Ok(d) => d,
        Err(r) => return *r,
    };

    // 路径必须落在某个媒体库根内 —— 绝不直接 open 用户给的路径。
    let storage = match storage_for_path(&state, &db, &body.path).await {
        Ok(Some(s)) => s,
        Ok(None) => return bad_request("字幕文件不在任何媒体库内"),
        Err(r) => return *r,
    };

    let bytes: Vec<u8> = match storage.read(&body.path).await {
        Ok(b) => b,
        Err(e) => return bad_request(e.to_string()),
    };
    let text = String::from_utf8_lossy(&bytes);

    let ext = std::path::Path::new(&body.path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let cues = match ext.as_str() {
        "srt" => whatmedia_subtitle::parse_srt(&text),
        "vtt" => whatmedia_subtitle::parse_vtt(&text),
        "ass" | "ssa" => whatmedia_subtitle::parse_ass(&text),
        other => return bad_request(format!("不支持的字幕格式：{other}")),
    };

    let out = if body.to.eq_ignore_ascii_case("srt") {
        (whatmedia_subtitle::to_srt(&cues), "text/plain; charset=utf-8")
    } else {
        (whatmedia_subtitle::to_vtt(&cues), "text/vtt; charset=utf-8")
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, out.1.to_string())],
        out.0,
    )
        .into_response()
}

// ─────────────────────────────────────────────────────────────
// 硬件
// ─────────────────────────────────────────────────────────────

/// 只回一个确认的空信封。
///
/// 心跳 / 停止这类端点没有数据要回，但**必须回一个 body** ——
/// `fetch` 对空响应体的 204 与 200 处理不一样，前端容易写错。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AckView {
    pub ok: bool,
}

/// 硬件探测结果。
///
/// `profiles` 是各加速后端的自述（VAAPI / NVENC / QSV / RKMPP / ...），
/// 没有 ffmpeg 时为空数组 —— 这时服务端只能 Direct Play + Remux。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HardwareProfileView {
    pub ffmpeg: Option<String>,
    pub profiles: Vec<HardwareProfile>,
    pub active_transcodes: usize,
    pub capacity: usize,
    pub streams: usize,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/hardware/profile",
        responses(
            (status = 200, description = "硬件与转码能力", body = HardwareProfileView),
        ),
        tag = "playback"
    )
)]
pub async fn hardware_profile(State(state): State<AppState>) -> Response {
    Json(HardwareProfileView {
        ffmpeg: state.transcode.ffmpeg.as_ref().map(|f| f.bin().to_string()),
        profiles: state.transcode.profiles.clone(),
        active_transcodes: state.transcode.active.load(Ordering::SeqCst),
        capacity: state.transcode.pool.capacity(),
        streams: state.transcode.streams.len(),
    })
    .into_response()
}

// ─────────────────────────────────────────────────────────────
// 内部工具
// ─────────────────────────────────────────────────────────────

/// 定位要播的文件：`file_id` 优先，否则取条目的第一个可用文件。
async fn resolve_file(
    db: &whatmedia_database::Db,
    item_id: Option<Id>,
    file_id: Option<Id>,
) -> anyhow::Result<Option<FileTarget>> {
    if let Some(f) = file_id {
        return whatmedia_database::repo::file_target(db, f).await;
    }
    let Some(item) = item_id else {
        return Ok(None);
    };
    let files = whatmedia_database::repo::files_for_item(db, item).await?;
    let Some((file, _)) = files.into_iter().find(|(f, _)| f.is_available) else {
        return Ok(None);
    };
    let mut t = whatmedia_database::repo::file_target(db, file.id).await?;
    if let Some(t) = t.as_mut() {
        t.title = String::new();
    }
    Ok(t)
}

/// 构造片源画像（含外挂字幕发现）。
async fn build_media(
    state: &AppState,
    db: &whatmedia_database::Db,
    target: &FileTarget,
) -> anyhow::Result<MediaCapability> {
    let streams: Vec<MediaStream> = whatmedia_database::repo::files_for_item(db, target.item_id)
        .await?
        .into_iter()
        .find(|(f, _)| f.id == target.file.id)
        .map(|(_, s)| s)
        .unwrap_or_default();

    let external = external_for(state, db, &target.file.path)
        .await
        .unwrap_or_default();

    Ok(MediaCapability::from_parts(
        target.file.container.clone(),
        target.file.duration_ms,
        target.file.bitrate.map(|b| (b / 1000) as u32),
        &streams,
        external,
    ))
}

async fn external_for(
    state: &AppState,
    db: &whatmedia_database::Db,
    media_path: &str,
) -> anyhow::Result<Vec<ExternalSubtitle>> {
    let storage = match storage_for_path(state, db, media_path).await {
        Ok(Some(s)) => s,
        Ok(None) => return Ok(Vec::new()),
        Err(r) => return Err(anyhow::anyhow!("storage 解析失败（HTTP {}）", r.status())),
    };
    Ok(discover_sidecars(storage.as_ref(), media_path)
        .await
        .unwrap_or_default())
}

/// 找出路径所属的媒体库 → 对应的 StorageProvider。
async fn storage_for_path(
    state: &AppState,
    db: &whatmedia_database::Db,
    path: &str,
) -> anyhow::Result<Option<Arc<whatmedia_storage::LocalStorage>>, Box<Response>> {
    let libs = match whatmedia_database::repo::list_libraries(db).await {
        Ok(l) => l,
        Err(e) => return Err(Box::new(internal(e))),
    };
    let p = std::path::Path::new(path);
    for lib in libs {
        let root = std::path::Path::new(&lib.root_path);
        if p.starts_with(root) {
            return state.storage_for(&lib.root_path).map(Some);
        }
    }
    Ok(None)
}

/// 回收上次运行遗留的流式输出目录。
///
/// **为什么必须有**：进程被 `kill -9` 或容器被强制重启时，
/// 正在写的 HLS 分片目录不会有人清理。放任不管的话 /cache 会一路涨到磁盘满。
///
/// 判据是"数据库里登记的目录"，不扫描整个 cache 目录 ——
/// 后者会把别的实例正在用的目录也删掉（多容器共挂一个卷时尤其危险）。
pub async fn reap_orphan_streams(
    db: &whatmedia_database::Db,
    hls_root: &std::path::Path,
) -> anyhow::Result<usize> {
    let orphans = whatmedia_database::repo::list_idle_stream_sessions(db, -1).await?;
    let mut n = 0;
    for s in orphans {
        let dir = std::path::Path::new(&s.output_dir);
        // 只删 hls_root 之下的目录：登记表被人改过时也不能删到别的地方去。
        if dir.starts_with(hls_root) {
            let _ = std::fs::remove_dir_all(dir);
            n += 1;
        }
        let _ = whatmedia_database::repo::set_stream_state(db, s.job_id, "cancelled").await;
        let _ = whatmedia_database::repo::delete_stream_session(db, s.job_id).await;
    }
    Ok(n)
}

/// 回收"很久没被访问"的在途流式输出。供周期性任务调用。
pub async fn reap_idle_streams(state: &AppState, idle_secs: i64) -> anyhow::Result<usize> {
    let Some(db) = state.db.clone() else {
        return Ok(0);
    };
    let idle = whatmedia_database::repo::list_idle_stream_sessions(&db, idle_secs).await?;
    let mut n = 0;
    for s in idle {
        // 先取消（让 ffmpeg 停下来），再删目录，最后注销。
        if let Some(h) = state.transcode.streams.stop(s.job_id) {
            let _ = std::fs::remove_dir_all(&h.dir);
            n += 1;
        }
        let _ = whatmedia_database::repo::delete_stream_session(&db, s.job_id).await;
    }
    Ok(n)
}

fn hls_dir_for(state: &AppState, file_id: Id) -> std::path::PathBuf {
    state.transcode.hls_root.join(file_id.to_string())
}

fn subtitle_url(file_id: Id, track_id: &str) -> Option<String> {
    // 外挂字幕走 POST（路径含特殊字符），这里只给内嵌轨生成 GET 链接。
    let idx = track_id.strip_prefix("internal:")?;
    Some(format!("/api/v1/stream/{file_id}/subtitle/{idx}"))
}

fn content_type(c: Option<&Container>) -> String {
    match c {
        Some(Container::Mp4) => "video/mp4",
        Some(Container::Mkv) => "video/x-matroska",
        Some(Container::Ts) | Some(Container::M2ts) => "video/mp2t",
        Some(Container::Webm) => "video/webm",
        Some(Container::Avi) => "video/x-msvideo",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// 解析 `Range: bytes=start-end`。
///
/// 返回 `Ok(None)` 表示没有 Range 头（正常 200）；
/// `Err(())` 表示格式非法或越界（416）。
fn parse_range_header(v: Option<&str>, total: u64) -> Result<Option<ByteRange>, ()> {
    let Some(raw) = v else { return Ok(None) };
    let rest = raw.strip_prefix("bytes=").ok_or(())?;
    // 多段 Range（逗号分隔）不支持 —— 媒体播放不会用它，支持了反而要处理 multipart。
    if rest.contains(',') {
        return Err(());
    }
    let (start_s, end_s) = match rest.split_once('-') {
        Some(p) => p,
        None => return Err(()),
    };

    if start_s.is_empty() {
        // `bytes=-500` 表示最后 500 字节。
        let suffix: u64 = end_s.parse().map_err(|_| ())?;
        if suffix == 0 || suffix > total {
            return Err(());
        }
        return Ok(Some(ByteRange::new(
            total.saturating_sub(suffix),
            Some(total - 1),
        )));
    }

    let start: u64 = start_s.parse().map_err(|_| ())?;
    if start >= total && total > 0 {
        return Err(());
    }
    let end = if end_s.is_empty() {
        None
    } else {
        let e: u64 = end_s.parse().map_err(|_| ())?;
        if e < start || e >= total {
            return Err(());
        }
        Some(e)
    };
    Ok(Some(ByteRange::new(start, end)))
}

#[allow(dead_code)]
fn now() -> chrono::DateTime<Utc> {
    Utc::now()
}

#[allow(dead_code)]
fn new_id() -> Uuid {
    Uuid::new_v4()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_header_parsing() {
        assert_eq!(parse_range_header(None, 1000), Ok(None));
        assert_eq!(
            parse_range_header(Some("bytes=0-99"), 1000),
            Ok(Some(ByteRange::new(0, Some(99))))
        );
        assert_eq!(
            parse_range_header(Some("bytes=500-"), 1000),
            Ok(Some(ByteRange::new(500, None)))
        );
        // 尾部区间
        assert_eq!(
            parse_range_header(Some("bytes=-500"), 1000),
            Ok(Some(ByteRange::new(500, Some(999))))
        );
    }

    #[test]
    fn malformed_or_out_of_bounds_range_is_416() {
        assert!(parse_range_header(Some("bytes=0-2000"), 1000).is_err());
        assert!(parse_range_header(Some("bytes=1000-"), 1000).is_err());
        assert!(parse_range_header(Some("bytes=10-5"), 1000).is_err());
        assert!(parse_range_header(Some("items=0-10"), 1000).is_err());
        assert!(parse_range_header(Some("bytes=0-9,20-29"), 1000).is_err());
        assert!(parse_range_header(Some("bytes=abc-def"), 1000).is_err());
    }

    #[test]
    fn content_type_by_container() {
        assert_eq!(content_type(Some(&Container::Mkv)), "video/x-matroska");
        assert_eq!(content_type(Some(&Container::Mp4)), "video/mp4");
        assert_eq!(content_type(None), "application/octet-stream");
    }

    #[test]
    fn subtitle_url_only_for_internal_tracks() {
        let id = Id::new_v4();
        assert_eq!(
            subtitle_url(id, "internal:3"),
            Some(format!("/api/v1/stream/{id}/subtitle/3"))
        );
        // 外挂字幕路径含冒号与反斜杠，不能进 URL。
        assert_eq!(subtitle_url(id, "external:/a b/c.srt"), None);
    }

    #[tokio::test]
    async fn registry_stop_cancels_and_removes() {
        let reg = StreamRegistry::default();
        let h = StreamHandle {
            job_id: Id::new_v4(),
            file_id: Id::new_v4(),
            dir: std::path::PathBuf::from("/tmp/x"),
            kind: "hls".into(),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        let flag = h.cancel.clone();
        reg.insert(h.clone());
        assert_eq!(reg.len(), 1);
        let stopped = reg.stop(h.job_id).expect("应当存在");
        assert!(flag.load(Ordering::SeqCst));
        assert_eq!(stopped.job_id, h.job_id);
        assert!(reg.is_empty());
    }

    #[test]
    fn server_capability_reflects_ffmpeg_presence() {
        let rt = TranscodeRuntime::new(None, Vec::new(), std::path::PathBuf::from("/tmp"));
        let c = rt.server_capability();
        assert!(!c.ffmpeg_available);
        assert!(!c.has_budget());
        assert_eq!(rt.pool.capacity(), 1);
    }
}
