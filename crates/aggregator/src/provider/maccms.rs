//! MacCMS V10 标准协议 Provider 实现。

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::time::Duration;

use super::VodProvider;
use crate::model::{MacCmsResponse, RawVodItem, VodSourceConfig};

pub struct MacCmsProvider {
    config: VodSourceConfig,
    client: reqwest::Client,
}

impl MacCmsProvider {
    pub fn new(config: VodSourceConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .unwrap_or_default();
        Self { config, client }
    }

    pub fn with_client(config: VodSourceConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }
}

#[async_trait]
impl VodProvider for MacCmsProvider {
    fn id(&self) -> &str {
        &self.config.id
    }

    fn name(&self) -> &str {
        &self.config.name
    }

    fn config(&self) -> &VodSourceConfig {
        &self.config
    }

    async fn search(&self, keyword: &str, timeout_ms: u64) -> Result<Vec<RawVodItem>> {
        let url = self.config.api_url.trim_end_matches('/');
        let endpoint = if url.contains('?') {
            format!("{url}&ac=detail&wd={keyword}")
        } else {
            format!("{url}?ac=detail&wd={keyword}")
        };

        let resp = self
            .client
            .get(&endpoint)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await
            .with_context(|| {
                format!("请求 MacCMS 采集源 [{}] 失败: {endpoint}", self.config.name)
            })?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "MacCMS 采集源 [{}] 返回异常 HTTP 状态码: {}",
                self.config.name,
                resp.status()
            );
        }

        let body_text = resp.text().await.context("读取响应文本失败")?;
        let parsed: MacCmsResponse = serde_json::from_str(&body_text)
            .with_context(|| format!("解析 MacCMS 源 [{}] JSON 失败", self.config.name))?;

        Ok(parsed.list)
    }

    async fn get_detail(&self, vod_id: &str, timeout_ms: u64) -> Result<Option<RawVodItem>> {
        let url = self.config.api_url.trim_end_matches('/');
        let endpoint = if url.contains('?') {
            format!("{url}&ac=detail&ids={vod_id}")
        } else {
            format!("{url}?ac=detail&ids={vod_id}")
        };

        let resp = self
            .client
            .get(&endpoint)
            .timeout(Duration::from_millis(timeout_ms))
            .send()
            .await
            .with_context(|| format!("请求详情失败: {endpoint}"))?;

        let parsed: MacCmsResponse = resp.json().await.context("解析详情 JSON 失败")?;
        Ok(parsed.list.into_iter().next())
    }
}
