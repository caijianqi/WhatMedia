//! 候选打分与自动确认判定。
//!
//! 刮削最容易出的事故是"刮错了还自信"——把 `1917 (2019)` 刮成 1917 年的另一部片。
//! 所以这里的核心不是"能不能匹配上"，而是**什么时候该认怂交给人工确认**。

use whatmedia_core::{MetadataQuery, MetadataRecord};

/// 可锁定的字段白名单。
///
/// 只有在这个列表里的字段才能被锁定。否则 UI 传一个 `locked_fields: ["id"]`
/// 就能让整条记录再也无法更新。
pub const LOCKABLE_FIELDS: &[&str] = &[
    "title",
    "original_title",
    "sort_title",
    "year",
    "tagline",
    "overview",
    "rating",
    "runtime_ms",
];

pub fn is_lockable(field: &str) -> bool {
    LOCKABLE_FIELDS.contains(&field)
}

/// 归一化标题：小写、去掉标点与多余空白。
///
/// 不处理变音符号 —— 中文用户的主力片源里 `é` 之类的出现频率极低，
/// 而引入 unidecode 之类的依赖不值当。真出问题再加。
pub fn normalize_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            last_space = false;
        } else if !last_space {
            out.push(' ');
            last_space = true;
        }
    }
    out.trim().to_string()
}

/// 去掉英文冠词后的排序标题：`The Matrix` → `matrix`。
/// 用来判断"是不是同一部片"，而不是用来显示。
fn strip_article(s: &str) -> &str {
    for a in ["the ", "a ", "an "] {
        if let Some(rest) = s.strip_prefix(a) {
            return rest;
        }
    }
    s
}

