//! HTTP 传输抽象。
//!
//! **为什么要有这一层**：TMDB / Bangumi 的 Provider 如果直接 `reqwest::get`，
//! 就只能在有网络、有 API Key 的环境里测试 —— 那等于没有测试。
//!
//! 把"发一个请求"抽成 trait 之后：
//!   · 生产用 `ReqwestTransport`
//!   · 测试用 `FixtureTransport`，喂录制好的 JSON
//! Provider 的解析逻辑、打分逻辑、合并逻辑就都能离线验证。

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

use whatmedia_core::WhatMediaError;

/// HTTP 状态码。Provider 要根据它决定重试还是放弃。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpStatus {
    Ok,
    /// 429 / 5xx：值得退避重试。
    Retryable,
    /// 401 / 403 / 404：重试没意义。
    Fatal,
}

impl HttpStatus {
    pub fn from_code(code: u16) -> Self {
        match code {
            200..=299 => HttpStatus::Ok,
            429 | 500..=599 => HttpStatus::Retryable,
            _ => HttpStatus::Fatal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: HttpStatus,
    pub code: Option<u16>,
    pub message: String,
}

impl HttpError {
    pub fn fatal(code: u16, message: impl Into<String>) -> Self {
        Self {
            status: HttpStatus::from_code(code),
            code: Some(code),
            message: message.into(),
        }
    }

    pub fn transport(message: impl Into<String>) -> Self {
        Self {
            status: HttpStatus::Retryable,
            code: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(c) => write!(f, "HTTP {c}: {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for HttpError {}

impl From<HttpError> for WhatMediaError {
    fn from(e: HttpError) -> Self {
        WhatMediaError::Metadata {
            provider: "http".into(),
            message: e.to_string(),
        }
    }
}

/// 一次 GET 的响应。
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub code: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> Result<serde_json::Value, HttpError> {
        serde_json::from_slice(&self.body).map_err(|e| {
            HttpError::fatal(
                self.code,
                format!("响应不是合法 JSON：{e}；前 200 字节：{}", {
                    let s = String::from_utf8_lossy(&self.body);
                    s.chars().take(200).collect::<String>()
                }),
            )
        })
    }
}

#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn get(&self, url: &str, bearer: Option<&str>) -> Result<HttpResponse, HttpError>;
}

// ─────────────────────────────────────────────────────────────
// 真实实现
// ─────────────────────────────────────────────────────────────

pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                // 家庭网络里 DNS 有时很慢；给每个请求一个硬上限，
                // 否则一次刮削可能被单个请求拖死。
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn get(&self, url: &str, bearer: Option<&str>) -> Result<HttpResponse, HttpError> {
        let mut req = self
            .client
            .get(url)
            .header("accept", "application/json")
            .header("user-agent", "whatmedia-media/0.1");

        if let Some(t) = bearer {
            req = req.bearer_auth(t);
        }

        let res = req
            .send()
            .await
            .map_err(|e| HttpError::transport(format!("请求 {url} 失败：{e}")))?;

        let code = res.status().as_u16();
        let body = res
            .bytes()
            .await
            .map_err(|e| HttpError::transport(format!("读取 {url} 响应体失败：{e}")))?
            .to_vec();

        if HttpStatus::from_code(code) == HttpStatus::Ok {
            Ok(HttpResponse { code, body })
        } else {
            Err(HttpError::fatal(
                code,
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(300)
                    .collect::<String>(),
            ))
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 测试实现
// ─────────────────────────────────────────────────────────────

/// 录制回放用的 transport。
///
/// `routes` 的 key 是**完整 URL 去掉 scheme**，这样测试里改 base_url 也不会失效。
/// 命中不到就返回 404 —— 测试因此能立刻发现"Provider 拼错了 URL"。
pub struct FixtureTransport {
    /// 完整 URL（含 query）→ 响应
    routes: HashMap<String, String>,
    /// 仅路径（不含 query）→ 响应。录 fixture 时通常不关心 query 参数，
    /// 所以这是真正会被命中的那一档。
    paths: HashMap<String, String>,
    /// 记录每个 URL 被请求了几次，用来验证限流与去重。
    pub calls: Mutex<Vec<String>>,
}

impl FixtureTransport {
    pub fn new(routes: Vec<(&str, &str)>) -> Self {
        Self {
            routes: routes
                .iter()
                .map(|(u, b)| (normalize_url(u), b.to_string()))
                .collect(),
            paths: routes
                .into_iter()
                .map(|(u, b)| (path_only(u), b.to_string()))
                .collect(),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub fn urls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

/// 去掉 scheme 与结尾斜杠，让 fixture 的 key 与 base_url 解耦。
fn normalize_url(u: &str) -> String {
    let u = u
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    u.trim_end_matches('/').to_string()
}

/// 再去掉 query —— 录 fixture 时几乎没人愿意手抄一长串 percent-encoding。
fn path_only(u: &str) -> String {
    normalize_url(u).split('?').next().unwrap_or("").to_string()
}

#[async_trait]
impl HttpTransport for FixtureTransport {
    async fn get(&self, url: &str, _bearer: Option<&str>) -> Result<HttpResponse, HttpError> {
        self.calls.lock().unwrap().push(url.to_string());
        let key = normalize_url(url);
        let body = self
            .routes
            .get(&key)
            .or_else(|| self.paths.get(&path_only(url)));
        match body {
            Some(body) => Ok(HttpResponse {
                code: 200,
                body: body.as_bytes().to_vec(),
            }),
            None => Err(HttpError::fatal(
                404,
                format!("fixture 未录制该 URL：{url}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fixture_hits_and_misses() {
        let t = FixtureTransport::new(vec![("https://api.example.com/movie/1", r#"{"a":1}"#)]);
        let r = t
            .get("https://api.example.com/movie/1", None)
            .await
            .unwrap();
        assert_eq!(r.json().unwrap()["a"], 1);

        // 换 base_url 仍然命中
        let r = t.get("http://api.example.com/movie/1", None).await;
        assert!(r.is_ok());

        let e = t
            .get("https://api.example.com/movie/2", None)
            .await
            .unwrap_err();
        assert_eq!(e.status, HttpStatus::Fatal);
        assert_eq!(t.call_count(), 3);
    }

    #[test]
    fn status_classification() {
        assert_eq!(HttpStatus::from_code(200), HttpStatus::Ok);
        assert_eq!(HttpStatus::from_code(429), HttpStatus::Retryable);
        assert_eq!(HttpStatus::from_code(503), HttpStatus::Retryable);
        assert_eq!(HttpStatus::from_code(404), HttpStatus::Fatal);
        assert_eq!(HttpStatus::from_code(401), HttpStatus::Fatal);
    }
}
