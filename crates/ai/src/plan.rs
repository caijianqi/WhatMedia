//! Plan → Preview → Confirm → Execute。
//!
//! 需求稿 §62 只画了四个框。落到代码里，关键是**令牌的四条硬约束**：
//!
//! | 约束 | 挡住的事故 |
//! | --- | --- |
//! | 绑定 actor | A 申请的动作被 B 执行 |
//! | 绑定参数哈希 | 预览"删 3 部" → 改参数 → 执行时变成"删 300 部" |
//! | 一次性 | 同一条确认被重放两次 |
//! | 有过期时间 | 挂了一天的待确认动作仍然有效 |
//!
//! 四条都有测试。**"预览→确认"绕过任意一条就等于没做。**

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use uuid::Uuid;

use crate::error::{AiError, AiErrorKind};
use crate::tool::{Preview, Risk};

/// 角色。需求稿 §65。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    Owner,
    Administrator,
    Adult,
    /// 儿童模式：能浏览能播，**不能改库**。
    Child,
    /// 访客：只读。
    Guest,
}

impl ActorRole {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "owner" => ActorRole::Owner,
            "administrator" | "admin" => ActorRole::Administrator,
            "adult" => ActorRole::Adult,
            "child" | "kid" => ActorRole::Child,
            "guest" => ActorRole::Guest,
            _ => ActorRole::Adult,
        }
    }

    /// 权限等级。只用于比较，不要拿它做别的事。
    fn rank(self) -> u8 {
        match self {
            // 儿童与访客同级：都只能读。儿童不是"比访客高一点"，
            // 两者的差别在内容过滤（那是播放层的事），不在写权限上。
            ActorRole::Guest | ActorRole::Child => 0,
            ActorRole::Adult => 1,
            ActorRole::Administrator => 2,
            ActorRole::Owner => 3,
        }
    }

    /// 能否请求某个风险级别的动作。
    ///
    /// 儿童与访客连预览都不给 —— 给他们一份"看得见但永远确认不了"的预览，
    /// 只会让他们反复点确认然后反复被拒。
    ///
    /// **门槛只有一处定义**：[`risk_requires`]。曾经这里自己写一份、
    /// runtime 里再写一份，结果"成人能不能删文件"这种问题出现了两个答案，
    /// 而实际执行的那个并不是注释里写的那个。这类分歧不会报错，只会
    /// 在某个具体权限场景里静默地给出错误结果。
    pub fn may_request(&self, risk: Risk) -> bool {
        self.rank() >= risk_requires(risk).rank()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ActorRole::Owner => "owner",
            ActorRole::Administrator => "administrator",
            ActorRole::Adult => "adult",
            ActorRole::Child => "child",
            ActorRole::Guest => "guest",
        }
    }
}

