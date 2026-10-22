//! 多源并发调度与聚合引擎。

use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinSet;

use crate::circuit_breaker::CircuitBreaker;
use crate::model::AggregatedVodItem;
use crate::normalizer::{cluster_items, ClusterInput};
use crate::provider::VodProvider;

struct ProviderEntry {
    provider: Arc<dyn VodProvider>,
    breaker: Mutex<CircuitBreaker>,
    weight: u32,
    timeout_ms: u64,
}

/// 多源聚合搜索核心引擎
#[derive(Default)]
pub struct AggregatorEngine {
    entries: Vec<Arc<ProviderEntry>>,
}

/// 单源返回事件 (用于流式输出)
#[derive(Debug, Clone)]
pub struct SourceSearchResult {
    pub source_id: String,
    pub source_name: String,
    pub success: bool,
    pub error_msg: Option<String>,
    pub items: Vec<crate::model::RawVodItem>,
}

impl AggregatorEngine {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// 注册一个采集源
    pub fn register(&mut self, provider: Arc<dyn VodProvider>) {
        let weight = provider.config().weight;
        let timeout_ms = provider.config().timeout_ms;
        self.entries.push(Arc::new(ProviderEntry {
            provider,
            breaker: Mutex::new(CircuitBreaker::default()),
            weight,
            timeout_ms,
        }));
        // 按权重升序（权重数值越小优先级越高）
        self.entries.sort_by_key(|e| e.weight);
    }

    /// 清空所有采集源
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// 当前已注册源数量
    pub fn count(&self) -> usize {
        self.entries.len()
    }

    /// 全量并发检索并聚类返回
    pub async fn search(&self, keyword: &str) -> Vec<AggregatedVodItem> {
        let mut join_set = JoinSet::new();

        for entry in &self.entries {
            let entry = Arc::clone(entry);
            let kw = keyword.to_string();

            join_set.spawn(async move {
                let id = entry.provider.id().to_string();
                let name = entry.provider.name().to_string();

                // 熔断探测
                let can_exec = {
                    let mut b = entry.breaker.lock().await;
                    b.can_execute()
                };

                if !can_exec {
                    tracing::warn!("采集源 [{name}] 处于熔断状态，自动跳过");
                    return (id, name, None);
                }

                match entry.provider.search(&kw, entry.timeout_ms).await {
                    Ok(items) => {
                        let mut b = entry.breaker.lock().await;
                        b.record_success();
                        (id, name, Some(items))
                    }
                    Err(e) => {
                        tracing::warn!("采集源 [{name}] 请求失败: {e:#}");
                        let mut b = entry.breaker.lock().await;
                        b.record_failure();
                        (id, name, None)
                    }
                }
            });
        }

        let mut cluster_inputs = Vec::new();

        while let Some(res) = join_set.join_next().await {
            if let Ok((source_id, source_name, Some(items))) = res {
                for item in items {
                    cluster_inputs.push(ClusterInput {
                        source_id: source_id.clone(),
                        source_name: source_name.clone(),
                        item,
                    });
                }
            }
        }

        cluster_items(cluster_inputs)
    }

