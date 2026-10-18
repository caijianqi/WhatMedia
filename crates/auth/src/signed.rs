//! 播放资源的签名 URL。
//!
//! **为什么有了 Cookie 还需要这个东西。** `<video>` / `<audio>` 标签和 hls.js 拉分片时，**不会**带上 SPA 的
//! `Authorization` 头 —— 那是 XHR/fetch 的专利。Cookie 能解决浏览器，
//! 解决不了 ExoPlayer、AVPlayer 和电视上的 WebView（各家的 Cookie 策略不一）。
//!
//! 所以流端点必须支持" URL 自带凭据"，代价是：
//!   · URL 会落在日志、Referer、分享剪贴板里 → 必须**有时效**
//!   · URL 一旦给出无法收回 → 必须**绑定到具体资源**，不能一把万能钥匙
//!   · 谁拿走都能用 → 服务端密钥要能轮换
//!
//! 构造：`sig = HMAC-SHA256(key, "wm1:{resource}:{exp_unix}")`
//!   · `wm1` 是域分离前缀，防止同一个密钥在别处被重放
//!   · `resource` 是**去掉 query 的路径**，签名不被参数顺序影响
//!   · `exp_unix` 是绝对过期时间，服务端自己不记时间在途。

use crate::AuthError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};

/// 签名载体的版本前缀。除非要彻底更换消息格式，否则不要动它 ——
/// 一动，所有在途的签名 URL 同时失效。
const DOMAIN_PREFIX: &str = "wm1";

/// HMAC-SHA256。
///
/// 自己实现（RFC 2104）而不是引 `hmac` crate：那要求 digest 0.11，
/// 与 workspace 锁定的 sha2 0.10 冲突，会同时编两棵 digest 树。
/// 正确性由 RFC 4231 的官方测试向量钉住 —— 不是靠"看起来对"。
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }

    let mut ipad = padded;
    for b in &mut ipad {
        *b ^= 0x36;
    }
    let mut opad = padded;
    for b in &mut opad {
        *b ^= 0x5c;
    }

    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);

    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// 签发密钥。每个进程启动时随机一个，也可以从配置里读固定值做多实例共享。
#[derive(Clone, Debug)]
pub struct SigningKey([u8; 32]);

impl SigningKey {
    /// 随机生成。单实例部署用它 —— 重启后旧签名失效，正好把在途的都清掉。
    pub fn generate() -> Self {
        let mut buf = [0u8; 32];
        getrandom::fill(&mut buf).expect("系统熵源不可用");
        Self(buf)
    }

    /// 从既有字节构造（多实例 / 容器重启后保持同一批签名有效）。
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// 从 base64 / 十六进制配置值构造。返回 `None` 表示**长度不对**，
    /// 故意不做"截断补零"这种宽容处理 —— 配置写错就该在启动日志里报错。
    pub fn from_base64(s: &str) -> Option<Self> {
        let raw = URL_SAFE_NO_PAD.decode(s.trim()).ok()?;
        raw.try_into().ok().map(Self)
    }

    /// 导出为 base64url，用于写回配置。
    pub fn to_base64(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0)
    }

    fn message(resource: &str, exp_unix: i64) -> String {
        format!("{DOMAIN_PREFIX}:{resource}:{exp_unix}")
    }

    /// 对 `(resource, exp)` 签名，返回 base64url 无 padding 的结果。
    pub fn sign(&self, resource: &str, exp_unix: i64) -> String {
        let mac = hmac_sha256(&self.0, Self::message(resource, exp_unix).as_bytes());
        URL_SAFE_NO_PAD.encode(mac)
    }

    /// 校验签名与时效。`Ok(())` 表示"资源、签名、时间三者都对得上"。
    pub fn verify(
        &self,
        resource: &str,
        exp_unix: i64,
        sig: &str,
        now_unix: i64,
    ) -> Result<(), AuthError> {
        let expected = self.sign(resource, exp_unix);
        if !crate::token::constant_time_eq(&expected, sig.trim()) {
            return Err(AuthError::BadSignature);
        }
        if exp_unix < now_unix {
            return Err(AuthError::SignatureExpired { exp_unix, now_unix });
        }
        Ok(())
    }

    /// 给资源路径拼上查询串。已经带 `?` 的路径会接着用 `&`。
    pub fn sign_query(&self, resource: &str, exp_unix: i64) -> String {
        let sig = self.sign(resource, exp_unix);
        let sep = if resource.contains('?') { '&' } else { '?' };
        format!("{resource}{sep}exp={exp_unix}&sig={sig}")
    }
}