/// 归一化编辑距离相似度，0.0 ~ 1.0。
///
/// 用 Levenshtein 而不是 token 交集：`Frieren` 和 `Frieren: Beyond Journey's End`
/// 是同一部片，token 交集能搞定；但 `Cowboy Bebop` 和 `Cowboy Behop`（刮削常见错拼）
/// 只有编辑距离能兜住。
pub fn similarity(a: &str, b: &str) -> f64 {
    let na = normalize_title(a);
    let nb = normalize_title(b);
    let a = strip_article(&na);
    let b = strip_article(&nb);
    if a == b {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let (n, m) = (av.len(), bv.len());

    // 长度差太大直接判低分，省掉一次 O(n*m)
    if (n as i32 - m as i32).abs() > (n.max(m) / 2) as i32 {
        return 0.0;
    }

    let mut prev: Vec<usize> = (0..=m).collect();
    let mut cur = vec![0usize; m + 1];
    for i in 1..=n {
        cur[0] = i;
        for j in 1..=m {
            let cost = if av[i - 1] == bv[j - 1] { 0 } else { 1 };
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }

    let dist = prev[m];
    1.0 - (dist as f64 / n.max(m) as f64)
}

/// 年份得分。
///
/// 年份是最强的判别信号，也是最容易骗人的信号：
/// 电影的"发行年"和"上映年"差一年很常见，但差两年基本就是另一部片了。
fn year_score(want: Option<i32>, got: Option<i32>) -> f64 {
    match (want, got) {
        (Some(w), Some(g)) => match (w - g).abs() {
            0 => 1.0,
            1 => 0.85,
            2 => 0.5,
            _ => 0.0,
        },
        // 一边没有年份：不奖不罚。很多用户目录里根本没有年份，
        // 因为缺数据而惩罚候选会导致大量误判。
        _ => 0.6,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// 直接采用。
    Auto,
    /// 需要人工确认。
    Review,
    /// 基本不可能是对的，UI 不该展示给用户。
    Reject,
}

#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    pub record: MetadataRecord,
    /// 0.0 ~ 1.0
    pub score: f64,
    pub title_score: f64,
    pub year_score: f64,
    pub confidence: Confidence,
}

/// 打分阈值。默认值是拍脑袋定的，但**必须可配置** ——
/// 1,000 部样本跑完之后要用真实命中率回来校准。
#[derive(Debug, Clone)]
pub struct MatchThresholds {
    /// 高于此分且领先第二名足够多 → 自动采用
    pub auto_accept: f64,
    /// 低于此分 → 直接丢弃
    pub reject: f64,
    /// 第一名与第二名的分差要求
    pub margin: f64,
}

impl Default for MatchThresholds {
    fn default() -> Self {
        Self {
            auto_accept: 0.86,
            reject: 0.45,
            margin: 0.06,
        }
    }
}

fn genre_boost(query_kind: Option<whatmedia_core::MediaKind>, genres: &[String]) -> f64 {
    let Some(k) = query_kind else { return 0.0 };
    match k {
        whatmedia_core::MediaKind::Anime => {
            let is_anime = genres.iter().any(|g| {
                g.contains("动画")
                    || g.to_ascii_lowercase().contains("animation")
                    || g.contains("动漫")
            });
            if is_anime {
                0.15
            } else {
                -0.05
            }
        }
        whatmedia_core::MediaKind::Documentary => {
            let is_doc = genres
                .iter()
                .any(|g| g.contains("纪录") || g.to_ascii_lowercase().contains("documentary"));
            if is_doc {
                0.15
            } else {
                -0.05
            }
        }
        whatmedia_core::MediaKind::Variety => {
            let is_variety = genres
                .iter()
                .any(|g| g.contains("综艺") || g.to_ascii_lowercase().contains("variety"));
            if is_variety {
                0.15
            } else {
                -0.05
            }
        }
        _ => 0.0,
    }
}

/// 清洗片名用于外部元数据接口（TMDB / Bangumi）搜索。
/// 剥离采集源或文件名中常见的年份后缀、版本修饰词、括号标签等。
pub fn clean_search_title(raw: &str) -> (String, Option<i32>) {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return (s, None);
    }

    // 1. 去除常见的中括号/小括号包裹的内容，如 [4K] [国语] (重制版) 【国语中字】
    static BRACKET_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let bracket_re = BRACKET_RE.get_or_init(|| {
        regex::Regex::new(r"\[[^\]]*\]|\([^\)]*\)|【[^】]*】|（[^）]*）").unwrap()
    });
    s = bracket_re.replace_all(&s, " ").to_string();

    // 2. 剥离副标题分隔符之后的部分，如 "经验派护士～数字化到底是什么啊！？～" -> "经验派护士"
    if let Some(idx) = s.find('～') {
        let prefix = s[..idx].trim();
        if prefix.chars().count() >= 2 {
            s = prefix.to_string();
        }
    }

    // 3. 提取并剥离 4 位数字年份（1950 ~ 2035）
    let mut year_hint = None;
    static YEAR_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let year_re = YEAR_RE.get_or_init(|| {
        regex::Regex::new(r"(19\d\d|20[0-3]\d)").unwrap()
    });
    if let Some(m) = year_re.find(&s) {
        if let Ok(y) = m.as_str().parse::<i32>() {
            year_hint = Some(y);
            let start = m.start();
            let end = m.end();
            s.replace_range(start..end, " ");
        }
    }

    // 4. 剥离中文/英文剧集常见修饰词（后缀）
    static MOD_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let mod_re = MOD_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)(重制版|重置版|年番|国语版|粤语版|国语|粤语|普通话|未删减版|未删减|抢先版|抢先看|特别篇|特辑|真人版|剧场版|修仙篇|篇|第[0-9一二三四五六七八九十]+季|season\s*\d+|s\d+)").unwrap()
    });
    s = mod_re.replace_all(&s, " ").to_string();

    // 5. 清理连续空格和首尾符号
    let mut cleaned = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if c.is_alphanumeric() || (c >= '\u{4e00}' && c <= '\u{9fa5}') {
            cleaned.push(c);
            last_space = false;
        } else if !last_space && (c.is_whitespace() || c == ':' || c == '：') {
            cleaned.push(' ');
            last_space = true;
        }
    }
    let res = cleaned.trim().to_string();
    if res.is_empty() {
        (raw.trim().to_string(), year_hint)
    } else {
        (res, year_hint)
    }
}

