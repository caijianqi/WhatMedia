//! 首批 8 个工具（P7 / 需求稿 §61）。
//!
//! 每个工具的返回值都是 `{ "text": ..., "data": ... }`：
//!   · `text` 人读得懂，**直接进对话**
//!   · `data` 结构化，给前端渲染成卡片 / 列表
//!
//! 分成两半只是为了让"AI 说了什么"和"前端显示什么"都能被单独测。

use async_trait::async_trait;
use serde_json::{json, Value};
use whatmedia_core::{Id, MediaKind};

use crate::error::AiError;
use crate::host::{
    human_runtime, human_size, AiHost, CapabilityReport, MediaHit, MediaQuery, MissingKind,
    RecommendSpec, SortKey, WatchState,
};
use crate::tool::{AiTool, ParamKind, ParamSpec, Preview, PreviewLine, Risk, ToolSpec};

// ─────────────────────────────────────────────────────────────
// 参数取值小工具
// ─────────────────────────────────────────────────────────────

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn req_str(args: &Value, key: &str) -> Result<String, AiError> {
    opt_str(args, key).ok_or_else(|| AiError::bad_args(format!("缺参数 {key}")))
}

fn opt_i64(args: &Value, key: &str) -> Option<i64> {
    args.get(key).and_then(|v| v.as_i64())
}

fn opt_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(|v| v.as_u64()).map(|n| n as u32)
}

fn opt_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn str_array(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn parse_kind(s: &str) -> Option<MediaKind> {
    match s.trim().to_lowercase().as_str() {
        "movie" | "电影" | "影片" => Some(MediaKind::Movie),
        "series" | "tv" | "电视剧" | "剧集" | "剧" => Some(MediaKind::Series),
        "season" | "季" => Some(MediaKind::Season),
        "episode" | "集" => Some(MediaKind::Episode),
        "anime" | "动漫" | "动画" => Some(MediaKind::Anime),
        "variety" | "综艺" => Some(MediaKind::Variety),
        "documentary" | "纪录片" => Some(MediaKind::Documentary),
        "concert" | "演唱会" => Some(MediaKind::Concert),
        "music" | "音乐" => Some(MediaKind::Music),
        "music_video" | "mv" => Some(MediaKind::MusicVideo),
        _ => None,
    }
}

fn parse_missing(s: &str) -> Option<MissingKind> {
    match s.trim().to_lowercase().as_str() {
        "poster" | "海报" | "封面" => Some(MissingKind::Poster),
        "subtitle" | "字幕" => Some(MissingKind::Subtitle),
        "metadata" | "元数据" => Some(MissingKind::Metadata),
        _ => None,
    }
}

fn parse_watch(s: &str) -> Option<WatchState> {
    match s.trim().to_lowercase().as_str() {
        "unwatched" | "没看过" | "未看" => Some(WatchState::Unwatched),
        "partial" | "没看完" | "看过一半" => Some(WatchState::Partial),
        "finished" | "看完" => Some(WatchState::Finished),
        _ => None,
    }
}

fn parse_sort(s: &str) -> Option<SortKey> {
    match s.trim().to_lowercase().as_str() {
        "size_desc" | "size" | "体积" => Some(SortKey::SizeDesc),
        "added_desc" | "added" | "最近添加" => Some(SortKey::AddedDesc),
        "rating_desc" | "rating" | "评分" => Some(SortKey::RatingDesc),
        "title_asc" | "title" | "标题" => Some(SortKey::TitleAsc),
        _ => None,
    }
}

fn parse_id(s: &str) -> Result<Id, AiError> {
    Id::parse_str(s.trim()).map_err(|_| AiError::bad_args(format!("不是合法的 id：{s}")))
}

/// 从通用参数里抽出 `MediaQuery`。`search_media` 与 `find_missing_poster` 共用。
fn query_from_args(args: &Value) -> MediaQuery {
    MediaQuery {
        text: opt_str(args, "text").or_else(|| opt_str(args, "query")),
        kind: opt_str(args, "kind").and_then(|s| parse_kind(&s)),
        genres: str_array(args, "genres"),
        region: opt_str(args, "region"),
        year_from: opt_i64(args, "year_from").map(|n| n as i32),
        year_to: opt_i64(args, "year_to").map(|n| n as i32),
        missing: str_array(args, "missing")
            .iter()
            .filter_map(|s| parse_missing(s))
            .collect(),
        added_within_days: opt_u32(args, "added_within_days"),
        runtime_min_ms: opt_i64(args, "runtime_min_ms"),
        runtime_max_ms: opt_i64(args, "runtime_max_ms"),
        duplicates_only: opt_bool(args, "duplicates_only"),
        watch: opt_str(args, "watch").and_then(|s| parse_watch(&s)),
        sort: opt_str(args, "sort").and_then(|s| parse_sort(&s)),
        limit: opt_u32(args, "limit").unwrap_or(20),
    }
}

fn hit_json(h: &MediaHit) -> Value {
    json!({
        "id": h.id.to_string(),
        "title": h.title,
        "kind": format!("{:?}", h.kind),
        "year": h.year,
        "genres": h.genres,
        "rating": h.rating,
        "runtime_ms": h.runtime_ms,
        "size_bytes": h.size_bytes,
        "has_poster": h.has_poster,
        "has_subtitle": h.has_subtitle,
        "progress": h.progress,
    })
}

fn render_hits(hits: &[MediaHit]) -> String {
    if hits.is_empty() {
        return "没有找到符合条件的条目。".to_string();
    }
    let mut lines = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let mut parts: Vec<String> = Vec::new();
        if let Some(y) = h.year {
            parts.push(y.to_string());
        }
        if let Some(size) = h.size_bytes {
            parts.push(human_size(size));
        }
        if let Some(rt) = h.runtime_ms {
            parts.push(human_runtime(rt));
        }
        let tail = if parts.is_empty() {
            String::new()
        } else {
            format!("（{}）", parts.join(" · "))
        };
        lines.push(format!("{}. 《{}》{}", i + 1, h.title, tail));
    }
    lines.join("\n")
}

