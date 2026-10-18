//! 一次对话的执行体。
//!
//! 两条路径，同一个出口：
//!
//! ```text
//! 有模型   → ModelProvider → tool_calls → ToolRegistry → AiHost
//! 无模型   → RuleEngine    → Intent      → ToolRegistry → AiHost
//! ```
//!
//! **规则引擎与模型走同一条 Tool 通路**，这是刻意的：
//! 若规则引擎能绕过工具层直接查库，"AI 只能走 Tool"就成了半句话。
//!
//! 防失控：`max_tool_rounds` 是硬闸。模型陷入"调工具→不满意→再调"
//! 的循环时必须有上限，否则一次提问能打爆整台机器。

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

use crate::agent::{self, AgentId};
use crate::error::{AiError, AiErrorKind};
use crate::host::AiHost;
use crate::intent::{Intent, RuleEngine};
use crate::plan::{Actor, ConfirmVault, PendingAction};
use crate::provider::{ChatMessage, ChatRequest, ModelProvider};
use crate::tool::{default_registry, ToolRegistry, ToolSpec};

#[derive(Debug, Clone, Copy)]
pub struct RuntimeConfig {
    /// 一次对话最多几轮工具调用。
    pub max_tool_rounds: usize,
    /// 确认令牌有效期（秒）。
    pub confirm_ttl_secs: i64,
    /// 模型不可用时是否降级到规则引擎。
    pub fallback_to_rules: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            max_tool_rounds: 6,
            confirm_ttl_secs: 600,
            fallback_to_rules: true,
        }
    }
}

/// 一次工具调用的留痕。进审计，也给前端展示"AI 到底查了什么"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolTrace {
    pub tool: String,
    pub args: Value,
    pub ok: bool,
    /// 结果摘要。不把整个结果塞进来 —— 审计里会爆炸。
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    /// 给用户的回答。
    pub answer: String,
    pub agent: AgentId,
    /// 有待确认动作时非空。前端必须渲染预览并等待用户点确认。
    pub pending: Option<PendingAction>,
    pub traces: Vec<ToolTrace>,
    /// 是否走了"模型不可用 → 规则引擎"的降级路径。
    pub degraded: bool,
}

#[derive(Debug)]
enum Outcome {
    Done(Value),
    // 装箱：PendingAction 比 Value 大一个数量级，不装箱的话
    // 每次返回都要搬 256 字节，而这个枚举在热路径上每轮工具调用都过一次。
    Pending(Box<PendingAction>),
}

pub struct AiRuntime {
    provider: Option<Arc<dyn ModelProvider>>,
    tools: Arc<ToolRegistry>,
    host: Arc<dyn AiHost>,
    vault: Arc<ConfirmVault>,
    cfg: RuntimeConfig,
}

impl AiRuntime {
    pub fn new(
        provider: Option<Arc<dyn ModelProvider>>,
        tools: Arc<ToolRegistry>,
        host: Arc<dyn AiHost>,
        cfg: RuntimeConfig,
    ) -> Self {
        Self {
            provider,
            tools,
            host,
            vault: Arc::new(ConfirmVault::new(cfg.confirm_ttl_secs)),
            cfg,
        }
    }

    pub fn is_available(&self) -> bool {
        self.provider.is_some() || self.cfg.fallback_to_rules
    }

    pub fn uses_model(&self) -> bool {
        self.provider.is_some()
    }

    pub fn tools(&self) -> Vec<ToolSpec> {
        self.tools.visible()
    }

    pub fn pending(&self, actor: &Actor) -> Vec<PendingAction> {
        self.vault.pending_for(&actor.id)
    }

    pub fn cancel(&self, token: &str) -> bool {
        self.vault.cancel(token)
    }

