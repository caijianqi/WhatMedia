//! 口令与 PIN 摘要。**argon2id**，不是 bcrypt、不是裸 SHA-256。
//!
//! 三个刻意的决定：
//!
//! 1. **输出 PHC 字符串**（`$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`）。
//!    参数写在串里，将来机器变快、想提高 `m` 值时，**老摘要照样能验** ——
//!    验证时读串里的参数，新存的用新参数。如果自己拼 `salt:hash`，
//!    升级算法那一天就只能强制所有人重置密码。
//! 2. **散列之前先把长度卡在 256 字节。** argon2 允许输入到 2^32−1 字节，
//!    不设上限意味着"用一个 1MB 的口令登录"就能让单核跑满好几秒 ——
//!    这是自家服务，但"自家"不等于可以不做 DoS 防护。
//! 3. **用户不存在时也执行一次假散列。** 登录接口的响应时间如果不一致，
//!    "账号不存在"和"密码错误"就成了可枚举的旁路。见 [`dummy_verify`]。

use argon2::password_hash::{phc::PasswordHash, PasswordHasher, PasswordVerifier};
use argon2::Argon2;

use crate::AuthError;

/// 输入上限，见模块文档第 2 点。
const MAX_INPUT_BYTES: usize = 256;

/// 口令最短长度。
///
/// **6 而不是 8**：这是唯一一处为了让"家庭场景里一个好记的口令"放宽的策略。
/// 8 是通行建议值，但它的代价是家里人在电视上要输一长串 ——
/// 而真正挡住爆破的不是长度下限，是 `lockout_threshold`（连败锁定）。
/// 想收紧回来只改这一个常量即可，校验与提示文案都从它派生。
pub const MIN_PASSWORD_LEN: usize = 6;

/// PIN 长度区间。PIN 是**四到十二位纯数字**，用于家长审批，不用于登录。
pub const PIN_LEN_RANGE: std::ops::RangeInclusive<usize> = 4..=12;

/// 对口令做策略校验。
///
/// 刻意只查长度和"不含控制字符"，不强制大小写数字符号：
/// 2026 年的口令策略共识是**长度优先**，组合规则只会让用户把 `1` 换成 `!`。
pub fn validate_password(plain: &str) -> Result<(), AuthError> {
    let n = plain.chars().count();
    if n < MIN_PASSWORD_LEN {
        return Err(AuthError::WeakPassword {
            min: MIN_PASSWORD_LEN,
            got: n,
        });
    }
    if plain.len() > MAX_INPUT_BYTES {
        return Err(AuthError::InputTooLong {
            max: MAX_INPUT_BYTES,
        });
    }
    if plain.chars().any(|c| c.is_control()) {
        return Err(AuthError::WeakPassword {
            min: MIN_PASSWORD_LEN,
            got: n,
        });
    }
    Ok(())
}

/// PIN 策略：纯数字，4–12 位。
pub fn validate_pin(pin: &str) -> Result<(), AuthError> {
    let n = pin.chars().count();
    if !PIN_LEN_RANGE.contains(&n) || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(AuthError::WeakPin);
    }
    Ok(())
}

fn salt_and_hash(plain: &str) -> Result<String, AuthError> {
    // 盐由 `hash_password` 内部生成（RECOMMENDED_SALT_LEN = 16 字节，取自 OS 熵源）。
    // 自己攒盐没任何好处，只会多一个"盐不够随机"的出错面。
    let hash = Argon2::default()
        .hash_password(plain.as_bytes())
        .map_err(|e| AuthError::Hash(e.to_string()))?;
    Ok(hash.to_string())
}

/// 生成口令摘要（PHC 串）。
pub fn hash_password(plain: &str) -> Result<String, AuthError> {
    validate_password(plain)?;
    salt_and_hash(plain)
}

/// 生成家长 PIN 摘要。PIN 与登录口令用同一套参数 ——
/// 它没有"更长更复杂"的必要，暴破成本由下面同一份爆破计数保护区承担。
pub fn hash_pin(pin: &str) -> Result<String, AuthError> {
    validate_pin(pin)?;
    salt_and_hash(pin)
}