/// HLS 播放列表里的分片是**相对路径**，播放器不会带着父级的 query 去拉。
///
/// 这导致一个很容易踩的坑：给 master.m3u8 签好了名，可它里面写的
/// `index.m3u8`、`seg-1.ts` 仍然是裸的相对路径。解决办法是让一张签名
/// 覆盖**整个 job 目录**：用 `HMAC(SigningKey, "hls:" + job_id)` 派生一把子钥匙，
/// 于是 `/hls/{job}/master.m3u8` 与 `/hls/{job}/seg-1.ts` 签的是同一把钥匙，
/// 而别的 job 签出来的互不可用。
pub fn derive_stream_key(root: &SigningKey, kind: &str, scope: &str) -> SigningKey {
    let mac = hmac_sha256(&root.0, format!("derive:{kind}:{scope}").as_bytes());
    SigningKey::from_bytes(mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 4231 Test Case 1。
    #[test]
    fn rfc4231_case_1() {
        let key = [0x0bu8; 20];
        let mac = hmac_sha256(&key, b"Hi There");
        assert_eq!(
            mac.as_slice(),
            &hex::decode("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7")
                .unwrap()
        );
    }

    /// RFC 4231 Test Case 2 —— "what do ya want for nothing?"。
    #[test]
    fn rfc4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            mac.as_slice(),
            &hex::decode("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
                .unwrap()
        );
    }

    /// RFC 4231 Test Case 4 —— 秘密长于 block（需要"先 hash 再 pad"那条分支）。
    #[test]
    fn rfc4231_case_4_long_key() {
        // key = 0x01..=0x19（25 字节），data = 0xcd 重复 50 次。
        let key = (0x01u8..=0x19).collect::<Vec<u8>>();
        let mac = hmac_sha256(&key, &[0xcd_u8; 50]);
        assert_eq!(
            mac.as_slice(),
            &hex::decode("82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b")
                .unwrap()
        );
    }

    /// 超长 key（>64 字节）走 hash-then-pad 分支，这条守护它。
    #[test]
    fn long_key_takes_hash_branch() {
        let key = [0x0bu8; 200];
        let mac = hmac_sha256(&key, b"long key input");
        // 只要是稳定的、且与"先 sha256 再 64 字节 pad"的构造一致即可。
        let mut padded = [0u8; 64];
        padded[..32].copy_from_slice(&Sha256::digest(key));
        let mut ipad = padded;
        ipad.iter_mut().for_each(|b| *b ^= 0x36);
        let mut opad = padded;
        opad.iter_mut().for_each(|b| *b ^= 0x5c);
        let mut inner = Sha256::new();
        inner.update(ipad);
        inner.update(b"long key input");
        let inner = inner.finalize();
        let mut outer = Sha256::new();
        outer.update(opad);
        outer.update(inner);
        // 右边是 sha2 的 GenericArray，它的 `as_slice` 在 0.10 里已标记废弃，
        // 走切片索引拿 `&[u8]`，与左边类型对齐。
        let expected = outer.finalize();
        assert_eq!(mac.as_slice(), &expected[..]);
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let key = SigningKey::generate();
        let sig = key.sign("/api/v1/stream/abc", 1_900_000_000);
        key.verify("/api/v1/stream/abc", 1_900_000_000, &sig, 1_899_999_000)
            .unwrap();
    }

    #[test]
    fn wrong_resource_rejected() {
        let key = SigningKey::generate();
        let sig = key.sign("/api/v1/stream/abc", 1_900_000_000);
        // 换了资源就必须失效 —— 否则一张签名能开任何一部片子。
        assert!(matches!(
            key.verify("/api/v1/stream/def", 1_900_000_000, &sig, 0),
            Err(AuthError::BadSignature)
        ));
    }

    #[test]
    fn expired_rejected() {
        let key = SigningKey::generate();
        let sig = key.sign("/api/v1/stream/abc", 1000);
        assert!(matches!(
            key.verify("/api/v1/stream/abc", 1000, &sig, 2000),
            Err(AuthError::SignatureExpired { .. })
        ));
    }

    #[test]
    fn tampered_exp_rejected() {
        // 攻击者最常用的动作是把 exp 改到未来，而不是去解签名。
        let key = SigningKey::generate();
        let sig = key.sign("/api/v1/stream/abc", 1000);
        assert!(matches!(
            key.verify("/api/v1/stream/abc", 1_900_000_000, &sig, 0),
            Err(AuthError::BadSignature)
        ));
    }

    #[test]
    fn key_serialises_roundtrip() {
        let k = SigningKey::generate();
        let s = k.to_base64();
        assert_eq!(s.len(), 43);
        let back = SigningKey::from_base64(&s).unwrap();
        let sig = k.sign("/x", 5);
        back.verify("/x", 5, &sig, 0).unwrap();
        assert!(SigningKey::from_base64("too-short").is_none());
    }

    #[test]
    fn derived_key_scopes_a_whole_hls_directory() {
        let root = SigningKey::generate();
        let playlist = derive_stream_key(&root, "hls", "job-42");
        let seg = derive_stream_key(&root, "hls", "job-42");
        let other = derive_stream_key(&root, "hls", "job-43");
        let sig_master = playlist.sign("/api/v1/hls/job-42/master.m3u8", 9_999_999_999);
        let sig_seg = seg.sign("/api/v1/hls/job-42/seg-1.ts", 9_999_999_999);
        playlist
            .verify(
                "/api/v1/hls/job-42/master.m3u8",
                9_999_999_999,
                &sig_master,
                0,
            )
            .unwrap();
        seg.verify("/api/v1/hls/job-42/seg-1.ts", 9_999_999_999, &sig_seg, 0)
            .unwrap();
        // 跨 job 不可互用 —— 这是 derive 存在的全部意义。
        assert!(matches!(
            other.verify("/api/v1/hls/job-42/seg-1.ts", 9_999_999_999, &sig_seg, 0),
            Err(AuthError::BadSignature)
        ));
    }

    #[test]
    fn query_separator_handles_existing_query() {
        let key = SigningKey::generate();
        let a = key.sign_query("/api/v1/stream/x", 10);
        assert!(a.starts_with("/api/v1/stream/x?exp=10&sig="), "{a}");
        let b = key.sign_query("/api/v1/stream/x?kind=hls", 10);
        assert!(
            b.starts_with("/api/v1/stream/x?kind=hls&exp=10&sig="),
            "{b}"
        );
    }
}
