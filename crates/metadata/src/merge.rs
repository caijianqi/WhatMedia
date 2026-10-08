//! 字段级合并与锁定（111.md §32）。
//!
//! 优先级链：**NFO > 手动锁定 > Provider**
//!
//! 这条链最容易写错的地方是"NFO 和 Provider 谁覆盖谁"。
//! 单看"字段有没有值"是不够的 —— NFO 给了标题之后，TMDB 也给了标题，
//! 后者必须输。所以除了 `locked`（用户改过）之外还需要 `filled`（更高优先级来源已填）。
//!
//! 两条规则合起来才是完整的语义：
//!   · `locked`  —— 用户手动改过，任何自动来源都不能碰
//!   · `filled`  —— 高优先级来源已经填过，低优先级来源不能碰

use std::collections::HashSet;

use mosaic_core::MetadataRecord;
use serde::{Deserialize, Serialize};

use crate::matcher::is_lockable;

/// `media_item` 上可被元数据覆盖的字段集合。
/// 刻意不包含 `id` / `library_id` / `kind` / `added_at` —— 那些不是元数据。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemMeta {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub tagline: Option<String>,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    pub runtime_ms: Option<i64>,
}

/// 从 `locked_fields` 列解析出的锁定集合。
///
/// 只接受白名单内的字段名：否则一个 `["id"]` 就能让记录永久无法更新。
pub fn parse_locked(json: &str) -> HashSet<String> {
    let parsed: Vec<String> = serde_json::from_str(json).unwrap_or_default();
    parsed.into_iter().filter(|f| is_lockable(f)).collect()
}

pub fn serialize_locked(locked: &HashSet<String>) -> String {
    let mut v: Vec<&String> = locked.iter().filter(|f| is_lockable(f)).collect();
    v.sort();
    serde_json::to_string(&v).unwrap_or_else(|_| "[]".into())
}

/// 合并上下文。一次刮削里按优先级顺序复用同一个实例。
#[derive(Debug, Clone, Default)]
pub struct MergeContext {
    /// 用户手动改过，任何来源都不能覆盖。
    pub locked: HashSet<String>,
    /// 高优先级来源已经填过，低优先级来源不能覆盖。
    pub filled: HashSet<String>,
}

impl MergeContext {
    pub fn new(locked: HashSet<String>) -> Self {
        Self {
            locked,
            filled: HashSet::new(),
        }
    }
}

/// 合并结果。
#[derive(Debug, Clone, Default)]
pub struct MergeReport {
    /// 实际被改动的字段名。写审计日志和返回给 UI 都用它。
    pub changed: Vec<String>,
    /// 因为被锁定而跳过的字段名。UI 要用它告诉用户"为什么没变"。
    pub skipped: Vec<String>,
}