/// 校验口令。
///
/// 返回 `Ok(false)` 表示"口令不对"，`Err` 表示"摘要本身坏了"
/// （被手工改过、用了不支持的算法）。这两件事在调用方有不同的含义，
/// 不能合并成一个 bool。
pub fn verify_password(plain: &str, phc: &str) -> Result<bool, AuthError> {
    if plain.len() > MAX_INPUT_BYTES {
        return Ok(false);
    }
    let parsed = PasswordHash::new(phc).map_err(|e| AuthError::BadHash(e.to_string()))?;
    Ok(Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok())
}

/// 校验 PIN。语义与 [`verify_password`] 一致。
pub fn verify_pin(pin: &str, phc: &str) -> Result<bool, AuthError> {
    verify_password(pin, phc)
}

/// 一次注定失败的散列，用于"用户不存在"时消耗等量的时间。
///
/// 不能图省事 `sleep(random)`：sleep 的抖动与 argon2 的抖动不在同一个量级上，
/// 统计上依然分得开。真跑一遍才是对的。
pub fn dummy_verify() {
    if let Ok(h) = salt_and_hash("dummy-password-for-timing-equalisation") {
        let _ = verify_password("dummy-password-for-timing-equalisation", &h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_password() {
        let h = hash_password("correct horse battery staple").unwrap();
        assert!(h.starts_with("$argon2id$"), "不是 argon2id：{h}");
        assert!(verify_password("correct horse battery staple", &h).unwrap());
        assert!(!verify_password("wrong password here!!", &h).unwrap());
    }

    #[test]
    fn same_password_yields_different_hashes() {
        // 盐不同 → 摘要必须不同，否则彩虹表友好。
        let a = hash_password("aaaaaaaaaa").unwrap();
        let b = hash_password("aaaaaaaaaa").unwrap();
        assert_ne!(a, b);
        assert!(verify_password("aaaaaaaaaa", &a).unwrap());
        assert!(verify_password("aaaaaaaaaa", &b).unwrap());
    }

    #[test]
    fn params_are_embedded_in_phc_string() {
        // m=19456,t=2,p=1 是 RFC 9106 的低内存档。将来换参数时这条测试要跟着改，
        // 它存在的意义是让"参数变了"这件事变得显式。
        let h = hash_password("bbbbbbbbbb").unwrap();
        assert!(h.contains("m=19456"), "{h}");
        assert!(h.contains("t=2"), "{h}");
        assert!(h.contains("p=1"), "{h}");
    }

    #[test]
    fn short_password_rejected() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("").is_err());
        assert!(validate_password("1234567890").is_ok());
    }

    #[test]
    fn overloaded_input_rejected_by_policy() {
        let huge = "x".repeat(MAX_INPUT_BYTES + 1);
        assert!(matches!(
            hash_password(&huge),
            Err(AuthError::InputTooLong { .. })
        ));
        // 但**校验**超长输入不算策略违规：老用户也许存了个长口令，
        // 此时放宽 Ms，但校验时只是让它失败、而不是炸 500。
        let h = hash_password("cccccccccc").unwrap();
        assert!(!verify_password(&huge, &h).unwrap());
    }

    #[test]
    fn control_characters_rejected() {
        assert!(validate_password("password\u{0}here").is_err());
    }

    #[test]
    fn pin_policy() {
        assert!(validate_pin("1234").is_ok());
        assert!(validate_pin("123456789012").is_ok());
        assert!(validate_pin("123").is_err());
        assert!(validate_pin("1234567890123").is_err());
        assert!(validate_pin("12a4").is_err());
        let h = hash_pin("8042").unwrap();
        assert!(verify_pin("8042", &h).unwrap());
        assert!(!verify_pin("8043", &h).unwrap());
    }

    #[test]
    fn corrupt_hash_is_an_error_not_a_false() {
        assert!(matches!(
            verify_password("whatever", "not-a-phc-string"),
            Err(AuthError::BadHash(_))
        ));
        assert!(matches!(
            verify_password("whatever", "$argon2id$v=19$m=19456,t=2,p=1$!!!$!!!"),
            Err(AuthError::BadHash(_))
        ));
    }

    #[test]
    fn dummy_verify_runs_and_fails_silently() {
        // 只求它不 panic 且耗时与正常验证同量级。
        dummy_verify();
    }
}