/// 角色门槛：只读谁都能发，写操作要 Adult 以上，破坏性操作要管理员。
///
/// 这是**唯一**的权限策略定义处。目前首批工具里还没有 Destructive 级
/// （改名 / 移动 / 删除留到以后），但门槛先钉死，免得加工具时随手放宽。
pub fn risk_requires(risk: Risk) -> ActorRole {
    match risk {
        Risk::ReadOnly => ActorRole::Guest,
        Risk::Write => ActorRole::Adult,
        Risk::Destructive => ActorRole::Administrator,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    pub id: String,
    pub role: ActorRole,
}

impl Actor {
    pub fn new(id: impl Into<String>, role: ActorRole) -> Self {
        Self {
            id: id.into(),
            role,
        }
    }

    /// 未登录 / 内部调用。角色给 Adult 而不是 Owner：
    /// 缺省不该是权限最大的那一档。
    pub fn anonymous() -> Self {
        Self {
            id: "anonymous".into(),
            role: ActorRole::Adult,
        }
    }
}

/// 参数指纹。改动任何参数都会让它变，令牌随之失效。
pub fn args_fingerprint(args: &Value) -> String {
    let canonical = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
    let mut h = Sha256::new();
    h.update(canonical.as_bytes());
    format!("{:x}", h.finalize())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    pub id: String,
    /// 一次性令牌。**不要**把它当成身份凭证：它只证明"用户看过这份预览"。
    pub token: String,
    pub tool: String,
    pub args: Value,
    pub args_fingerprint: String,
    pub risk: Risk,
    pub preview: Preview,
    pub actor_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl PendingAction {
    /// 剩多少秒过期。前端拿它做倒计时。
    pub fn seconds_left(&self, now: DateTime<Utc>) -> i64 {
        (self.expires_at - now).num_seconds().max(0)
    }
}

pub struct ConfirmVault {
    inner: Mutex<Vault>,
    ttl: Duration,
}

#[derive(Default)]
struct Vault {
    pending: HashMap<String, PendingAction>,
    /// 已用过的令牌。留着是为了把"重放"和"无效"区分开 ——
    /// 两者都是找不到，但对用户的提示完全不同。
    consumed: HashSet<String>,
}

impl ConfirmVault {
    /// `ttl_secs` 默认 600 秒（10 分钟）。挂着不动的待确认动作不该一直有效。
    pub fn new(ttl_secs: i64) -> Self {
        Self {
            inner: Mutex::new(Vault::default()),
            ttl: Duration::seconds(ttl_secs.max(1)),
        }
    }

    pub fn issue(
        &self,
        tool: &str,
        args: Value,
        risk: Risk,
        preview: Preview,
        actor: &Actor,
        now: DateTime<Utc>,
    ) -> PendingAction {
        let token = Uuid::new_v4().simple().to_string();
        let action = PendingAction {
            id: Uuid::new_v4().simple().to_string(),
            token: token.clone(),
            tool: tool.into(),
            args: args.clone(),
            args_fingerprint: args_fingerprint(&args),
            risk,
            preview,
            actor_id: actor.id.clone(),
            created_at: now,
            expires_at: now + self.ttl,
        };
        self.inner
            .lock()
            .unwrap()
            .pending
            .insert(token, action.clone());
        action
    }

    /// 取走并执行。**取走即销毁**，第二次必定失败。
    pub fn take(
        &self,
        token: &str,
        actor: &Actor,
        args: &Value,
        now: DateTime<Utc>,
    ) -> Result<PendingAction, AiError> {
        self.take_inner(token, actor, Some(args), now)
    }

    /// 用**预览时那份参数**取走。确认接口走这条：
    /// 用户说"确认"时不该、也无力重新拼一遍参数。
    pub fn take_as_issued(
        &self,
        token: &str,
        actor: &Actor,
        now: DateTime<Utc>,
    ) -> Result<PendingAction, AiError> {
        self.take_inner(token, actor, None, now)
    }

    fn take_inner(
        &self,
        token: &str,
        actor: &Actor,
        args: Option<&Value>,
        now: DateTime<Utc>,
    ) -> Result<PendingAction, AiError> {
        let mut v = self.inner.lock().unwrap();
        if let Some(action) = v.pending.get(token) {
            if action.expires_at <= now {
                v.pending.remove(token);
                v.consumed.insert(token.to_string());
                return Err(AiError::new(
                    AiErrorKind::TokenExpired,
                    "确认已过期，请重新发起",
                ));
            }
            if action.actor_id != actor.id {
                return Err(AiError::new(
                    AiErrorKind::TokenMismatch,
                    "这条待确认动作属于另一个用户",
                ));
            }
            let fingerprint = match args {
                Some(a) => args_fingerprint(a),
                None => action.args_fingerprint.clone(),
            };
            if action.args_fingerprint != fingerprint {
                // 预览之后参数被改了 —— 这是最危险的一类绕过，直接作废。
                v.pending.remove(token);
                v.consumed.insert(token.to_string());
                return Err(AiError::new(
                    AiErrorKind::TokenMismatch,
                    "参数与预览时不一致，请重新确认",
                ));
            }
            let action = v.pending.remove(token).expect("刚刚还在");
            v.consumed.insert(token.to_string());
            return Ok(action);
        }
        if v.consumed.contains(token) {
            return Err(AiError::new(
                AiErrorKind::TokenReplayed,
                "这条确认已经用过了，不能重复执行",
            ));
        }
        // 令牌不存在时也给 TokenMismatch（403），而不是 404。
        // 刻意让"令牌不存在"与"令牌不是你的"无法区分：
        // 返回 404 等于给了一个枚举有效令牌的神谕。
        Err(AiError::new(
            AiErrorKind::TokenMismatch,
            "没有这条待确认动作",
        ))
    }

    /// 取消（用户点了"取消"）。返回是否真的取消了一条。
    pub fn cancel(&self, token: &str) -> bool {
        let mut v = self.inner.lock().unwrap();
        match v.pending.remove(token) {
            Some(_) => {
                v.consumed.insert(token.to_string());
                true
            }
            None => false,
        }
    }

    /// 某人的待确认动作清单。
    pub fn pending_for(&self, actor_id: &str) -> Vec<PendingAction> {
        self.inner
            .lock()
            .unwrap()
            .pending
            .values()
            .filter(|a| a.actor_id == actor_id)
            .cloned()
            .collect()
    }

    /// 清掉过期的。返回清掉条数（也给运维端点用）。
    pub fn purge_expired(&self, now: DateTime<Utc>) -> usize {
        let mut v = self.inner.lock().unwrap();
        let before = v.pending.len();
        v.pending.retain(|_, a| a.expires_at > now);
        before - v.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Preview;

    fn actor(id: &str, role: ActorRole) -> Actor {
        Actor::new(id, role)
    }

    fn vault() -> ConfirmVault {
        ConfirmVault::new(600)
    }

    fn issue(v: &ConfirmVault, actor: &Actor, args: Value) -> PendingAction {
        v.issue(
            "scan_library",
            args,
            Risk::Write,
            Preview::new("测试动作"),
            actor,
            Utc::now(),
        )
    }

    #[test]
    fn happy_path_takes_once() {
        let v = vault();
        let a = actor("u1", ActorRole::Adult);
        let args = serde_json::json!({"library_id": "x"});
        let p = issue(&v, &a, args.clone());
        let taken = v.take(&p.token, &a, &args, Utc::now()).unwrap();
        assert_eq!(taken.tool, "scan_library");
    }

    #[test]
    fn replay_is_rejected() {
        let v = vault();
        let a = actor("u1", ActorRole::Adult);
        let args = serde_json::json!({});
        let p = issue(&v, &a, args.clone());
        let _ = v.take(&p.token, &a, &args, Utc::now()).unwrap();
        let e = v.take(&p.token, &a, &args, Utc::now()).unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenReplayed);
    }

    #[test]
    fn take_as_issued_uses_the_previewed_args() {
        // 确认接口只带 token：用户不该、也无力重新拼一遍参数。
        let v = vault();
        let a = actor("u1", ActorRole::Adult);
        let p = issue(&v, &a, serde_json::json!({"library_id": "x"}));
        let taken = v.take_as_issued(&p.token, &a, Utc::now()).unwrap();
        assert_eq!(taken.args["library_id"], "x");
        // 取过之后同样是作废的
        assert_eq!(
            v.take_as_issued(&p.token, &a, Utc::now()).unwrap_err().kind,
            AiErrorKind::TokenReplayed
        );
    }

    #[test]
    fn another_actor_cannot_execute() {
        let v = vault();
        let a = actor("u1", ActorRole::Adult);
        let b = actor("u2", ActorRole::Adult);
        let args = serde_json::json!({});
        let p = issue(&v, &a, args.clone());
        let e = v.take(&p.token, &b, &args, Utc::now()).unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenMismatch);
    }

    #[test]
    fn changed_args_invalidate_the_token() {
        // "预览删 3 部 → 改参数 → 执行删 300 部"必须被挡住。
        let v = vault();
        let a = actor("u1", ActorRole::Adult);
        let p = issue(&v, &a, serde_json::json!({"limit": 3}));
        let e = v
            .take(&p.token, &a, &serde_json::json!({"limit": 300}), Utc::now())
            .unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenMismatch);
        // 参数被改过之后，原参数也不能再用了 —— 令牌已作废。
        let e2 = v
            .take(&p.token, &a, &serde_json::json!({"limit": 3}), Utc::now())
            .unwrap_err();
        assert_eq!(e2.kind, AiErrorKind::TokenReplayed);
    }

    #[test]
    fn expired_token_is_rejected() {
        let v = vault();
        let a = actor("u1", ActorRole::Adult);
        let args = serde_json::json!({});
        let p = issue(&v, &a, args.clone());
        let later = Utc::now() + Duration::seconds(601);
        let e = v.take(&p.token, &a, &args, later).unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenExpired);
    }

    #[test]
    fn cancel_makes_token_unusable() {
        let v = vault();
        let a = actor("u1", ActorRole::Adult);
        let args = serde_json::json!({});
        let p = issue(&v, &a, args.clone());
        assert!(v.cancel(&p.token));
        let e = v.take(&p.token, &a, &args, Utc::now()).unwrap_err();
        assert_eq!(e.kind, AiErrorKind::TokenReplayed);
        assert!(v.pending_for("u1").is_empty());
    }

    #[test]
    fn child_and_guest_cannot_request_writes() {
        // 连预览都不给，避免"看得见却永远确认不了"。
        assert!(!ActorRole::Child.may_request(Risk::Write));
        assert!(!ActorRole::Guest.may_request(Risk::Destructive));
        assert!(ActorRole::Child.may_request(Risk::ReadOnly));
        assert!(ActorRole::Owner.may_request(Risk::Destructive));
    }

    #[test]
    fn fingerprint_is_stable_and_sensitive() {
        let a = args_fingerprint(&serde_json::json!({"x": 1, "y": 2}));
        let b = args_fingerprint(&serde_json::json!({"y": 2, "x": 1}));
        // 键顺序不同应视为同一份参数（serde_json 默认按 key 排序）
        assert_eq!(a, b);
        assert_ne!(a, args_fingerprint(&serde_json::json!({"x": 1, "y": 3})));
    }

    #[test]
    fn anonymous_is_not_the_most_privileged_role() {
        assert_eq!(Actor::anonymous().role, ActorRole::Adult);
    }

    #[test]
    fn risk_tiers_map_to_roles() {
        assert_eq!(risk_requires(Risk::ReadOnly), ActorRole::Guest);
        assert_eq!(risk_requires(Risk::Write), ActorRole::Adult);
        assert_eq!(risk_requires(Risk::Destructive), ActorRole::Administrator);
    }

    #[test]
    fn may_request_delegates_to_the_single_tier_table() {
        // 门槛只有一处定义：改了 risk_requires，may_request 必须跟着变。
        assert!(ActorRole::Guest.may_request(Risk::ReadOnly));
        assert!(!ActorRole::Guest.may_request(Risk::Write));
        assert!(!ActorRole::Child.may_request(Risk::Write));
        assert!(ActorRole::Adult.may_request(Risk::Write));
        assert!(!ActorRole::Adult.may_request(Risk::Destructive));
        assert!(ActorRole::Administrator.may_request(Risk::Destructive));
        assert!(ActorRole::Owner.may_request(Risk::Destructive));
    }
}
