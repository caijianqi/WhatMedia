//! OpenAI 兼容实现：覆盖 DeepSeek / Qwen / GLM / vLLM / llama.cpp。
//!
//! 这几家的对话接口形状一致（`POST {base}/chat/completions`），
//! 差别只在 base_url、模型名、要不要 API Key。所以是**一个实现 + 一份预设表**，
//! 不是七个实现 —— 七个实现意味着七套要各自维护的解析代码。
//!
//! Ollama 不是这套形状（`/api/chat`，且 `message.tool_calls[].function.arguments`
//! 是对象而不是字符串），单独在 `ollama.rs` 里。

use async_trait::async_trait;
use serde_json::{json, Value};
use std::fmt;

use crate::error::AiError;
use crate::provider::{ChatMessage, ChatRequest, ChatResponse, ModelProvider, ToolCallRequest};
use crate::transport::HttpTransport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    DeepSeek,
    Qwen,
    Glm,
    MiniMax,
    /// 不走这个文件 —— 形状不同。列在这里只是为了配置与文档的统一。
    Ollama,
    Vllm,
    LlamaCpp,
    /// 自建网关 / 代理，必须显式给 base_url。
    Custom,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "openai",
            ProviderKind::DeepSeek => "deepseek",
            ProviderKind::Qwen => "qwen",
            ProviderKind::Glm => "glm",
            ProviderKind::MiniMax => "minimax",
            ProviderKind::Ollama => "ollama",
            ProviderKind::Vllm => "vllm",
            ProviderKind::LlamaCpp => "llama_cpp",
            ProviderKind::Custom => "custom",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "openai" => Some(ProviderKind::OpenAi),
            "deepseek" => Some(ProviderKind::DeepSeek),
            "qwen" | "tongyi" | "dashscope" => Some(ProviderKind::Qwen),
            "glm" | "zhipu" | "bigmodel" => Some(ProviderKind::Glm),
            "minimax" | "abab" => Some(ProviderKind::MiniMax),
            "ollama" => Some(ProviderKind::Ollama),
            "vllm" => Some(ProviderKind::Vllm),
            "llama_cpp" | "llamacpp" | "llama.cpp" => Some(ProviderKind::LlamaCpp),
            "custom" => Some(ProviderKind::Custom),
            _ => None,
        }
    }

    /// 默认接入点。`Custom` 没有默认值 —— 配不出来比猜一个错的强。
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            ProviderKind::OpenAi => Some("https://api.openai.com/v1"),
            ProviderKind::DeepSeek => Some("https://api.deepseek.com/v1"),
            // 通义千问的 OpenAI 兼容模式，注意不是 /api/v1。
            ProviderKind::Qwen => Some("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            ProviderKind::Glm => Some("https://open.bigmodel.cn/api/paas/v4"),
            ProviderKind::MiniMax => Some("https://api.minimax.chat/v1"),
            ProviderKind::Ollama => Some("http://localhost:11434"),
            ProviderKind::Vllm => Some("http://localhost:8000/v1"),
            ProviderKind::LlamaCpp => Some("http://localhost:8080/v1"),
            ProviderKind::Custom => None,
        }
    }

    /// 本地推理（Ollama / vLLM / llama.cpp）不需要 Key，
    /// 因此**缺 Key 不该让 provider 建不起来**，否则家庭用户开箱即用不了。
    pub fn requires_api_key(self) -> bool {
        matches!(
            self,
            ProviderKind::OpenAi
                | ProviderKind::DeepSeek
                | ProviderKind::Qwen
                | ProviderKind::Glm
                | ProviderKind::MiniMax
        )
    }

    pub fn default_model(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "gpt-4o-mini",
            ProviderKind::DeepSeek => "deepseek-chat",
            ProviderKind::Qwen => "qwen-plus",
            ProviderKind::Glm => "glm-4-flash",
            ProviderKind::MiniMax => "MiniMax-Text-01",
            ProviderKind::Ollama => "qwen2.5:7b",
            ProviderKind::Vllm => "Qwen/Qwen2.5-7B-Instruct",
            ProviderKind::LlamaCpp => "local",
            ProviderKind::Custom => "local",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::OpenAi => "OpenAI",
            ProviderKind::DeepSeek => "DeepSeek",
            ProviderKind::Qwen => "通义千问",
            ProviderKind::Glm => "智谱 GLM",
            ProviderKind::MiniMax => "MiniMax",
            ProviderKind::Ollama => "Ollama",
            ProviderKind::Vllm => "vLLM",
            ProviderKind::LlamaCpp => "llama.cpp",
            ProviderKind::Custom => "自定义",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub struct OpenAiCompatible {
    kind: ProviderKind,
    base_url: String,
    model: String,
    api_key: Option<String>,
    transport: std::sync::Arc<dyn HttpTransport>,
}

impl OpenAiCompatible {
    pub fn new(
        kind: ProviderKind,
        base_url: Option<String>,
        model: impl Into<String>,
        api_key: Option<String>,
        transport: std::sync::Arc<dyn HttpTransport>,
    ) -> Result<Self, AiError> {
        let base = base_url
            .filter(|s| !s.trim().is_empty())
            .or_else(|| kind.default_base_url().map(str::to_string))
            .ok_or_else(|| {
                AiError::bad_args("provider = custom 时必须给 base_url（或改用内置 provider）")
            })?;
        let model = {
            let m = model.into();
            if m.trim().is_empty() {
                kind.default_model().to_string()
            } else {
                m
            }
        };
        if kind.requires_api_key() && api_key.as_deref().map(|s| s.is_empty()).unwrap_or(true) {
            return Err(AiError::model_unavailable(format!(
                "{} 需要 API Key：把密钥放进环境变量，并把 ai.api_key_env 指到它",
                kind.label()
            )));
        }
        Ok(Self {
            kind,
            base_url: base.trim_end_matches('/').to_string(),
            model,
            api_key,
            transport,
        })
    }

    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn body(&self, req: &ChatRequest) -> Value {
        let messages: Vec<Value> = req.messages.iter().map(msg_to_json).collect();
        let mut body = json!({
            "model": self.model,
            "messages": messages,
        });
        if !req.tools.is_empty() {
            let tools: Vec<Value> = req
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = json!(tools);
        }
        if let Some(temp) = req.temperature {
            body["temperature"] = json!(temp);
        }
        body
    }
}

fn msg_to_json(m: &ChatMessage) -> Value {
    let mut v = json!({ "role": m.role.as_str(), "content": m.content });
    if !m.tool_calls.is_empty() {
        v["tool_calls"] = json!(m
            .tool_calls
            .iter()
            .map(|c| {
                json!({
                    "id": c.id,
                    "type": "function",
                    "function": { "name": c.name, "arguments": c.arguments },
                })
            })
            .collect::<Vec<_>>());
    }
    if let Some(id) = &m.tool_call_id {
        v["tool_call_id"] = json!(id);
    }
    v
}

#[async_trait]
impl ModelProvider for OpenAiCompatible {
    fn id(&self) -> &str {
        self.kind.as_str()
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AiError> {
        let res = self
            .transport
            .post_json(&self.endpoint(), self.api_key.as_deref(), &self.body(&req))
            .await?;
        if !(200..300).contains(&res.code) {
            return Err(AiError::new(
                crate::error::AiErrorKind::ModelProtocol,
                format!(
                    "模型返回 HTTP {}：{}",
                    res.code,
                    String::from_utf8_lossy(&res.body)
                        .chars()
                        .take(200)
                        .collect::<String>()
                ),
            ));
        }
        parse_chat_completion(&res.json()?)
    }
}

/// 解析 OpenAI 形状的响应。
///
/// 抽成自由函数而不是方法：Ollama 的响应结构不同，但可以复用
/// 里面"取第一个 choice"的思路；更重要的是它单独可测。
pub(crate) fn parse_chat_completion(v: &Value) -> Result<ChatResponse, AiError> {
    let choice = v
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or_else(|| AiError::model_protocol("响应里没有 choices[0]"))?;
    let message = choice.get("message").unwrap_or(choice);
    let content = message
        .get("content")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let mut tool_calls = Vec::new();
    if let Some(list) = message.get("tool_calls").and_then(|t| t.as_array()) {
        for (i, c) in list.iter().enumerate() {
            let id = c
                .get("id")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            let func = c.get("function").unwrap_or(c);
            let name = func
                .get("name")
                .and_then(|s| s.as_str())
                .ok_or_else(|| AiError::model_protocol("tool_calls 里缺 function.name"))?
                .to_string();
            // arguments 绝大多数实现给字符串，但也有给对象的（Ollama、部分网关）。
            let arguments = match func.get("arguments") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => other.to_string(),
                None => "{}".to_string(),
            };
            if id.is_empty() {
                // 没有 id 就没法回填结果。给一个稳定的合成 id，
                // 而不是直接报错 —— 有的兼容网关确实不回 id。
                tool_calls.push(ToolCallRequest {
                    id: format!("auto-{i}"),
                    name,
                    arguments,
                });
            } else {
                tool_calls.push(ToolCallRequest {
                    id,
                    name,
                    arguments,
                });
            }
        }
    }

    if content.is_none() && tool_calls.is_empty() {
        return Err(AiError::model_protocol(
            "模型既没给文本也没给工具调用 —— 按不可用处理，而不是当成空回答",
        ));
    }

    let usage = v.get("usage").map(|u| crate::provider::TokenUsage {
        prompt_tokens: u.get("prompt_tokens").and_then(|n| n.as_u64()),
        completion_tokens: u.get("completion_tokens").and_then(|n| n.as_u64()),
    });

    Ok(ChatResponse {
        content,
        tool_calls,
        finish_reason: choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .map(str::to_string),
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ChatMessage, ToolDefinition};
    use crate::transport::FixtureTransport;

    #[test]
    fn presets_cover_all_documented_providers() {
        for (s, kind) in [
            ("openai", ProviderKind::OpenAi),
            ("deepseek", ProviderKind::DeepSeek),
            ("qwen", ProviderKind::Qwen),
            ("glm", ProviderKind::Glm),
            ("minimax", ProviderKind::MiniMax),
            ("ollama", ProviderKind::Ollama),
            ("vllm", ProviderKind::Vllm),
            ("llama_cpp", ProviderKind::LlamaCpp),
        ] {
            assert_eq!(ProviderKind::parse(s), Some(kind), "{s}");
            assert!(kind.default_base_url().is_some(), "{s} 缺默认 base_url");
        }
        // 未知 provider 要被认出来，而不是悄悄退化成 OpenAI ——
        // 那会让用户的请求打到错误的厂商上去。
        assert_eq!(ProviderKind::parse("some-new-vendor"), None);
    }

    #[test]
    fn local_inference_does_not_require_a_key() {
        assert!(!ProviderKind::Ollama.requires_api_key());
        assert!(!ProviderKind::Vllm.requires_api_key());
        assert!(ProviderKind::OpenAi.requires_api_key());
        // Custom 没有默认地址，必须显式给。
        assert_eq!(ProviderKind::Custom.default_base_url(), None);
    }

    #[test]
    fn missing_key_for_hosted_provider_is_unavailable() {
        let t: std::sync::Arc<dyn HttpTransport> =
            std::sync::Arc::new(FixtureTransport::new(vec![]));
        // 用 .err() 而不是 .unwrap_err()：后者要求成功类型也实现 Debug，
        // 而 OpenAiCompatible 持有 Arc<dyn HttpTransport>（trait 对象没有 Debug）。
        let e = OpenAiCompatible::new(ProviderKind::OpenAi, None, "", None, t)
            .err()
            .unwrap();
        assert_eq!(e.kind, crate::error::AiErrorKind::ModelUnavailable);
    }

    #[tokio::test]
    async fn request_shape_matches_openai() {
        let t = std::sync::Arc::new(FixtureTransport::new(vec![(
            "chat/completions".into(),
            r#"{"choices":[{"message":{"content":"你好"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#.into(),
        )]));
        let p = OpenAiCompatible::new(
            ProviderKind::OpenAi,
            None,
            "gpt-4o-mini",
            Some("sk-test".into()),
            t.clone(),
        )
        .unwrap();

        let resp = p
            .chat(ChatRequest {
                messages: vec![ChatMessage::user("hi")],
                tools: vec![ToolDefinition {
                    name: "search_media".into(),
                    description: "搜".into(),
                    parameters: json!({"type": "object"}),
                }],
                temperature: Some(0.2),
            })
            .await
            .unwrap();

        assert_eq!(resp.content.as_deref(), Some("你好"));
        let (url, body) = &t.recorded()[0];
        assert_eq!(url, "https://api.openai.com/v1/chat/completions");
        assert_eq!(body["model"], "gpt-4o-mini");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "search_media");
        // temperature 是 f32，进 JSON 后不等于 f64 的 0.2。比差值而不是比值。
        assert!((body["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(resp.usage.as_ref().unwrap().prompt_tokens, Some(10));
    }

    #[test]
    fn parses_tool_calls_with_string_arguments() {
        let v: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"tool_calls":[{"id":"call_1","function":{"name":"search_media","arguments":"{\"q\":\"星际穿越\"}"}}]}}]}"#,
        )
        .unwrap();
        let r = parse_chat_completion(&v).unwrap();
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "search_media");
        assert_eq!(r.tool_calls[0].arguments, r#"{"q":"星际穿越"}"#);
    }

    #[test]
    fn tolerates_object_arguments_and_missing_id() {
        // 有的兼容网关把 arguments 给成对象，有的不回 id。
        let v: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"tool_calls":[{"function":{"name":"get_media","arguments":{"id":"1"}}}]}}]}"#,
        )
        .unwrap();
        let r = parse_chat_completion(&v).unwrap();
        assert_eq!(r.tool_calls[0].id, "auto-0");
        assert_eq!(r.tool_calls[0].arguments, r#"{"id":"1"}"#);
    }

    #[test]
    fn empty_answer_is_treated_as_protocol_error() {
        // 既无文本又无工具调用 —— 当成"模型不可用"，而不是"AI 回答：什么都没找到"。
        let v: Value = serde_json::from_str(r#"{"choices":[{"message":{"content":""}}]}"#).unwrap();
        let e = parse_chat_completion(&v).unwrap_err();
        assert_eq!(e.kind, crate::error::AiErrorKind::ModelProtocol);
    }
}