    /// 流式并发检索 (通过 mpsc 发送每源检索到的数据)
    pub async fn search_stream(
        &self,
        keyword: &str,
        tx: tokio::sync::mpsc::Sender<SourceSearchResult>,
    ) {
        let mut join_set = JoinSet::new();

        for entry in &self.entries {
            let entry = Arc::clone(entry);
            let kw = keyword.to_string();
            let tx = tx.clone();

            join_set.spawn(async move {
                let source_id = entry.provider.id().to_string();
                let source_name = entry.provider.name().to_string();

                let can_exec = {
                    let mut b = entry.breaker.lock().await;
                    b.can_execute()
                };

                if !can_exec {
                    let _ = tx
                        .send(SourceSearchResult {
                            source_id,
                            source_name,
                            success: false,
                            error_msg: Some("Circuit breaker open".into()),
                            items: Vec::new(),
                        })
                        .await;
                    return;
                }

                match entry.provider.search(&kw, entry.timeout_ms).await {
                    Ok(items) => {
                        {
                            let mut b = entry.breaker.lock().await;
                            b.record_success();
                        }
                        let _ = tx
                            .send(SourceSearchResult {
                                source_id,
                                source_name,
                                success: true,
                                error_msg: None,
                                items,
                            })
                            .await;
                    }
                    Err(e) => {
                        {
                            let mut b = entry.breaker.lock().await;
                            b.record_failure();
                        }
                        let _ = tx
                            .send(SourceSearchResult {
                                source_id,
                                source_name,
                                success: false,
                                error_msg: Some(e.to_string()),
                                items: Vec::new(),
                            })
                            .await;
                    }
                }
            });
        }

        while join_set.join_next().await.is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{RawVodItem, VodSourceConfig};
    use anyhow::Result;
    use async_trait::async_trait;

    struct MockProvider {
        config: VodSourceConfig,
        should_fail: bool,
        results: Vec<RawVodItem>,
    }

    #[async_trait]
    impl VodProvider for MockProvider {
        fn id(&self) -> &str {
            &self.config.id
        }
        fn name(&self) -> &str {
            &self.config.name
        }
        fn config(&self) -> &VodSourceConfig {
            &self.config
        }
        async fn search(&self, _keyword: &str, _timeout_ms: u64) -> Result<Vec<RawVodItem>> {
            if self.should_fail {
                anyhow::bail!("Mock provider error");
            }
            Ok(self.results.clone())
        }
        async fn get_detail(&self, _vod_id: &str, _timeout_ms: u64) -> Result<Option<RawVodItem>> {
            Ok(self.results.first().cloned())
        }
    }

    #[tokio::test]
    async fn test_engine_concurrent_search_and_cluster() {
        let mut engine = AggregatorEngine::new();

        // 源 1：成功
        let p1 = Arc::new(MockProvider {
            config: VodSourceConfig {
                id: "src_1".into(),
                name: "量子源".into(),
                api_url: "https://lz.test".into(),
                is_active: true,
                weight: 10,
                timeout_ms: 1000,
            },
            should_fail: false,
            results: vec![RawVodItem {
                vod_id: "101".into(),
                vod_name: "奥本海默 [4K]".into(),
                vod_sub: None,
                vod_year: Some(2023),
                vod_pic: Some("https://lz.test/opp.jpg".into()),
                vod_remarks: Some("HD".into()),
                type_name: Some("电影".into()),
                vod_actor: None,
                vod_director: None,
                vod_content: None,
                vod_play_from: Some("lzm3u8".into()),
                vod_play_url: Some("正片$https://lz.test/1.m3u8".into()),
            }],
        });

        // 源 2：故障
        let p2 = Arc::new(MockProvider {
            config: VodSourceConfig {
                id: "src_2".into(),
                name: "故障源".into(),
                api_url: "https://bad.test".into(),
                is_active: true,
                weight: 20,
                timeout_ms: 1000,
            },
            should_fail: true,
            results: Vec::new(),
        });

        // 源 3：成功 (同名，触发聚类)
        let p3 = Arc::new(MockProvider {
            config: VodSourceConfig {
                id: "src_3".into(),
                name: "非凡源".into(),
                api_url: "https://ff.test".into(),
                is_active: true,
                weight: 15,
                timeout_ms: 1000,
            },
            should_fail: false,
            results: vec![RawVodItem {
                vod_id: "201".into(),
                vod_name: "奥本海默【国英双字】".into(),
                vod_sub: None,
                vod_year: Some(2023),
                vod_pic: None,
                vod_remarks: None,
                type_name: None,
                vod_actor: None,
                vod_director: None,
                vod_content: None,
                vod_play_from: Some("ffm3u8".into()),
                vod_play_url: Some("HD$https://ff.test/2.m3u8".into()),
            }],
        });

        engine.register(p1);
        engine.register(p2);
        engine.register(p3);

        let results = engine.search("奥本海默").await;

        // 验证即使源 2 故障，整体搜索依然成功，且源 1 与源 3 聚合为 1 个实体
        assert_eq!(results.len(), 1);
        let opp = &results[0];
        assert_eq!(opp.clean_title, "奥本海默");
        assert_eq!(opp.routes.len(), 2);
    }
}
