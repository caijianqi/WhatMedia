//! AI 助手的 HTTP 端点与真实宿主实现（P7）。
//!
//! # 这一层做什么
//!
//! `crates/ai` 里只有契约与编排，它**故意不认识数据库**。
//! 真正把"AI 的问题"翻译成"SQL 查询"的活在这里：
//!
//! ```text
//!   HTTP ──► ai.rs handler ──► AiRuntime ──► Tool ──► DbHost（本文件）──► SQL
//!                                  │                      └── 唯一会写库的地方
//!                                  └── 危险工具在这里停下，等用户确认
//! ```
//!
//! # 三条不能破的规矩
//!
//! 1. **模型不可用时降级，不报错。** 只有"整个 AI 开关没开"才返回 503，
//!    其余一律走规则引擎并在 `degraded` 里如实标注。
//!    用户问"有没有缺海报的电影"，本地没跑模型时答得出来才是真的有用。
//! 2. **危险操作必须两步。** `/ai/chat` 只会返回一张待确认工单（`pending`），
//!    真正执行要拿令牌打 `/ai/confirm`。没有"一次请求直接生效"的捷径。
//! 3. **AI 挂了不能连累核心。** 端点失败只影响 `/api/v1/ai/*`，
//!    `/api/v1/media` 等端点与 AI 完全不共享状态。

use async_trait::async_trait;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::QueryBuilder;
use std::sync::Arc;
use uuid::Uuid;
use whatmedia_ai::host::duplicate_key;
use whatmedia_ai::{
    build_provider, Actor, ActorRole, AiError, AiHost, AiRuntime, CapabilityReport, DuplicateGroup,
    IdentifyOutcome, MediaHit, MediaQuery, MissingKind, PendingAction, RecommendSpec, ScanTicket,
    SortKey, ToolSpec, WatchState,
};
// 排序用 `crates/ai` 那一份，不在这里另起炉灶（理由见 `whatmedia_ai::sort_hits` 的注释）。
use whatmedia_ai::sort_hits;
use whatmedia_core::{Id, MediaKind, PlaybackStrategy};
use whatmedia_database::Db;
use whatmedia_playback::{
    decide, ClientCapability, DecisionInput, MediaCapability, ServerCapability,
};

use crate::{bad_request, error_response, unavailable, AppState};

