//! 元数据与图片：让媒体库"看得懂"。
//!
//! 模块分层（依赖方向自上而下，不得反向）：
//!
//! ```text
//! pipeline   编排：队列 / 分数决策 / 落库
//!   ├─ matcher  候选打分与自动确认判定
//!   ├─ merge    字段级锁定与优先级链（NFO > 手动 > Provider）
//!   ├─ images   图片缓存策略（LRU）
//!   └─ tmdb / bangumi / nfo   具体数据源
//!        └─ transport  HTTP 抽象（可注入 fixture，离线可测）
//! ```
//!
//! 一条铁律：**本 crate 不写 SQL**。所有持久化走 `whatmedia-database::repo`。

pub mod bangumi;
pub mod images;
pub mod matcher;
pub mod merge;
pub mod nfo;
pub mod pipeline;
pub mod ratelimit;
pub mod thumbs;
pub mod tmdb;
pub mod transport;

pub use bangumi::{BangumiConfig, BangumiProvider};
pub use images::{placeholder_svg, EvictReport, ImageCache, ImageCacheConfig};
pub use matcher::{pick_auto, score_candidates, Confidence, MatchThresholds, ScoredCandidate};
pub use merge::{parse_locked, serialize_locked, set_locked, ItemMeta, MergeContext, MergeReport};
pub use pipeline::{Cancel, CandidateView, Outcome, ScrapeConfig, ScrapeStats, Scraper};
pub use ratelimit::RateLimiter;
pub use thumbs::{thumbnail_timestamp, FfmpegThumbRunner, VARIANTS};
pub use tmdb::{TmdbConfig, TmdbProvider};
pub use transport::{FixtureTransport, HttpTransport, ReqwestTransport};

use std::sync::Arc;

use whatmedia_core::MetadataProvider;

/// 按配置装配 Provider 链。顺序即优先级：前面的先试，分数高的胜出。
///
/// Bangumi 放在 TMDB 之后：中文动漫在 Bangumi 上命中率更高，
/// 但 Bangumi 没有演职员和 backdrop，所以两者靠打分竞争而不是互斥。
pub fn build_providers(cfg: &ProviderChainConfig) -> Vec<Arc<dyn MetadataProvider>> {
    let mut out: Vec<Arc<dyn MetadataProvider>> = Vec::new();

    if let Some(key) = &cfg.tmdb_api_key {
        out.push(Arc::new(TmdbProvider::new(TmdbConfig {
            api_key: Some(key.clone()),
            language: cfg.language.clone(),
            requests_per_second: cfg.requests_per_second,
            ..Default::default()
        })));
    } else if let Some(token) = &cfg.tmdb_bearer {
        out.push(Arc::new(TmdbProvider::new(TmdbConfig {
            bearer: Some(token.clone()),
            language: cfg.language.clone(),
            requests_per_second: cfg.requests_per_second,
            ..Default::default()
        })));
    }

    if cfg.bangumi_enabled {
        out.push(Arc::new(BangumiProvider::new(BangumiConfig {
            requests_per_second: cfg.requests_per_second.min(2.0),
            ..Default::default()
        })));
    }

    out
}

#[derive(Debug, Clone, Default)]
pub struct ProviderChainConfig {
    pub tmdb_api_key: Option<String>,
    /// v4 Bearer Token。两个都配时 TMDB 用 bearer。
    pub tmdb_bearer: Option<String>,
    pub bangumi_enabled: bool,
    pub language: String,
    pub requests_per_second: f64,
}
