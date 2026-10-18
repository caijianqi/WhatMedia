//! 多用户鉴权（一次性补上 P9 的地基）。
//!
//! 这个 crate 是**纯计算**的：口令摘要、票据、签名、角色与分级判断。
//! 它不碰数据库、不发网络请求、不读时钟之外的环境。
//! 所有持久化都放进 `crates/database::repo::auth`，全部串联在 `crates/api` 完成。
//!
//! 为什么要这样切：鉴权是全系统最经不起"环境偶发失败"的地方。
//! 一个既不查库又不联网的 crate 可以被几百条毫秒级单测逐格覆盖，
//! 而一旦把 I/O 混进来，就只能写集成测试 —— 于是没人写。
//!
//! ## 四条不可动摇的规则
//!
//! 1. **默认不放行。** 任何请求拿不出有效凭据就是 401。不存在"开发模式免鉴权"开关。
//! 2. **凭据必须可吊销。** 票据落库 + 支持"登出所有设备"，JWT 做不到这件事。
//! 3. **库存的是摘要不是明文。** 库被拖走不等于会话被接管。
//! 4. **儿童的限制由服务端裁决。** 前端可以有不同 UI，但闸门在服务端。

pub mod password;
pub mod policy;
pub mod signed;
pub mod token;

pub use password::{
    dummy_verify, hash_password, hash_pin, validate_password, validate_pin, verify_password,
    verify_pin, MIN_PASSWORD_LEN, PIN_LEN_RANGE,
};
pub use policy::{allows_rating, level_of, Ability, Denied, Level, Principal, Role};
pub use signed::{derive_stream_key, hmac_sha256, SigningKey};
pub use token::{bearer_value, constant_time_eq, cookie_value, hash_token, new_token};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("口令太短：至少 {min} 个字符（当前 {got}）")]
    WeakPassword { min: usize, got: usize },

    #[error("PIN 必须是 4–12 位纯数字")]
    WeakPin,

    #[error("输入超过 {max} 字节")]
    InputTooLong { max: usize },

    #[error("散列失败：{0}")]
    Hash(String),

    #[error("摘要格式非法：{0}")]
    BadHash(String),

    #[error("签名无效")]
    BadSignature,

    #[error("签名已于 {exp_unix} 过期（现在 {now_unix}）")]
    SignatureExpired { exp_unix: i64, now_unix: i64 },

    #[error("缺少凭据")]
    MissingCredential,

    #[error("凭据无效或已过期")]
    InvalidCredential,

    #[error("该设备尚未获得家长批准")]
    ApprovalRequired,

    #[error("账户已被停用")]
    AccountDisabled,

    #[error("登录失败次数过多，请稍后再试")]
    AccountLocked,

    #[error("用户名已被占用")]
    UsernameTaken,

    #[error("操作未被允许")]
    Forbidden,
}

impl AuthError {
    /// 映射到 HTTP 状态码。放在这里而不是 API 层，是为了让"错误 → 响应"这件事
    /// 有唯一出处：改一处，所有端点跟着变。
    pub fn status_code(&self) -> u16 {
        match self {
            AuthError::MissingCredential | AuthError::InvalidCredential => 401,
            AuthError::BadSignature | AuthError::SignatureExpired { .. } => 401,
            AuthError::AccountLocked => 429,
            AuthError::ApprovalRequired => 403,
            AuthError::AccountDisabled | AuthError::Forbidden => 403,
            AuthError::WeakPassword { .. }
            | AuthError::WeakPin
            | AuthError::InputTooLong { .. }
            | AuthError::UsernameTaken => 400,
            AuthError::Hash(_) | AuthError::BadHash(_) => 500,
        }
    }

    /// 稳定的机器可读错误码，供前端分支。
    pub fn code(&self) -> &'static str {
        match self {
            AuthError::MissingCredential => "UNAUTHENTICATED",
            AuthError::InvalidCredential => "INVALID_CREDENTIAL",
            AuthError::BadSignature => "BAD_SIGNATURE",
            AuthError::SignatureExpired { .. } => "SIGNATURE_EXPIRED",
            AuthError::ApprovalRequired => "APPROVAL_REQUIRED",
            AuthError::AccountDisabled => "ACCOUNT_DISABLED",
            AuthError::AccountLocked => "ACCOUNT_LOCKED",
            AuthError::UsernameTaken => "USERNAME_TAKEN",
            AuthError::Forbidden => "FORBIDDEN",
            AuthError::WeakPassword { .. } | AuthError::WeakPin => "WEAK_CREDENTIAL",
            AuthError::InputTooLong { .. } => "INPUT_TOO_LONG",
            AuthError::Hash(_) | AuthError::BadHash(_) => "AUTH_INTERNAL",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_error_has_a_code_and_a_status() {
        let all = [
            AuthError::WeakPassword { min: 8, got: 3 },
            AuthError::WeakPin,
            AuthError::InputTooLong { max: 256 },
            AuthError::Hash("x".into()),
            AuthError::BadHash("y".into()),
            AuthError::BadSignature,
            AuthError::SignatureExpired {
                exp_unix: 1,
                now_unix: 2,
            },
            AuthError::MissingCredential,
            AuthError::InvalidCredential,
            AuthError::ApprovalRequired,
            AuthError::AccountDisabled,
            AuthError::AccountLocked,
            AuthError::UsernameTaken,
            AuthError::Forbidden,
        ];
        for e in all {
            assert!(!e.code().is_empty());
            assert!(e.status_code() >= 400);
        }
    }

    #[test]
    fn authentication_shapes_are_401_and_authorisation_is_403() {
        assert_eq!(AuthError::MissingCredential.status_code(), 401);
        assert_eq!(AuthError::InvalidCredential.status_code(), 401);
        assert_eq!(AuthError::ApprovalRequired.status_code(), 403);
        assert_eq!(AuthError::Forbidden.status_code(), 403);
    }
}
