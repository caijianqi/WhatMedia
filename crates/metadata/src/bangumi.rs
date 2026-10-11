//! Bangumi.tv Provider（动漫）。
//!
//! 中文动漫圈的主要数据源。TMDB 对日番的中文译名覆盖率很差，
//! `葬送的芙莉莲` 在 TMDB 上常常只有日文原名 —— 这就是必须有第二个 Provider 的原因。
//!
//! API：https://api.bgm.tv/v0/ （公开读接口，无需鉴权）

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use whatmedia_core::{MetadataImage, MetadataProvider, MetadataQuery, MetadataRecord, Result};

use crate::ratelimit::{retry, RateLimiter};
use crate::transport::{HttpTransport, ReqwestTransport};

#[derive(Debug, Clone)]
pub struct BangumiConfig {
    pub base_url: String,
    pub language: String,
    pub requests_per_second: f64,
    /// 2 = 动画。搜别的类型会召回一堆游戏和书籍。
    pub subject_type: u8,
}

impl Default for BangumiConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.bgm.tv/v0".into(),
            language: "zh-CN".into(),
            // Bangumi 没有明说配额，但它是小站，别打太狠。
            requests_per_second: 2.0,
            subject_type: 2,
        }
    }
}

pub struct BangumiProvider {
    cfg: BangumiConfig,
    transport: Arc<dyn HttpTransport>,
    limiter: RateLimiter,
}

impl BangumiProvider {
    pub fn new(cfg: BangumiConfig) -> Self {
        let limiter = RateLimiter::per_second(cfg.requests_per_second);
        Self {
            cfg,
            transport: Arc::new(ReqwestTransport::new()),
            limiter,
        }
    }

    pub fn with_transport(cfg: BangumiConfig, transport: Arc<dyn HttpTransport>) -> Self {
        let limiter = RateLimiter::per_second(cfg.requests_per_second);
        Self {
            cfg,
            transport,
            limiter,
        }
    }

