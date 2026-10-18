//! Ollama（`POST {base}/api/chat`）。
//!
//! **为什么单独一个实现**：Ollama 不是 OpenAI 兼容形状。
//! 三处差别，每一处都能让"直接复用 OpenAI 实现"静默出错：
//!
//! 1. 路径是 `/api/chat`，不是 `/chat/completions`
//! 2. 响应体是 `{ message: {...}, done: true }`，没有 `choices[]`
//! 3. `tool_calls[].function.arguments` 是**对象**，不是 JSON 字符串
//!    （OpenAI 那边是字符串；按字符串解析会得到一个空对象）
//!
//! 另外必须显式带 `"stream": false`：Ollama 默认流式，
//! 会返回一串 NDJSON 而不是一个 JSON 对象。

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::{AiError, AiErrorKind};
use crate::openai::ProviderKind;
use crate::provider::{ChatRequest, ChatResponse, ModelProvider, TokenUsage, ToolCallRequest};
use crate::transport::HttpTransport;

pub struct OllamaProvider {
    base_url: String,
    model: String,
    transport: std::sync::Arc<dyn HttpTransport>,
}

impl OllamaProvider {
    pub fn new(
        base_url: Option<String>,
        model: impl Into<String>,
        transport: std::sync::Arc<dyn HttpTransport>,
    ) -> Self {
        let base = base_url
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| ProviderKind::Ollama.default_base_url().unwrap().to_string());
        let model = {
            let m = model.into();
            if m.trim().is_empty() {
                ProviderKind::Ollama.default_model().to_string()
            } else {
                m
            }
        };
        Self {
            base_url: base.trim_end_matches('/').to_string(),
            model,
            transport,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("{}/api/chat", self.base_url)
    }

    fn body(&self, req: &ChatRequest) -> Value {
        let messages: Vec<Value> = req
            .messages
            .iter()
            .map(|m| {
                let mut v = json!({ "role": m.role.as_str(), "content": m.content });
                if !m.tool_calls.is_empty() {
                    v["tool_calls"] = json!(m
                        .tool_calls
                        .iter()
                        .map(|c| {
                            json!({
                                "function": {
                                    "name": c.name,
                                    // Ollama 收对象，不是字符串。
                                    "arguments": serde_json::from_str::<Value>(&c.arguments)
                                        .unwrap_or_else(|_| json!({})),
                                }
                            })
                        })
                        .collect::<Vec<_>>());
                }
                v
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "messages": messages,
            // 默认流式会返回 NDJSON；显式关掉。
            "stream": false,
        });
        if !req.tools.is_empty() {
            body["tools"] = json!(req
                .tools
                .iter()
                .map(|t| json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                }))
                .collect::<Vec<_>>());
        }
        body
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AiError> {
        let res = self
            .transport
            .post_json(&self.endpoint(), None, &self.body(&req))
            .await?;
        if !(200..300).contains(&res.code) {
            return Err(AiError::new(
                AiErrorKind::ModelProtocol,
                format!(
                    "Ollama 返回 HTTP {}：{}",
                    res.code,
                    String::from_utf8_lossy(&res.body)
                        .chars()
                        .take(200)
                        .collect::<String>()
                ),
            ));
        }
        parse_ollama(&res.json()?)
    }
}

