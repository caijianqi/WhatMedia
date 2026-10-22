//! WhatMedia 多源影视聚合模块。
//!
//! 提供 MacCMS V10 标准协议解析、Tokio 高并发广播检索、
//! 滑动窗口自适应熔断、标题清洗与多线路聚类能力。

pub mod circuit_breaker;
pub mod engine;
pub mod model;
pub mod normalizer;
pub mod parser;
pub mod provider;

pub use circuit_breaker::{CircuitBreaker, CircuitState};
pub use engine::{AggregatorEngine, SourceSearchResult};
pub use model::{
    AggregatedVodItem, MacCmsResponse, RawVodItem, VodEpisode, VodRoute, VodSourceConfig,
};
pub use normalizer::{cluster_items, string_similarity, ClusterInput, TitleNormalizer};
pub use parser::{parse_episodes, parse_routes};
pub use provider::{MacCmsProvider, VodProvider};