fn result(text: String, data: Value) -> Value {
    json!({ "text": text, "data": data })
}

// ─────────────────────────────────────────────────────────────
// 1. search_media（只读）
// ─────────────────────────────────────────────────────────────

pub struct SearchMediaTool;

#[async_trait]
impl AiTool for SearchMediaTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_media".into(),
            description:
                "按条件检索媒体库。支持片名、类型、类型片、年份、缺失项（海报/字幕/元数据）、\
                          添加时间、时长、观看状态、排序。查不到时缩小条件再试，不要编造结果。"
                    .into(),
            params: vec![
                ParamSpec::new("text", ParamKind::String, "片名关键词"),
                ParamSpec::new(
                    "kind",
                    ParamKind::String,
                    "movie/series/episode/anime/documentary/variety/concert/music，中文也认",
                ),
                ParamSpec::new("genres", ParamKind::Array, "类型片，如 [\"Sci-Fi\"]"),
                ParamSpec::new(
                    "missing",
                    ParamKind::Array,
                    "缺什么：poster/subtitle/metadata",
                ),
                ParamSpec::new("added_within_days", ParamKind::Integer, "最近 N 天内添加的"),
                ParamSpec::new("runtime_max_ms", ParamKind::Integer, "时长上限（毫秒）"),
                ParamSpec::new("runtime_min_ms", ParamKind::Integer, "时长下限（毫秒）"),
                ParamSpec::new("year_from", ParamKind::Integer, "年份下限"),
                ParamSpec::new("year_to", ParamKind::Integer, "年份上限"),
                ParamSpec::new("duplicates_only", ParamKind::Boolean, "只要重复的"),
                ParamSpec::new("watch", ParamKind::String, "unwatched/partial/finished"),
                ParamSpec::new(
                    "sort",
                    ParamKind::String,
                    "size_desc/added_desc/rating_desc/title_asc",
                ),
                ParamSpec::new("limit", ParamKind::Integer, "返回条数，默认 20，上限 200"),
            ],
            risk: Risk::ReadOnly,
        }
    }

    async fn preview(&self, _host: &dyn AiHost, _args: &Value) -> Result<Preview, AiError> {
        Err(AiError::internal("search_media 是只读工具，不需要预览"))
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let q = query_from_args(args);
        let hits = host.search_media(&q).await?;
        let text = if hits.is_empty() {
            format!("没有找到符合条件的条目（{}）。", q.describe())
        } else {
            format!(
                "找到 {} 条（{}）：\n{}",
                hits.len(),
                q.describe(),
                render_hits(&hits)
            )
        };
        let items: Vec<Value> = hits.iter().map(hit_json).collect();
        Ok(result(text, json!({ "items": items, "query": q })))
    }
}

