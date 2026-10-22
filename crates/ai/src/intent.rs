//! 自然语言 → 结构化意图（规则引擎）。
//!
//! 需求稿 §72 的原则："LLM 负责理解意图，数据库负责真正查询。"
//! 这里实现的是**模型缺席时的那一半**。
//!
//! 为什么值得写一份规则引擎，而不是"没有模型就报不可用"：
//!
//! 1. 家庭服务器常年离线，本地模型也可能没起。**降级到"能答一部分"
//!    比"整个 AI 不可用"有用得多。**
//! 2. 它是**离线可验证的基线**。DoD 里"§59 七条样例通过率 ≥ 80%"
//!    在没有 API Key 的机器上也能被真实测量，而不是一句"待真机"。
//! 3. 模型在时，它的结果用作**校验**：模型给出的 `MediaQuery`
//!    若与规则引擎解析出的硬约束冲突，以约束为准。
//!
//! 规则引擎**不猜**。没认出来就 `Unknown`，由运行时明确回答"这句我没理解" ——
//! 猜错了去查库，比答不上来更糟（用户会以为库里真的没有）。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use whatmedia_core::MediaKind;

use crate::host::{MediaQuery, MissingKind, RecommendSpec, SortKey, WatchState};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Intent {
    /// 查库。
    Query(MediaQuery),
    /// 推荐。
    Recommend(RecommendSpec),
    /// 直接对应一个工具调用（如"扫描一下媒体库"）。
    Action { tool: String, args: Value },
    /// 没理解。**绝不猜**。
    Unknown { reason: String },
}

impl Intent {
    /// 转成一次工具调用。规则引擎的结果最终也要走 Tool，
    /// 不允许在别处另开一条通路（ADR-0006 §1）。
    pub fn tool_call(&self) -> Option<(String, Value)> {
        match self {
            Intent::Query(q) => Some(("search_media".into(), query_to_args(q))),
            Intent::Recommend(s) => Some(("recommend_media".into(), recommend_to_args(s))),
            Intent::Action { tool, args } => Some((tool.clone(), args.clone())),
            Intent::Unknown { .. } => None,
        }
    }
}

fn query_to_args(q: &MediaQuery) -> Value {
    let mut m = serde_json::Map::new();
    if let Some(t) = &q.text {
        m.insert("text".into(), json!(t));
    }
    if let Some(k) = q.kind {
        m.insert("kind".into(), json!(kind_str(k)));
    }
    if !q.genres.is_empty() {
        m.insert("genres".into(), json!(q.genres));
    }
    if let Some(r) = &q.region {
        m.insert("region".into(), json!(r));
    }
    if let Some(v) = q.year_from {
        m.insert("year_from".into(), json!(v));
    }
    if let Some(v) = q.year_to {
        m.insert("year_to".into(), json!(v));
    }
    if !q.missing.is_empty() {
        let missing: Vec<&str> = q.missing.iter().map(|m| missing_str(*m)).collect();
        m.insert("missing".into(), json!(missing));
    }
    if let Some(d) = q.added_within_days {
        m.insert("added_within_days".into(), json!(d));
    }
    if let Some(v) = q.runtime_min_ms {
        m.insert("runtime_min_ms".into(), json!(v));
    }
    if let Some(v) = q.runtime_max_ms {
        m.insert("runtime_max_ms".into(), json!(v));
    }
    if q.duplicates_only {
        m.insert("duplicates_only".into(), json!(true));
    }
    if let Some(w) = q.watch {
        m.insert("watch".into(), json!(watch_str(w)));
    }
    if let Some(s) = q.sort {
        m.insert("sort".into(), json!(sort_str(s)));
    }
    m.insert("limit".into(), json!(q.limit.max(1)));
    Value::Object(m)
}

fn recommend_to_args(s: &RecommendSpec) -> Value {
    let mut m = serde_json::Map::new();
    if !s.genres.is_empty() {
        m.insert("genres".into(), json!(s.genres));
    }
    if let Some(k) = s.kind {
        m.insert("kind".into(), json!(kind_str(k)));
    }
    if let Some(v) = s.max_runtime_ms {
        m.insert("max_runtime_ms".into(), json!(v));
    }
    m.insert("exclude_watched".into(), json!(s.exclude_watched));
    m.insert("limit".into(), json!(s.limit.max(1)));
    Value::Object(m)
}