// ─────────────────────────────────────────────────────────────
// 契约视图
// ─────────────────────────────────────────────────────────────
//
// 刻意不把 `PendingAction` / `ToolTrace` 原样导出：它们带 `serde_json::Value`
// 字段，直接进 OpenAPI 会退化成 `any`，前端等于拿不到类型。
// 这里把它们压平成"前端真正用得上的那几个字段"。

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiChatRequest {
    /// 用户的问题。空串会被直接拒掉。
    pub message: String,
    /// `owner` / `administrator` / `adult` / `child` / `guest`。缺省 adult。
    #[serde(default)]
    pub role: Option<String>,
    /// 操作者 id。待确认令牌与它绑定，换人执行会被拒。
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiConfirmRequest {
    pub token: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AiChatReply {
    pub answer: String,
    /// 路由到的 Agent：`media` / `library` / `subtitle` / `maintenance` / `recommendation`。
    pub agent: String,
    pub pending: Option<PendingActionView>,
    pub traces: Vec<ToolTraceView>,
    /// true 表示这次是靠规则引擎答的（模型没配 / 连不上）。
    /// 前端据此显示"离线模式"角标，别让用户以为模型真懂他的话。
    pub degraded: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PendingActionView {
    pub id: String,
    /// 一次性令牌。**不要**把它当成身份凭证：它只证明"用户看过这份预览"。
    pub token: String,
    pub tool: String,
    /// `read_only` / `write` / `destructive`。
    pub risk: String,
    pub summary: String,
    pub lines: Vec<String>,
    pub warnings: Vec<String>,
    /// 剩余秒数。前端做倒计时用，到 0 令牌即失效。
    pub expires_in_secs: i64,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolTraceView {
    pub tool: String,
    pub ok: bool,
    pub summary: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolSpecView {
    pub name: String,
    pub description: String,
    pub risk: String,
    pub params: Vec<ToolParamView>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolParamView {
    pub name: String,
    pub kind: String,
    pub description: String,
    pub required: bool,
}

/// 列表端点统一走 `{ data: [...] }`，与项目其余部分一致。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ToolListReply {
    pub data: Vec<ToolSpecView>,
}

fn reply_of(r: whatmedia_ai::TurnResult) -> AiChatReply {
    let now = Utc::now();
    AiChatReply {
        answer: r.answer,
        agent: r.agent.as_str().to_string(),
        pending: r
            .pending
            .as_ref()
            .map(|p: &PendingAction| PendingActionView {
                expires_in_secs: p.seconds_left(now),
                id: p.id.clone(),
                token: p.token.clone(),
                tool: p.tool.clone(),
                risk: p.risk.as_str().to_string(),
                summary: p.preview.summary.clone(),
                lines: p
                    .preview
                    .lines
                    .iter()
                    .map(|l| match &l.detail {
                        Some(d) => format!("{}：{}（{}）", l.target, l.action, d),
                        None => format!("{}：{}", l.target, l.action),
                    })
                    .collect(),
                warnings: p.preview.warnings.clone(),
            }),
        traces: r
            .traces
            .iter()
            .map(|t| ToolTraceView {
                tool: t.tool.clone(),
                ok: t.ok,
                summary: t.summary.clone(),
            })
            .collect(),
        degraded: r.degraded,
    }
}

fn actor_of(role: Option<&str>, user_id: Option<&str>) -> Actor {
    let role = role.map(ActorRole::parse).unwrap_or(ActorRole::Adult);
    let id = user_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("anonymous")
        .to_string();
    Actor::new(id, role)
}

// ─────────────────────────────────────────────────────────────
// 端点
// ─────────────────────────────────────────────────────────────

/// 取运行时。**AI 未启用**与**运行时没装配**是两个不同的 503，
/// 前端提示语不一样：前者是"去设置里打开"，后者是"服务还没起来"。
fn runtime_or_503(state: &AppState) -> Result<Arc<AiRuntime>, Box<Response>> {
    if !state.config.ai.enabled {
        return Err(Box::new(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "AI_DISABLED",
            "AI 助手未启用（ai.enabled = false）",
        )));
    }
    state
        .ai
        .clone()
        .ok_or_else(|| Box::new(unavailable("AI 运行时尚未装配")))
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/ai/chat",
        request_body = AiChatRequest,
        responses(
            (status = 200, description = "回答。危险工具会在 `pending` 里返回待确认工单", body = AiChatReply),
            (status = 400, description = "问题为空", body = crate::ErrorBody),
            (status = 403, description = "角色无权执行该操作", body = crate::ErrorBody),
            (status = 503, description = "AI 未启用或运行时未装配", body = crate::ErrorBody),
        ),
        tag = "ai"
    )
)]
pub async fn chat(State(state): State<AppState>, Json(body): Json<AiChatRequest>) -> Response {
    if body.message.trim().is_empty() {
        return bad_request("message 不能为空");
    }
    let rt = match runtime_or_503(&state) {
        Ok(r) => r,
        Err(r) => return *r,
    };

    let actor = actor_of(body.role.as_deref(), body.user_id.as_deref());
    match rt.ask(&actor, &body.message).await {
        Ok(r) => (StatusCode::OK, Json(reply_of(r))).into_response(),
        Err(e) => ai_error_response(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/ai/confirm",
        request_body = AiConfirmRequest,
        responses(
            (status = 200, description = "已执行", body = AiChatReply),
            (status = 400, description = "令牌为空或参数被改过", body = crate::ErrorBody),
            (status = 403, description = "令牌属于另一个用户", body = crate::ErrorBody),
            (status = 409, description = "令牌已用过", body = crate::ErrorBody),
            (status = 410, description = "令牌已过期", body = crate::ErrorBody),
            (status = 503, description = "AI 未启用", body = crate::ErrorBody),
        ),
        tag = "ai"
    )
)]
pub async fn confirm(
    State(state): State<AppState>,
    Json(body): Json<AiConfirmRequest>,
) -> Response {
    if body.token.trim().is_empty() {
        return bad_request("token 不能为空");
    }
    let rt = match runtime_or_503(&state) {
        Ok(r) => r,
        Err(r) => return *r,
    };

    let actor = actor_of(body.role.as_deref(), body.user_id.as_deref());
    match rt.confirm(&actor, &body.token).await {
        Ok(r) => (StatusCode::OK, Json(reply_of(r))).into_response(),
        Err(e) => ai_error_response(e),
    }
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/ai/tools",
        responses(
            (status = 200, description = "当前可用的工具。危险工具未开启时不出现在列表里", body = ToolListReply),
            (status = 503, description = "AI 未启用", body = crate::ErrorBody),
        ),
        tag = "ai"
    )
)]
pub async fn list_tools(State(state): State<AppState>) -> Response {
    if !state.config.ai.enabled {
        return (StatusCode::OK, Json(ToolListReply { data: vec![] })).into_response();
    }
    let Some(rt) = state.ai.clone() else {
        return (StatusCode::OK, Json(ToolListReply { data: vec![] })).into_response();
    };
    let data = rt.tools().into_iter().map(tool_view).collect();
    (StatusCode::OK, Json(ToolListReply { data })).into_response()
}

