//! TMDB Provider。
//!
//! 只做三件事：拼 URL、解析 JSON、把结果翻译成 `MetadataRecord`。
//! 重试与限流在 `ratelimit` / `transport` 里，不在这里 —— 否则每个 Provider 都要抄一遍。

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use mosaic_core::{
    MetadataCredit, MetadataImage, MetadataProvider, MetadataQuery, MetadataRecord, Result,
};

use crate::ratelimit::{retry, RateLimiter};
use crate::transport::{HttpTransport, ReqwestTransport};

/// 各 kind 的默认尺寸。图片是元数据里体积最大的部分，
/// 选错尺寸会让 1,000 部电影的缓存从 300MB 变成 3GB。
const IMAGE_SIZES: &[(&str, &str)] = &[
    ("poster", "w500"),
    ("backdrop", "w1280"),
    ("still", "w1280"),
    ("logo", "w500"),
    ("profile", "w185"),
];

fn size_for(kind: &str) -> &'static str {
    IMAGE_SIZES
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, s)| *s)
        .unwrap_or("original")
}

#[derive(Debug, Clone)]
pub struct TmdbConfig {
    /// v3 API Key（走 query 参数）。
    pub api_key: Option<String>,
    /// v4 Bearer Token。两个都给时优先用它。
    pub bearer: Option<String>,
    pub language: String,
    pub base_url: String,
    pub image_base: String,
    pub include_adult: bool,
    /// 演职员裁剪上限。一部片能有一百多号人，全存下来没有意义。
    pub max_cast: usize,
    pub requests_per_second: f64,
}

impl Default for TmdbConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            bearer: None,
            language: "zh-CN".into(),
            base_url: "https://api.themoviedb.org/3".into(),
            image_base: "https://image.tmdb.org/t/p".into(),
            include_adult: false,
            max_cast: 30,
            // TMDB 官方给的是 ~50/s，但家庭 IP 常常和其他服务共享出口，
            // 保守一点换稳定，刮削慢几分钟不影响体验。
            requests_per_second: 4.0,
        }
    }
}

pub struct TmdbProvider {
    cfg: TmdbConfig,
    transport: Arc<dyn HttpTransport>,
    limiter: RateLimiter,
}

impl TmdbProvider {
    pub fn new(cfg: TmdbConfig) -> Self {
        let limiter = RateLimiter::per_second(cfg.requests_per_second);
        Self {
            cfg,
            transport: Arc::new(ReqwestTransport::new()),
            limiter,
        }
    }

    /// 注入 transport —— 测试用 `FixtureTransport`。
    pub fn with_transport(cfg: TmdbConfig, transport: Arc<dyn HttpTransport>) -> Self {
        let limiter = RateLimiter::per_second(cfg.requests_per_second);
        Self {
            cfg,
            transport,
            limiter,
        }
    }

    fn auth(&self, url: &mut String) {
        if let Some(k) = &self.cfg.api_key {
            url.push_str(&format!(
                "{separator}api_key={k}",
                separator = if url.contains('?') { '&' } else { '?' }
            ));
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
            url.push_str(&utf8_urlencode(v));
        }
        self.auth(&mut url);

        let transport = self.transport.clone();
        let bearer = self.cfg.bearer.clone();
        let url2 = url.clone();

        let value = retry(3, move || {
            let transport = transport.clone();
            let bearer = bearer.clone();
            let url = url2.clone();
            let limiter = &self.limiter;
            async move {
                limiter.acquire().await;
                let res = transport.get(&url, bearer.as_deref()).await?;
                res.json()
            }
        })
        .await
        .map_err(|e| mosaic_core::MosaicError::Metadata {
            provider: "tmdb".into(),
            message: e.to_string(),
        })?;

        Ok(value)
    }

    fn image_url(&self, path: &str, kind: &str) -> String {
        format!(
            "{}/{}{}",
            self.cfg.image_base.trim_end_matches('/'),
            size_for(kind),
            path
        )
    }
}

/// 只转义真正会破坏 URL 的字符。用 percent-encoding crate 更正确，
/// 但为一个查询参数引入依赖不划算。
pub(crate) fn utf8_urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// TMDB 的日期是 `1999-03-31` 或空串。空串必须当成年份缺失。
fn year_of(v: &Value, key: &str) -> Option<i32> {
    let s = v.get(key)?.as_str()?;
    s.get(0..4)?.parse().ok()
}

