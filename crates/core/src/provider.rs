//! 六个核心抽象契约。
//!
//! **签名冻结（P0）**：见 docs/adr/0000-abstractions.md。
//! 修改任一既有方法签名都视为架构变更，必须先写 ADR。
//! 允许：新增带默认实现的方法、新增类型、新增实现。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::broadcast;

use crate::error::Result;
use crate::model::{ByteRange, HdrType, Id, VideoCodec};

// ─────────────────────────────────────────────────────────────
// ① StorageProvider —— 上层媒体系统不得关心文件位于什么存储
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub size: Option<i64>,
    pub modified_ns: Option<i64>,
}

#[async_trait]
pub trait StorageProvider: Send + Sync {
    fn id(&self) -> &str;

    async fn list(&self, dir: &str) -> Result<Vec<StorageEntry>>;
    async fn stat(&self, path: &str) -> Result<StorageEntry>;
    async fn exists(&self, path: &str) -> Result<bool>;

    /// Direct Play 与代理播放的核心：支持 Range 的局部读取。
    async fn range_read(&self, path: &str, range: ByteRange) -> Result<Vec<u8>>;
    async fn read(&self, path: &str) -> Result<Vec<u8>>;
    async fn write(&self, path: &str, data: &[u8]) -> Result<()>;

    async fn rename(&self, from: &str, to: &str) -> Result<()>;
    async fn delete(&self, path: &str) -> Result<()>;
}

// ─────────────────────────────────────────────────────────────
// ② MetadataProvider —— 必须 Provider 化，不得绑定单一数据源
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataQuery {
    pub title: String,
    pub year: Option<i32>,
    pub language: Option<String>,
    /// 集标题。动漫里 `Frieren - 07` 的目录提示有时比剧名更准。
    #[serde(default)]
    pub episode_title: Option<String>,
}

/// 一次刮削能拿到的全部信息。
///
/// 字段只会增加不会删除（见 ADR-0000：模型定稿后只允许加字段）。
/// 所有 `Option` 都是"Provider 没给"而不是"没有"，合并时据此跳过。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetadataRecord {
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    pub original_title: Option<String>,
    /// 排序用的标题。TMDB 会给 `The Matrix` 一个 `Matrix, The` 之类的形式。
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub tagline: Option<String>,
    pub overview: Option<String>,
    pub runtime_ms: Option<i64>,
    pub rating: Option<f64>,
    pub genres: Vec<String>,
    pub images: Vec<MetadataImage>,
    pub credits: Vec<MetadataCredit>,
}

/// 演职员。P2 的"人物 / 演职员表关系"就落在这张表上。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataCredit {
    pub name: String,
    /// `actor` / `director` / `writer`。存小写，查询时不要再做大小写转换。
    pub role: String,
    pub character: Option<String>,
    pub sort_order: i32,
    /// Provider 侧的人物 id，用于以后抓人物头像。
    pub provider_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataImage {
    /// `poster` / `backdrop` / `still` / `logo` / `profile`
    pub kind: String,
    pub url: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// 评分用的偏好值：同一 kind 多张图时挑最高的。
    #[serde(default)]
    pub vote: Option<f64>,
}

#[async_trait]
pub trait MetadataProvider: Send + Sync {
    fn id(&self) -> &str;

    async fn search_movie(&self, query: &MetadataQuery) -> Result<Vec<MetadataRecord>>;
    async fn search_series(&self, query: &MetadataQuery) -> Result<Vec<MetadataRecord>>;
    async fn get_movie(&self, provider_id: &str) -> Result<MetadataRecord>;
    async fn get_series(&self, provider_id: &str) -> Result<MetadataRecord>;
    async fn get_episode(
        &self,
        series_id: &str,
        season: i32,
        episode: i32,
    ) -> Result<MetadataRecord>;
}

