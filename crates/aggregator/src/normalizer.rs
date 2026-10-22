//! 影视标题清洗、相似度比对与多源聚类引擎。

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::model::{AggregatedVodItem, RawVodItem, VodRoute};
use crate::parser::parse_routes;

/// 标题清洗器
pub struct TitleNormalizer {
    junk_regex: regex::Regex,
    bracket_regex: regex::Regex,
}

impl Default for TitleNormalizer {
    fn default() -> Self {
        Self {
            // 匹配各种无意义的画质、音轨、广告修饰标签
            junk_regex: regex::Regex::new(
                r"(?i)\b(4k|1080p|720p|2160p|uhd|fhd|hd|bd|tc|ts|dvd|web-dl|h264|h265|x264|x265|aac)\b|超清|高清|蓝光|抢先版|枪版|完整版|未删减版|未删减|国语版|国语|粤语|中字|双字|中英双字|原盘|自制",
            )
            .unwrap(),
            // 匹配各类括号内容：【...】、[...]、(...)
            bracket_regex: regex::Regex::new(r"\[.*?\]|【.*?】|\(.*?\)|（.*?）").unwrap(),
        }
    }
}

impl TitleNormalizer {
    /// 清洗标题得到用于聚类的标准词
    pub fn clean(&self, title: &str) -> String {
        let without_brackets = self.bracket_regex.replace_all(title, " ");
        let without_junk = self.junk_regex.replace_all(&without_brackets, " ");

        // 统一小写，替换多余空白
        let mut clean = String::with_capacity(without_junk.len());
        let mut prev_space = true;
        for c in without_junk.chars() {
            if c.is_whitespace() || c == '_' || c == '-' || c == '.' || c == '·' {
                if !prev_space {
                    clean.push(' ');
                    prev_space = true;
                }
            } else {
                for lc in c.to_lowercase() {
                    clean.push(lc);
                }
                prev_space = false;
            }
        }
        clean.trim().to_string()
    }
}

/// 计算两个字符串的 Levenshtein 相似度 (0.0 ~ 1.0)
pub fn string_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();

    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 || b_len == 0 {
        return 0.0;
    }

    let mut dp = vec![vec![0usize; b_len + 1]; a_len + 1];

    for (i, row) in dp.iter_mut().enumerate().take(a_len + 1) {
        row[0] = i;
    }
    for (j, val) in dp[0].iter_mut().enumerate().take(b_len + 1) {
        *val = j;
    }

    for i in 1..=a_len {
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            dp[i][j] = (dp[i - 1][j] + 1)
                .min(dp[i][j - 1] + 1)
                .min(dp[i - 1][j - 1] + cost);
        }
    }

    let dist = dp[a_len][b_len];
    let max_len = a_len.max(b_len);
    1.0 - (dist as f64 / max_len as f64)
}

/// 聚类分组结果结构体
pub struct ClusterInput {
    pub source_id: String,
    pub source_name: String,
    pub item: RawVodItem,
}

/// 将来自不同采集源的原始数据进行去重与多线路聚类
pub fn cluster_items(inputs: Vec<ClusterInput>) -> Vec<AggregatedVodItem> {
    let normalizer = TitleNormalizer::default();
    let mut clustered: Vec<AggregatedVodItem> = Vec::new();
    // 快速查找映射：(clean_title, year) -> clustered index
    let mut index_map: HashMap<(String, Option<u32>), usize> = HashMap::new();

    for input in inputs {
        let clean_title = normalizer.clean(&input.item.vod_name);
        if clean_title.is_empty() {
            continue;
        }

        let parsed_routes = parse_routes(&input.source_id, &input.source_name, &input.item);
        if parsed_routes.is_empty() {
            continue;
        }

        let key = (clean_title.clone(), input.item.vod_year);

        // 先尝试精确匹配 (clean_title, year)
        if let Some(&idx) = index_map.get(&key) {
            merge_into_cluster(&mut clustered[idx], parsed_routes, input.item);
            continue;
        }

        // 尝试模糊相似度比对（年份相同，或年份未知但标题极高相似）
        let mut matched_idx = None;
        for (i, target) in clustered.iter().enumerate() {
            let year_match = match (target.year, input.item.vod_year) {
                (Some(y1), Some(y2)) => y1 == y2,
                _ => true,
            };

            if year_match {
                let sim = string_similarity(&target.clean_title, &clean_title);
                if sim >= 0.88 {
                    matched_idx = Some(i);
                    break;
                }
            }
        }

        if let Some(idx) = matched_idx {
            merge_into_cluster(&mut clustered[idx], parsed_routes, input.item);
        } else {
            // 新聚类条目
            let id = generate_cluster_id(&clean_title, input.item.vod_year);
            let item = AggregatedVodItem {
                id,
                title: input.item.vod_name,
                clean_title: clean_title.clone(),
                sub_title: input.item.vod_sub,
                year: input.item.vod_year,
                poster: input.item.vod_pic,
                type_name: input.item.type_name,
                remark: input.item.vod_remarks,
                actor: input.item.vod_actor,
                director: input.item.vod_director,
                content: input.item.vod_content,
                routes: parsed_routes,
            };
            let new_idx = clustered.len();
            index_map.insert(key, new_idx);
            clustered.push(item);
        }
    }

    // 对每个聚合条目的线路按选集数量降序排列
    for item in &mut clustered {
        item.routes.sort_by_key(|a| std::cmp::Reverse(a.episodes.len()));
    }

    clustered
}