fn parse_ollama(v: &Value) -> Result<ChatResponse, AiError> {
    let message = v
        .get("message")
        .ok_or_else(|| AiError::model_protocol("Ollama 响应里没有 message"))?;
    let content = message
        .get("content")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut tool_calls = Vec::new();
    if let Some(list) = message.get("tool_calls").and_then(|t| t.as_array()) {
        for (i, c) in list.iter().enumerate() {
            let func = c.get("function").unwrap_or(c);
            let name = func
                .get("name")
                .and_then(|s| s.as_str())
                .ok_or_else(|| AiError::model_protocol("Ollama 的 tool_calls 缺 function.name"))?
                .to_string();
            // arguments 是对象：原样序列化回 JSON 字符串，
            // 交给上层统一按字符串解析。
            let arguments = match func.get("arguments") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "{}".to_string(),
            };
            tool_calls.push(ToolCallRequest {
                // Ollama 不回调用 id，合成一个稳定的。
                id: format!("ollama-{i}"),
                name,
                arguments,
            });
        }
    }

    if content.is_none() && tool_calls.is_empty() {
        return Err(AiError::model_protocol("Ollama 既没给文本也没给工具调用"));
    }

    let usage = TokenUsage {
        prompt_tokens: v.get("prompt_eval_count").and_then(|n| n.as_u64()),
        completion_tokens: v.get("eval_count").and_then(|n| n.as_u64()),
    };

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason: v
            .get("done")
            .and_then(|d| d.as_bool())
            .map(|d| if d { "stop" } else { "length" })
            .map(str::to_string),
        usage: Some(usage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatMessage, ToolDefinition};
    use crate::transport::FixtureTransport;

    #[tokio::test]
    async fn posts_to_api_chat_with_stream_off() {
        let t = std::sync::Arc::new(FixtureTransport::new(vec![(
            "api/chat".into(),
            r#"{"model":"qwen2.5:7b","message":{"role":"assistant","content":"好"},"done":true,"prompt_eval_count":12,"eval_count":4}"#
                .into(),
        )]));
        let p = OllamaProvider::new(None, "qwen2.5:7b", t.clone());
        let r = p
            .chat(ChatRequest {
                messages: vec![ChatMessage::user("hi")],
                tools: vec![ToolDefinition {
                    name: "search_media".into(),
                    description: "d".into(),
                    parameters: json!({"type":"object"}),
                }],
                temperature: None,
            })
            .await
            .unwrap();

        assert_eq!(r.content.as_deref(), Some("好"));
        assert_eq!(r.finish_reason.as_deref(), Some("stop"));
        // Ollama 的 eval_count 换算成 provider 统一的 completion_tokens
        assert_eq!(r.usage.unwrap().completion_tokens, Some(4));

        let (url, body) = &t.recorded()[0];
        assert_eq!(url, "http://localhost:11434/api/chat");
        // 不加 stream:false 会拿到一串 NDJSON，解析必然失败。
        assert_eq!(body["stream"], false);
        assert_eq!(body["model"], "qwen2.5:7b");
        assert_eq!(body["tools"][0]["function"]["name"], "search_media");
    }

    #[test]
    fn object_arguments_are_normalized_to_string() {
        let v: Value = serde_json::from_str(
            r#"{"message":{"tool_calls":[{"function":{"name":"get_media","arguments":{"id":"abc"}}}]},"done":true}"#,
        )
        .unwrap();
        let r = parse_ollama(&v).unwrap();
        assert_eq!(r.tool_calls[0].id, "ollama-0");
        assert_eq!(r.tool_calls[0].arguments, r#"{"id":"abc"}"#);
    }

    #[test]
    fn tool_results_are_sent_back_as_objects() {
        // 回填时 Ollama 期望 history 里的 tool_calls 是对象形态。
        let t: std::sync::Arc<dyn HttpTransport> =
            std::sync::Arc::new(FixtureTransport::new(vec![]));
        let p = OllamaProvider::new(None, "m", t.clone());
        let body = p.body(&ChatRequest {
            messages: vec![ChatMessage {
                role: crate::provider::Role::Assistant,
                content: String::new(),
                tool_calls: vec![ToolCallRequest {
                    id: "x".into(),
                    name: "search_media".into(),
                    arguments: r#"{"q":"a"}"#.into(),
                }],
                tool_call_id: None,
            }],
            tools: vec![],
            temperature: None,
        });
        assert_eq!(
            body["messages"][0]["tool_calls"][0]["function"]["arguments"],
            json!({"q": "a"})
        );
    }
}
