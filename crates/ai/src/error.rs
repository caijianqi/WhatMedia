//! AI 链路的错误类型。
//!
//! 分两类，API 层据此映射 HTTP 状态：
//!
//! | 错误 | HTTP | 含义 |
//! | --- | --- | --- |
//! | `ModelUnavailable` | 503 `AI_UNAVAILABLE` | 没配模型 / 断网 / 超时。**核心媒体功能不受影响** |
//! | `Denied` | 403 | 角色不够或危险工具被关 |
//! | `ConfirmRequired` | 200 + pending | 不是错误，是一个待确认动作 |
//! | 其余 | 400 / 500 | 参数不对或宿主出错 |

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiErrorKind {
    /// 模型不可用。断网、超时、没配 provider 都归这里。
    ModelUnavailable,
    /// 模型回了，但内容没法用（不是 JSON / 缺 choices）。
    ModelProtocol,
    /// 工具不存在。危险工具被关掉时也用它 —— 让模型以为工具不存在，
    /// 比告诉它"存在但不给你用"更能避免反复重试。
    ToolNotFound,
    /// 参数不符合工具声明。
    BadArgs,
    /// 权限不足：角色不够、危险工具被禁用。
    Denied,
    /// 需要二次确认才能继续。
    ConfirmRequired,
    /// 确认令牌已用过。
    TokenReplayed,
    /// 确认令牌过期。
    TokenExpired,
    /// 令牌与当前 actor 或参数不匹配。
    TokenMismatch,
    /// 宿主（业务侧）出错。
    Host,
    Internal,
}

impl AiErrorKind {
    /// 稳定错误码，直接进 API 的错误信封。
    pub fn code(&self) -> &'static str {
        match self {
            AiErrorKind::ModelUnavailable => "AI_UNAVAILABLE",
            AiErrorKind::ModelProtocol => "AI_MODEL_PROTOCOL",
            AiErrorKind::ToolNotFound => "AI_TOOL_NOT_FOUND",
            AiErrorKind::BadArgs => "AI_BAD_ARGS",
            AiErrorKind::Denied => "AI_DENIED",
            AiErrorKind::ConfirmRequired => "AI_CONFIRM_REQUIRED",
            AiErrorKind::TokenReplayed => "AI_TOKEN_REPLAYED",
            AiErrorKind::TokenExpired => "AI_TOKEN_EXPIRED",
            AiErrorKind::TokenMismatch => "AI_TOKEN_MISMATCH",
            AiErrorKind::Host => "AI_HOST_ERROR",
            AiErrorKind::Internal => "AI_INTERNAL",
        }
    }

    /// 对应的 HTTP 状态。`ConfirmRequired` 不是失败，给 200。
    pub fn status(&self) -> u16 {
        match self {
            AiErrorKind::ModelUnavailable => 503,
            AiErrorKind::ModelProtocol => 502,
            AiErrorKind::ToolNotFound => 404,
            AiErrorKind::BadArgs => 400,
            AiErrorKind::Denied => 403,
            AiErrorKind::ConfirmRequired => 200,
            AiErrorKind::TokenReplayed => 409,
            AiErrorKind::TokenExpired => 410,
            AiErrorKind::TokenMismatch => 403,
            AiErrorKind::Host => 500,
            AiErrorKind::Internal => 500,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{kind:?}: {message}")]
pub struct AiError {
    pub kind: AiErrorKind,
    pub message: String,
}

impl AiError {
    pub fn new(kind: AiErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn model_unavailable(message: impl Into<String>) -> Self {
        Self::new(AiErrorKind::ModelUnavailable, message)
    }

    pub fn model_protocol(message: impl Into<String>) -> Self {
        Self::new(AiErrorKind::ModelProtocol, message)
    }

    pub fn tool_not_found(name: impl Into<String>) -> Self {
        Self::new(
            AiErrorKind::ToolNotFound,
            format!("没有这个工具：{}", name.into()),
        )
    }

    pub fn bad_args(message: impl Into<String>) -> Self {
        Self::new(AiErrorKind::BadArgs, message)
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(AiErrorKind::Denied, message)
    }

    pub fn host(message: impl Into<String>) -> Self {
        Self::new(AiErrorKind::Host, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(AiErrorKind::Internal, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        // 错误码进了 API 契约，改名等于破坏前端。
        assert_eq!(AiErrorKind::ModelUnavailable.code(), "AI_UNAVAILABLE");
        assert_eq!(AiErrorKind::Denied.code(), "AI_DENIED");
        assert_eq!(AiErrorKind::TokenReplayed.code(), "AI_TOKEN_REPLAYED");
    }

    #[test]
    fn confirm_required_is_not_a_failure() {
        assert_eq!(AiErrorKind::ConfirmRequired.status(), 200);
        assert_eq!(AiErrorKind::ModelUnavailable.status(), 503);
    }
}