// ─────────────────────────────────────────────────────────────
// 2. get_media（只读）
// ─────────────────────────────────────────────────────────────

pub struct GetMediaTool;

#[async_trait]
impl AiTool for GetMediaTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_media".into(),
            description: "按 id 取一条媒体的详情（版本、音轨、字幕、续播点、文件体积）。".into(),
            params: vec![ParamSpec::new("id", ParamKind::String, "条目 id").required()],
            risk: Risk::ReadOnly,
        }
    }

    async fn preview(&self, _host: &dyn AiHost, _args: &Value) -> Result<Preview, AiError> {
        Err(AiError::internal("get_media 是只读工具，不需要预览"))
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let id = parse_id(&req_str(args, "id")?)?;
        let hit = host.get_media(id).await?;
        match hit {
            Some(h) => {
                let text = format!(
                    "《{}》{} · {} · 海报{} · 字幕{}",
                    h.title,
                    h.year.map(|y| format!("（{y}）")).unwrap_or_default(),
                    h.runtime_ms
                        .map(human_runtime)
                        .unwrap_or_else(|| "时长未知".into()),
                    if h.has_poster { "有" } else { "无" },
                    if h.has_subtitle { "有" } else { "无" },
                );
                Ok(result(text, json!({ "item": hit_json(&h) })))
            }
            None => Ok(result(format!("没有 id 为 {id} 的条目。"), json!(null))),
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 3. find_missing_poster（只读）
// ─────────────────────────────────────────────────────────────

pub struct FindMissingPosterTool;

#[async_trait]
impl AiTool for FindMissingPosterTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find_missing_poster".into(),
            description: "找出缺少海报的条目。传 kind 可以只看电影或只看电视剧。".into(),
            params: vec![
                ParamSpec::new(
                    "kind",
                    ParamKind::String,
                    "movie/series/anime 等，不传=全部",
                ),
                ParamSpec::new("limit", ParamKind::Integer, "返回条数，默认 20"),
            ],
            risk: Risk::ReadOnly,
        }
    }

    async fn preview(&self, _host: &dyn AiHost, _args: &Value) -> Result<Preview, AiError> {
        Err(AiError::internal(
            "find_missing_poster 是只读工具，不需要预览",
        ))
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let q = MediaQuery {
            kind: opt_str(args, "kind").and_then(|s| parse_kind(&s)),
            missing: vec![MissingKind::Poster],
            limit: opt_u32(args, "limit").unwrap_or(20),
            ..Default::default()
        };
        let hits = host.search_media(&q).await?;
        let text = if hits.is_empty() {
            "没有缺海报的条目。".to_string()
        } else {
            format!("有 {} 条缺海报：\n{}", hits.len(), render_hits(&hits))
        };
        let items: Vec<Value> = hits.iter().map(hit_json).collect();
        Ok(result(text, json!({ "items": items })))
    }
}

// ─────────────────────────────────────────────────────────────
// 4. find_duplicate（只读）
// ─────────────────────────────────────────────────────────────

pub struct FindDuplicateTool;

