//! AI 能力层：模型接入、工具授权、确认式执行。
//!
//! 这一层的边界由 ADR-0006（`ADR-0006`）钉死：
//!
//! 1. **AI 只能调工具，工具只能调 [`host::AiHost`]。**
//!    本 crate 里没有 SQL、没有 `std::fs`、没有 `Command`。
//!    想加能力，先往 `AiHost` 上加方法，再包成工具 —— 别在工具里直连业务。
//! 2. **危险操作必须 Plan → Preview → Confirm → Execute**（[`plan`]）。
//!    模型永远拿不到"直接生效"的写接口，它只能拿到一张待确认的工单。
//! 3. **模型不是必需品。** 没配模型时 [`intent::RuleEngine`] 顶上，
//!    配了但连不上时同样降级（[`runtime::RuntimeConfig::fallback_to_rules`]）。
//!
//! # 模块地图
//!
//! | 模块 | 职责 |
//! |---|---|
//! | [`provider`] | 模型抽象（`ModelProvider`），OpenAI 兼容 / Ollama 两种实现 |
//! | [`transport`] | HTTP 抽象，便于测试与"本机没跑模型"场景 |
//! | [`tool`] | 工具契约（`AiTool` / `ToolSpec` / `Risk`）与注册表 |
//! | [`tools`] | 首批 8 个工具的实现 |
//! | [`host`] | 业务能力接口（`AiHost`）—— AI 与业务之间唯一的门 |
//! | [`plan`] | 待确认动作与一次性令牌 |
//! | [`runtime`] | 一问一答的完整编排（含降级与回合上限） |
//! | [`intent`] | 离线可用的中文意图解析 |
//! | [`agent`] | 五个 Agent 的人设与工具白名单 |
//! | [`mcp`] | JSON-RPC 2.0 的 MCP Server / Client |

pub mod agent;
pub mod error;
pub mod host;
pub mod intent;
pub mod mcp;
pub mod ollama;
pub mod openai;
pub mod plan;
pub mod provider;
pub mod runtime;
pub mod tool;
pub mod tools;
pub mod transport;

use std::sync::Arc;

pub use error::{AiError, AiErrorKind};
pub use host::{
    AggregatedVodHit, AiHost, CapabilityReport, DuplicateGroup, IdentifyOutcome, MediaHit,
    MediaQuery, MissingKind, RecommendSpec, ScanTicket, SortKey, WatchState,
};
// 排序是**单一真源**：`AiHost` 的每个实现都必须用这一个。
// 此前 `crates/api/src/ai.rs` 自己抄了一份，默认排序写成了 AddedDesc（这里是 TitleAsc），
// 于是"用 fixture 测出来的顺序"和"真实 DbHost 的顺序"不是同一件事 —— 测试证明不了真实行为。
pub use host::sort_hits;
pub use intent::{Intent, RuleEngine};
pub use mcp::{McpClient, McpServer};
pub use ollama::OllamaProvider;
pub use openai::{OpenAiCompatible, ProviderKind};
pub use plan::{Actor, ActorRole, ConfirmVault, PendingAction};
pub use provider::{ChatMessage, ChatRequest, ChatResponse, ModelProvider, Role, StaticProvider};
pub use runtime::{build_runtime, AiRuntime, RuntimeConfig, ToolTrace, TurnResult};
pub use tool::{AiTool, ParamKind, ParamSpec, Preview, PreviewLine, Risk, ToolRegistry, ToolSpec};
pub use transport::{HttpTransport, ReqwestTransport};

// ---------------------------------------------------------------------------
// 组装
// ---------------------------------------------------------------------------

/// 按配置造出"模型 + 工具 + 运行时"。
///
/// `provider` 传 `None` 是合法且常见的：没配模型时整站 AI 走规则引擎，
/// 而不是 500。真正该报错的是"配了模型但连不上"，那是运行时的事，
/// 由 `AiRuntime::ask` 的降级分支处理。
pub fn from_config(
    cfg: &whatmedia_config::AiConfig,
    provider: Option<Arc<dyn ModelProvider>>,
    host: Arc<dyn AiHost>,
) -> AiRuntime {
    build_runtime(cfg, provider, host)
}

/// 把 `AiConfig` 翻译成一个模型客户端。
///
/// 返回 `None` 的场景全部应被当作"降级"而不是"故障"：
/// - `enabled == false`
/// - 厂商名不认识
/// - 需要 key 但环境变量里没有（含空串）
/// - 构造 HTTP 客户端失败（极少见，但别让它 panic）
///
/// 刻意不在这里打 error 日志：没配模型是常态，不是异常。
pub fn build_provider(cfg: &whatmedia_config::AiConfig) -> Option<Arc<dyn ModelProvider>> {
    if !cfg.enabled {
        return None;
    }
    let kind = ProviderKind::parse(&cfg.provider)?;

    let base_url = cfg.base_url.clone().filter(|s| !s.trim().is_empty());

    let api_key = if kind.requires_api_key() {
        let v = cfg.api_key()?;
        Some(v)
    } else {
        None
    };

    let model = if cfg.model.trim().is_empty() {
        String::new()
    } else {
        cfg.model.clone()
    };

    let transport = Arc::new(ReqwestTransport::new(cfg.timeout_secs.max(1)).ok()?);

    match kind {
        ProviderKind::Ollama => Some(Arc::new(OllamaProvider::new(base_url, model, transport))),
        other => OpenAiCompatible::new(other, base_url, model, api_key, transport)
            .ok()
            .map(|p| Arc::new(p) as Arc<dyn ModelProvider>),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatmedia_config::AiConfig;

    fn cfg_of(provider: &str) -> AiConfig {
        AiConfig {
            enabled: true,
            provider: provider.into(),
            ..AiConfig::default()
        }
    }

    #[test]
    fn disabled_config_yields_no_provider() {
        assert!(build_provider(&AiConfig::default()).is_none());
    }

    #[test]
    fn unknown_provider_yields_no_provider() {
        assert!(build_provider(&cfg_of("不存在的厂商")).is_none());
    }

    #[test]
    fn custom_provider_without_base_url_yields_no_provider() {
        assert!(build_provider(&cfg_of("custom")).is_none());
    }

    #[test]
    fn ollama_needs_no_api_key() {
        // 即使 WHATMEDIA_AI_API_KEY 没设，Ollama 也该能构造出来
        let cfg = AiConfig {
            model: "qwen2.5:7b".into(),
            ..cfg_of("ollama")
        };
        assert!(
            build_provider(&cfg).is_some(),
            "Ollama 不需要 key，不该因环境变量缺失而放弃"
        );
    }

    #[test]
    fn from_config_without_provider_still_works() {
        let rt = from_config(
            &AiConfig::default(),
            None,
            Arc::new(host::InMemoryHost::new(vec![])),
        );
        assert!(!rt.uses_model());
        // 没模型也要能用 —— 这是 P7 的硬要求
        assert!(rt.tools().iter().any(|s| s.name == "search_media"));
    }
}