fn images_of(v: &Value, image_base: &str, base: &TmdbProvider) -> Vec<MetadataImage> {
    let mut out = Vec::new();

    if let Some(p) = str_of(v, "poster_path") {
        out.push(MetadataImage {
            kind: "poster".into(),
            url: base.image_url(&p, "poster"),
            width: None,
            height: None,
            vote: v.get("vote_average").and_then(|x| x.as_f64()),
        });
    }
    if let Some(p) = str_of(v, "backdrop_path") {
        out.push(MetadataImage {
            kind: "backdrop".into(),
            url: base.image_url(&p, "backdrop"),
            width: None,
            height: None,
            vote: None,
        });
    }

    // `images` 里的 logo 才是真正的透明 logo，`backdrop_path` 不是。
    if let Some(logos) = v.pointer("/images/logos").and_then(|x| x.as_array()) {
        let best = logos
            .iter()
            .filter(|l| l.get("file_path").and_then(|p| p.as_str()).is_some())
            .max_by_key(|l| {
                (l.get("vote_average")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0)
                    * 100.0) as i64
            });
        if let Some(l) = best {
            if let Some(p) = l.get("file_path").and_then(|p| p.as_str()) {
                out.push(MetadataImage {
                    kind: "logo".into(),
                    url: format!(
                        "{}/{}{}",
                        image_base.trim_end_matches('/'),
                        size_for("logo"),
                        p
                    ),
                    width: None,
                    height: None,
                    vote: l.get("vote_average").and_then(|x| x.as_f64()),
                });
            }
        }
    }
    out
}

fn credits_of(v: &Value, max_cast: usize) -> Vec<MetadataCredit> {
    let mut out = Vec::new();

    if let Some(cast) = v.pointer("/credits/cast").and_then(|x| x.as_array()) {
        for c in cast.iter().take(max_cast) {
            if let Some(name) = str_of(c, "name") {
                out.push(MetadataCredit {
                    name,
                    role: "actor".into(),
                    character: str_of(c, "character"),
                    sort_order: c.get("order").and_then(|o| o.as_i64()).unwrap_or(999) as i32,
                    provider_id: c.get("id").map(|i| i.to_string()),
                });
            }
        }
    }

    if let Some(crew) = v.pointer("/credits/crew").and_then(|x| x.as_array()) {
        for c in crew.iter() {
            let job = c.get("job").and_then(|j| j.as_str()).unwrap_or("");
            let role = match job {
                "Director" => "director",
                "Writer" | "Screenplay" | "Story" => "writer",
                _ => continue,
            };
            if let Some(name) = str_of(c, "name") {
                if out.iter().any(|x| x.name == name && x.role == role) {
                    continue;
                }
                out.push(MetadataCredit {
                    name,
                    role: role.into(),
                    character: None,
                    sort_order: 1000 + out.len() as i32,
                    provider_id: c.get("id").map(|i| i.to_string()),
                });
            }
        }
    }
    out
}

fn record_from_movie(v: &Value, p: &TmdbProvider) -> MetadataRecord {
    MetadataRecord {
        provider: "tmdb".into(),
        provider_id: v
            .get("id")
            .map(|i| i.to_string())
            .unwrap_or_default(),
        title: str_of(v, "title").unwrap_or_default(),
        original_title: str_of(v, "original_title"),
        sort_title: None,
        year: year_of(v, "release_date"),
        tagline: str_of(v, "tagline"),
        overview: str_of(v, "overview"),
        runtime_ms: v
            .get("runtime")
            .and_then(|r| r.as_i64())
            .filter(|r| *r > 0)
            .map(|r| r * 60_000),
        rating: v.get("vote_average").and_then(|r| r.as_f64()),
        genres: v
            .get("genres")
            .and_then(|g| g.as_array())
            .map(|arr| arr.iter().filter_map(|g| str_of(g, "name")).collect())
            .unwrap_or_default(),
        images: images_of(v, &p.cfg.image_base, p),
        credits: credits_of(v, p.cfg.max_cast),
    }
}

fn record_from_tv(v: &Value, p: &TmdbProvider) -> MetadataRecord {
    MetadataRecord {
        provider: "tmdb".into(),
        provider_id: v.get("id").map(|i| i.to_string()).unwrap_or_default(),
        title: str_of(v, "name").unwrap_or_default(),
        original_title: str_of(v, "original_name"),
        sort_title: None,
        year: year_of(v, "first_air_date"),
        tagline: str_of(v, "tagline"),
        overview: str_of(v, "overview"),
        // 剧集用单集时长，不是总时长
        runtime_ms: v
            .get("episode_run_time")
            .and_then(|r| r.as_array())
            .and_then(|a| a.first())
            .and_then(|r| r.as_i64())
            .filter(|r| *r > 0)
            .map(|r| r * 60_000),
        rating: v.get("vote_average").and_then(|r| r.as_f64()),
        genres: v
            .get("genres")
            .and_then(|g| g.as_array())
            .map(|arr| arr.iter().filter_map(|g| str_of(g, "name")).collect())
            .unwrap_or_default(),
        images: images_of(v, &p.cfg.image_base, p),
        credits: credits_of(v, p.cfg.max_cast),
    }
}