#[async_trait]
impl AiTool for FindDuplicateTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "find_duplicate".into(),
            description: "按标题+年份找重复条目。只报告，不删除 —— 删哪个要由用户决定。".into(),
            params: vec![ParamSpec::new(
                "limit",
                ParamKind::Integer,
                "返回组数，默认 20",
            )],
            risk: Risk::ReadOnly,
        }
    }

    async fn preview(&self, _host: &dyn AiHost, _args: &Value) -> Result<Preview, AiError> {
        Err(AiError::internal("find_duplicate 是只读工具，不需要预览"))
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let limit = opt_u32(args, "limit").unwrap_or(20);
        let groups = host.duplicates(limit).await?;
        if groups.is_empty() {
            return Ok(result("没有发现重复条目。".into(), json!({ "groups": [] })));
        }
        let mut lines = Vec::new();
        for (i, g) in groups.iter().enumerate() {
            let total: i64 = g.hits.iter().filter_map(|h| h.size_bytes).sum();
            lines.push(format!(
                "{}. 《{}》{} 有 {} 个版本，共占用 {}",
                i + 1,
                g.title,
                g.year.map(|y| format!("（{y}）")).unwrap_or_default(),
                g.hits.len(),
                human_size(total)
            ));
        }
        let gj: Vec<Value> = groups
            .iter()
            .map(|g| {
                json!({
                    "title": g.title,
                    "year": g.year,
                    "hits": g.hits.iter().map(hit_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(result(lines.join("\n"), json!({ "groups": gj })))
    }
}

// ─────────────────────────────────────────────────────────────
// 5. recommend_media（只读）
// ─────────────────────────────────────────────────────────────

pub struct RecommendMediaTool;

#[async_trait]
impl AiTool for RecommendMediaTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "recommend_media".into(),
            description: "推荐片单。可按类型片与时长上限过滤，默认排除已看过的。\
                          “两小时以内能看完的”就是 max_runtime_ms=7200000。"
                .into(),
            params: vec![
                ParamSpec::new("genres", ParamKind::Array, "类型片"),
                ParamSpec::new("kind", ParamKind::String, "movie/series/anime 等"),
                ParamSpec::new("max_runtime_ms", ParamKind::Integer, "时长上限（毫秒）"),
                ParamSpec::new(
                    "exclude_watched",
                    ParamKind::Boolean,
                    "排除看过的，默认 true",
                ),
                ParamSpec::new("limit", ParamKind::Integer, "推荐条数，默认 10"),
            ],
            risk: Risk::ReadOnly,
        }
    }

    async fn preview(&self, _host: &dyn AiHost, _args: &Value) -> Result<Preview, AiError> {
        Err(AiError::internal("recommend_media 是只读工具，不需要预览"))
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let spec = RecommendSpec {
            genres: str_array(args, "genres"),
            max_runtime_ms: opt_i64(args, "max_runtime_ms"),
            kind: opt_str(args, "kind").and_then(|s| parse_kind(&s)),
            // 推荐默认排除看过的：用户问"推荐"时几乎从不是想再看一遍。
            exclude_watched: args
                .get("exclude_watched")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            limit: opt_u32(args, "limit").unwrap_or(10),
        };
        let hits = host.recommend(&spec).await?;
        let text = if hits.is_empty() {
            "库里没有符合条件的片子可以推荐。".to_string()
        } else {
            format!("推荐这 {} 部：\n{}", hits.len(), render_hits(&hits))
        };
        let items: Vec<Value> = hits.iter().map(hit_json).collect();
        Ok(result(text, json!({ "items": items })))
    }
}

// ─────────────────────────────────────────────────────────────
// 6. get_playback_capability（只读）
// ─────────────────────────────────────────────────────────────

pub struct GetPlaybackCapabilityTool;

#[async_trait]
impl AiTool for GetPlaybackCapabilityTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "get_playback_capability".into(),
            description: "判断某部片在某个客户端上能不能直出、要不要转码，并说明原因。\
                          “为什么我的 4K 片在转码”就问它。"
                .into(),
            params: vec![
                ParamSpec::new("item_id", ParamKind::String, "条目 id").required(),
                ParamSpec::new(
                    "client_id",
                    ParamKind::String,
                    "客户端能力档，如 web-chrome / tauri-mpv / android-exoplayer",
                ),
            ],
            risk: Risk::ReadOnly,
        }
    }

    async fn preview(&self, _host: &dyn AiHost, _args: &Value) -> Result<Preview, AiError> {
        Err(AiError::internal(
            "get_playback_capability 是只读工具，不需要预览",
        ))
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let id = parse_id(&req_str(args, "item_id")?)?;
        let client = opt_str(args, "client_id");
        let report: CapabilityReport = host.capability(id, client.as_deref()).await?;
        let mut text = format!(
            "《{}》：{}（{}）",
            report.title, report.strategy, report.reason
        );
        if !report.warnings.is_empty() {
            text.push_str(&format!("\n注意：{}", report.warnings.join("；")));
        }
        Ok(result(text, json!({ "capability": report })))
    }
}