    /// 一次提问。
    pub async fn ask(&self, actor: &Actor, message: &str) -> Result<TurnResult, AiError> {
        let agent = agent::route(message);
        let mut traces = Vec::new();

        if let Some(p) = &self.provider {
            match self
                .run_with_model(p, agent, actor, message, &mut traces)
                .await
            {
                Ok(r) => return Ok(r),
                Err(e) if e.kind == AiErrorKind::ModelUnavailable && self.cfg.fallback_to_rules => {
                    tracing::warn!("模型不可用，降级到规则引擎：{}", e.message);
                }
                Err(e) => return Err(e),
            }
        }

        if !self.cfg.fallback_to_rules {
            return Err(AiError::model_unavailable(
                "没有配置模型，且未开启规则降级（ai.fallback_to_rules = false）",
            ));
        }
        self.run_with_rules(agent, actor, message, traces).await
    }

    async fn run_with_model(
        &self,
        provider: &Arc<dyn ModelProvider>,
        agent: AgentId,
        actor: &Actor,
        message: &str,
        traces: &mut Vec<ToolTrace>,
    ) -> Result<TurnResult, AiError> {
        let mut messages = vec![
            ChatMessage::system(agent::compose_system_prompt(agent)),
            ChatMessage::user(message),
        ];
        // 只把 Agent 白名单里的工具交给模型。白名单是硬约束，
        // 提示词里再写一遍"不要用别的工具"只是提醒。
        let tools = self.tools_for_agent(agent);

        for _round in 0..self.cfg.max_tool_rounds {
            let resp = provider
                .chat(ChatRequest {
                    messages: messages.clone(),
                    tools: tools.clone(),
                    temperature: Some(0.2),
                })
                .await?;

            if resp.tool_calls.is_empty() {
                let answer = resp
                    .content
                    .unwrap_or_else(|| "（模型没有给出回答）".into());
                return Ok(TurnResult {
                    answer,
                    agent,
                    pending: None,
                    traces: traces.clone(),
                    degraded: false,
                });
            }

            messages.push(ChatMessage {
                role: crate::provider::Role::Assistant,
                content: resp.content.clone().unwrap_or_default(),
                tool_calls: resp.tool_calls.clone(),
                tool_call_id: None,
            });

            for call in &resp.tool_calls {
                let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|e| {
                    tracing::warn!("模型给的工具参数不是合法 JSON（{}）：{}", e, call.arguments);
                    Value::Object(Default::default())
                });
                match self
                    .dispatch(agent, actor, &call.name, args.clone())
                    .await?
                {
                    Outcome::Done(v) => {
                        let summary = v
                            .get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or("")
                            .chars()
                            .take(120)
                            .collect::<String>();
                        traces.push(ToolTrace {
                            tool: call.name.clone(),
                            args,
                            ok: true,
                            summary: summary.clone(),
                        });
                        messages.push(ChatMessage::tool_result(&call.id, &v));
                    }
                    Outcome::Pending(action) => {
                        // 危险工具：停下来等确认，不再继续这一轮。
                        traces.push(ToolTrace {
                            tool: call.name.clone(),
                            args,
                            ok: false,
                            summary: "等待用户确认".into(),
                        });
                        return Ok(TurnResult {
                            answer: format!("需要先你确认：{}", action.preview.summary),
                            agent,
                            pending: Some(*action),
                            traces: traces.clone(),
                            degraded: false,
                        });
                    }
                }
            }
        }

