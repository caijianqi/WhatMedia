//! 工具层：AI 能做的每一件事都必须在这里登记。
//!
//! 三条硬规则（ADR-0006 §2、§3）：
//!
//! 1. **没有登记的事，AI 做不了。** 这是"AI 只能走 Tool"的落地方式 ——
//!    不是靠提示词约束模型，而是靠"压根没有别的代码路径"。
//! 2. **危险工具默认不存在。** `allow_dangerous=false` 时它们连 `tools/list`
//!    都不返回。让模型"看得见但调不动"只会诱导它反复重试。
//! 3. **危险工具必须给出人读得懂的预览**，否则不给确认令牌。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::AiError;
use crate::host::AiHost;
use crate::provider::ToolDefinition;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// 只查询。直接执行。
    ReadOnly,
    /// 改库或触发任务。必须预览 + 确认。
    Write,
    /// 删除 / 移动 / 批量覆盖。必须预览 + 确认，且预览要列出每一条受影响对象。
    Destructive,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::ReadOnly => "read_only",
            Risk::Write => "write",
            Risk::Destructive => "destructive",
        }
    }

    pub fn needs_confirm(self) -> bool {
        !matches!(self, Risk::ReadOnly)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParamKind {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
}

impl ParamKind {
    /// 对外契约里的类型名（`/api/v1/ai/tools` 与 MCP 都用它）。
    pub fn label(self) -> &'static str {
        self.schema_type()
    }

    fn schema_type(self) -> &'static str {
        match self {
            ParamKind::String => "string",
            ParamKind::Integer => "integer",
            ParamKind::Number => "number",
            ParamKind::Boolean => "boolean",
            ParamKind::Array => "array",
            ParamKind::Object => "object",
        }
    }

    fn matches(self, v: &Value) -> bool {
        match self {
            ParamKind::String => v.is_string(),
            ParamKind::Integer => v.is_i64() || v.is_u64(),
            ParamKind::Number => v.is_number(),
            ParamKind::Boolean => v.is_boolean(),
            ParamKind::Array => v.is_array(),
            ParamKind::Object => v.is_object(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamSpec {
    pub name: String,
    pub description: String,
    pub kind: ParamKind,
    #[serde(default)]
    pub required: bool,
}

impl ParamSpec {
    pub fn new(name: &str, kind: ParamKind, description: &str) -> Self {
        Self {
            name: name.into(),
            kind,
            description: description.into(),
            required: false,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    /// **给模型看的**。写清楚什么时候该用它、什么时候不该，
    /// 模型选错工具多半是描述写得含糊。
    pub description: String,
    pub params: Vec<ParamSpec>,
    pub risk: Risk,
}

impl ToolSpec {
    /// 生成 JSON Schema。手写而不是引 schemars：
    /// 这里只需要 object/properties/required 三个关键字，引一个依赖不值得。
    pub fn parameters_schema(&self) -> Value {
        let mut props = serde_json::Map::new();
        let mut required = Vec::new();
        for p in &self.params {
            props.insert(
                p.name.clone(),
                json!({ "type": p.kind.schema_type(), "description": p.description }),
            );
            if p.required {
                required.push(json!(p.name));
            }
        }
        json!({
            "type": "object",
            "properties": props,
            "required": required,
        })
    }

    pub fn to_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters_schema(),
        }
    }

    /// 参数校验。**在调用工具之前**，绝不让非法参数进到业务层。
    pub fn validate(&self, args: &Value) -> Result<(), AiError> {
        let obj = args
            .as_object()
            .ok_or_else(|| AiError::bad_args(format!("{} 的参数必须是对象", self.name)))?;
        for p in &self.params {
            match obj.get(&p.name) {
                None if p.required => {
                    return Err(AiError::bad_args(format!(
                        "{} 缺必填参数 {}",
                        self.name, p.name
                    )))
                }
                Some(v) if !p.kind.matches(v) => {
                    return Err(AiError::bad_args(format!(
                        "{} 的参数 {} 类型不对，应为 {}",
                        self.name,
                        p.name,
                        p.kind.schema_type()
                    )))
                }
                _ => {}
            }
        }
        // 未知参数直接拒绝而不是忽略：模型拼错参数名时，
        // 忽略会让它以为"工具执行了但没生效"，然后反复重试。
        for k in obj.keys() {
            if !self.params.iter().any(|p| &p.name == k) {
                return Err(AiError::bad_args(format!("{} 不接受参数 {k}", self.name)));
            }
        }
        Ok(())
    }
}

/// 预览里的一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewLine {
    /// 受影响的对象（片名、库名、路径）。
    pub target: String,
    pub action: String,
    pub detail: Option<String>,
}

/// 危险动作的预览。**必须让人在不看参数的情况下看懂会发生什么。**
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preview {
    pub summary: String,
    pub lines: Vec<PreviewLine>,
    pub warnings: Vec<String>,
}

impl Preview {
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            lines: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

#[async_trait]
pub trait AiTool: Send + Sync {
    fn spec(&self) -> ToolSpec;
    /// 危险动作的预览。只读工具不会被问到它。
    async fn preview(&self, host: &dyn AiHost, args: &Value) -> Result<Preview, AiError>;
    /// 真正执行。**只在确认通过之后**被危险工具调用。
    async fn invoke(&self, host: &dyn AiHost, args: &Value) -> Result<Value, AiError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn AiTool>>,
    allow_dangerous: bool,
}

impl ToolRegistry {
    pub fn new(allow_dangerous: bool) -> Self {
        Self {
            tools: BTreeMap::new(),
            allow_dangerous,
        }
    }

    pub fn register(&mut self, tool: Arc<dyn AiTool>) {
        self.tools.insert(tool.spec().name.clone(), tool);
    }

    pub fn allow_dangerous(&self) -> bool {
        self.allow_dangerous
    }

    /// 可见的工具清单。危险工具被禁时**不出现在清单里**。
    pub fn visible(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|t| t.spec())
            .filter(|s| self.allow_dangerous || s.risk == Risk::ReadOnly)
            .collect()
    }

    /// 对外 MCP 只暴露只读工具（ADR-0006 §7）。这条没有开关。
    pub fn externally_visible(&self) -> Vec<ToolSpec> {
        self.tools
            .values()
            .map(|t| t.spec())
            .filter(|s| s.risk == Risk::ReadOnly)
            .collect()
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn AiTool>, AiError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| AiError::tool_not_found(name))?;
        let spec = tool.spec();
        if spec.risk.needs_confirm() && !self.allow_dangerous {
            // 用 NotFound 而不是 Denied：模型看到"没有这个工具"就不会重试。
            return Err(AiError::tool_not_found(name));
        }
        Ok(tool.clone())
    }

    /// 不过滤地查工具是否存在。
    ///
    /// 用来把"被 `allow_dangerous` 关掉"和"这个工具真的不存在"区分开：
    /// 前者要给用户一句人话，后者是代码 bug。[`ToolRegistry::get`] 对两者
    /// 都返回 `ToolNotFound`（对模型是对的），所以上层需要这个方法来分辨。
    pub fn spec_unfiltered(&self, name: &str) -> Option<ToolSpec> {
        self.tools.get(name).map(|t| t.spec())
    }
}

/// 首批 8 个工具（P7）。集中在这里装配，便于一处看清全貌。
pub fn default_registry(allow_dangerous: bool) -> ToolRegistry {
    let mut r = ToolRegistry::new(allow_dangerous);
    r.register(Arc::new(crate::tools::SearchMediaTool));
    r.register(Arc::new(crate::tools::GetMediaTool));
    r.register(Arc::new(crate::tools::FindMissingPosterTool));
    r.register(Arc::new(crate::tools::FindDuplicateTool));
    r.register(Arc::new(crate::tools::RecommendMediaTool));
    r.register(Arc::new(crate::tools::GetPlaybackCapabilityTool));
    r.register(Arc::new(crate::tools::ScanLibraryTool));
    r.register(Arc::new(crate::tools::IdentifyMediaTool));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ToolSpec {
        ToolSpec {
            name: "t".into(),
            description: "d".into(),
            params: vec![
                ParamSpec::new("a", ParamKind::String, "x").required(),
                ParamSpec::new("n", ParamKind::Integer, "y"),
            ],
            risk: Risk::ReadOnly,
        }
    }

    #[test]
    fn schema_has_required_list() {
        let s = spec().parameters_schema();
        assert_eq!(s["type"], "object");
        assert_eq!(s["required"][0], "a");
        assert_eq!(s["properties"]["n"]["type"], "integer");
    }

    #[test]
    fn validation_rejects_missing_required() {
        let s = spec();
        assert!(s.validate(&json!({})).is_err());
        assert!(s.validate(&json!({"a": "x"})).is_ok());
    }

    #[test]
    fn validation_rejects_wrong_type() {
        let s = spec();
        let e = s.validate(&json!({"a": "x", "n": "3"})).unwrap_err();
        assert_eq!(e.kind, crate::error::AiErrorKind::BadArgs);
    }

    #[test]
    fn unknown_param_is_rejected_not_ignored() {
        // 模型拼错参数名时，忽略会让它以为"执行了但没生效"然后反复重试。
        let s = spec();
        assert!(s.validate(&json!({"a": "x", "b": 1})).is_err());
    }

    #[test]
    fn dangerous_tools_are_absent_when_disabled() {
        let reg = default_registry(false);
        let names: Vec<String> = reg.visible().iter().map(|s| s.name.clone()).collect();
        assert!(names.contains(&"search_media".to_string()));
        assert!(!names.contains(&"scan_library".to_string()));
        assert!(!names.contains(&"identify_media".to_string()));
        // 连 get 都要报"没有这个工具"，而不是"有但不给你用"。
        assert_eq!(
            // .err().unwrap() 而非 .unwrap_err()：后者要求 Arc<dyn AiTool> 也 Debug
            reg.get("scan_library").err().unwrap().kind,
            crate::error::AiErrorKind::ToolNotFound
        );
    }

    #[test]
    fn external_exposure_is_read_only_even_when_dangerous_allowed() {
        // 对外 MCP 没有"预览→确认"这一环，危险工具一律不出去。
        let reg = default_registry(true);
        let names: Vec<String> = reg
            .externally_visible()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        assert!(names.contains(&"search_media".to_string()));
        assert!(!names.contains(&"scan_library".to_string()));
        assert!(!names.contains(&"identify_media".to_string()));
    }

    #[test]
    fn default_registry_has_eight_tools() {
        assert_eq!(default_registry(true).visible().len(), 8);
    }
}