// ─────────────────────────────────────────────────────────────
// 7. scan_library（写：触发一次扫描任务）
// ─────────────────────────────────────────────────────────────

pub struct ScanLibraryTool;

#[async_trait]
impl AiTool for ScanLibraryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "scan_library".into(),
            description: "触发一次媒体库扫描（增量）。这是写操作：会给机器加负载，\
                          必须让用户先看到预览再确认。不传 library_id 表示扫全部。"
                .into(),
            params: vec![ParamSpec::new(
                "library_id",
                ParamKind::String,
                "媒体库 id；不传=全部",
            )],
            risk: Risk::Write,
        }
    }

    async fn preview(&self, host: &dyn AiHost, args: &Value) -> Result<Preview, AiError> {
        let target = match opt_str(args, "library_id") {
            Some(id) => {
                let id = parse_id(&id)?;
                let hit = host.get_media(id).await?;
                hit.map(|h| h.title).unwrap_or_else(|| id.to_string())
            }
            None => "全部媒体库".to_string(),
        };
        let mut warnings = vec![
            "扫描会占用 CPU 与磁盘 IO，播放中的会话可能被拖慢。".to_string(),
            "扫描是增量的：未变动的文件不会重新探测。".to_string(),
        ];
        if opt_str(args, "library_id").is_none() {
            warnings.push("没有指定媒体库，将扫描全部库。".into());
        }
        Ok(Preview {
            summary: format!("将对「{target}」触发一次增量扫描"),
            lines: vec![PreviewLine {
                target,
                action: "增量扫描".into(),
                detail: Some("mtime+size 指纹未变的文件会跳过".into()),
            }],
            warnings,
        })
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let library = match opt_str(args, "library_id") {
            Some(s) => Some(parse_id(&s)?),
            None => None,
        };
        let ticket = host.scan_library(library).await?;
        Ok(result(
            format!("已启动扫描任务 {}（{}）。", ticket.task_id, ticket.library),
            json!({ "ticket": ticket }),
        ))
    }
}

// ─────────────────────────────────────────────────────────────
// 8. identify_media（写：覆盖元数据）
// ─────────────────────────────────────────────────────────────

pub struct IdentifyMediaTool;

#[async_trait]
impl AiTool for IdentifyMediaTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "identify_media".into(),
            description: "重新识别一条媒体并用 Provider 的元数据覆盖它。这是写操作：\
                          用户手动锁定的字段不会被覆盖，但其余字段会被改写。"
                .into(),
            params: vec![ParamSpec::new("item_id", ParamKind::String, "条目 id").required()],
            risk: Risk::Write,
        }
    }

    async fn preview(&self, host: &dyn AiHost, args: &Value) -> Result<Preview, AiError> {
        let id = parse_id(&req_str(args, "item_id")?)?;
        let hit = host
            .get_media(id)
            .await?
            .ok_or_else(|| AiError::bad_args(format!("条目不存在：{id}")))?;
        Ok(Preview {
            summary: format!("将重新识别《{}》并覆盖元数据", hit.title),
            lines: vec![PreviewLine {
                target: hit.title.clone(),
                action: "重新识别".into(),
                detail: Some("overview / poster / 类型 / 演职员 会被 Provider 的结果覆盖".into()),
            }],
            warnings: vec![
                "用户手动锁定过的字段不会被覆盖（字段级锁定）。".into(),
                "识别结果与现在不同时，海报会被换掉。".into(),
            ],
        })
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let id = parse_id(&req_str(args, "item_id")?)?;
        let out = host.identify_media(id).await?;
        Ok(result(
            format!(
                "《{}》已重新识别（{}），改动字段：{}。",
                out.title,
                out.provider.as_deref().unwrap_or("未知来源"),
                out.changed_fields.join("、")
            ),
            json!({ "outcome": out }),
        ))
    }
}

