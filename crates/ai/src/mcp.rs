//! MCP：对外暴露只读工具 + 调用外部 MCP（需求稿 §64）。
//!
//! **一条硬边界**：对外只登记 `Risk::ReadOnly` 的工具（ADR-0006 §7）。
//! 外部 AI 拿不到我们的用户会话，也就走不完"预览 → 确认"这一环 ——
//! 把危险工具暴露出去，等于开一个绕过二次确认的后门。
//! 这条不提供开关。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::error::AiError;
use crate::host::AiHost;
use crate::provider::ToolDefinition;
use crate::tool::ToolRegistry;

pub const JSONRPC: &str = "2.0";
/// 我们实现的协议版本。暴露给对端握手用。
pub const PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpRequest {
    pub jsonrpc: String,
    /// 通知（不需要响应）没有 id。
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpError {
    pub code: i32,
    pub message: String,
}

impl McpError {
    pub fn parse_error(m: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: m.into(),
        }
    }
    pub fn method_not_found(m: impl Into<String>) -> Self {
        Self {
            code: -32601,
            message: m.into(),
        }
    }
    pub fn invalid_params(m: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: m.into(),
        }
    }
    /// 自定义：危险工具不对外暴露。
    pub fn not_exposed(m: impl Into<String>) -> Self {
        Self {
            code: -32001,
            message: m.into(),
        }
    }
    pub fn internal(m: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: m.into(),
        }
    }
}

pub struct McpServer {
    registry: Arc<ToolRegistry>,
    host: Arc<dyn AiHost>,
}

impl McpServer {
    pub fn new(registry: Arc<ToolRegistry>, host: Arc<dyn AiHost>) -> Self {
        Self { registry, host }
    }