fn tool_view(s: ToolSpec) -> ToolSpecView {
    ToolSpecView {
        name: s.name,
        description: s.description,
        risk: s.risk.as_str().to_string(),
        params: s
            .params
            .into_iter()
            .map(|p| ToolParamView {
                name: p.name,
                kind: p.kind.label().to_string(),
                description: p.description,
                required: p.required,
            })
            .collect(),
    }
}

/// 把 `AiError` 映射成 HTTP。
///
/// 状态码由 `AiErrorKind::status()` 单点定义，这里不再重复判断 ——
/// 两边各写一份迟早会漂移，而漂移的表现是"该 503 的给了 500"，
/// 前端据此做出的重试/降级决策就全错了。
fn ai_error_response(e: AiError) -> Response {
    let status = StatusCode::from_u16(e.kind.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    error_response(status, e.kind.code(), e.message)
}

// ─────────────────────────────────────────────────────────────
// 真实宿主
// ─────────────────────────────────────────────────────────────

/// 危险动作的执行器。
///
/// 单独抽成 trait 而不是让 `DbHost` 直接持有 `AppState`：
/// `AppState` 里有 `Arc<AiRuntime>`，而 `AiRuntime` 又持有 `Arc<dyn AiHost>` ——
/// 直接持有会形成引用环，谁都释放不掉。
#[async_trait]
pub trait AiActions: Send + Sync {
    async fn scan_library(&self, library: Option<Id>) -> Result<ScanTicket, AiError>;
    async fn identify_media(&self, item_id: Id) -> Result<IdentifyOutcome, AiError>;
}

/// 数据库宿主：把 `MediaQuery` 翻成 SQL。
pub struct DbHost {
    db: Arc<Db>,
    actions: Arc<dyn AiActions>,
    server: ServerCapability,
}

impl DbHost {
    pub fn new(db: Arc<Db>, actions: Arc<dyn AiActions>, server: ServerCapability) -> Self {
        Self {
            db,
            actions,
            server,
        }
    }
}

/// 一次查询最多扫多少个候选。
///
/// 这是**有意为之的上界**：AI 查询带一堆内存侧谓词（缺海报、重复、观看状态），
/// 把它们全下推成 SQL 会让这条查询变成维护噩梦。取一个足够大的窗口在
/// 内存里过滤，对家庭媒体库（几千到几万条）够用；真到了十万条量级，
/// 该做的是给这些谓词加索引 + 下推，而不是把这个常数调大。
const CANDIDATE_WINDOW: u32 = 2_000;

#[async_trait]
impl AiHost for DbHost {
    async fn search_media(&self, q: &MediaQuery) -> Result<Vec<MediaHit>, AiError> {
        let limit = q.limit.clamp(1, 200) as usize;
        let mut rows = fetch_window(&self.db, q).await?;
        attach_genres(&self.db, &mut rows).await?;

        let mut out: Vec<MediaHit> = rows.into_iter().filter(|h| matches_query(h, q)).collect();

        if q.duplicates_only {
            out = only_duplicates(out);
        }
        sort_hits(&mut out, q.sort);
        out.truncate(limit);
        Ok(out)
    }

    async fn get_media(&self, id: Id) -> Result<Option<MediaHit>, AiError> {
        let mut rows = fetch_one(&self.db, id).await?;
        attach_genres(&self.db, &mut rows).await?;
        Ok(rows.into_iter().next())
    }

    async fn duplicates(&self, limit: u32) -> Result<Vec<DuplicateGroup>, AiError> {
        let q = MediaQuery {
            limit: CANDIDATE_WINDOW,
            sort: Some(SortKey::TitleAsc),
            ..Default::default()
        };
        let mut rows = fetch_window(&self.db, &q).await?;
        attach_genres(&self.db, &mut rows).await?;

        let mut groups: Vec<DuplicateGroup> = Vec::new();
        for hit in rows {
            let key = duplicate_key(&hit.title, hit.year);
            if let Some(g) = groups.iter_mut().find(|g| g.key == key) {
                g.hits.push(hit);
            } else {
                groups.push(DuplicateGroup {
                    key,
                    title: hit.title.clone(),
                    year: hit.year,
                    hits: vec![hit],
                });
            }
        }
        // 只有一个版本的不是重复
        groups.retain(|g| g.hits.len() > 1);
        groups.truncate(limit.clamp(1, 200) as usize);
        Ok(groups)
    }

    async fn capability(
        &self,
        item_id: Id,
        client_id: Option<&str>,
    ) -> Result<CapabilityReport, AiError> {
        let detail = whatmedia_database::repo::get_item(&self.db, item_id)
            .await
            .map_err(|e| AiError::host(format!("读取条目失败：{e}")))?
            .ok_or_else(|| AiError::bad_args(format!("没有这个条目：{item_id}")))?;

        let files = whatmedia_database::repo::files_for_item(&self.db, item_id)
            .await
            .map_err(|e| AiError::host(format!("读取媒体文件失败：{e}")))?;

        let Some((file, streams)) = files.into_iter().find(|(f, _)| f.is_available) else {
            return Ok(CapabilityReport {
                item_id,
                title: detail.item.title.clone(),
                strategy: "unsupported".into(),
                reason: "没有可用文件".into(),
                container: None,
                video: None,
                audio: Vec::new(),
                subtitles: Vec::new(),
                warnings: vec!["该条目没有标记为可用的媒体文件，可能需要重新扫描".into()],
            });
        };

        let media = MediaCapability::from_parts(
            file.container.clone(),
            file.duration_ms,
            file.bitrate.map(|b| (b / 1000) as u32),
            &streams,
            Vec::new(),
        );
        let client = ClientCapability::by_id(client_id.unwrap_or("web"));
        let d = decide(DecisionInput {
            media: &media,
            client: &client,
            server: &self.server,
            network_kbps: None,
            start_ms: 0,
            hls_dir: None,
            seg_seconds: 6,
        });

        Ok(CapabilityReport {
            item_id,
            title: detail.item.title.clone(),
            strategy: strategy_str(d.strategy).to_string(),
            reason: d.summary(),
            container: file.container.as_ref().map(serde_str),
            video: media.video.as_ref().map(|v| serde_str(&v.codec)),
            audio: media.audio.iter().map(|a| serde_str(&a.codec)).collect(),
            subtitles: media
                .subtitles
                .iter()
                .map(|s| match &s.language {
                    Some(l) => format!("{}（{}）", serde_str(&s.codec), l),
                    None => serde_str(&s.codec),
                })
                .collect(),
            warnings: d.reasons.iter().map(|r| format!("{r:?}")).collect(),
        })
    }

    async fn recommend(&self, spec: &RecommendSpec) -> Result<Vec<MediaHit>, AiError> {
        let q = MediaQuery {
            kind: spec.kind,
            genres: spec.genres.clone(),
            runtime_max_ms: spec.max_runtime_ms,
            watch: if spec.exclude_watched {
                Some(WatchState::Unwatched)
            } else {
                None
            },
            sort: Some(SortKey::RatingDesc),
            limit: spec.limit,
            ..Default::default()
        };
        self.search_media(&q).await
    }

    // ── 危险动作：只有 runtime 确认后才会走到这里 ──

    async fn scan_library(&self, library: Option<Id>) -> Result<ScanTicket, AiError> {
        self.actions.scan_library(library).await
    }

    async fn identify_media(&self, item_id: Id) -> Result<IdentifyOutcome, AiError> {
        self.actions.identify_media(item_id).await
    }
}

fn strategy_str(s: PlaybackStrategy) -> &'static str {
    match s {
        PlaybackStrategy::DirectPlay => "direct_play",
        PlaybackStrategy::Remux => "remux",
        PlaybackStrategy::Transcode => "transcode",
        PlaybackStrategy::Unsupported => "unsupported",
    }
}

