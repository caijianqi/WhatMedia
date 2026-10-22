//! MacCMS V10 播放源与选集协议解析器。

use crate::model::{RawVodItem, VodEpisode, VodRoute};

/// 将 `RawVodItem` 的 `vod_play_from` 与 `vod_play_url` 解析为结构化线路 `Vec<VodRoute>`
pub fn parse_routes(source_id: &str, source_name: &str, raw: &RawVodItem) -> Vec<VodRoute> {
    let play_from_str = raw.vod_play_from.as_deref().unwrap_or("").trim();
    let play_url_str = raw.vod_play_url.as_deref().unwrap_or("").trim();

    if play_url_str.is_empty() {
        return Vec::new();
    }

    // 线路以 $$$ 切割
    let from_list: Vec<&str> = if play_from_str.is_empty() {
        Vec::new()
    } else {
        play_from_str.split("$$$").map(|s| s.trim()).collect()
    };

    let url_list: Vec<&str> = play_url_str.split("$$$").map(|s| s.trim()).collect();

    let mut routes = Vec::new();

    for (i, &route_content) in url_list.iter().enumerate() {
        if route_content.is_empty() {
            continue;
        }

        let play_from = from_list
            .get(i)
            .copied()
            .filter(|s| !s.is_empty())
            .unwrap_or("m3u8")
            .to_string();

        let route_display_name = format!("{source_name} [{play_from}]");
        let episodes = parse_episodes(route_content);

        if !episodes.is_empty() {
            routes.push(VodRoute {
                source_id: source_id.to_string(),
                source_name: route_display_name,
                play_from,
                episodes,
            });
        }
    }

    routes
}

/// 解析单条线路内的剧集列表 (分隔符支持 `#`、`\r\n`、`\n`)
pub fn parse_episodes(content: &str) -> Vec<VodEpisode> {
    let mut episodes = Vec::new();

    // 规范化换行符为 #
    let normalized = content.replace("\r\n", "#").replace('\n', "#");
    let parts = normalized.split('#');

    let mut index = 1;
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let (name, url) = if let Some((n, u)) = part.split_once('$') {
            (n.trim(), u.trim())
        } else if part.starts_with("http://") || part.starts_with("https://") {
            // 没有 $ 分隔符，纯 URL 形式
            ("", part)
        } else {
            // 无效切片
            continue;
        };

        if url.is_empty() || (!url.starts_with("http://") && !url.starts_with("https://")) {
            continue;
        }

        let final_name = if name.is_empty() {
            format!("第{:02}集", index)
        } else {
            name.to_string()
        };

        episodes.push(VodEpisode {
            name: final_name,
            play_url: url.to_string(),
            index,
        });

        index += 1;
    }

    episodes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_standard_routes() {
        let raw = RawVodItem {
            vod_id: "101".into(),
            vod_name: "繁花".into(),
            vod_sub: None,
            vod_year: Some(2023),
            vod_pic: None,
            vod_remarks: None,
            type_name: None,
            vod_actor: None,
            vod_director: None,
            vod_content: None,
            vod_play_from: Some("lzm3u8$$$ffm3u8".into()),
            vod_play_url: Some(
                "第01集$https://lz.com/1.m3u8#第02集$https://lz.com/2.m3u8$$$第01集$https://ff.com/1.m3u8#第02集$https://ff.com/2.m3u8"
                    .into(),
            ),
        };

        let routes = parse_routes("src_1", "优质资源", &raw);
        assert_eq!(routes.len(), 2);

        assert_eq!(routes[0].source_id, "src_1");
        assert_eq!(routes[0].source_name, "优质资源 [lzm3u8]");
        assert_eq!(routes[0].play_from, "lzm3u8");
        assert_eq!(routes[0].episodes.len(), 2);
        assert_eq!(routes[0].episodes[0].name, "第01集");
        assert_eq!(routes[0].episodes[0].play_url, "https://lz.com/1.m3u8");
        assert_eq!(routes[0].episodes[0].index, 1);
        assert_eq!(routes[0].episodes[1].index, 2);

        assert_eq!(routes[1].play_from, "ffm3u8");
        assert_eq!(routes[1].episodes.len(), 2);
    }

    #[test]
    fn test_parse_newline_and_no_dollar_sign() {
        let raw = RawVodItem {
            vod_id: "102".into(),
            vod_name: "电影".into(),
            vod_sub: None,
            vod_year: Some(2024),
            vod_pic: None,
            vod_remarks: None,
            type_name: None,
            vod_actor: None,
            vod_director: None,
            vod_content: None,
            vod_play_from: None, // 缺失
            vod_play_url: Some("https://example.com/movie.m3u8\r\nhttps://example.com/movie2.m3u8".into()),
        };

        let routes = parse_routes("src_2", "备用站", &raw);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].play_from, "m3u8");
        assert_eq!(routes[0].episodes.len(), 2);
        assert_eq!(routes[0].episodes[0].name, "第01集");
        assert_eq!(routes[0].episodes[0].play_url, "https://example.com/movie.m3u8");
        assert_eq!(routes[0].episodes[1].name, "第02集");
    }

    #[test]
    fn test_parse_dirty_empty_data() {
        let raw = RawVodItem {
            vod_id: "103".into(),
            vod_name: "空".into(),
            vod_sub: None,
            vod_year: None,
            vod_pic: None,
            vod_remarks: None,
            type_name: None,
            vod_actor: None,
            vod_director: None,
            vod_content: None,
            vod_play_from: Some("$$$".into()),
            vod_play_url: Some("###$$$invalid_url#   ".into()),
        };

        let routes = parse_routes("src_3", "脏数据站", &raw);
        assert!(routes.is_empty());
    }
}