fn record_from_episode(v: &Value) -> MetadataRecord {
    MetadataRecord {
        provider: "tmdb".into(),
        provider_id: v.get("id").map(|i| i.to_string()).unwrap_or_default(),
        title: str_of(v, "name").unwrap_or_default(),
        original_title: None,
        sort_title: None,
        year: year_of(v, "air_date"),
        tagline: None,
        overview: str_of(v, "overview"),
        runtime_ms: None,
        rating: v.get("vote_average").and_then(|r| r.as_f64()),
        genres: Vec::new(),
        images: str_of(v, "still_path").map_or(Vec::new(), |s| {
            vec![MetadataImage {
                kind: "still".into(),
                url: format!("https://image.tmdb.org/t/p/{}{}", size_for("still"), s),
                width: None,
                height: None,
                vote: v.get("vote_average").and_then(|x| x.as_f64()),
            }]
        }),
        credits: Vec::new(),
    }
}

#[async_trait]
impl MetadataProvider for TmdbProvider {
    fn id(&self) -> &str {
        "tmdb"
    }

    async fn search_movie(&self, query: &MetadataQuery) -> Result<Vec<MetadataRecord>> {
        let mut params: Vec<(&str, String)> = vec![
            ("query", query.title.clone()),
            ("include_adult", self.cfg.include_adult.to_string()),
            ("language", self.cfg.language.clone()),
        ];
        if let Some(y) = query.year {
            params.push(("year", y.to_string()));
        }

        let v = self.get_json("/search/movie", &params).await?;
        let arr = v.get("results").and_then(|r| r.as_array());
        Ok(arr
            .map(|a| a.iter().map(|m| record_from_movie(m, self)).collect())
            .unwrap_or_default())
    }

    async fn search_series(&self, query: &MetadataQuery) -> Result<Vec<MetadataRecord>> {
        let mut params: Vec<(&str, String)> = vec![
            ("query", query.title.clone()),
            ("include_adult", self.cfg.include_adult.to_string()),
            ("language", self.cfg.language.clone()),
        ];
        if let Some(y) = query.year {
            params.push(("first_air_date_year", y.to_string()));
        }

        let v = self.get_json("/search/tv", &params).await?;
        let arr = v.get("results").and_then(|r| r.as_array());
        Ok(arr
            .map(|a| a.iter().map(|m| record_from_tv(m, self)).collect())
            .unwrap_or_default())
    }

    async fn get_movie(&self, provider_id: &str) -> Result<MetadataRecord> {
        let v = self
            .get_json(
                &format!("/movie/{provider_id}"),
                &[
                    ("language", self.cfg.language.clone()),
                    ("append_to_response", "credits,images".into()),
                ],
            )
            .await?;
        Ok(record_from_movie(&v, self))
    }

    async fn get_series(&self, provider_id: &str) -> Result<MetadataRecord> {
        let v = self
            .get_json(
                &format!("/tv/{provider_id}"),
                &[
                    ("language", self.cfg.language.clone()),
                    ("append_to_response", "credits,images".into()),
                ],
            )
            .await?;
        Ok(record_from_tv(&v, self))
    }