/// 枚举没有 Display，用 serde 的 snake_case 取字符串。
/// 好处是与库里存的值、与前端收到的值必然一致。
fn serde_str<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".into())
}

// ─────────────────────────────────────────────────────────────
// SQL
// ─────────────────────────────────────────────────────────────
//
// 一条查询把"AI 关心的那几个判别位"一次取全：
// 有没有海报 / 有没有字幕轨 / 有没有正文 / 最新播放进度 / 代表文件路径 / 总大小。
// 分成六条查询各查一遍，在万级库上就是六倍的往返。

const HIT_SQL_HDR: &str = r#"
SELECT
    i.id                AS id,
    i.kind              AS kind,
    i.title             AS title,
    i.year              AS year,
    i.rating            AS rating,
    i.runtime_ms        AS runtime_ms,
    i.added_at          AS added_at,
    EXISTS (SELECT 1 FROM image img
             WHERE img.item_id = i.id AND img.kind = 'poster')              AS has_poster,
    EXISTS (SELECT 1 FROM media_stream s
              JOIN media_file f    ON f.id = s.file_id
              JOIN media_version v ON v.id = f.version_id
             WHERE v.item_id = i.id AND s.stream_type = 'subtitle')         AS has_subtitle,
    (i.overview IS NOT NULL AND i.overview <> '')                           AS has_metadata,
    (SELECT MAX(r.position_ms * 1.0 / NULLIF(r.duration_ms, 0))
       FROM playback_session r WHERE r.item_id = i.id)                      AS progress,
    (SELECT f2.path FROM media_file f2
       JOIN media_version v2 ON v2.id = f2.version_id
      WHERE v2.item_id = i.id AND f2.is_available = 1
      ORDER BY v2.is_default DESC, f2.size_bytes DESC LIMIT 1)              AS path,
    (SELECT COALESCE(SUM(f3.size_bytes), 0) FROM media_file f3
       JOIN media_version v3 ON v3.id = f3.version_id
      WHERE v3.item_id = i.id)                                              AS size_bytes
