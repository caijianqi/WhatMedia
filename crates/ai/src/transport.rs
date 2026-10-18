//! HTTP 传输抽象（与 `crates/metadata` 同一套路）。
//!
//! **为什么要有这一层**：模型调用如果直接 `reqwest::post`，
//! 就只能在有网络、有 API Key 的环境里测试 —— 那等于没有测试。
//! 抽成 trait 之后，provider 的请求构造与响应解析都能离线验证；
//! "断网"也从一个难以复现的环境问题，变成 `FailingTransport` 一行。

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Mutex;

use crate::error::{AiError, AiErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpStatus {
    Ok,
    /// 429 / 5xx / 连接失败：值得重试或降级。
    Retryable,
    /// 401 / 403 / 404 / 400：重试没意义。
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

    /// 连不上 / DNS 失败 / 超时。**没有状态码** —— 请求根本没到服务端。
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

impl From<HttpError> for AiError {
    fn from(e: HttpError) -> Self {
        // 连不上模型服务 = 模型不可用，不是内部错误。
        // 这一句决定了"断网时 AI 端点返回 503 而不是 500"，
        // 也决定了核心媒体功能不受影响（因为它们压根不走这条路径）。
        let kind = match e.status {
            HttpStatus::Retryable => AiErrorKind::ModelUnavailable,
            HttpStatus::Ok | HttpStatus::Fatal => AiErrorKind::ModelProtocol,
        };
        AiError::new(kind, e.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub code: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn json(&self) -> Result<Value, AiError> {
        serde_json::from_slice(&self.body).map_err(|e| {
            AiError::model_protocol(format!(
                "模型响应不是合法 JSON：{e}；前 200 字节：{}",
                String::from_utf8_lossy(&self.body)
                    .chars()
                    .take(200)
                    .collect::<String>()
            ))
        })
    }
}

#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn post_json(
        &self,
        url: &str,
        bearer: Option<&str>,
        body: &Value,
    ) -> Result<HttpResponse, HttpError>;
}

// ─────────────────────────────────────────────────────────────
// 真实实现
// ─────────────────────────────────────────────────────────────

pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new(timeout_secs: u64) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs.max(1)))
            .build()?;
        Ok(Self { client })
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn post_json(
        &self,
        url: &str,
        bearer: Option<&str>,
        body: &Value,
    ) -> Result<HttpResponse, HttpError> {
        let mut req = self.client.post(url).json(body);
        if let Some(k) = bearer.filter(|s| !s.is_empty()) {
            req = req.bearer_auth(k);
        }
        let res = req
            .send()
            .await
            .map_err(|e| HttpError::transport(format!("请求模型失败（{url}）：{e}")))?;
        let code = res.status().as_u16();
        let body = res
            .bytes()
            .await
            .map_err(|e| HttpError::transport(format!("读取模型响应失败：{e}")))?
            .to_vec();
        Ok(HttpResponse { code, body })
    }
}

// ─────────────────────────────────────────────────────────────
// 测试实现
// ─────────────────────────────────────────────────────────────

/// 按 URL 片段匹配，返回录制好的 JSON。
pub struct FixtureTransport {
    routes: Vec<(String, String)>,
    recorded: Mutex<Vec<(String, Value)>>,
}

impl FixtureTransport {
    pub fn new(routes: Vec<(String, String)>) -> Self {
        Self {
            routes,
            recorded: Mutex::new(Vec::new()),
        }
    }

    /// 取回发送过的请求，用于断言 URL / body 拼对了。
    pub fn recorded(&self) -> Vec<(String, Value)> {
        self.recorded.lock().unwrap().clone()
    }
}

#[async_trait]
impl HttpTransport for FixtureTransport {
    async fn post_json(
        &self,
        url: &str,
        _bearer: Option<&str>,
        body: &Value,
    ) -> Result<HttpResponse, HttpError> {
        self.recorded
            .lock()
            .unwrap()
            .push((url.to_string(), body.clone()));
        for (frag, resp) in &self.routes {
            if url.contains(frag.as_str()) {
                return Ok(HttpResponse {
                    code: 200,
                    body: resp.as_bytes().to_vec(),
                });
            }
        }
        Err(HttpError::fatal(
            404,
            format!("fixture 没有匹配 {url} 的路由"),
        ))
    }
}

/// 模拟断网 / 服务没起。**每个请求都失败**，用来验证降级路径。
pub struct FailingTransport {
    pub message: String,
}

impl FailingTransport {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
impl HttpTransport for FailingTransport {
    async fn post_json(
        &self,
        url: &str,
        _bearer: Option<&str>,
        _body: &Value,
    ) -> Result<HttpResponse, HttpError> {
        Err(HttpError::transport(format!("{}（{url}）", self.message)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_error_is_model_unavailable_not_internal() {
        // 这一条决定了断网时 AI 端点返回 503 而不是 500。
        let e: AiError = HttpError::transport("连不上").into();
        assert_eq!(e.kind, AiErrorKind::ModelUnavailable);
        let e: AiError = HttpError::fatal(401, "key 不对").into();
        assert_eq!(e.kind, AiErrorKind::ModelProtocol);
    }

    #[tokio::test]
    async fn fixture_records_requests() {
        let t = FixtureTransport::new(vec![(
            "chat/completions".into(),
            r#"{"choices":[{"message":{"content":"ok"}}]}"#.into(),
        )]);
        let res = t
            .post_json(
                "https://api.openai.com/v1/chat/completions",
                Some("sk-x"),
                &serde_json::json!({"a": 1}),
            )
            .await
            .unwrap();
        assert_eq!(res.code, 200);
        assert_eq!(t.recorded().len(), 1);
        assert_eq!(
            t.recorded()[0].0,
            "https://api.openai.com/v1/chat/completions"
        );
    }
}