pub fn kind_str(k: MediaKind) -> &'static str {
    match k {
        MediaKind::Movie => "movie",
        MediaKind::Series => "series",
        MediaKind::Season => "season",
        MediaKind::Episode => "episode",
        MediaKind::Anime => "anime",
        MediaKind::Variety => "variety",
        MediaKind::Documentary => "documentary",
        MediaKind::Concert => "concert",
        MediaKind::Music => "music",
        MediaKind::MusicVideo => "music_video",
        MediaKind::ShortVideo => "mixed",
        MediaKind::HomeVideo => "mixed",
        MediaKind::Mixed => "mixed",
    }
}

fn missing_str(m: MissingKind) -> &'static str {
    match m {
        MissingKind::Poster => "poster",
        MissingKind::Subtitle => "subtitle",
        MissingKind::Metadata => "metadata",
    }
}

fn watch_str(w: WatchState) -> &'static str {
    match w {
        WatchState::Unwatched => "unwatched",
        WatchState::Partial => "partial",
        WatchState::Finished => "finished",
    }
}

fn sort_str(s: SortKey) -> &'static str {
    match s {
        SortKey::SizeDesc => "size_desc",
        SortKey::AddedDesc => "added_desc",
        SortKey::RatingDesc => "rating_desc",
        SortKey::TitleAsc => "title_asc",
    }
}

// ─────────────────────────────────────────────────────────────
// 词表
// ─────────────────────────────────────────────────────────────

const GENRES: &[(&str, &str)] = &[
    ("科幻", "Sci-Fi"),
    ("动作", "Action"),
    ("喜剧", "Comedy"),
    ("恐怖", "Horror"),
    ("惊悚", "Thriller"),
    ("悬疑", "Mystery"),
    ("爱情", "Romance"),
    ("剧情", "Drama"),
    ("犯罪", "Crime"),
    ("奇幻", "Fantasy"),
    ("冒险", "Adventure"),
    ("战争", "War"),
    ("历史", "History"),
    ("武侠", "Action"),
];

const KINDS: &[(&str, MediaKind)] = &[
    ("电视剧", MediaKind::Series),
    ("剧集", MediaKind::Series),
    ("动漫", MediaKind::Anime),
    ("动画", MediaKind::Anime),
    ("纪录片", MediaKind::Documentary),
    ("综艺", MediaKind::Variety),
    ("演唱会", MediaKind::Concert),
    ("电影", MediaKind::Movie),
    ("影片", MediaKind::Movie),
    ("片子", MediaKind::Movie),
];

const REGIONS: &[(&str, &str)] = &[
    ("国产", "CN"),
    ("中国", "CN"),
    ("华语", "CN"),
    ("中文", "CN"),
    ("日本", "JP"),
    ("日剧", "JP"),
    ("日影", "JP"),
    ("韩国", "KR"),
    ("韩剧", "KR"),
    ("欧美", "US"),
    ("美国", "US"),
];

