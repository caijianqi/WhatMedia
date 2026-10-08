//! 确定性 id。
//!
//! **为什么不用随机 UUID**：扫描和刮削会反复跑同一批文件。
//! id 随机的后果是每次扫描都造出一批新作品，用户手动改的标题、
//! 续播点、海报选择全部丢失。
//!
//! 所以 id 全部用 UUIDv5 从"自然键"派生：同样的输入永远得到同样的 id，
//! 重复操作天然收敛到同一行。

use uuid::Uuid;

use crate::model::Id;

/// 命名空间。固定常量，改了就等于把所有 id 换一遍 —— 千万别改。
const NS: Uuid = Uuid::from_bytes([
    0x6b, 0xa7, 0xb8, 0x11, 0x9d, 0xad, 0x11, 0xd1, 0x80, 0xb4, 0x00, 0xc0, 0x4f, 0xd4, 0x30, 0xc8,
]);

/// 从自然键派生 id。
///
/// `kind` 参与散列，所以 `("item", ["Matrix"])` 和 `("series", ["Matrix"])`
/// 不会撞车 —— 同名电影和同名剧集应该共存。
pub fn deterministic_id(kind: &str, parts: &[&str]) -> Id {
    let mut key = String::with_capacity(64);
    key.push_str(kind);
    for p in parts {
        key.push('\x1f');
        key.push_str(p);
    }
    Uuid::new_v5(&NS, key.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_stable() {
        let a = deterministic_id("item", &["lib1", "The Matrix", "1999"]);
        let b = deterministic_id("item", &["lib1", "The Matrix", "1999"]);
        assert_eq!(a, b, "同样的输入必须得到同样的 id");
    }

    /// kind 参与散列：同名电影和同名剧集必须能共存。
    #[test]
    fn kind_is_part_of_the_key() {
        assert_ne!(
            deterministic_id("item", &["The Matrix"]),
            deterministic_id("series", &["The Matrix"])
        );
    }

    /// 分段边界要参与散列，否则 `["ab","c"]` 和 `["a","bc"]` 会撞车。
    #[test]
    fn separator_prevents_collision() {
        assert_ne!(
            deterministic_id("x", &["ab", "c"]),
            deterministic_id("x", &["a", "bc"])
        );
    }

    /// 版本号是 5 —— 日志里能一眼看出这是确定性 id 而不是随机 id。
    #[test]
    fn is_uuid_v5() {
        let id = deterministic_id("item", &["x"]);
        assert_eq!(id.get_version_num(), 5);
    }
}