// ─────────────────────────────────────────────────────────────
// 9. search_aggregated_vod（只读：全网多源流媒体聚合检索）
// ─────────────────────────────────────────────────────────────

pub struct SearchVodTool;

#[async_trait]
impl AiTool for SearchVodTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "search_aggregated_vod".into(),
            description: "全网/多源聚合搜索在线流媒体视频（影视/综艺/动漫/纪录片）。\
                          当本地媒体库未收录或查不到某部片子，或者用户明确要求全网搜索、在线找片时使用。"
                .into(),
            params: vec![
                ParamSpec::new("query", ParamKind::String, "搜索片名或影视关键词").required(),
            ],
            risk: Risk::ReadOnly,
        }
    }

    async fn preview(&self, _host: &dyn AiHost, _args: &Value) -> Result<Preview, AiError> {
        Err(AiError::internal(
            "search_aggregated_vod 是只读工具，不需要预览",
        ))
    }

    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError> {
        let q = req_str(args, "query")?;
        let hits = host.search_vod(&q).await?;
        let text = if hits.is_empty() {
            format!("全网聚合搜索未找到与「{q}」匹配的在线片源。建议缩短关键词或检查片名拼写。")
        } else {
            let mut lines = Vec::new();
            for (i, h) in hits.iter().enumerate() {
                let mut meta = Vec::new();
                if let Some(y) = h.year {
                    meta.push(y.to_string());
                }
                if let Some(cat) = &h.category {
                    meta.push(cat.clone());
                }
                meta.push(format!("{} 条线路", h.route_count));
                meta.push(format!("共 {} 集", h.total_episodes));
                if let Some(note) = &h.latest_note {
                    meta.push(note.clone());
                }
                lines.push(format!("{}. 《{}》（{}）", i + 1, h.title, meta.join(" · ")));
            }
            format!(
                "在全网聚合片源中找到 {} 部相关影视：\n{}\n（可直接在客户端顶栏聚合搜索面板或多源列表中点击即播）",
                hits.len(),
                lines.join("\n")
            )
        };
        Ok(result(text, json!({ "query": q, "hits": hits })))
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::InMemoryHost;
    use chrono::{Duration, Utc};

    fn id(n: u8) -> Id {
        Id::parse_str(&format!("00000000-0000-0000-0000-{n:012}")).unwrap()
    }

    fn host() -> InMemoryHost {
        let now = Utc::now();
        let a = MediaHit {
            id: id(1),
            title: "星际穿越".into(),
            kind: MediaKind::Movie,
            year: Some(2014),
            genres: vec!["Sci-Fi".into()],
            rating: Some(9.0),
            runtime_ms: Some(10_140_000),
            size_bytes: Some(30_000_000_000),
            added_at: Some(now - Duration::hours(3)),
            has_poster: false,
            has_subtitle: true,
            has_metadata: true,
            progress: Some(0.4),
            path: None,
        };
        let b = MediaHit {
            id: id(2),
            title: "盗梦空间".into(),
            kind: MediaKind::Movie,
            year: Some(2010),
            genres: vec!["Sci-Fi".into(), "Action".into()],
            rating: Some(8.8),
            runtime_ms: Some(8_820_000),
            size_bytes: Some(15_000_000_000),
            added_at: Some(now - Duration::days(30)),
            has_poster: true,
            has_subtitle: false,
            has_metadata: true,
            progress: None,
            path: None,
        };
        InMemoryHost::new(vec![a, b]).with_now(now)
    }

    #[tokio::test]
    async fn search_media_renders_human_text() {
        let h = host();
        let v = SearchMediaTool
            .invoke(&h, &json!({"kind": "movie", "sort": "size_desc"}))
            .await
            .unwrap();
        let text = v["text"].as_str().unwrap();
        assert!(text.contains("星际穿越"), "{text}");
        // 按体积倒序，大的在前
        assert!(text.find("星际穿越").unwrap() < text.find("盗梦空间").unwrap());
        assert_eq!(v["data"]["items"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn find_missing_poster() {
        let h = host();
        let v = FindMissingPosterTool.invoke(&h, &json!({})).await.unwrap();
        assert!(v["text"].as_str().unwrap().contains("星际穿越"));
        assert!(!v["text"].as_str().unwrap().contains("盗梦空间"));
    }

    #[tokio::test]
    async fn recommend_excludes_watched_by_default() {
        let h = host();
        // 星际穿越 progress=0.4 算看过；默认排除
        let v = RecommendMediaTool
            .invoke(&h, &json!({"limit": 5}))
            .await
            .unwrap();
        let items = v["data"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "盗梦空间");
    }

    #[tokio::test]
    async fn recommend_respects_runtime_cap() {
        let h = host();
        let v = RecommendMediaTool
            .invoke(
                &h,
                &json!({"max_runtime_ms": 9_000_000, "exclude_watched": false}),
            )
            .await
            .unwrap();
        let items = v["data"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["title"], "盗梦空间");
    }

    #[tokio::test]
    async fn get_media_missing_id_is_not_an_error() {
        let h = host();
        let v = GetMediaTool
            .invoke(&h, &json!({"id": "00000000-0000-0000-0000-000000000099"}))
            .await
            .unwrap();
        assert!(v["text"].as_str().unwrap().contains("没有 id"));
    }

    #[tokio::test]
    async fn get_media_rejects_garbage_id() {
        let h = host();
        let e = GetMediaTool
            .invoke(&h, &json!({"id": "not-an-id"}))
            .await
            .unwrap_err();
        assert_eq!(e.kind, crate::error::AiErrorKind::BadArgs);
    }

    #[tokio::test]
    async fn scan_library_preview_is_human_readable() {
        let h = host();
        let p = ScanLibraryTool.preview(&h, &json!({})).await.unwrap();
        assert!(p.summary.contains("全部媒体库"));
        assert!(!p.lines.is_empty());
        // 预览必须提前说清楚代价
        assert!(p.warnings.iter().any(|w| w.contains("CPU")));
    }

    #[tokio::test]
    async fn identify_preview_lists_affected_object() {
        let h = host();
        let p = IdentifyMediaTool
            .preview(&h, &json!({"item_id": id(1).to_string()}))
            .await
            .unwrap();
        assert!(p.summary.contains("星际穿越"));
        assert_eq!(p.lines[0].target, "星际穿越");
    }

    #[tokio::test]
    async fn identify_preview_rejects_unknown_item() {
        let h = host();
        let e = IdentifyMediaTool
            .preview(&h, &json!({"item_id": id(99).to_string()}))
            .await
            .unwrap_err();
        assert_eq!(e.kind, crate::error::AiErrorKind::BadArgs);
    }

    #[tokio::test]
    async fn capability_relays_report() {
        let h = host();
        let v = GetPlaybackCapabilityTool
            .invoke(
                &h,
                &json!({"item_id": id(1).to_string(), "client_id": "web-chrome"}),
            )
            .await
            .unwrap();
        assert!(v["text"].as_str().unwrap().contains("星际穿越"));
    }

    #[tokio::test]
    async fn read_only_tools_refuse_to_preview() {
        // 只读工具被问到 preview 说明调用方写错了分支。
        let h = host();
        assert!(SearchMediaTool.preview(&h, &json!({})).await.is_err());
        assert!(SearchVodTool.preview(&h, &json!({})).await.is_err());
    }

    #[tokio::test]
    async fn search_vod_tool_renders_human_text_and_data() {
        use crate::host::AggregatedVodHit;
        let mut h = host();
        h.vod_hits = vec![AggregatedVodHit {
            title: "繁花".into(),
            category: Some("国产剧".into()),
            year: Some(2023),
            route_count: 3,
            total_episodes: 30,
            latest_note: Some("全30集".into()),
        }];
        let v = SearchVodTool
            .invoke(&h, &json!({"query": "繁花"}))
            .await
            .unwrap();
        let text = v["text"].as_str().unwrap();
        assert!(text.contains("繁花"));
        assert!(text.contains("3 条线路"));
        assert_eq!(v["data"]["hits"][0]["title"], "繁花");
    }
}
