//! 会话票据的生成、摘要与恒定时间比较。
//!
//! 一句话概括这条链路：**明文只出现一次**。
//!
//! 登录响应把明文 token 交给客户端一次，服务端当场算出 SHA-256 摘要存库，
//! 明文即刻丢弃。之后每次请求验证时，是把"来的 token 摘要"与"库里的摘要"
//! 比较 —— 数据库被拖走也换不回登录态。
//!
//! 用 SHA-256 而不是再上一层 argon2 是有意的：token 本身已经是 256 位随机数，
//! 它在密钥空间里是均匀的，慢散列只能让每次 API 调用变慢，换不来任何安全性。

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

/// 令牌随机字节数。256 位 —— 与 SHA-256 输出同宽，摘要不成为瓶颈。
pub const TOKEN_BYTES: usize = 32;

/// 生成一枚新票据。
///
/// 返回 `(明文, 摘要)`。明文塞进登录响应体 / Set-Cookie，**不要落库**；
/// 摘要才是写进 `session.token_hash` 的东西。
pub fn new_token() -> (String, String) {
    let mut buf = [0u8; TOKEN_BYTES];
    getrandom::fill(&mut buf).expect("系统熵源不可用");
    let plain = URL_SAFE_NO_PAD.encode(buf);
    let digest = hash_token(&plain);
    (plain, digest)
}

/// 票据摘要。与 [`new_token`] 的第二个返回值同构。
pub fn hash_token(plain: &str) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(plain.as_bytes()) {
        use std::fmt::Write as _;
        // 小写十六进制定宽；用 format! 每字节一次分配，这里的调用频次不值得。
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// 恒定时间字符串比较。
///
/// 先比长度（长度差不是秘密），再做全 XOR 累积 —— 全程没有分支，
/// 编译器没法把它优化成"发现不同就提前返回"。
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.as_bytes().iter().zip(b.as_bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 从 `Authorization` 头里取出 Bearer 值。
///
/// 匹配**大小写不敏感**，因为 RFC 7235 的 scheme 是不区分大小写的，
/// 而某些 HTTP 库（以及曾经某个版本的 Tauri 插件）会原样透传用户写的 `bearer`。
pub fn bearer_value(header: &str) -> Option<&str> {
    let rest = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))?;
    let v = rest.trim();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 从 `Cookie` 头里取指定键。
///
/// 手写解析器而不是引 `cookie` crate：这里只需要一个键，
/// 而且必须容忍"空格不一致、`;` 后没空格、值里有 `=`"这些现实写法。
pub fn cookie_value<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    for part in header.split(';') {
        let part = part.trim();
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        if k.trim() == key {
            return Some(v.trim());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_url_safe_and_unique() {
        let (a, ha) = new_token();
        let (b, hb) = new_token();
        assert_ne!(a, b, "两次票据必须不同");
        assert_ne!(ha, hb);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "明文含 URL 不安全字符：{a}"
        );
        // 32 字节 → base64 无 padding 是 43 个字符。
        assert_eq!(a.len(), 43, "长度不像 256 位：{a}");
    }

    #[test]
    fn digest_is_stable_64_hex() {
        // SHA-256("abc") 的公开测试向量，防止哪天有人把它换成截断版本。
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(hash_token("abc").len(), 64);
    }

    #[test]
    fn hash_matches_new_token_output() {
        let (plain, digest) = new_token();
        assert_eq!(hash_token(&plain), digest);
    }

    #[test]
    fn constant_time_eq_semantics() {
        assert!(constant_time_eq("abcdef", "abcdef"));
        assert!(!constant_time_eq("abcdef", "abcdeg"));
        assert!(!constant_time_eq("abc", "abcdef"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn bearer_parsing() {
        assert_eq!(bearer_value("Bearer abc.def"), Some("abc.def"));
        assert_eq!(bearer_value("bearer abc.def"), Some("abc.def"));
        assert_eq!(bearer_value("Bearer  spaced "), Some("spaced"));
        assert_eq!(bearer_value("Bearer "), None);
        assert_eq!(bearer_value("Basic xyz"), None);
    }

    #[test]
    fn cookie_parsing() {
        let h = "wm_session=tok123; theme=dark;id=9=x";
        assert_eq!(cookie_value(h, "wm_session"), Some("tok123"));
        assert_eq!(cookie_value(h, "theme"), Some("dark"));
        assert_eq!(cookie_value(h, "id"), Some("9=x"));
        assert_eq!(cookie_value(h, "nope"), None);
    }
}