fn merge_into_cluster(target: &mut AggregatedVodItem, new_routes: Vec<VodRoute>, raw: RawVodItem) {
    // 补全缺失的基础元数据字段
    if target.poster.is_none() || target.poster.as_deref() == Some("") {
        target.poster = raw.vod_pic;
    }
    if target.remark.is_none() || target.remark.as_deref() == Some("") {
        target.remark = raw.vod_remarks;
    }
    if target.content.is_none() || target.content.as_deref() == Some("") {
        target.content = raw.vod_content;
    }
    if target.actor.is_none() || target.actor.as_deref() == Some("") {
        target.actor = raw.vod_actor;
    }
    if target.director.is_none() || target.director.as_deref() == Some("") {
        target.director = raw.vod_director;
    }

    // 合并线路 (按 source_id + play_from 避免完全重复线路)
    for route in new_routes {
        let exists = target
            .routes
            .iter()
            .any(|r| r.source_id == route.source_id && r.play_from == route.play_from);
        if !exists {
            target.routes.push(route);
        }
    }
}

fn generate_cluster_id(clean_title: &str, year: Option<u32>) -> String {
    let mut hasher = DefaultHasher::new();
    clean_title.hash(&mut hasher);
    year.hash(&mut hasher);
    format!("agg_{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_title_cleaning() {
        let normalizer = TitleNormalizer::default();
        assert_eq!(
            normalizer.clean("繁花 [4K超清国语中字] (2023未删减版)"),
            "繁花"
        );
        assert_eq!(
            normalizer.clean("【蓝光1080P】流浪地球2"),
            "流浪地球2"
        );
        assert_eq!(
            normalizer.clean("Oppenheimer.2023.UHD.Remux"),
            "oppenheimer 2023 remux"
        );
    }

    #[test]
    fn test_similarity() {
        assert_eq!(string_similarity("繁花", "繁花"), 1.0);
        assert!(string_similarity("流浪地球", "流浪地球2") > 0.7);
        assert!(string_similarity("阿凡达", "泰坦尼克号") < 0.2);
    }

    #[test]
    fn test_cluster_items() {
        let inputs = vec![
            ClusterInput {
                source_id: "src_lz".into(),
                source_name: "量子资源".into(),
                item: RawVodItem {
                    vod_id: "1".into(),
                    vod_name: "繁花 [4K版]".into(),
                    vod_sub: None,
                    vod_year: Some(2023),
                    vod_pic: Some("https://lz.com/p.jpg".into()),
                    vod_remarks: Some("30集全".into()),
                    type_name: Some("电视剧".into()),
                    vod_actor: None,
                    vod_director: None,
                    vod_content: None,
                    vod_play_from: Some("lzm3u8".into()),
                    vod_play_url: Some("第01集$https://lz.com/1.m3u8".into()),
                },
            },
            ClusterInput {
                source_id: "src_ff".into(),
                source_name: "非凡资源".into(),
                item: RawVodItem {
                    vod_id: "2".into(),
                    vod_name: "繁花【国语】".into(),
                    vod_sub: None,
                    vod_year: Some(2023),
                    vod_pic: None,
                    vod_remarks: None,
                    type_name: None,
                    vod_actor: None,
                    vod_director: None,
                    vod_content: Some("大时代下的上海".into()),
                    vod_play_from: Some("ffm3u8".into()),
                    vod_play_url: Some(
                        "第01集$https://ff.com/1.m3u8#第02集$https://ff.com/2.m3u8".into(),
                    ),
                },
            },
        ];

        let clusters = cluster_items(inputs);
        assert_eq!(clusters.len(), 1);

        let item = &clusters[0];
        assert_eq!(item.clean_title, "繁花");
        assert_eq!(item.year, Some(2023));
        assert_eq!(item.poster, Some("https://lz.com/p.jpg".into()));
        assert_eq!(item.content, Some("大时代下的上海".into()));
        // 应当聚合成两条线路
        assert_eq!(item.routes.len(), 2);
        // 按集数降序，非凡资源有 2 集，排在前面
        assert_eq!(item.routes[0].source_id, "src_ff");
        assert_eq!(item.routes[0].episodes.len(), 2);
        assert_eq!(item.routes[1].source_id, "src_lz");
        assert_eq!(item.routes[1].episodes.len(), 1);
    }
}