/// 中文数字 → 值。够用就行：时长里出现十位以上的小时数没有意义。
fn chinese_digit(c: char) -> Option<u32> {
    match c {
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}

/// 从文本里找时长上限（毫秒）。
///
/// 认：`两小时以内` / `2小时内` / `90分钟以内` / `一个半小时` / `两小时`。
/// 认不出来返回 None —— **不猜**。
///
/// 单位顺序有讲究：先扫单个数字，再扫"十几/几十"，
/// 否则「十五分钟」会先被 '五' 匹配成 5 分钟。
pub fn parse_duration_ms(raw: &str) -> Option<i64> {
    let text: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let chars: Vec<char> = text.chars().collect();
    let mut ms: Option<i64> = None;

    // 阿拉伯数字 + 单位。
    // **必须跳过已消费的数字串**：否则「90分钟」里的 '0' 会被当成
    // 第二个独立数字，把 90 覆盖成 0 —— 这个 bug 很安静，只会让
    // "90分钟以内" 悄悄变成 "0 分钟以内"。
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let n: i64 = chars[i..j].iter().collect::<String>().parse().unwrap_or(0);
            let rest: String = chars[j..].iter().take(5).collect();
            if rest.starts_with("个半小时") {
                ms = Some(n * 3_600_000 + 1_800_000);
            } else if rest.starts_with("小时")
                || rest.starts_with("个小时")
                || rest.starts_with("h")
            {
                ms = Some(n * 3_600_000);
            } else if rest.starts_with("分钟") || rest.starts_with("分") || rest.starts_with("min")
            {
                ms = Some(n * 60_000);
            }
            i = j;
        } else {
            i += 1;
        }
    }

    // 中文数字 + 单位
    for i in 0..chars.len() {
        if let Some(d) = chinese_digit(chars[i]) {
            let n = d as i64;
            let rest: String = chars[i + 1..].iter().take(5).collect();
            if rest.starts_with("个半小时") {
                ms = Some(n * 3_600_000 + 1_800_000);
            } else if rest.starts_with("小时") || rest.starts_with("个小时") {
                ms = Some(n * 3_600_000);
            } else if rest.starts_with("分钟") || rest.starts_with("分") {
                ms = Some(n * 60_000);
            }
        }
    }

    // 十几 / 几十：十五分钟、二十小时。放在最后，让它能覆盖上面那个"五分钟"。
    for i in 0..chars.len() {
        if chars[i] == '十' {
            let next = chars.get(i + 1).copied().and_then(chinese_digit);
            let total = next.map(|d| 10 + d as i64).unwrap_or(10);
            let skip = if next.is_some() { i + 2 } else { i + 1 };
            let rest: String = chars[skip..].iter().take(3).collect();
            if rest.starts_with("小时") {
                ms = Some(total * 3_600_000);
            } else if rest.starts_with("分钟") || rest.starts_with("分") {
                ms = Some(total * 60_000);
            }
        }
    }

    if ms.is_none() && text.contains("半小时") {
        ms = Some(1_800_000);
    }
    ms
}

/// 从文本里找"最近 N 天"。
fn parse_added_days(raw: &str) -> Option<u32> {
    // 先去掉空白：`近 5 天` 与 `近5天` 必须等价。
    let text: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    if text.contains("昨天") {
        return Some(1);
    }
    if text.contains("前天") {
        return Some(2);
    }
    if text.contains("今天") || text.contains("今日") {
        return Some(1);
    }
    if text.contains("上周") || text.contains("上个星期") {
        return Some(7);
    }
    if text.contains("本周") || text.contains("这周") || text.contains("这个星期") {
        return Some(7);
    }
    if text.contains("本月") || text.contains("这个月") {
        return Some(30);
    }
    if text.contains("上个月") {
        return Some(60);
    }
    // “最近三天” / “近 5 天”
    let chars: Vec<char> = text.chars().collect();
    for (i, _) in chars.iter().enumerate() {
        let window: String = chars[i..].iter().take(6).collect();
        if window.starts_with("最近") || window.starts_with("近") {
            let tail: String = chars[i + 1..].iter().take(6).collect();
            for (j, c) in tail.chars().enumerate() {
                if let Some(d) = chinese_digit(c).or_else(|| c.to_digit(10)) {
                    let rest: String = tail.chars().skip(j + 1).take(3).collect();
                    if rest.starts_with("天") || rest.starts_with("日") {
                        return Some(d);
                    }
                    if rest.starts_with("周")
                        || rest.starts_with("个星期")
                        || rest.starts_with("星期")
                    {
                        return Some(d * 7);
                    }
                    if rest.starts_with("月") {
                        return Some(d * 30);
                    }
                }
            }
        }
    }
    None
}

fn first_match<'a, T: Copy>(text: &str, table: &'a [(&'a str, T)]) -> Option<T> {
    table
        .iter()
        .find(|(k, _)| text.contains(k))
        .map(|(_, v)| *v)
}

fn all_matches(text: &str, table: &[(&str, &str)]) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in table {
        if text.contains(k) && !out.iter().any(|x| x == v) {
            out.push((*v).to_string());
        }
    }
    out
}