/// 把一条 Provider / NFO 记录合并进 `ItemMeta`。
///
/// `priority` 为 `true` 时（NFO 这类高优先级来源），填过的字段会进入 `filled`，
/// 后续低优先级来源就覆盖不了了。
pub fn merge_record(
    current: &mut ItemMeta,
    incoming: &MetadataRecord,
    ctx: &mut MergeContext,
    high_priority: bool,
) -> MergeReport {
    let mut report = MergeReport::default();

    macro_rules! apply {
        ($field:ident, $value:expr) => {
            let name = stringify!($field);
            if ctx.locked.contains(name) {
                if !report.skipped.contains(&name.to_string()) {
                    report.skipped.push(name.to_string());
                }
            } else if ctx.filled.contains(name) {
                // 高优先级已填，静默跳过
            } else if let Some(v) = $value {
                let changed = match &current.$field {
                    Some(old) => old != &v,
                    None => true,
                };
                if changed {
                    report.changed.push(name.to_string());
                }
                current.$field = Some(v);
                if high_priority {
                    ctx.filled.insert(name.to_string());
                }
            }
        };
    }

    // 空标题不写入：Provider 偶尔会返回空串，写成空标题会让海报墙变一片空白。
    apply!(title, non_empty(Some(incoming.title.clone())));
    apply!(original_title, non_empty(incoming.original_title.clone()));
    apply!(sort_title, non_empty(incoming.sort_title.clone()));
    apply!(year, incoming.year);
    apply!(tagline, non_empty(incoming.tagline.clone()));
    apply!(overview, non_empty(incoming.overview.clone()));
    apply!(rating, incoming.rating);
    apply!(runtime_ms, incoming.runtime_ms);

    report
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// 锁定（或解锁）一个字段。
///
/// 返回 false 表示字段名不在白名单里 —— 调用方应该回 400 而不是静默忽略。
pub fn set_locked(locked: &mut HashSet<String>, field: &str, on: bool) -> bool {
    if !is_lockable(field) {
        return false;
    }
    if on {
        locked.insert(field.to_string());
    } else {
        locked.remove(field);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec() -> MetadataRecord {
        MetadataRecord {
            provider: "tmdb".into(),
            provider_id: "603".into(),
            title: "The Matrix".into(),
            original_title: Some("The Matrix".into()),
            sort_title: Some("Matrix, The".into()),
            year: Some(1999),
            tagline: Some("Welcome to the Real World".into()),
            overview: Some("A computer hacker".into()),
            rating: Some(8.2),
            runtime_ms: Some(8_160_000),
            ..Default::default()
        }
    }

    #[test]
    fn provider_fills_empty_item() {
        let mut item = ItemMeta::default();
        let mut ctx = MergeContext::default();
        let r = merge_record(&mut item, &rec(), &mut ctx, false);
        assert_eq!(item.title.as_deref(), Some("The Matrix"));
        assert_eq!(item.year, Some(1999));
        assert!(r.changed.contains(&"title".to_string()));
        assert!(r.skipped.is_empty());
    }

    /// DoD 直接对应：元数据写入后不得覆盖用户手动修改的字段。
    #[test]
    fn locked_field_survives_rescrape() {
        let mut item = ItemMeta::default();
        let mut ctx = MergeContext::default();
        merge_record(&mut item, &rec(), &mut ctx, false);

        // 用户把标题改成了自己喜欢的译名
        item.title = Some("黑客帝国".into());
        let mut locked = HashSet::new();
        locked.insert("title".to_string());
        ctx.locked = locked;

        let mut incoming = rec();
        incoming.title = "The Matrix Reloaded".into();
        incoming.year = Some(2003);

        let r = merge_record(&mut item, &incoming, &mut ctx, false);

        assert_eq!(item.title.as_deref(), Some("黑客帝国"), "锁定字段被覆盖了");
        assert_eq!(item.year, Some(2003), "未锁定字段应当更新");
        assert!(r.skipped.contains(&"title".to_string()));
        assert!(r.changed.contains(&"year".to_string()));
    }

    /// §32 优先级链：NFO 填过的字段，Provider 不能覆盖。
    #[test]
    fn nfo_beats_provider() {
        let mut item = ItemMeta::default();
        let mut ctx = MergeContext::default();

        let mut nfo = rec();
        nfo.provider = "nfo".into();
        nfo.title = "黑客帝国".into();
        nfo.rating = None; // NFO 没写评分

        merge_record(&mut item, &nfo, &mut ctx, true);
        assert_eq!(item.title.as_deref(), Some("黑客帝国"));

        merge_record(&mut item, &rec(), &mut ctx, false);
        assert_eq!(
            item.title.as_deref(),
            Some("黑客帝国"),
            "NFO 标题被 Provider 覆盖了"
        );
        assert_eq!(item.rating, Some(8.2), "NFO 没给的字段应当由 Provider 补上");
    }

    /// 手动锁定优先级最高：即便 NFO 是高优先级来源也动不了。
    #[test]
    fn locked_beats_nfo() {
        let mut item = ItemMeta {
            title: Some("我改的标题".into()),
            ..Default::default()
        };
        let mut locked = HashSet::new();
        locked.insert("title".to_string());
        let mut ctx = MergeContext::new(locked);

        let mut nfo = rec();
        nfo.title = "NFO 标题".into();
        let r = merge_record(&mut item, &nfo, &mut ctx, true);
        assert_eq!(item.title.as_deref(), Some("我改的标题"));
        assert!(r.skipped.contains(&"title".to_string()));
    }

    /// Provider 返回空串不能把已有的好数据清掉。
    #[test]
    fn empty_strings_do_not_wipe() {
        let mut item = ItemMeta::default();
        let mut ctx = MergeContext::default();
        merge_record(&mut item, &rec(), &mut ctx, false);

        let mut empty = rec();
        empty.title = "   ".into();
        empty.overview = Some(String::new());
        merge_record(&mut item, &empty, &mut ctx, false);

        assert_eq!(item.title.as_deref(), Some("The Matrix"));
        assert!(item.overview.is_some());
    }

    #[test]
    fn lock_roundtrip_and_whitelist() {
        let json = r#"["title","id","overview"]"#;
        let locked = parse_locked(json);
        assert!(locked.contains("title"));
        assert!(locked.contains("overview"));
        assert!(!locked.contains("id"), "非白名单字段必须被过滤掉");

        let back = serialize_locked(&locked);
        assert_eq!(parse_locked(&back), locked);

        let mut l = HashSet::new();
        assert!(set_locked(&mut l, "title", true));
        assert!(!set_locked(&mut l, "provider_id", true));
        assert!(set_locked(&mut l, "title", false));
        assert!(l.is_empty());
    }

    /// 重复刮削同一条数据时，第二次不应产生任何 changed。
    #[test]
    fn rescrape_is_noop() {
        let mut item = ItemMeta::default();
        let mut ctx = MergeContext::default();
        merge_record(&mut item, &rec(), &mut ctx, false);
        let r = merge_record(&mut item, &rec(), &mut ctx, false);
        assert!(r.changed.is_empty(), "重复刮削不应产生变更：{r:?}");
    }
}