    async fn get_episode(
        &self,
        series_id: &str,
        season: i32,
        episode: i32,
    ) -> Result<MetadataRecord> {
        let v = self
            .get_json(
                &format!("/tv/{series_id}/season/{season}/episode/{episode}"),
                &[("language", self.cfg.language.clone())],
            )
            .await?;
        Ok(record_from_episode(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::FixtureTransport;

    const SEARCH: &str = r#"{
      "results": [
        {"id": 603, "title": "The Matrix", "original_title": "The Matrix",
         "release_date": "1999-03-30", "overview": "A computer hacker learns",
         "vote_average": 8.2, "runtime": 136, "poster_path": "/p.jpg",
         "backdrop_path": "/b.jpg",
         "genres": [{"name": "Action"}, {"name": "Science Fiction"}]}
      ]
    }"#;

    const DETAIL: &str = r#"{
      "id": 603, "title": "The Matrix", "original_title": "The Matrix",
      "release_date": "1999-03-30", "tagline": "Welcome to the Real World",
      "overview": "A computer hacker", "vote_average": 8.2, "runtime": 136,
      "poster_path": "/p.jpg", "backdrop_path": "/b.jpg",
      "genres": [{"name": "Action"}],
      "images": {"logos": [{"file_path": "/l1.png", "vote_average": 5.4},
                           {"file_path": "/l2.png", "vote_average": 9.1}]},
      "credits": {
        "cast": [{"name": "Keanu Reeves", "character": "Neo", "order": 0, "id": 6384},
                 {"name": "Laurence Fishburne", "character": "Morpheus", "order": 1, "id": 2975}],
        "crew": [{"name": "Lana Wachowski", "job": "Director", "id": 114},
                 {"name": "Lilly Wachowski", "job": "Director", "id": 115},
                 {"name": "Bill Pope", "job": "Director of Photography", "id": 116}]
      }
    }"#;

    fn provider(routes: Vec<(&str, &str)>) -> TmdbProvider {
        let cfg = TmdbConfig {
            api_key: Some("KEY".into()),
            ..Default::default()
        };
        TmdbProvider::with_transport(cfg, Arc::new(FixtureTransport::new(routes)))
    }

    #[tokio::test]
    async fn search_movie_parses() {
        let p = provider(vec![("https://api.themoviedb.org/3/search/movie", SEARCH)]);
        let r = p
            .search_movie(&MetadataQuery {
                title: "The Matrix".into(),
                year: Some(1999),
                language: None,
                episode_title: None,
            })
            .await
            .unwrap();

        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "The Matrix");
        assert_eq!(r[0].year, Some(1999));
        assert_eq!(r[0].runtime_ms, Some(136 * 60_000));
        assert_eq!(r[0].genres, vec!["Action", "Science Fiction"]);
        assert_eq!(r[0].images[0].kind, "poster");
        assert!(r[0].images[0].url.contains("w500"));
    }

    #[tokio::test]
    async fn detail_parses_credits_and_logo() {
        let p = provider(vec![("https://api.themoviedb.org/3/movie/603", DETAIL)]);
        let r = p.get_movie("603").await.unwrap();

        assert_eq!(r.tagline.as_deref(), Some("Welcome to the Real World"));
        assert_eq!(r.credits.iter().filter(|c| c.role == "actor").count(), 2);
        assert_eq!(r.credits.iter().filter(|c| c.role == "director").count(), 2);
        // 摄影师不该进演职员表
        assert!(r.credits.iter().all(|c| c.role != "crew"));
        assert_eq!(r.credits[0].character.as_deref(), Some("Neo"));

        let logo = r.images.iter().find(|i| i.kind == "logo").unwrap();
        // 两张 logo 里挑评分高的那张
        assert!(logo.url.contains("/l2.png"), "{}", logo.url);
    }

    /// 空字符串必须当成"没有"，不能让标题变成 ""。
    #[tokio::test]
    async fn empty_release_date_is_no_year() {
        const EMPTY: &str = r#"{"id": 1, "title": "X", "release_date": ""}"#;
        let p = provider(vec![("https://api.themoviedb.org/3/movie/1", EMPTY)]);
        let r = p.get_movie("1").await.unwrap();
        assert_eq!(r.year, None);
        assert_eq!(r.overview, None);
    }

    #[tokio::test]
    async fn search_series_uses_first_air_date_year() {
        const TV: &str = r#"{"results": [{"id": 1396, "name": "Breaking Bad",
            "first_air_date": "2008-01-20", "vote_average": 8.9,
            "episode_run_time": [47], "poster_path": "/bb.jpg"}]}"#;
        let p = provider(vec![("https://api.themoviedb.org/3/search/tv", TV)]);
        let r = p
            .search_series(&MetadataQuery {
                title: "Breaking Bad".into(),
                year: Some(2008),
                language: None,
                episode_title: None,
            })
            .await
            .unwrap();
        assert_eq!(r[0].title, "Breaking Bad");
        assert_eq!(r[0].year, Some(2008));
        assert_eq!(r[0].runtime_ms, Some(47 * 60_000));
    }

    #[test]
    fn url_encoding_is_safe() {
        assert_eq!(utf8_urlencode("The Matrix"), "The%20Matrix");
        assert_eq!(utf8_urlencode("a-b_c.d~e"), "a-b_c.d~e");
        // 中文与 & 都要转义
        assert_eq!(
            utf8_urlencode("疯狂&麦克斯"),
            "%E7%96%AF%E7%8B%82%26%E9%BA%A6%E5%85%8B%E6%96%AF"
        );
    }
}