        Err(AiError::model_protocol(format!(
            "工具调用超过 {} 轮仍未收敛，中止",
            self.cfg.max_tool_rounds
        )))
    }

    async fn run_with_rules(
        &self,
        agent: AgentId,
        actor: &Actor,
        message: &str,
        mut traces: Vec<ToolTrace>,
    ) -> Result<TurnResult, AiError> {
        let intent = RuleEngine::parse(message);
        let Some((tool, args)) = intent.tool_call() else {
            let reason = match &intent {
                Intent::Unknown { reason } => reason.clone(),
                _ => "无法转成工具调用".into(),
            };
            // 明确说没听懂，绝不假装执行过。
            return Ok(TurnResult {
                answer: format!(
                    "这句我没理解（{reason}）。可以换个说法，比如“找出没有海报的电影”。"
                ),
                agent,
                pending: None,
                traces,
                degraded: true,
            });
        };

        let outcome = match self.dispatch(agent, actor, &tool, args.clone()).await {
            Ok(o) => o,
            // 工具存在、但被 allow_dangerous 关掉了。
            // ToolRegistry 对这种情况也报 ToolNotFound（对模型是对的，
            // 让它别重试），但对用户必须翻成人话 —— 否则他会以为
            // "这个助手不支持扫描"，而实际上是他自己没开开关。
            Err(e)
                if e.kind == AiErrorKind::ToolNotFound
                    && self.tools.spec_unfiltered(&tool).is_some() =>
            {
                return Ok(TurnResult {
                    answer: "这个操作会改动媒体库，但当前没有开启危险操作\
                             （ai.allow_dangerous = false）。开启后再问一次就行。"
                        .into(),
                    agent,
                    pending: None,
                    traces,
                    degraded: true,
                });
            }
            Err(e) => return Err(e),
        };

        match outcome {
            Outcome::Done(v) => {
                let answer = v
                    .get("text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("（没有结果）")
                    .to_string();
                traces.push(ToolTrace {
                    tool,
                    args,
                    ok: true,
                    summary: answer.chars().take(120).collect(),
                });
                Ok(TurnResult {
                    answer,
                    agent,
                    pending: None,
                    traces,
                    degraded: true,
                })
            }
            Outcome::Pending(action) => Ok(TurnResult {
                answer: format!("需要先你确认：{}", action.preview.summary),
                agent,
                pending: Some(*action),
                traces,
                degraded: true,
            }),
        }
    }

    fn tools_for_agent(&self, agent: AgentId) -> Vec<crate::provider::ToolDefinition> {
        self.tools
            .visible()
            .iter()
            .filter(|s| agent.tools().contains(&s.name.as_str()))
            .map(|s| s.to_definition())
            .collect()
    }

    /// 唯一执行工具的地方。**危险工具在这里只会产出待确认动作。**
    async fn dispatch(
        &self,
        agent: AgentId,
        actor: &Actor,
        name: &str,
        args: Value,
    ) -> Result<Outcome, AiError> {
        if !agent.tools().contains(&name) {
            return Err(AiError::denied(format!("{:?} 不能使用工具 {name}", agent)));
        }
        let tool = self.tools.get(name)?;
        let spec = tool.spec();
        if !actor.role.may_request(spec.risk) {
            return Err(AiError::denied(format!(
                "角色 {} 不能请求 {} 级操作",
                actor.role.as_str(),
                spec.risk.as_str()
            )));
        }
        spec.validate(&args)?;

        if spec.risk.needs_confirm() {
            let preview = tool.preview(self.host.as_ref(), &args).await?;
            let action = self
                .vault
                .issue(name, args, spec.risk, preview, actor, Utc::now());
            return Ok(Outcome::Pending(Box::new(action)));
        }

        let value = tool.invoke(self.host.as_ref(), &args).await?;
        Ok(Outcome::Done(value))
    }

    /// 确认并执行。
    pub async fn confirm(&self, actor: &Actor, token: &str) -> Result<TurnResult, AiError> {
        let action = self.vault.take_as_issued(token, actor, Utc::now())?;
        // 角色在**请求时**查过一次，执行时再查一次：
        // 会话期间用户可能从成人切换到儿童模式。
        if !actor.role.may_request(action.risk) {
            return Err(AiError::denied(format!(
                "角色 {} 不能执行 {} 级操作",
                actor.role.as_str(),
                action.risk.as_str()
            )));
        }
        let tool = self.tools.get(&action.tool)?;
        let value = tool.invoke(self.host.as_ref(), &action.args).await?;
        let answer = value
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("已执行")
            .to_string();
        // 先截摘要再移走 answer —— 顺序反过来会撞借用检查。
        let summary: String = answer.chars().take(120).collect();
        Ok(TurnResult {
            answer,
            agent: AgentId::Media,
            pending: None,
            traces: vec![ToolTrace {
                tool: action.tool.clone(),
                args: action.args.clone(),
                ok: true,
                summary,
            }],
            degraded: false,
        })
    }
}