/// 给候选列表打分并排序。
///
/// 返回按分数降序排列的候选，每个都带 `confidence`。
pub fn score_candidates(
    query: &MetadataQuery,
    candidates: Vec<MetadataRecord>,
    thresholds: &MatchThresholds,
) -> Vec<ScoredCandidate> {
    let (clean_q_title, clean_year) = clean_search_title(&query.title);
    let q_year = query.year.or(clean_year);

    let mut scored: Vec<ScoredCandidate> = candidates
        .into_iter()
        .map(|r| {
            // 标题：原名和译名都试一遍，且结合原始标题与净化后标题取最高者。
            // 例如原名为 "凡人修仙传2020"，净化为 "凡人修仙传"，与 TMDB "凡人修仙传" 即可完美匹配。
            let by_title = similarity(&query.title, &r.title)
                .max(similarity(&clean_q_title, &r.title));
            let by_original = r
                .original_title
                .as_deref()
                .map(|o| similarity(&query.title, o).max(similarity(&clean_q_title, o)))
                .unwrap_or(0.0);
            let title_score = by_title.max(by_original);

            let ys = year_score(q_year, r.year);
            let gb = genre_boost(query.kind, &r.genres);

            // 标题占 7 成、年份占 3 成，加上类型偏好修正。
            let score = (title_score * 0.7 + ys * 0.3 + gb).clamp(0.0, 1.0);

            ScoredCandidate {
                record: r,
                score,
                title_score,
                year_score: ys,
                confidence: Confidence::Reject,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 先取不可变借用算完，再取可变借用写入 —— 否则 borrow checker 不让过。
    let rank = scored.first().map(|c| c.score).unwrap_or(0.0);
    let runner_up = scored.get(1).map(|c| c.score).unwrap_or(0.0);
    let decided = if rank < thresholds.reject {
        Confidence::Reject
    } else if rank >= thresholds.auto_accept && (rank - runner_up) >= thresholds.margin {
        Confidence::Auto
    } else if scored.len() == 1 && scored[0].title_score >= 0.85 {
        // 单一高置信候选：即便缺年份或没有第二名拉开分差，也自动采纳，避免界面变成无元数据
        Confidence::Auto
    } else if scored.first().map(|c| c.title_score >= 0.95 && (c.title_score - runner_up) >= thresholds.margin).unwrap_or(false) {
        Confidence::Auto
    } else {
        Confidence::Review
    };
    if let Some(best) = scored.first_mut() {
        best.confidence = decided;
    }

    scored
}

/// 只保留 `Auto` 及以上的，且返回最优的那个。
pub fn pick_auto(candidates: &[ScoredCandidate]) -> Option<&ScoredCandidate> {
    candidates
        .first()
        .filter(|c| c.confidence == Confidence::Auto)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(title: &str, year: Option<i32>) -> MetadataRecord {
        MetadataRecord {
            provider: "test".into(),
            provider_id: "1".into(),
            title: title.into(),
            year,
            ..Default::default()
        }
    }

    #[test]
    fn normalize_strips_punctuation() {
        assert_eq!(normalize_title("The.Matrix.1999"), "the matrix 1999");
        assert_eq!(
            normalize_title("Spider-Man:  No Way Home"),
            "spider man no way home"
        );
        assert_eq!(
            normalize_title("[SubsPlease] Frieren - 07"),
            "subsplease frieren 07"
        );
    }

    #[test]
    fn similarity_basics() {
        assert_eq!(similarity("The Matrix", "The Matrix"), 1.0);
        assert_eq!(similarity("the matrix", "The.Matrix"), 1.0);
        assert!(similarity("Cowboy Bebop", "Cowboy Behop") > 0.8);
        assert!(similarity("The Matrix", "Mad Max") < 0.6);
    }

    /// 这条是对 需求稿 §32 的直接验证：年份差一年仍应匹配，差十年必须拒绝。
    #[test]
    fn year_mismatch_is_penalized() {
        let q = MetadataQuery {
            title: "Blade Runner 2049".into(),
            year: Some(2017),
            ..Default::default()
        };
        let cands = vec![
            rec("Blade Runner 2049", Some(2017)),
            rec("Blade Runner", Some(1982)),
        ];
        let scored = score_candidates(&q, cands, &MatchThresholds::default());
        assert_eq!(scored[0].record.title, "Blade Runner 2049");
        assert_eq!(scored[0].confidence, Confidence::Auto, "{:?}", scored[0]);
    }

    /// `1917 (2019)` 这个经典陷阱：文件名里的 1917 是片名，不是年份。
    /// 打分器不能因为"标题里含 1917"就把它认成 1917 年的片子。
    #[test]
    fn nineteen_seventeen_trap() {
        let q = MetadataQuery {
            title: "1917".into(),
            year: Some(2019),
            ..Default::default()
        };
        let cands = vec![rec("1917", Some(2019)), rec("1917: The War", Some(1917))];
        let scored = score_candidates(&q, cands, &MatchThresholds::default());
        assert_eq!(scored[0].record.year, Some(2019));
        assert_eq!(scored[0].confidence, Confidence::Auto);
    }

    /// 两个分数接近的候选必须降级为人工确认，不能自动选一个。
    #[test]
    fn close_candidates_require_review() {
        let q = MetadataQuery {
            title: "The Office".into(),
            year: Some(2005),
            ..Default::default()
        };
        let cands = vec![
            MetadataRecord {
                original_title: Some("The Office".into()),
                ..rec("The Office", Some(2005))
            },
            MetadataRecord {
                original_title: Some("The Office".into()),
                ..rec("The Office (US)", Some(2005))
            },
        ];
        let scored = score_candidates(&q, cands, &MatchThresholds::default());
        assert_eq!(
            scored[0].confidence,
            Confidence::Review,
            "分差过小应要求人工确认：{:?}",
            scored[0]
        );
    }

    /// 完全不相关的候选要被丢弃，而不是排在第一位让用户去勾。
    #[test]
    fn unrelated_is_rejected() {
        let q = MetadataQuery {
            title: "Frieren".into(),
            year: Some(2023),
            ..Default::default()
        };
        let cands = vec![rec("Fast & Furious", Some(2009))];
        let scored = score_candidates(&q, cands, &MatchThresholds::default());
        assert_eq!(scored[0].confidence, Confidence::Reject);
        assert!(pick_auto(&scored).is_none());
    }

    #[test]
    fn lockable_whitelist_is_enforced() {
        assert!(is_lockable("title"));
        assert!(is_lockable("overview"));
        assert!(!is_lockable("id"));
        assert!(!is_lockable("provider_id"));
    }

    #[test]
    fn anime_genre_disambiguation_boosts_anime_candidate() {
        let q = MetadataQuery {
            title: "凡人修仙传".into(),
            year: None,
            language: None,
            episode_title: None,
            kind: Some(whatmedia_core::MediaKind::Anime),
        };
        let cands = vec![
            MetadataRecord {
                title: "凡人修仙传".into(),
                year: Some(2025),
                genres: vec!["剧情".into(), "古装".into()],
                ..Default::default()
            },
            MetadataRecord {
                title: "凡人修仙传".into(),
                year: Some(2020),
                genres: vec!["动画".into(), "奇幻".into()],
                ..Default::default()
            },
        ];
        let scored = score_candidates(&q, cands, &MatchThresholds::default());
        assert_eq!(scored[0].record.year, Some(2020));
        assert_eq!(scored[0].confidence, Confidence::Auto);
    }

    #[test]
    fn test_clean_search_title() {
        let (t1, y1) = clean_search_title("凡人修仙传2020");
        assert_eq!(t1, "凡人修仙传");
        assert_eq!(y1, Some(2020));

        let (t2, y2) = clean_search_title("凡人修仙传重制版");
        assert_eq!(t2, "凡人修仙传");
        assert_eq!(y2, None);

        let (t3, _) = clean_search_title("斗破苍穹 年番");
        assert_eq!(t3, "斗破苍穹");

        let (t4, _) = clean_search_title("数码宝贝 国语版");
        assert_eq!(t4, "数码宝贝");

        let (t5, _) = clean_search_title("经验派护士～数字化到底是什么啊！？～");
        assert_eq!(t5, "经验派护士");
    }
}