    async fn get_json(&self, path: &str, params: &[(&str, String)]) -> Result<Value> {
        let mut url = format!("{}{}", self.cfg.base_url.trim_end_matches('/'), path);
        let mut first = true;
        for (k, v) in params {
            url.push_str(if first { "?" } else { "&" });
            first = false;
            url.push_str(k);
            url.push('=');
            url.push_str(&crate::tmdb::utf8_urlencode(v));
        }

        let transport = self.transport.clone();
        let url2 = url.clone();
        retry(3, move || {
            let transport = transport.clone();
            let url = url2.clone();
            let limiter = &self.limiter;
            async move {
                limiter.acquire().await;
                transport.get(&url, None).await?.json()
            }
        })
        .await
        .map_err(|e| whatmedia_core::WhatMediaError::Metadata {
            provider: "bangumi".into(),
            message: e.to_string(),
        })
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Bangumi 的中文名在 `name_cn`，原名在 `name`。
/// 中文用户更希望看到译名，所以 `title` 用 `name_cn` 优先。
fn record_from_subject(v: &Value) -> MetadataRecord {
    let title = str_of(v, "name_cn")
        .or_else(|| str_of(v, "name"))
        .unwrap_or_default();

    let mut images = Vec::new();
    // `images.large` 是 2x 海报，够用；`common` 太小。
    let url = v
        .pointer("/images/large")
        .or_else(|| v.pointer("/images/common"))
        .and_then(|i| i.as_str())
        .map(|s| s.to_string());
    if let Some(u) = url {
        images.push(MetadataImage {
            kind: "poster".into(),
            url: u,
            width: None,
            height: None,
            vote: None,
        });
    }

    MetadataRecord {
        provider: "bangumi".into(),
        provider_id: v.get("id").map(|i| i.to_string()).unwrap_or_default(),
        title,
        original_title: str_of(v, "name"),
        sort_title: None,
        year: str_of(v, "date")
            .and_then(|d| d.get(0..4).map(|y| y.to_string()))
            .and_then(|y| y.parse().ok()),
        tagline: None,
        overview: str_of(v, "summary"),
        runtime_ms: None,
        rating: v.pointer("/rating/score").and_then(|s| s.as_f64()),
        genres: Vec::new(),
        images,
        credits: Vec::new(),
    }
}

#[async_trait]
impl MetadataProvider for BangumiProvider {
    fn id(&self) -> &str {
        "bangumi"
    }

    async fn search_movie(&self, query: &MetadataQuery) -> Result<Vec<MetadataRecord>> {
        // Bangumi 是一个库，动画电影也在里面，所以 search_movie 与 series 走同一条路。
        self.search_series(query).await
    }

    async fn search_series(&self, query: &MetadataQuery) -> Result<Vec<MetadataRecord>> {
        let v = self
            .get_json(
                "/search/subjects",
                &[
                    ("keyword", query.title.clone()),
                    ("filter[type]", self.cfg.subject_type.to_string()),
                ],
            )
            .await?;

        // 搜索结果里 `data` 是数组，且每项带 `id`；详情字段是残缺的，
        // 所以只取 id 再逐个拉详情。多花几个请求，换来字段一致性。
        let ids: Vec<String> = v
            .pointer("/data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|x| x.get("id").map(|i| i.to_string()))
                    .take(5)
                    .collect()
            })
            .unwrap_or_default();

        let mut out = Vec::new();
        for id in ids {
            if let Ok(r) = self.get_series(&id).await {
                if !r.title.is_empty() {
                    out.push(r);
                }
            }
        }
        Ok(out)
    }

    async fn get_movie(&self, provider_id: &str) -> Result<MetadataRecord> {
        self.get_series(provider_id).await
    }

    async fn get_series(&self, provider_id: &str) -> Result<MetadataRecord> {
        let v = self
            .get_json(&format!("/subjects/{provider_id}"), &[])
            .await?;
        Ok(record_from_subject(&v))
    }

    async fn get_episode(
        &self,
        _series_id: &str,
        _season: i32,
        episode: i32,
    ) -> Result<MetadataRecord> {
        // Bangumi 的"集"是扁平的（没有季概念，第 25 集就是第 25 集），
        // 而 WhatMedia 的模型是季/集两层。这里不做猜测转换 ——
        // 拿不到就返回一条只带集号的记录，由上层决定要不要用。
        Ok(MetadataRecord {
            provider: "bangumi".into(),
            provider_id: format!("{_series_id}:{episode}"),
            title: format!("第 {episode} 集"),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FixtureTransport;

    const SUBJECT: &str = r#"{
      "id": 447051, "name": "葬送のフリーレン", "name_cn": "葬送的芙莉莲",
      "date": "2023-09-29", "summary": "魔王被打倒之后",
      "images": {"common": "https://xxx/s.jpg", "large": "https://xxx/l.jpg"},
      "rating": {"score": 8.4, "total": 1234}
    }"#;

    const SEARCH: &str = r#"{"data": [{"id": 447051}, {"id": 999}]}"#;

    #[tokio::test]
    async fn prefers_chinese_title() {
        let p = BangumiProvider::with_transport(
            BangumiConfig::default(),
            Arc::new(FixtureTransport::new(vec![(
                "https://api.bgm.tv/v0/subjects/447051",
                SUBJECT,
            )])),
        );
        let r = p.get_series("447051").await.unwrap();
        assert_eq!(r.title, "葬送的芙莉莲", "中文译名应优先");
        assert_eq!(r.original_title.as_deref(), Some("葬送のフリーレン"));
        assert_eq!(r.year, Some(2023));
        assert_eq!(r.rating, Some(8.4));
        assert!(r.images[0].url.contains("l.jpg"), "应用 large 而非 common");
    }

    #[tokio::test]
    async fn search_expands_to_details() {
        let p = BangumiProvider::with_transport(
            BangumiConfig::default(),
            Arc::new(FixtureTransport::new(vec![
                ("https://api.bgm.tv/v0/search/subjects", SEARCH),
                ("https://api.bgm.tv/v0/subjects/447051", SUBJECT),
            ])),
        );
        let r = p
            .search_series(&MetadataQuery {
                title: "葬送的芙莉莲".into(),
                year: None,
                language: None,
                episode_title: None,
            })
            .await
            .unwrap();
        // 999 在 fixture 里没有详情，应被跳过而不是报错
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "葬送的芙莉莲");
    }
}
