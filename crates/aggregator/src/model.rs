//! 多源聚合核心数据模型。

use serde::{Deserialize, Deserializer, Serialize};

/// 视频采集源配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VodSourceConfig {
    /// 唯一标识，如 "api_lz"
    pub id: String,
    /// 友好名称，如 "量子资源"
    pub name: String,
    /// MacCMS API 接口根地址，如 "https://cj.lziapi.com/api.php/provide/vod/"
    pub api_url: String,
    /// 是否激活
    #[serde(default = "default_true")]
    pub is_active: bool,
    /// 权重（数字越小优先级越高）
    #[serde(default = "default_weight")]
    pub weight: u32,
    /// 单源超时阈值 (毫秒)
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
}

fn default_true() -> bool {
    true
}

fn default_weight() -> u32 {
    100
}

fn default_timeout() -> u64 {
    2500
}

/// 选集信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VodEpisode {
    /// 集数名称，如 "第01集"、"HD国语"、"预告片"
    pub name: String,
    /// 播放流地址，通常为 m3u8
    pub play_url: String,
    /// 排序序号 (从 1 开始)
    pub index: u32,
}

/// 播放线路 (对应具体采集站的一个播放列表)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct VodRoute {
    /// 采集源 ID
    pub source_id: String,
    /// 采集源名称或线路名，如 "量子资源 [lzm3u8]"
    pub source_name: String,
    /// 原始编码标识，如 "lzm3u8"
    pub play_from: String,
    /// 选集列表
    pub episodes: Vec<VodEpisode>,
}

/// 聚合后的独立影视实体
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AggregatedVodItem {
    /// 确定性聚合 ID (如 SHA256 派生)
    pub id: String,
    /// 规范化主标题
    pub title: String,
    /// 清洗后的纯净比对标题 (全小写、去修饰词)
    pub clean_title: String,
    /// 原始标题或副标题
    pub sub_title: Option<String>,
    /// 上映年份
    pub year: Option<u32>,
    /// 封面海报 URL
    pub poster: Option<String>,
    /// 分类名称 (电影 / 电视剧 / 动漫等)
    pub type_name: Option<String>,
    /// 更新状态 (如 "更新至30集")
    pub remark: Option<String>,
    /// 主演
    pub actor: Option<String>,
    /// 导演
    pub director: Option<String>,
    /// 简介
    pub content: Option<String>,
    /// 聚合的多条播放线路
    pub routes: Vec<VodRoute>,
}

/// MacCMS V10 返回的单条原始数据
///
/// 兼容类型不确定性（如 `vod_id` / `vod_year` 可能是数字也可能是字符串）
#[derive(Debug, Clone, Deserialize)]
pub struct RawVodItem {
    #[serde(deserialize_with = "deserialize_string_or_number")]
    pub vod_id: String,
    #[serde(default)]
    pub vod_name: String,
    #[serde(default)]
    pub vod_sub: Option<String>,
    #[serde(default, deserialize_with = "deserialize_opt_u32")]
    pub vod_year: Option<u32>,
    #[serde(default)]
    pub vod_pic: Option<String>,
    #[serde(default)]
    pub vod_remarks: Option<String>,
    #[serde(default)]
    pub type_name: Option<String>,
    #[serde(default)]
    pub vod_actor: Option<String>,
    #[serde(default)]
    pub vod_director: Option<String>,
    #[serde(default)]
    pub vod_content: Option<String>,
    #[serde(default)]
    pub vod_play_from: Option<String>,
    #[serde(default)]
    pub vod_play_url: Option<String>,
}

/// MacCMS V10 标准响应格式
#[derive(Debug, Clone, Deserialize)]
pub struct MacCmsResponse {
    #[serde(default)]
    pub code: Option<serde_json::Value>,
    #[serde(default)]
    pub msg: Option<String>,
    #[serde(default)]
    pub page: Option<serde_json::Value>,
    #[serde(default)]
    pub pagecount: Option<serde_json::Value>,
    #[serde(default)]
    pub limit: Option<serde_json::Value>,
    #[serde(default)]
    pub total: Option<serde_json::Value>,
    #[serde(default)]
    pub list: Vec<RawVodItem>,
}

/// 兼容 string 或 number 的反序列化器
fn deserialize_string_or_number<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(deserializer)?;
    match v {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Ok(String::new()),
    }
}

/// 兼容 "2023" 或 2023 或 "" 或 null 的年份反序列化器
fn deserialize_opt_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let v = Option::<serde_json::Value>::deserialize(deserializer)?;
    match v {
        Some(serde_json::Value::Number(n)) => Ok(n.as_u64().map(|x| x as u32)),
        Some(serde_json::Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                Ok(None)
            } else {
                Ok(s.parse::<u32>().ok())
            }
        }
        _ => Ok(None),
    }
}