/// 去掉引导动词与标点，剩下的当作片名关键词。
///
/// 只在**没有任何其他信号**时才用它 —— 否则"哪些视频最占空间"
/// 会把"视频"当成片名，查出来 0 条。
fn extract_text(raw: &str) -> Option<String> {
    let mut s = raw.to_string();
    for v in [
        "帮我",
        "给我",
        "请",
        "找一下",
        "找出",
        "找找",
        "查找",
        "查询",
        "搜索",
        "搜一下",
        "搜",
        "列出",
        "列出一下",
        "有哪些",
        "哪些",
        "哪个",
        "有什么",
        "我想看",
        "想看",
        "看一下",
        "找",
        "推荐",
        "来几部",
        "有没有",
    ] {
        s = s.replace(v, "");
    }
    for v in ["的", "了", "吗", "呢", "啊", "呀", "吧", "一下"] {
        s = s.replace(v, "");
    }
    let s: String = s
        .trim()
        .trim_matches(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '。' | '？' | '?' | '！' | '!' | '，' | ',' | '、' | '.' | '"' | '“' | '”'
                )
        })
        .to_string();
    if s.chars().count() < 2 {
        None
    } else {
        Some(s)
    }
}

pub struct RuleEngine;

impl RuleEngine {
    pub fn parse(raw: &str) -> Intent {
        let text = raw.to_lowercase();
        if text.trim().is_empty() {
            return Intent::Unknown {
                reason: "空问题".into(),
            };
        }

        // ── 直接对应动作 ──────────────────────────────────
        if text.contains("扫描") || text.contains("扫一下") || text.contains("重新扫描") {
            return Intent::Action {
                tool: "scan_library".into(),
                args: json!({}),
            };
        }

        if text.contains("全网")
            || text.contains("网上找")
            || text.contains("在线找")
            || text.contains("聚合搜")
            || text.contains("云端搜")
        {
            if let Some(t) = extract_text(raw) {
                let clean_t = t
                    .replace("全网", "")
                    .replace("在线", "")
                    .replace("聚合", "")
                    .replace("网上", "")
                    .replace("云端", "")
                    .trim()
                    .to_string();
                let final_q = if clean_t.is_empty() { t } else { clean_t };
                return Intent::Action {
                    tool: "search_aggregated_vod".into(),
                    args: json!({ "query": final_q }),
                };
            }
        }

        let kind = first_match(&text, KINDS);
        let genres = all_matches(&text, GENRES);
        let region = first_match(&text, REGIONS).map(|s: &str| s.to_string());
        let duplicates_only = text.contains("重复") || text.contains("一模一样");

        let mut missing = Vec::new();
        if text.contains("没有海报")
            || text.contains("缺海报")
            || text.contains("无海报")
            || text.contains("没海报")
            || text.contains("海报缺失")
        {
            missing.push(MissingKind::Poster);
        }
        if text.contains("没有字幕")
            || text.contains("缺字幕")
            || text.contains("无字幕")
            || text.contains("没字幕")
            || text.contains("字幕缺失")
        {
            missing.push(MissingKind::Subtitle);
        }
        if text.contains("没有元数据") || text.contains("缺元数据") {
            missing.push(MissingKind::Metadata);
        }

        let watch = if text.contains("没看完")
            || text.contains("看过一半")
            || text.contains("看到一半")
            || text.contains("未看完")
            || text.contains("看了一半")
        {
            Some(WatchState::Partial)
        } else if text.contains("看完") {
            Some(WatchState::Finished)
        } else if text.contains("没看过")
            || text.contains("未看")
            || text.contains("从没看")
            || text.contains("还没看")
        {
            Some(WatchState::Unwatched)
        } else {
            None
        };

        let sort = if text.contains("最占空间")
            || text.contains("占空间")
            || text.contains("最大的")
            || text.contains("体积最大")
        {
            Some(SortKey::SizeDesc)
        } else if text.contains("评分最高") || text.contains("最高分") {
            Some(SortKey::RatingDesc)
        } else if text.contains("最近添加")
            || text.contains("最新添加")
            || text.contains("最近加入")
        {
            Some(SortKey::AddedDesc)
        } else {
            None
        };

        let added_within_days = parse_added_days(&text);
        let runtime_max_ms = parse_duration_ms(&text);

        // ── 推荐 ─────────────────────────────────────────
        let wants_recommend = text.contains("推荐")
            || text.contains("看点什么")
            || text.contains("有什么好看")
            || text.contains("来几部");
        if wants_recommend {
            return Intent::Recommend(RecommendSpec {
                genres: genres.clone(),
                max_runtime_ms: runtime_max_ms,
                kind,
                exclude_watched: !text.contains("看过的") && !text.contains("再看一遍"),
                limit: 10,
            });
        }

        // ── 查库 ─────────────────────────────────────────
        let has_signal = kind.is_some()
            || !genres.is_empty()
            || !missing.is_empty()
            || watch.is_some()
            || sort.is_some()
            || added_within_days.is_some()
            || runtime_max_ms.is_some()
            || duplicates_only
            || region.is_some();

        if duplicates_only {
            // find_duplicate 比 search_media 更贴切：它会分组并算出占用。
            return Intent::Action {
                tool: "find_duplicate".into(),
                args: json!({ "limit": 20 }),
            };
        }
        if missing.len() == 1 && matches!(missing[0], MissingKind::Poster) && watch.is_none() {
            return Intent::Action {
                tool: "find_missing_poster".into(),
                args: json!({ "limit": 20 }),
            };
        }

        if has_signal {
            return Intent::Query(MediaQuery {
                text: None,
                kind,
                genres,
                region,
                year_from: None,
                year_to: None,
                missing,
                added_within_days,
                runtime_min_ms: None,
                runtime_max_ms,
                duplicates_only,
                watch,
                sort,
                limit: 20,
            });
        }

        // 没有任何结构化信号：剩下的可能是个片名。
        match extract_text(raw) {
            Some(t) => Intent::Query(MediaQuery {
                text: Some(t),
                limit: 20,
                ..Default::default()
            }),
            None => Intent::Unknown {
                reason: "没听懂这句在问什么".into(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(i: &Intent) -> &MediaQuery {
        match i {
            Intent::Query(q) => q,
            other => panic!("不是 Query：{other:?}"),
        }
    }

    // ── 需求稿 §59 的七条样例 ──────────────────────────────
    // DoD 要求通过率 ≥ 80%（7 条里至少 6 条）。离线实测。

    #[test]
    fn sample_1_missing_poster_movies() {
        let i = RuleEngine::parse("找出没有海报的电影。");
        assert_eq!(i.tool_call().unwrap().0, "find_missing_poster");
    }

    #[test]
    fn sample_2_series_added_yesterday() {
        let i = RuleEngine::parse("昨天添加了哪些电视剧？");
        let q = q(&i);
        assert_eq!(q.kind, Some(MediaKind::Series));
        assert_eq!(q.added_within_days, Some(1));
    }

    #[test]
    fn sample_3_unfinished_scifi_movies() {
        let i = RuleEngine::parse("找我没看完的科幻电影。");
        let q = q(&i);
        assert_eq!(q.kind, Some(MediaKind::Movie));
        assert_eq!(q.watch, Some(WatchState::Partial));
        assert!(q.genres.iter().any(|g| g == "Sci-Fi"));
    }

    #[test]
    fn sample_4_largest_by_size() {
        let i = RuleEngine::parse("哪些视频最占空间？");
        let q = q(&i);
        assert_eq!(q.sort, Some(SortKey::SizeDesc));
        // 不能把"视频"当成片名，否则查出来 0 条。
        assert!(q.text.is_none());
    }

    #[test]
    fn sample_5_duplicate_movies() {
        let i = RuleEngine::parse("找一下重复电影。");
        assert_eq!(i.tool_call().unwrap().0, "find_duplicate");
    }

    #[test]
    fn sample_6_series_without_subtitles() {
        let i = RuleEngine::parse("哪些电视剧没有字幕？");
        let q = q(&i);
        assert_eq!(q.kind, Some(MediaKind::Series));
        assert!(q.missing.contains(&MissingKind::Subtitle));
    }

    #[test]
    fn sample_7_recommend_under_two_hours() {
        let i = RuleEngine::parse("推荐两小时以内能看完的电影。");
        match &i {
            Intent::Recommend(s) => {
                assert_eq!(s.max_runtime_ms, Some(7_200_000));
                assert_eq!(s.kind, Some(MediaKind::Movie));
                assert!(s.exclude_watched);
            }
            other => panic!("应是 Recommend：{other:?}"),
        }
    }

    /// DoD 的量化口径：七条里至少六条能解析出结构化意图。
    #[test]
    fn dod_pass_rate_is_at_least_eighty_percent() {
        let samples = [
            "找出没有海报的电影。",
            "昨天添加了哪些电视剧？",
            "找我没看完的科幻电影。",
            "哪些视频最占空间？",
            "找一下重复电影。",
            "哪些电视剧没有字幕？",
            "推荐两小时以内能看完的电影。",
        ];
        let ok = samples
            .iter()
            .filter(|s| !matches!(RuleEngine::parse(s), Intent::Unknown { .. }))
            .count();
        assert!(
            ok * 100 >= samples.len() * 80,
            "通过率 {}/{} 低于 80%",
            ok,
            samples.len()
        );
    }

    // ── 细节 ──────────────────────────────────────────────

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("两小时以内"), Some(7_200_000));
        assert_eq!(parse_duration_ms("2小时内"), Some(7_200_000));
        assert_eq!(parse_duration_ms("90分钟以内"), Some(5_400_000));
        assert_eq!(parse_duration_ms("一个半小时"), Some(5_400_000));
        assert_eq!(parse_duration_ms("半小时"), Some(1_800_000));
        assert_eq!(parse_duration_ms("十五分钟"), Some(900_000));
        // 认不出来就不要硬凑一个数字出来。
        assert_eq!(parse_duration_ms("时长别太长"), None);
    }

    #[test]
    fn added_ranges() {
        assert_eq!(parse_added_days("昨天"), Some(1));
        assert_eq!(parse_added_days("上周"), Some(7));
        assert_eq!(parse_added_days("最近三天"), Some(3));
        assert_eq!(parse_added_days("近 5 天"), Some(5));
        assert_eq!(parse_added_days("最近两周"), Some(14));
        assert_eq!(parse_added_days("随便什么时候"), None);
    }

    #[test]
    fn scan_maps_to_action() {
        let i = RuleEngine::parse("扫描一下媒体库");
        assert_eq!(i.tool_call().unwrap().0, "scan_library");
    }

    #[test]
    fn bare_title_becomes_text_query() {
        let i = RuleEngine::parse("找一下星际穿越");
        let q = q(&i);
        assert_eq!(q.text.as_deref(), Some("星际穿越"));
    }

    #[test]
    fn unknown_is_explicit_and_never_guessed() {
        // 猜错了去查库比答不上来更糟：用户会以为库里真的没有。
        let i = RuleEngine::parse("嗯");
        match i {
            Intent::Unknown { .. } => {}
            other => panic!("应是 Unknown：{other:?}"),
        }
        assert!(RuleEngine::parse("嗯").tool_call().is_none());
    }

    #[test]
    fn empty_input_is_unknown() {
        assert!(matches!(RuleEngine::parse("   "), Intent::Unknown { .. }));
    }

    #[test]
    fn region_and_genre_are_extracted() {
        let i = RuleEngine::parse("找国产科幻片");
        let q = q(&i);
        assert_eq!(q.region.as_deref(), Some("CN"));
        assert!(q.genres.iter().any(|g| g == "Sci-Fi"));
    }

    #[test]
    fn tool_call_args_round_trip() {
        let (tool, args) = RuleEngine::parse("找我没看完的科幻电影。")
            .tool_call()
            .unwrap();
        assert_eq!(tool, "search_media");
        assert_eq!(args["kind"], "movie");
        assert_eq!(args["watch"], "partial");
        assert_eq!(args["genres"][0], "Sci-Fi");
    }

    #[test]
    fn full_network_search_maps_to_vod_action() {
        let (tool, args) = RuleEngine::parse("帮我全网搜索一下繁花")
            .tool_call()
            .unwrap();
        assert_eq!(tool, "search_aggregated_vod");
        assert_eq!(args["query"], "繁花");
    }
}
