//! 模型层抽象。
//!
//! **为什么不绑定单一厂商**：需求稿 §63 要求同时支持
//! OpenAI 兼容 / DeepSeek / Qwen / GLM / Ollama / vLLM / llama.cpp / 本地模型。
//! 除了 Ollama，其余几家的对话接口都是 OpenAI 兼容的
//! （`POST {base}/v1/chat/completions`），差别只在 base_url 与模型名；
//! Ollama 是 `/api/chat` 且响应形状不同，所以单独一个实现。
//!
//! 抽象出 `ModelProvider` 之后：
//!   · 换模型 = 换一个 base_url，业务代码不动
//!   · 测试用 `StaticProvider` 喂脚本化响应
//!   · `None` 表示"没有模型"，AI 端点降级为 503（见 ADR-0006 §5）

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::AiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    /// 工具结果回填。OpenAI 兼容接口里 role 是 `tool`。
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// 模型请求的一次工具调用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// OpenAI 兼容接口回传的调用 id，回填结果时要原样带回。
    pub id: String,
    pub name: String,
    /// **是 JSON 字符串，不是对象**。OpenAI 的 `function.arguments` 是 string。
    pub arguments: String,
}

/// 给模型的工具声明。形状与 OpenAI 的 `tools[]` 一致。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema（object）。由 `ToolSpec::parameters_schema()` 生成。
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    /// 普通文本。工具回填消息里可以是 JSON 字符串。
    #[serde(default)]
    pub content: String,
    /// assistant 消息里带的工具调用。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRequest>,
    /// role=tool 时对应的调用 id。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// 工具结果回填。`result` 会被序列化成字符串 —— OpenAI 要求 content 是字符串。
    pub fn tool_result(call_id: impl Into<String>, result: &Value) -> Self {
        Self {
            role: Role::Tool,
            content: serde_json::to_string(result).unwrap_or_else(|_| "{}".into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(call_id.into()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatResponse {
    /// 最终文本。模型选择直接回答时有值，选择调工具时通常为空。
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCallRequest>,
    pub finish_reason: Option<String>,
    pub usage: Option<TokenUsage>,
}

impl ChatResponse {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: Some(content.into()),
            ..Default::default()
        }
    }
}

#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// 人类可读的标识，进日志与审计。
    fn id(&self) -> &str;
    fn model(&self) -> &str;
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AiError>;
}

/// 测试与"没配模型"两种场景用。
///
/// `responses` 用完后返回 `ModelUnavailable` —— 让调用方走降级路径，
/// 而不是静默返回空回答（那会让用户以为 AI 真的什么都找不到）。
pub struct StaticProvider {
    id: String,
    model: String,
    responses: std::sync::Mutex<std::collections::VecDeque<ChatResponse>>,
    /// 记下收到的请求，便于断言"我们确实把工具声明发出去了"。
    recorded: std::sync::Mutex<Vec<ChatRequest>>,
}

impl StaticProvider {
    pub fn new(responses: Vec<ChatResponse>) -> Self {
        Self {
            id: "static".into(),
            model: "static".into(),
            responses: std::sync::Mutex::new(responses.into()),
            recorded: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// 断言用：取出所有收到的请求。
    pub fn recorded(&self) -> Vec<ChatRequest> {
        self.recorded.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelProvider for StaticProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, AiError> {
        self.recorded
            .lock()
            .map_err(|_| AiError::internal("StaticProvider 的请求记录被污染"))?
            .push(req);
        let next = self
            .responses
            .lock()
            .map_err(|_| AiError::internal("StaticProvider 的响应队列被污染"))?
            .pop_front();
        next.ok_or_else(|| AiError::model_unavailable("StaticProvider 的脚本化响应已用尽"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_match_openai() {
        assert_eq!(Role::System.as_str(), "system");
        assert_eq!(Role::Tool.as_str(), "tool");
    }

    #[test]
    fn tool_result_content_is_a_string() {
        // OpenAI 要求 role=tool 的 content 是字符串，不是对象。
        let m = ChatMessage::tool_result("call_1", &serde_json::json!({"ok": 1}));
        assert_eq!(m.role, Role::Tool);
        assert_eq!(m.content, r#"{"ok":1}"#);
        assert_eq!(m.tool_call_id.as_deref(), Some("call_1"));
    }

    #[tokio::test]
    async fn static_provider_exhausts_and_then_degrades() {
        let p = StaticProvider::new(vec![ChatResponse::text("hi")]);
        let _ = p
            .chat(ChatRequest::default())
            .await
            .expect("第一次有脚本化响应");
        let err = p.chat(ChatRequest::default()).await.unwrap_err();
        assert_eq!(err.kind, crate::error::AiErrorKind::ModelUnavailable);
    }
}
