//! OpenAPI 3.1 契约导出（P4-2）。
//!
//! **为什么要有这个模块**：前端手写的 API 类型必然与服务端漂移，
//! 而漂移是静默的 —— 少一个字段不会报错，只会在某个页面渲染成 `undefined`。
//! 这里把 axum 的路由与 Rust 结构体变成一份 OpenAPI 文档，
//! 前端类型由它生成（`make openapi`），手写类型被彻底取代。
//!
//! **为什么整个模块在 `openapi` feature 下**：`utoipa` 是一个编译期依赖，
//! 而 `whatmedia-core` 是依赖图的根。为了不让"生成契约"这件事给所有下游
//! 加上一个依赖，schema 标注与这里全部走 feature 开关；默认构建不受影响。

use utoipa::OpenApi;

use crate::metadata::{
    IdentifyRequest, LockRequest, LockResult, ReviewItem, ScrapeAccepted, ScrapeRequest,
    ScrapeRunView, StatusView,
};
use crate::playback::{
    AckView, CreateSessionBody, DecideView, HardwareProfileView, HeartbeatBody, ReasonView,
    SessionView, SubtitleConvertBody, SubtitleItemView, SubtitleView,
};
use crate::{
    CreateLibrary, EpisodeParams, ErrorBody, ErrorDetail, Paging, ScanAccepted, ScanRequest,
    ScanTaskView, SearchParams,
};

/// 契约文档。CI 用 `make openapi-check` 校验它与代码同步。
#[derive(OpenApi)]
#[openapi(
    info(
        title = "WhatMedia Media Server API",
        version = "0.1.0",
        description = "媒体库 / 元数据 / 播放的唯一对外出口。\n\n\
            约定：前缀 `/api/v1`，全 JSON；错误统一 `{ error: { code, message, trace_id } }`；\n\
            列表一律 `{ data: [...], page: { total, page, page_size } }`。"
    ),
    tags(
        (name = "system", description = "健康检查与后台任务"),
        (name = "media", description = "媒体库与条目"),
        (name = "metadata", description = "刮削、锁定与图片缓存"),
        (name = "playback", description = "播放决策、会话与流"),
    ),
    paths(
        // system
        crate::healthz,
        crate::list_tasks,
        crate::start_scan,
        // media
        crate::list_libraries,
        crate::create_library,
        crate::list_media,
        crate::list_movies,
        crate::list_series,
        crate::list_episodes,
        crate::get_item,
        crate::search,
        // metadata
        crate::metadata::start_scrape,
        crate::metadata::scrape_stats,
        crate::metadata::scrape_runs,
        crate::metadata::review_queue,
        crate::metadata::candidates,
        crate::metadata::cache_stats,
        crate::metadata::cache_evict,
        crate::metadata::identify,
        crate::metadata::set_locks,
        crate::metadata::item_images,
        crate::metadata::item_image,
        // playback
        crate::playback::decide_playback,
        crate::playback::create_playback_session,
        crate::playback::session_heartbeat,
        crate::playback::session_stop,
        crate::playback::resume_list,
        crate::playback::list_streams,
        crate::playback::hardware_profile,
        crate::playback::item_subtitles,
        crate::playback::external_subtitle,
        crate::playback::stream_direct,
        crate::playback::stream_remux,
        crate::playback::hls_master,
        crate::playback::hls_stop,
        crate::playback::hls_file,
    ),
    components(schemas(
        // ── 传输层 ────────────────────────────────────────
        ErrorBody,
        ErrorDetail,
        AckViewLike,
        // ── core ─────────────────────────────────────────
        whatmedia_core::MediaKind,
        whatmedia_core::StreamType,
        whatmedia_core::VideoCodec,
        whatmedia_core::AudioCodec,
        whatmedia_core::SubtitleCodec,
        whatmedia_core::HdrType,
        whatmedia_core::Container,
        whatmedia_core::PlaybackStrategy,
        whatmedia_core::PlaybackSource,
        whatmedia_core::MediaStream,
        whatmedia_core::MediaFile,
        whatmedia_core::HardwareKind,
        whatmedia_core::HardwareProfile,
        whatmedia_core::PageInfo,
        whatmedia_database::repo::StreamSession,
        whatmedia_core::Library,
        whatmedia_core::Page<whatmedia_database::repo::ItemSummary>,
        whatmedia_core::Page<whatmedia_database::repo::SeriesSummary>,
        whatmedia_core::Page<whatmedia_database::repo::EpisodeSummary>,
        // ── 媒体库/条目 ───────────────────────────────────
        Paging,
        EpisodeParams,
        SearchParams,
        CreateLibrary,
        ScanRequest,
        ScanAccepted,
        ScanTaskView,
        whatmedia_scanner::ScanProgress,
        whatmedia_database::repo::ItemQuery,
        whatmedia_database::repo::ItemSort,
        whatmedia_database::repo::SortOrder,
        whatmedia_database::repo::ItemSummary,
        whatmedia_database::repo::ItemDetail,
        whatmedia_database::repo::VersionWithFiles,
        whatmedia_database::repo::FileWithStreams,
        whatmedia_database::repo::SeriesSummary,
        whatmedia_database::repo::EpisodeSummary,
        // ── 元数据 ────────────────────────────────────────
        ScrapeRequest,
        ScrapeAccepted,
        ScrapeRunView,
        ReviewItem,
        StatusView,
        IdentifyRequest,
        LockRequest,
        LockResult,
        whatmedia_database::repo::JobStats,
        whatmedia_database::repo::image::CacheStats,
        whatmedia_database::repo::image::ItemImage,
        whatmedia_metadata::CandidateView,
        whatmedia_metadata::EvictReport,
        // ── 播放 ──────────────────────────────────────────
        DecideView,
        ReasonView,
        SubtitleView,
        SessionView,
        CreateSessionBody,
        HeartbeatBody,
        AckView,
        HardwareProfileView,
        SubtitleItemView,
        SubtitleConvertBody,
        whatmedia_database::repo::ResumePoint,
    ))
)]
pub struct ApiDoc;

/// 占位：让 `components(schemas(...))` 里能带上传输层的小信封。
///
/// utoipa 要求 `components` 里列的类型都在文档里出现过，
/// 而 `{ status }` / `{ ok }` 这两种确认信封分散在几个端点上，
/// 用一个别名类型集中声明，免得列三遍。
#[allow(dead_code)]
pub type AckViewLike = AckView;

/// 导出 JSON 形式的契约文档。
pub fn openapi_json() -> String {
    ApiDoc::openapi().to_json().unwrap_or_else(|e| {
        // 序列化失败说明标注写坏了，宁可让导出命令失败也不要产出半份文档。
        panic!("OpenAPI 文档序列化失败：{e}")
    })
}