FROM media_item i
"#;

/// 排序子句必须是白名单：SQL 排序方向没法参数化，拼字符串就是注入口子。
fn order_by(sort: Option<SortKey>) -> &'static str {
    match sort.unwrap_or(SortKey::AddedDesc) {
        SortKey::SizeDesc => "size_bytes DESC, i.title ASC",
        SortKey::AddedDesc => "i.added_at DESC",
        SortKey::RatingDesc => "i.rating DESC, i.added_at DESC",
        SortKey::TitleAsc => "i.title ASC",
    }
}

async fn fetch_window(db: &Db, q: &MediaQuery) -> Result<Vec<MediaHit>, AiError> {
    let term = q
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| format!("%{t}%"));

    // 用 QueryBuilder 而不是拼字符串：sqlx 0.9 起动态 SQL 会被
    // `SqlSafeStr` 拦下，而这里确实需要动态（过滤条件组合不定）。
    // 所有**值**一律走 push_bind，所有**结构**一律来自白名单常量。
    let mut b = QueryBuilder::<sqlx::Sqlite>::new(HIT_SQL_HDR);
    let mut has_where = false;

    if let Some(k) = q.kind {
        b.push(" WHERE i.kind = ").push_bind(serde_str(&k));
        has_where = true;
    }
    if let Some(t) = &term {
        b.push(if has_where { " AND " } else { " WHERE " });
        b.push("(i.title LIKE ");
        b.push_bind(t.clone());
        b.push(" OR i.original_title LIKE ");
        b.push_bind(t.clone());
        b.push(" OR i.sort_title LIKE ");
        b.push_bind(t.clone());
        b.push(")");
    }
    // order_by 是枚举白名单，不接受任何来自请求的文本。
    b.push(" ORDER BY ").push(order_by(q.sort));
    b.push(" LIMIT ").push_bind(CANDIDATE_WINDOW as i64);

    let rows = b
        .build_query_as::<HitRow>()
        .fetch_all(db.pool())
        .await
        .map_err(|e| AiError::host(format!("查询媒体库失败：{e}")))?;
    Ok(rows.into_iter().map(hit_of).collect())
}

async fn fetch_one(db: &Db, id: Id) -> Result<Vec<MediaHit>, AiError> {
    let mut b = QueryBuilder::<sqlx::Sqlite>::new(HIT_SQL_HDR);
    b.push(" WHERE i.id = ").push_bind(id.to_string());
    let rows = b
        .build_query_as::<HitRow>()
        .fetch_all(db.pool())
        .await
        .map_err(|e| AiError::host(format!("查询条目失败：{e}")))?;
    Ok(rows.into_iter().map(hit_of).collect())
}