/// 从配置装配一个运行时。供 API 层使用。
pub fn build_runtime(
    cfg: &whatmedia_config::AiConfig,
    provider: Option<Arc<dyn ModelProvider>>,
    host: Arc<dyn AiHost>,
) -> AiRuntime {
    AiRuntime::new(
        provider,
        Arc::new(default_registry(cfg.allow_dangerous)),
        host,
        RuntimeConfig {
            max_tool_rounds: cfg.max_tool_rounds.max(1),
            confirm_ttl_secs: 600,
            fallback_to_rules: cfg.fallback_to_rules,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{InMemoryHost, MediaHit};
    use crate::plan::ActorRole;
    use crate::provider::{ChatResponse, StaticProvider, ToolCallRequest};
    use crate::tool::default_registry;
    use whatmedia_core::{Id, MediaKind};

    fn host() -> Arc<InMemoryHost> {
        let now = Utc::now();
        let a = MediaHit {
            id: Id::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            title: "星际穿越".into(),
            kind: MediaKind::Movie,
            year: Some(2014),
            genres: vec!["Sci-Fi".into()],
            rating: Some(9.0),
            runtime_ms: Some(10_140_000),
            size_bytes: Some(30_000_000_000),
            added_at: Some(now),
            has_poster: false,
            has_subtitle: true,
            has_metadata: true,
            progress: Some(0.4),
            path: None,
        };
        Arc::new(InMemoryHost::new(vec![a]).with_now(now))
    }

    fn runtime(
        provider: Option<Arc<dyn ModelProvider>>,
        dangerous: bool,
    ) -> (AiRuntime, Arc<InMemoryHost>) {
        let h = host();
        let r = AiRuntime::new(
            provider,
            Arc::new(default_registry(dangerous)),
            h.clone(),
            RuntimeConfig::default(),
        );
        (r, h)
    }

    fn adult() -> Actor {
        Actor::new("u1", ActorRole::Adult)
    }

    // ── 规则降级路径 ──────────────────────────────────────

    #[tokio::test]
    async fn no_model_still_answers_via_rules() {
        let (r, _) = runtime(None, false);
        let out = r.ask(&adult(), "找出没有海报的电影。").await.unwrap();
        assert!(out.degraded);
        assert!(out.answer.contains("星际穿越"));
        // 规则引擎也必须走 Tool：留痕里要有 search_media / find_missing_poster
        assert!(!out.traces.is_empty());
    }

    #[tokio::test]
    async fn unparsable_question_says_so() {
        let (r, _) = runtime(None, false);
        let out = r.ask(&adult(), "嗯").await.unwrap();
        assert!(out.answer.contains("我没理解"));
        assert!(out.traces.is_empty());
    }

    #[tokio::test]
    async fn gated_dangerous_tool_gets_a_plain_answer() {
        // 危险工具被关时连 tools/list 都不出现。对模型报 NotFound 是对的
        // （它就不会重试），但用户看到"没有这个工具"只会以为助手不支持。
        let (r, h) = runtime(None, false);
        let out = r.ask(&adult(), "扫描一下媒体库").await.unwrap();
        assert!(out.pending.is_none());
        assert!(out.answer.contains("危险操作"), "实际回答：{}", out.answer);
        assert!(h.scan_calls().is_empty(), "没确认就不该碰宿主");
    }

    #[tokio::test]
    async fn rules_engine_cannot_bypass_the_tool_layer() {
        // 规则引擎与模型共用 dispatch，所以同样受 Agent 白名单约束：
        // Media Agent 没有 scan_library，即便开了危险操作也该被挡下。
        let (r, _) = runtime(None, true);
        let e = r
            .dispatch(
                AgentId::Media,
                &adult(),
                "scan_library",
                serde_json::json!({}),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(e.kind, AiErrorKind::Denied);
    }

    #[tokio::test]
    async fn dangerous_tool_stops_at_preview() {
        let (r, h) = runtime(None, true);
        let out = r.ask(&adult(), "扫描一下媒体库").await.unwrap();
        let pending = out.pending.expect("应产出待确认动作");
        assert!(pending.preview.summary.contains("扫描"));
        // 未经确认，宿主一次都没被调用
        assert!(h.scan_calls().is_empty());
    }

    #[tokio::test]
    async fn identify_media_stops_at_preview_until_confirmed() {
        // 为什么这条必须直接 `dispatch` 而不能走 `ask()`：
        // 规则引擎里**没有** identify_media 的问法（intent.rs 无匹配），只有模型会调它。
        // 于是"未确认就不碰宿主"这条断言此前对 identify 路径一次都没验证过 ——
        // 而它恰恰是会覆盖元数据的写操作。这是覆盖缺口，不是可选项。
        let (r, h) = runtime(None, true);
        let out = r
            .dispatch(
                AgentId::Library,
                &adult(),
                "identify_media",
                serde_json::json!({ "item_id": "00000000-0000-0000-0000-000000000001" }),
            )
            .await
            .unwrap();
        let action = match out {
            Outcome::Pending(a) => *a,
            Outcome::Done(_) => panic!("写操作未经确认就执行了"),
        };
        assert!(h.identify_calls().is_empty(), "没确认就不该碰宿主");

        let done = r.confirm(&adult(), &action.token).await.unwrap();
        assert!(done.answer.contains("已重新识别"), "实际：{}", done.answer);
        assert_eq!(h.identify_calls().len(), 1);

        let e = r.confirm(&adult(), &action.token).await.unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenReplayed);
        assert_eq!(h.identify_calls().len(), 1, "重放绝不能执行第二次");
    }

    #[tokio::test]
    async fn confirm_executes_exactly_once() {
        let (r, h) = runtime(None, true);
        let out = r.ask(&adult(), "扫描一下媒体库").await.unwrap();
        let token = out.pending.unwrap().token;
        let done = r.confirm(&adult(), &token).await.unwrap();
        assert!(done.answer.contains("已启动扫描"));
        assert_eq!(h.scan_calls().len(), 1);

        // 第二次确认必须失败
        let e = r.confirm(&adult(), &token).await.unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenReplayed);
        assert_eq!(h.scan_calls().len(), 1);
    }

    #[tokio::test]
    async fn another_user_cannot_confirm() {
        let (r, h) = runtime(None, true);
        let out = r.ask(&adult(), "扫描一下媒体库").await.unwrap();
        let token = out.pending.unwrap().token;
        let other = Actor::new("u2", ActorRole::Adult);
        let e = r.confirm(&other, &token).await.unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenMismatch);
        assert!(h.scan_calls().is_empty());
    }

    #[tokio::test]
    async fn child_cannot_request_writes() {
        let (r, h) = runtime(None, true);
        let kid = Actor::new("kid", ActorRole::Child);
        let e = r.ask(&kid, "扫描一下媒体库").await;
        // Media Agent 白名单不含 scan_library；换 Library Agent 也该被角色挡住
        assert!(e.is_err() || e.unwrap().pending.is_none());
        assert!(h.scan_calls().is_empty());
    }

    #[tokio::test]
    async fn library_agent_child_is_denied() {
        // 直接走 dispatch 的写路径，验证角色门槛确实生效。
        let (r, h) = runtime(None, true);
        let kid = Actor::new("kid", ActorRole::Child);
        let e = r
            .dispatch(
                AgentId::Library,
                &kid,
                "scan_library",
                serde_json::json!({}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.kind, AiErrorKind::Denied);
        assert!(h.scan_calls().is_empty());
    }

    #[tokio::test]
    async fn cancel_stops_the_action() {
        let (r, h) = runtime(None, true);
        let out = r.ask(&adult(), "扫描一下媒体库").await.unwrap();
        let token = out.pending.unwrap().token;
        assert!(r.cancel(&token));
        assert_eq!(
            r.confirm(&adult(), &token).await.unwrap_err().kind,
            AiErrorKind::TokenReplayed
        );
        assert!(h.scan_calls().is_empty());
    }

    // ── 模型路径 ──────────────────────────────────────────

    #[tokio::test]
    async fn model_tool_call_is_executed() {
        let p = Arc::new(StaticProvider::new(vec![
            ChatResponse {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: "c1".into(),
                    name: "search_media".into(),
                    arguments: r#"{"limit":5}"#.into(),
                }],
                finish_reason: None,
                usage: None,
            },
            ChatResponse::text("找到《星际穿越》。"),
        ]));
        let (r, _) = runtime(Some(p), false);
        let out = r.ask(&adult(), "找一下星际穿越").await.unwrap();
        assert!(!out.degraded);
        assert!(out.answer.contains("星际穿越"));
        assert_eq!(out.traces.len(), 1);
        assert_eq!(out.traces[0].tool, "search_media");
    }

    #[tokio::test]
    async fn model_can_answer_directly_without_tools() {
        let p = Arc::new(StaticProvider::new(vec![ChatResponse::text("你好")]));
        let (r, _) = runtime(Some(p), false);
        let out = r.ask(&adult(), "你好").await.unwrap();
        assert_eq!(out.answer, "你好");
        assert!(out.traces.is_empty());
    }

    #[tokio::test]
    async fn model_unavailable_falls_back_to_rules() {
        // 脚本化响应耗尽 = 模型不可用
        let p = Arc::new(StaticProvider::new(vec![]));
        let (r, _) = runtime(Some(p), false);
        let out = r.ask(&adult(), "找出没有海报的电影。").await.unwrap();
        assert!(out.degraded);
        assert!(out.answer.contains("星际穿越"));
    }

    #[tokio::test]
    async fn fallback_can_be_turned_off() {
        // 运维可能希望"模型挂了就明确报错"，而不是给出一个降级答案。
        let strict = AiRuntime::new(
            Some(Arc::new(StaticProvider::new(vec![])) as Arc<dyn ModelProvider>),
            Arc::new(default_registry(false)),
            host(),
            RuntimeConfig {
                fallback_to_rules: false,
                ..Default::default()
            },
        );
        let e = strict.ask(&adult(), "找一下星际穿越").await.unwrap_err();
        assert_eq!(e.kind, AiErrorKind::ModelUnavailable);
    }

    #[tokio::test]
    async fn tool_round_limit_is_enforced() {
        // 模型一直要调工具，从不收敛 —— 必须有上限。
        let calls: Vec<ChatResponse> = (0..12)
            .map(|i| ChatResponse {
                content: None,
                tool_calls: vec![ToolCallRequest {
                    id: format!("c{i}"),
                    name: "search_media".into(),
                    arguments: r#"{"limit":1}"#.into(),
                }],
                finish_reason: None,
                usage: None,
            })
            .collect();
        let p = Arc::new(StaticProvider::new(calls));
        let (r, _) = runtime(Some(p), false);
        let e = r.ask(&adult(), "找片子").await.unwrap_err();
        assert_eq!(e.kind, AiErrorKind::ModelProtocol);
        assert!(e.message.contains("超过"));
    }

    #[tokio::test]
    async fn dangerous_tool_via_model_also_waits_for_confirmation() {
        let p = Arc::new(StaticProvider::new(vec![ChatResponse {
            content: None,
            tool_calls: vec![ToolCallRequest {
                id: "c1".into(),
                name: "scan_library".into(),
                arguments: "{}".into(),
            }],
            finish_reason: None,
            usage: None,
        }]));
        let (r, h) = runtime(Some(p), true);
        let out = r.ask(&adult(), "扫描媒体库").await.unwrap();
        assert!(out.pending.is_some());
        assert!(h.scan_calls().is_empty());
    }

    #[tokio::test]
    async fn agent_tool_whitelist_is_enforced() {
        let (r, _) = runtime(None, true);
        // Media Agent 没有 scan_library
        let e = r
            .dispatch(
                AgentId::Media,
                &adult(),
                "scan_library",
                serde_json::json!({}),
            )
            .await
            .unwrap_err();
        assert_eq!(e.kind, AiErrorKind::Denied);
    }

    #[test]
    fn is_available_depends_on_provider_or_fallback() {
        let (r, _) = runtime(None, false);
        assert!(r.is_available());
        assert!(!r.uses_model());
    }
}