    /// 处理一行 JSON-RPC。**通知返回 None**（不回复）。
    pub async fn handle(&self, raw: &str) -> Option<String> {
        let req: McpRequest = match serde_json::from_str(raw) {
            Ok(r) => r,
            Err(e) => {
                return Some(
                    json!({
                        "jsonrpc": JSONRPC,
                        "id": Value::Null,
                        "error": McpError::parse_error(format!("不是合法 JSON：{e}")),
                    })
                    .to_string(),
                )
            }
        };
        // 通知：id 缺失即不回复。
        let id = req.id.clone()?;
        let (result, error) = match self.dispatch(&req).await {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        let mut out = json!({ "jsonrpc": JSONRPC, "id": id });
        if let Some(r) = result {
            out["result"] = r;
        }
        if let Some(e) = error {
            out["error"] = json!(e);
        }
        Some(out.to_string())
    }

    async fn dispatch(&self, req: &McpRequest) -> Result<Value, McpError> {
        match req.method.as_str() {
            "initialize" | "ping" => Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": { "name": "whatmedia", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "tools": { "listChanged": false } },
            })),
            "tools/list" => {
                let tools: Vec<McpTool> = self
                    .registry
                    .externally_visible()
                    .iter()
                    .map(|s| McpTool {
                        name: s.name.clone(),
                        description: s.description.clone(),
                        input_schema: s.parameters_schema(),
                    })
                    .collect();
                Ok(json!({ "tools": tools }))
            }
            "tools/call" => {
                let name = req
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| McpError::invalid_params("缺 params.name"))?;
                let args = req
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                // 只读以外的工具在这里被挡掉，即使 registry 里开着。
                let spec = self
                    .registry
                    .externally_visible()
                    .into_iter()
                    .find(|s| s.name == name)
                    .ok_or_else(|| {
                        McpError::not_exposed(format!("工具 {name} 不对外提供（只暴露只读工具）"))
                    })?;
                let tool = self
                    .registry
                    .get(name)
                    .map_err(|e| McpError::method_not_found(e.message))?;
                spec.validate(&args)
                    .map_err(|e| McpError::invalid_params(e.message))?;
                let out = tool
                    .invoke(self.host.as_ref(), &args)
                    .await
                    .map_err(|e| McpError::internal(e.message))?;
                Ok(json!({
                    "content": [ { "type": "text", "text": out.get("text").cloned().unwrap_or(Value::Null) } ],
                    "structuredContent": out.get("data").cloned().unwrap_or(Value::Null),
                    "isError": false,
                }))
            }
            other => Err(McpError::method_not_found(format!("不支持的方法：{other}"))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

// ─────────────────────────────────────────────────────────────
// MCP Client：调用外部 MCP 服务
// ─────────────────────────────────────────────────────────────

/// 外部 MCP 的工具在这里是"别人家的能力"，
/// 因此**不进我们的 ToolRegistry**，也就不可能被本系统的 Agent 误用。
pub struct McpClient {
    endpoint: String,
    transport: Arc<dyn crate::transport::HttpTransport>,
    next_id: std::sync::atomic::AtomicU64,
}

impl McpClient {
    pub fn new(
        endpoint: impl Into<String>,
        transport: Arc<dyn crate::transport::HttpTransport>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            transport,
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn envelope(&self, method: &str, params: Value) -> Value {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        json!({ "jsonrpc": JSONRPC, "id": id, "method": method, "params": params })
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, AiError> {
        let res = self
            .transport
            .post_json(&self.endpoint, None, &self.envelope(method, params))
            .await?;
        let v = res.json()?;
        if let Some(err) = v.get("error") {
            return Err(AiError::host(format!(
                "外部 MCP 返回错误：{}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("未知")
            )));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn list_tools(&self) -> Result<Vec<ToolDefinition>, AiError> {
        let result = self.call("tools/list", json!({})).await?;
        let mut out = Vec::new();
        for t in result
            .get("tools")
            .and_then(|t| t.as_array())
            .unwrap_or(&vec![])
        {
            out.push(ToolDefinition {
                name: t
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .into(),
                description: t
                    .get("description")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .into(),
                parameters: t.get("input_schema").cloned().unwrap_or_else(|| json!({})),
            });
        }
        Ok(out)
    }

    pub async fn call_tool(&self, name: &str, args: &Value) -> Result<Value, AiError> {
        self.call("tools/call", json!({ "name": name, "arguments": args }))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::InMemoryHost;
    use crate::tool::default_registry;
    use crate::transport::FixtureTransport;
    use whatmedia_core::{Id, MediaKind};

    fn server() -> McpServer {
        let hit = crate::host::MediaHit {
            id: Id::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            title: "星际穿越".into(),
            kind: MediaKind::Movie,
            year: Some(2014),
            genres: vec![],
            rating: None,
            runtime_ms: None,
            size_bytes: None,
            added_at: None,
            has_poster: true,
            has_subtitle: true,
            has_metadata: true,
            progress: None,
            path: None,
        };
        let host = Arc::new(InMemoryHost::new(vec![hit]));
        // 故意打开 allow_dangerous：对外暴露仍然必须只剩只读工具。
        McpServer::new(Arc::new(default_registry(true)), host)
    }

    #[tokio::test]
    async fn tools_list_only_exposes_read_only() {
        let s = server();
        let out = s
            .handle(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#)
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let names: Vec<&str> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"search_media"));
        assert!(!names.contains(&"scan_library"));
        assert!(!names.contains(&"identify_media"));
    }

    #[tokio::test]
    async fn calling_a_dangerous_tool_is_refused() {
        let s = server();
        let out = s
            .handle(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"scan_library","arguments":{}}}"#,
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], -32001);
    }

    #[tokio::test]
    async fn read_only_call_works() {
        let s = server();
        let out = s
            .handle(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_media","arguments":{"limit":5}}}"#,
            )
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["id"], 3);
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("星际穿越"));
    }

    #[tokio::test]
    async fn notifications_get_no_reply() {
        let s = server();
        assert!(s
            .handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn malformed_json_returns_parse_error() {
        let s = server();
        let out = s.handle("not json").await.unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn unknown_method_is_reported() {
        let s = server();
        let out = s
            .handle(r#"{"jsonrpc":"2.0","id":9,"method":"nope","params":{}}"#)
            .await
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn client_lists_and_calls_external_tools() {
        let t = Arc::new(FixtureTransport::new(vec![(
            "mcp".into(),
            r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[{"name":"ext_search","description":"d","input_schema":{"type":"object"}}]}}"#
                .into(),
        )]));
        let c = McpClient::new("http://example.com/mcp", t.clone());
        let tools = c.list_tools().await.unwrap();
        assert_eq!(tools[0].name, "ext_search");
        let (url, body) = &t.recorded()[0];
        assert_eq!(url, "http://example.com/mcp");
        assert_eq!(body["method"], "tools/list");
        // id 必须递增，否则并发调用时响应对不上。
        assert_eq!(body["id"], 1);
    }

    #[tokio::test]
    async fn client_surfaces_remote_errors() {
        let t = Arc::new(FixtureTransport::new(vec![(
            "mcp".into(),
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"bad params"}}"#.into(),
        )]));
        let c = McpClient::new("http://example.com/mcp", t);
        let e = c.call_tool("x", &json!({})).await.unwrap_err();
        assert_eq!(e.kind, crate::error::AiErrorKind::Host);
    }
}