/// 补类型。单独一条 `IN (...)` 而不是每行一条：
/// 两百行两百条查询，光往返就够慢了。
async fn attach_genres(db: &Db, hits: &mut [MediaHit]) -> Result<(), AiError> {
    if hits.is_empty() {
        return Ok(());
    }
    let mut b = QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT ig.item_id, g.name FROM item_genre ig \
         JOIN genre g ON g.id = ig.genre_id WHERE ig.item_id IN (",
    );
    let mut sep = b.separated(", ");
    for h in hits.iter() {
        sep.push_bind(h.id.to_string());
    }
    b.push(")");

    let rows = b
        .build_query_as::<(String, String)>()
        .fetch_all(db.pool())
        .await
        .map_err(|e| AiError::host(format!("查询类型失败：{e}")))?;

    for (item_id, name) in rows {
        if let Ok(id) = Uuid::parse_str(&item_id) {
            if let Some(h) = hits.iter_mut().find(|h| h.id == id) {
                h.genres.push(name);
            }
        }
    }
    for h in hits.iter_mut() {
        h.genres.sort();
        h.genres.dedup();
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct HitRow {
    id: String,
    kind: String,
    title: String,
    year: Option<i32>,
    rating: Option<f64>,
    runtime_ms: Option<i64>,
    added_at: String,
    has_poster: bool,
    has_subtitle: bool,
    has_metadata: bool,
    progress: Option<f64>,
    path: Option<String>,
    size_bytes: Option<i64>,
}

fn hit_of(r: HitRow) -> MediaHit {
    let id = Uuid::parse_str(&r.id).unwrap_or_default();
    let added_at = DateTime::parse_from_rfc3339(&r.added_at)
        .map(|d| d.with_timezone(&Utc))
        .ok();
    MediaHit {
        id,
        title: r.title,
        kind: kind_of(&r.kind),
        year: r.year,
        genres: Vec::new(),
        rating: r.rating,
        runtime_ms: r.runtime_ms,
        size_bytes: r.size_bytes,
        added_at,
        has_poster: r.has_poster,
        has_subtitle: r.has_subtitle,
        has_metadata: r.has_metadata,
        progress: r.progress.map(|p| p as f32),
        path: r.path,
    }
}

/// 库里的 kind 是 snake_case 字符串。认不出来归 Movie 而不是报错：
/// 一条脏数据不该让整个 AI 查询失败。
fn kind_of(s: &str) -> MediaKind {
    serde_json::from_value::<MediaKind>(serde_json::Value::String(s.to_string()))
        .unwrap_or(MediaKind::Movie)
}

// ─────────────────────────────────────────────────────────────
// 内存侧谓词
// ─────────────────────────────────────────────────────────────
//
// 这些过滤放在 Rust 而不是 SQL 里。理由写在 CANDIDATE_WINDOW 上：
// 谓词多、组合多，先保证正确与可测，等真成了瓶颈再下推。

fn matches_query(h: &MediaHit, q: &MediaQuery) -> bool {
    if let Some(k) = q.kind {
        if h.kind != k {
            return false;
        }
    }
    if let Some(from) = q.year_from {
        if h.year.map(|y| y < from).unwrap_or(true) {
            return false;
        }
    }
    if let Some(to) = q.year_to {
        if h.year.map(|y| y > to).unwrap_or(true) {
            return false;
        }
    }
    if let Some(min) = q.runtime_min_ms {
        if h.runtime_ms.map(|r| r < min).unwrap_or(true) {
            return false;
        }
    }
    if let Some(max) = q.runtime_max_ms {
        if h.runtime_ms.map(|r| r > max).unwrap_or(true) {
            return false;
        }
    }
    for m in &q.missing {
        // 「缺海报」= 没有海报。写成"有海报"是这里最容易犯的错，
        // 而且不会报错，只会让结果永远是空的。
        let present = match m {
            MissingKind::Poster => h.has_poster,
            MissingKind::Subtitle => h.has_subtitle,
            MissingKind::Metadata => h.has_metadata,
        };
        if present {
            return false;
        }
    }
    for g in &q.genres {
        if !h.genres.iter().any(|x| x == g) {
            return false;
        }
    }
    if let Some(days) = q.added_within_days {
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        if h.added_at.map(|t| t < cutoff).unwrap_or(true) {
            return false;
        }
    }
    if let Some(w) = q.watch {
        if h.watch_state() != w {
            return false;
        }
    }
    true
}

fn only_duplicates(hits: Vec<MediaHit>) -> Vec<MediaHit> {
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for h in &hits {
        *counts.entry(duplicate_key(&h.title, h.year)).or_insert(0) += 1;
    }
    hits.into_iter()
        .filter(|h| counts.get(&duplicate_key(&h.title, h.year)).copied() > Some(1))
        .collect()
}

/// 危险动作的真实执行器。
///
/// 只持有"做事需要的东西"，不持有 `AppState` —— 那会与
/// `Arc<AiRuntime>` 构成引用环。
pub struct AppActions {
    db: Arc<Db>,
    scans: crate::ScanRegistry,
    ffprobe: Option<Arc<whatmedia_media::FfprobeRunner>>,
    scraper: Option<Arc<whatmedia_metadata::Scraper>>,
}

impl AppActions {
    pub fn new(state: &AppState) -> Option<Self> {
        Some(Self {
            db: Arc::new(state.db.clone()?),
            scans: state.scans.clone(),
            ffprobe: state.ffprobe.clone(),
            scraper: state.scraper.clone(),
        })
    }
}

#[async_trait]
impl AiActions for AppActions {
    /// 扫描。与 `POST /api/v1/scan` 走同一套机制（后台 tokio 任务 +
    /// `ScanRegistry` 登记进度），AI 只是多了一个入口，不是另一套实现。
    async fn scan_library(&self, library: Option<Id>) -> Result<ScanTicket, AiError> {
        let libs = whatmedia_database::repo::list_libraries(&self.db)
            .await
            .map_err(|e| AiError::host(format!("读取媒体库失败：{e}")))?;
        let lib = match library {
            Some(id) => libs
                .into_iter()
                .find(|l| l.id == id)
                .ok_or_else(|| AiError::bad_args(format!("没有这个媒体库：{id}")))?,
            None => libs.into_iter().next().ok_or_else(|| {
                AiError::bad_args("还没有任何媒体库，先用 POST /api/v1/scan 建一个")
            })?,
        };

        let root = std::path::Path::new(&lib.root_path)
            .canonicalize()
            .map(|p| whatmedia_storage::clean_path(&p))
            .map_err(|e| AiError::host(format!("媒体库根目录不可访问：{e}")))?;
        let storage = Arc::new(
            whatmedia_storage::LocalStorage::new("local", &root)
                .map_err(|e| AiError::host(format!("打开媒体库失败：{e}")))?,
        );

        let cfg = whatmedia_scanner::ScanConfig {
            incremental: true,
            probe: self.ffprobe.is_some(),
            ffprobe: self.ffprobe.clone(),
            ..Default::default()
        };
        let scanner = Arc::new(whatmedia_scanner::Scanner::new(
            storage,
            self.db.as_ref().clone(),
            cfg,
        ));

        let task_id = Uuid::new_v4();
        let (tx, mut rx) = tokio::sync::broadcast::channel::<whatmedia_scanner::ScanProgress>(64);
        self.scans.insert(crate::ScanTaskView {
            id: task_id,
            library: lib.name.clone(),
            progress: whatmedia_scanner::ScanProgress::default(),
        });

        let registry = self.scans.clone();
        let scan_lib = lib.clone();
        tokio::spawn(async move {
            let ctrl = whatmedia_scanner::ScanControl::new();
            let _ = scanner.scan(&scan_lib, ctrl, Some(tx)).await;
            while let Ok(p) = rx.recv().await {
                registry.update(task_id, p.clone());
                if p.finished {
                    break;
                }
            }
        });

        Ok(ScanTicket {
            task_id: task_id.to_string(),
            library: lib.name.clone(),
        })
    }

    async fn identify_media(&self, item_id: Id) -> Result<IdentifyOutcome, AiError> {
        let scraper = self
            .scraper
            .as_ref()
            .ok_or_else(|| AiError::denied("未配置任何元数据 Provider，无法重新识别"))?;

        let before = whatmedia_database::repo::get_item(&self.db, item_id)
            .await
            .map_err(|e| AiError::host(format!("读取条目失败：{e}")))?;
        let Some(before) = before else {
            return Err(AiError::bad_args(format!("没有这个条目：{item_id}")));
        };

        scraper
            .identify(item_id, "", "")
            .await
            .map_err(|e| AiError::host(format!("识别失败：{e}")))?;

        let after = whatmedia_database::repo::get_item(&self.db, item_id)
            .await
            .map_err(|e| AiError::host(format!("读取条目失败：{e}")))?;

        // 预览里承诺了"会改哪些字段"，执行后就得如实报出来 ——
        // 一个字段都没变也要说清楚，否则用户以为识别失败了。
        let mut changed = Vec::new();
        let mut provider = None;
        let mut provider_id = None;
        if let Some(a) = &after {
            if before.item.title != a.item.title {
                changed.push("title".into());
            }
            if before.item.year != a.item.year {
                changed.push("year".into());
            }
            if before.overview != a.overview {
                changed.push("overview".into());
            }
            if before.item.rating != a.item.rating {
                changed.push("rating".into());
            }
            // provider / provider_id 不在 ItemDetail 里，单独取一行
            let (p, pid) = provider_of(&self.db, item_id).await?;
            if p.is_some() {
                changed.push("provider".into());
            }
            provider = p;
            provider_id = pid;
        }

        Ok(IdentifyOutcome {
            item_id,
            title: after
                .as_ref()
                .map(|a| a.item.title.clone())
                .unwrap_or_else(|| before.item.title.clone()),
            provider,
            provider_id,
            // 一个字段都没变也要如实说"没变"，否则用户以为识别失败了。
            changed_fields: changed,
        })
    }
}

/// 取条目当前绑定的元数据来源。`ItemDetail` 不带这两个字段，
/// 与其为了它们把整个结构扩一遍，不如单独查一行。
async fn provider_of(db: &Db, item_id: Id) -> Result<(Option<String>, Option<String>), AiError> {
    let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT provider, provider_id FROM media_item WHERE id = ?",
    )
    .bind(item_id.to_string())
    .fetch_optional(db.pool())
    .await
    .map_err(|e| AiError::host(format!("读取条目来源失败：{e}")))?;
    Ok(row.unwrap_or((None, None)))
}

// ─────────────────────────────────────────────────────────────
// 装配
// ─────────────────────────────────────────────────────────────

/// 按配置装配 AI 运行时。没有模型也照装 —— 走规则引擎。
///
/// 返回 `None` 只表示"数据库没就绪"，此时 `/ai/chat` 会如实返回
/// `DB_UNAVAILABLE`，而不是假装 AI 能用。
pub fn build_ai_runtime(state: &AppState, actions: Arc<dyn AiActions>) -> Option<Arc<AiRuntime>> {
    let db = Arc::new(state.db.clone()?);
    let host = Arc::new(DbHost::new(
        db,
        actions,
        state.transcode.server_capability(),
    ));
    let provider = build_provider(&state.config.ai);
    Some(Arc::new(whatmedia_ai::from_config(
        &state.config.ai,
        provider,
        host,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatmedia_ai::RuleEngine;

    #[test]
    fn order_by_is_a_closed_set() {
        // 排序方向只能来自枚举。这个测试是给"以后有人想加自定义排序"的警告：
        // 想加就在 SortKey 里加一个变体，别在 SQL 里拼外部字符串。
        for k in [
            SortKey::SizeDesc,
            SortKey::AddedDesc,
            SortKey::RatingDesc,
            SortKey::TitleAsc,
        ] {
            let s = order_by(Some(k));
            assert!(s.contains("DESC") || s.contains("ASC"), "{s}");
            assert!(!s.contains(';'), "排序子句里不该出现语句分隔符");
        }
    }

    #[test]
    fn kind_round_trips_through_the_db_text() {
        assert_eq!(kind_of("movie"), MediaKind::Movie);
        assert_eq!(kind_of("episode"), MediaKind::Episode);
        // 认不出来归 Movie 而不是 panic
        assert_eq!(kind_of("不存在的类型"), MediaKind::Movie);
    }

    #[test]
    fn strategy_names_match_the_contract() {
        assert_eq!(strategy_str(PlaybackStrategy::DirectPlay), "direct_play");
        assert_eq!(strategy_str(PlaybackStrategy::Unsupported), "unsupported");
    }

    #[test]
    fn missing_predicates_are_negative() {
        // 「缺海报」必须筛出没有海报的，别写成筛出有海报的。
        let hit = MediaHit {
            id: Id::nil(),
            title: "x".into(),
            kind: MediaKind::Movie,
            year: Some(2000),
            genres: vec![],
            rating: None,
            runtime_ms: None,
            size_bytes: None,
            added_at: Some(Utc::now()),
            has_poster: true,
            has_subtitle: false,
            has_metadata: false,
            progress: None,
            path: None,
        };
        let q = MediaQuery {
            missing: vec![MissingKind::Poster],
            ..Default::default()
        };
        assert!(!matches_query(&hit, &q), "有海报的条目不该算缺海报");
        let q2 = MediaQuery {
            missing: vec![MissingKind::Subtitle],
            ..Default::default()
        };
        assert!(matches_query(&hit, &q2));
    }

    #[test]
    fn duplicates_need_at_least_two() {
        let mk = |t: &str| MediaHit {
            id: Id::new_v4(),
            title: t.into(),
            kind: MediaKind::Movie,
            year: Some(2000),
            genres: vec![],
            rating: None,
            runtime_ms: None,
            size_bytes: None,
            added_at: Some(Utc::now()),
            has_poster: true,
            has_subtitle: true,
            has_metadata: true,
            progress: None,
            path: None,
        };
        let hits = vec![mk("星际穿越"), mk("星际穿越"), mk("盗梦空间")];
        let out = only_duplicates(hits);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|h| h.title == "星际穿越"));
    }

    #[test]
    fn rule_engine_still_answers_without_any_model() {
        // 无模型时的兜底：这句问法必须能解析出"缺海报的电影"。
        let intent = RuleEngine::parse("找出没有海报的电影");
        let (tool, _) = intent.tool_call().expect("这句应该能解析出工具调用");
        // 缺海报走专用工具，不是通用搜索 —— 说明规则引擎在按语义分流
        assert_eq!(tool, "find_missing_poster");
    }

    #[test]
    fn actor_defaults_to_adult_not_owner() {
        // 缺省角色必须是权限较小的一档
        let a = actor_of(None, None);
        assert_eq!(a.id, "anonymous");
        assert!(!a.role.may_request(whatmedia_ai::Risk::Destructive));
    }
}