// ─────────────────────────────────────────────────────────────
// ③ HardwareAccelerationProvider —— 把不同硬件统一起来
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HardwareKind {
    /// 软件兜底。任何环境都必须可用。
    Software,
    IntelQsv,
    NvidiaNvenc,
    AmdVaapi,
    AmdAmf,
    AppleVideoToolbox,
    /// Rockchip MPP。FFmpeg 主线支持不完整，注意可切换后端设计。
    RockchipMpp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProfile {
    pub kind: HardwareKind,
    pub decoders: Vec<VideoCodec>,
    pub encoders: Vec<VideoCodec>,
    pub supports_zero_copy: bool,
    pub supports_tone_mapping: bool,
    pub max_concurrent_sessions: u32,
}

pub trait HardwareAccelerationProvider: Send + Sync {
    fn id(&self) -> &str;
    fn kind(&self) -> HardwareKind;

    fn probe(&self) -> Result<HardwareProfile>;
    fn supports_decode(&self, codec: &VideoCodec) -> bool;
    fn supports_encode(&self, codec: &VideoCodec) -> bool;

    /// 返回追加到 ffmpeg 命令行的参数片段（不含 `-i`）。
    fn decoder_args(&self, codec: &VideoCodec) -> Vec<String>;
    fn encoder_args(&self, codec: &VideoCodec) -> Vec<String>;
    fn scale_filter(&self, width: u32, height: u32) -> Option<String>;
    fn tonemap_filter(&self, from: HdrType, to: HdrType) -> Option<String>;
}

// ─────────────────────────────────────────────────────────────
// ④ ModelProvider —— 不绑定单一大模型服务商
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub system: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<JsonValue>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub content: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: JsonValue,
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;
    fn supports_tool_call(&self) -> bool;
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;
}

// ─────────────────────────────────────────────────────────────
// ⑤ MediaPlayer —— Web / 桌面 / 移动 / TV 共用同一套上层接口
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerState {
    Idle,
    Loading,
    Playing,
    Paused,
    Buffering,
    Ended,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlayerEvent {
    StateChanged { state: PlayerState },
    Position { position_ms: i64 },
    Duration { duration_ms: i64 },
    Error { message: String },
    Ended,
}

#[async_trait]
pub trait MediaPlayer: Send + Sync {
    async fn load(&self, source: crate::model::PlaybackSource) -> Result<()>;
    async fn play(&self) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn seek(&self, position_ms: i64) -> Result<()>;

    async fn set_audio_track(&self, stream_index: i32) -> Result<()>;
    async fn set_subtitle_track(&self, stream_index: Option<i32>) -> Result<()>;
    async fn set_quality(&self, height: Option<u32>) -> Result<()>;

    async fn state(&self) -> PlayerState;
    async fn position_ms(&self) -> i64;
    async fn duration_ms(&self) -> Option<i64>;

    /// 播放状态、进度、错误的统一事件通道（SSE/WS 与 UI 都消费它）。
    fn subscribe(&self) -> broadcast::Receiver<PlayerEvent>;
}

// ─────────────────────────────────────────────────────────────
// ⑥ AiTool —— AI 只能通过 Tool 触达业务，禁止直连 SQL/Shell/FS
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    /// 查询 / 搜索 / 分析 / 推荐 / 诊断 —— 可自动执行。
    ReadOnly,
    /// 删除 / 移动 / 重命名 / 批量修改 / 清理 —— 必须 Plan→Preview→Confirm→Execute。
    Dangerous,
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub user_id: Option<Id>,
    pub device_id: Option<String>,
    pub trace_id: String,
    /// 危险操作的二次确认凭据。为空即拒绝执行 Dangerous 工具。
    pub confirmation: Option<String>,
}

#[async_trait]
pub trait AiTool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn risk(&self) -> ToolRisk;
    /// JSON Schema，供模型做 function calling。
    fn schema(&self) -> JsonValue;

    /// Dangerous 工具在 ctx.confirmation 为空时必须返回 MosaicError::AiDenied。
    async fn invoke(&self, ctx: ToolContext, args: JsonValue) -> Result<JsonValue>;
}
