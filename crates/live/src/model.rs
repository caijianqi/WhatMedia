//! 直播的领域模型。
//!
//! 这里的类型是**草稿（Draft）**：解析器的输出，还没有 id、还没有落库。
//! 落库与 id 分配在 `crates/api/src/live.rs`，因为那需要数据库。
//! 分开的好处是解析器能纯函数化、能拿语料离线测。

/// 直播源的形态。
///
/// 这是闭合集合，解析失败一律报错而不是兜底成 `m3u` ——
/// 静默兜底会让用户以为"配了一个 tuner"，实际被当成订阅去下载。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveSourceKind {
    /// M3U 订阅（最常见：一个 URL 或一个本地文件）
    M3u,
    /// 只有节目单，没有频道（给已有频道补 EPG 用）
    Xmltv,
    /// HDHomeRun：自己就输出 `/lineup.json` 与 HTTP 流
    HdHomeRun,
    /// DVB / ATSC 调谐器。**本项目的核心不做**（见方案 L3），
    /// 只保留这个变体以便"外部系统转成 M3U 再导入"时标记来源。
    Dvb,
}

impl LiveSourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::M3u => "m3u",
            Self::Xmltv => "xmltv",
            Self::HdHomeRun => "hdhomerun",
            Self::Dvb => "dvb",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "m3u" | "m3u8" => Some(Self::M3u),
            "xmltv" | "epg" => Some(Self::Xmltv),
            "hdhomerun" | "hdhr" => Some(Self::HdHomeRun),
            "dvb" | "atsc" => Some(Self::Dvb),
            _ => None,
        }
    }

    /// 这个形态需要"下载一个订阅地址"吗。
    pub fn needs_url(self) -> bool {
        matches!(self, Self::M3u | Self::Xmltv)
    }
}

/// 流地址的类型。
///
/// 判断它不为别的，是为了**决定要不要加 `-re`**：
/// 本地文件按实时速率读是对的，UDP 多播加 `-re` 反而会丢包。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Http,
    Https,
    Rtmp,
    /// `udp://@239.1.1.1:1234` 这类多播
    Udp,
    /// 本地或挂载路径
    File,
    /// 认不出来。按 http 处理。
    Unknown,
}

impl StreamKind {
    /// 从 URL 猜。够用就好 —— 猜错了最坏情况是参数不合适，不会崩。
    pub fn from_url(url: &str) -> Self {
        let u = url.trim().to_ascii_lowercase();
        if u.starts_with("https://") {
            Self::Https
        } else if u.starts_with("http://") {
            Self::Http
        } else if u.starts_with("rtmp://") || u.starts_with("rtmps://") {
            Self::Rtmp
        } else if u.starts_with("udp://") || u.starts_with("rtp://") {
            Self::Udp
        } else if u.starts_with("file://") {
            Self::File
        } else if u.contains("://") {
            Self::Unknown
        } else {
            Self::File
        }
    }

    /// ffmpeg 要不要按实时速率读输入。
    ///
    /// **多播不能加 `-re`**：多播本来就是实时推的，再限速只会让 buffer 欠载。
    pub fn wants_realtime_input(self) -> bool {
        matches!(self, Self::Http | Self::Https | Self::Rtmp | Self::Unknown)
    }

    /// 要不要挂重连参数。多播没有"连接"概念，加了反而怪。
    pub fn wants_reconnect(self) -> bool {
        matches!(self, Self::Http | Self::Https | Self::Rtmp)
    }
}

/// 一个待入库的直播源。
#[derive(Debug, Clone)]
pub struct SourceDraft {
    pub name: String,
    pub kind: LiveSourceKind,
    pub url: Option<String>,
    pub epg_url: Option<String>,
    pub number_offset: i64,
}

/// 一个待入库的频道。
///
/// `sort_order` 由解析器按出现顺序给，落库时再加上源的 `number_offset`
/// 算成真正的频道号 —— 多个订阅都从 1 开始会撞号。
#[derive(Debug, Clone)]
pub struct ChannelDraft {
    pub name: String,
    pub stream_url: String,
    pub logo_url: Option<String>,
    pub group_title: Option<String>,
    /// XMLTV 的 channel id（`tvg-id`）。EPG 靠它对上号。
    pub epg_channel_id: Option<String>,
    /// 显式指定的频道号（`tvg-chno`）。没有就按顺序发。
    pub number: Option<i64>,
    pub sort_order: i64,
}

/// 一条待入库的节目。
#[derive(Debug, Clone)]
pub struct ProgramDraft {
    pub epg_channel_id: String,
    pub start_at: String,
    pub end_at: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
}

/// 一个"可以调的频道"：源 + 频道 + 当前节目。
///
/// 这是 API 返回给前端的形状。
#[derive(Debug, Clone, serde::Serialize)]
pub struct TunableChannel {
    pub id: String,
    pub number: Option<i64>,
    pub name: String,
    pub group: Option<String>,
    pub logo_url: Option<String>,
    /// 时移窗口（秒）。0 表示只能看实时边缘。
    pub dvr_window_secs: i64,
    /// 当前在播的节目标题（有 EPG 才有）
    pub now_playing: Option<String>,
}
