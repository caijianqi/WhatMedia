//! 电视直播（Live TV / IPTV）的**纯逻辑层**：订阅解析、频道模型、时移窗口计算。
//!
//! 这一层刻意不碰 I/O：不建 HTTP 客户端、不碰数据库、不起进程。
//! 拉流与转封装由 `crates/transcoder` 负责，持久化与端点由 `crates/api` 负责。
//!
//! 这样做的理由是**可测**：M3U 的方言比想象中多（有的没有 `#EXTINF`、
//! 有的属性顺序不一样、有的用 `tvg-logo` 有的用 `logo`、还有 BOM 和 CRLF），
//! 靠真拉一个订阅来测是不现实的，必须能拿语料离线跑。
//!
//! 与 `media_item` 的关系见 `直播方案` 原则 1：
//! 直播不是媒体资产，这里的类型**不复用** `MediaItem`。

pub mod m3u;
pub mod model;
pub mod xmltv;

pub use m3u::parse_m3u;
pub use model::{
    ChannelDraft, LiveSourceKind, ProgramDraft, SourceDraft, StreamKind, TunableChannel,
};
pub use xmltv::parse_xmltv;

/// 解析失败。
///
/// 刻意做成一个扁平枚举而不是 eachhow：调用方（API 层）需要区分
/// "这个订阅本身是坏的，别重试了" 与 "网络抖了一下，下次再来"。
#[derive(Debug, thiserror::Error)]
pub enum LiveError {
    #[error("订阅内容不是 M3U：缺少 #EXTM3U 头")]
    NotAPlaylist,
    #[error("订阅是空的：一个频道都没有")]
    EmptyPlaylist,
    #[error("XMLTV 格式错误：{0}")]
    BadXml(String),
}

/// 判断一个订阅文本是不是 M3U。
///
/// 单独抽出来自 `parse_m3u` 是因为 API 层要在**下载完之后、入库之前**先判一次，
/// 给用户一个"你贴的这不是 M3U"的明确反馈，而不是"同步成功，0 个频道"。
pub fn looks_like_m3u(text: &str) -> bool {
    let head = text.trim_start_matches('\u{feff}');
    head.starts_with("#EXTM3U")
}
