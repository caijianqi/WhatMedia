//! M3U / M3U8 订阅解析。
//!
//! 纯函数，无 I/O —— 语料能离线跑。
//!
//! ## 为什么自己写而不用现成 crate
//!
//! M3U 不是一个"格式"，是一堆方言凑出来的共识。实测会遇到的：
//!
//! | 现象 | 处理 |
//! |---|---|
//! | UTF-8 BOM | 开头剥掉 |
//! | CRLF / 混合换行 | 按行 trim |
//! | 属性名大小写与前缀不统一（`tvg-logo` / `logo` / `TVG-LOGO`） | 归一化后匹配 |
//! | 没有 `#EXTINF` 的裸 URL 列表 | 用 URL 推导频道名，不丢频道 |
//! | `#EXTINF` 与 URL 之间夹着 `#KODIPROP` 等别的指令 | 跳过 |
//! | 显示名带逗号 | 只认**第一个不在引号内**的逗号做分隔 |
//! | 只有 `#EXTM3U` 没有频道 | 报 `EmptyPlaylist`（而不是"同步成功 0 个"） |

use crate::model::ChannelDraft;
use crate::LiveError;

/// 解析一份 M3U 文本，返回按出现顺序排列的频道草稿。
pub fn parse_m3u(text: &str) -> Result<Vec<ChannelDraft>, LiveError> {
    let body = text.trim_start_matches('\u{feff}');
    if !body.trim_start().starts_with("#EXTM3U") {
        return Err(LiveError::NotAPlaylist);
    }

    let mut out: Vec<ChannelDraft> = Vec::new();
    let mut pending: Option<ChannelDraft> = None;

    for raw in body.lines() {
        let line = raw.trim().trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            pending = Some(extinf_to_draft(rest));
            continue;
        }

        // 别的 `#` 指令一律跳过。夹在 #EXTINF 与 URL 之间是合法的
        // （Kodi 的 #KODIPROP / #EXTVLCOPT 就这么写）。
        if line.starts_with('#') {
            continue;
        }

        // 走到这里是 URL 行。
        let mut draft = pending.take().unwrap_or_else(|| ChannelDraft {
            name: String::new(),
            stream_url: String::new(),
            logo_url: None,
            group_title: None,
            epg_channel_id: None,
            number: None,
            sort_order: 0,
        });

        // 过滤协议非法或明显无法播放的链接（如 javascript、本地占位符、非流媒体文件）
        if !is_valid_stream_url(line) {
            continue;
        }

        if draft.name.is_empty() {
            draft.name = name_from_url(line);
        }

        // 过滤广告、交流群、纯更新时间等垃圾通告条目
        if is_junk_channel_name(&draft.name) {
            continue;
        }

        draft.stream_url = line.to_string();
        draft.sort_order = out.len() as i64;
        out.push(draft);
    }

    if out.is_empty() {
        return Err(LiveError::EmptyPlaylist);
    }
    Ok(out)
}

/// 判定是否为合规且可播放的流媒体链接。
pub fn is_valid_stream_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return false;
    }

    // 1. 协议白名单
    let is_valid_scheme = trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("rtmp://")
        || trimmed.starts_with("rtsp://")
        || trimmed.starts_with("udp://")
        || trimmed.starts_with("rtp://")
        || trimmed.starts_with("srt://");
    if !is_valid_scheme {
        return false;
    }

    // 2. 提取 host，过滤回环或空 host
    let lower = trimmed.to_ascii_lowercase();
    let after_scheme = if let Some(pos) = lower.find("://") {
        &lower[pos + 3..]
    } else {
        return false;
    };

    let host_part = after_scheme
        .split(['/', ':', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    if host_part.is_empty() {
        return false;
    }

    let invalid_hosts = ["127.0.0.1", "localhost", "0.0.0.0", "null", "undefined"];
    if invalid_hosts.contains(&host_part) {
        return false;
    }

    // 3. 过滤明显不是流媒体的文件扩展名（静态网页、安装包等）
    let path_part = after_scheme
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let invalid_exts = [
        ".html", ".htm", ".apk", ".zip", ".rar", ".7z", ".exe", ".iso", ".pdf", ".docx", ".xlsx",
        ".doc", ".txt", ".tar", ".gz",
    ];
    for ext in invalid_exts {
        if path_part.ends_with(ext) {
            return false;
        }
    }

    true
}

/// 判定频道名称是否为广告、通告、更新时间等垃圾信息。
pub fn is_junk_channel_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    let junk_keywords = [
        "更新时间",
        "广告招租",
        "商务合作",
        "进群交流",
        "交流群",
        "免责声明",
        "失效请联系",
        "购买请联系",
        "微信群",
        "qq群",
        "备用发布页",
    ];
    for kw in junk_keywords {
        if lower.contains(kw) {
            return true;
        }
    }
    // 过滤形如纯日期戳的名称 "2026-10-12"
    if trimmed.starts_with("202") && trimmed.contains('-') && trimmed.len() <= 20 {
        return true;
    }
    false
}

/// 解析一份国内通用 TXT 格式的直播订阅（如 `频道名,URL` 或 `分组,#genre#`）。
pub fn parse_txt(text: &str) -> Result<Vec<ChannelDraft>, LiveError> {
    let body = text.trim_start_matches('\u{feff}');
    let mut out: Vec<ChannelDraft> = Vec::new();
    let mut current_group: Option<String> = None;

    for raw in body.lines() {
        let line = raw.trim().trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }

        // 分组标记：形如 `📺央视频道,#genre#` 或 `卫视频道，#genre#`
        let is_genre = line.ends_with(",#genre#")
            || line.ends_with("，#genre#")
            || line.ends_with(",#genre")
            || line.ends_with("，#genre");
        if is_genre {
            let genre_name = line
                .trim_end_matches("#genre#")
                .trim_end_matches("#genre")
                .trim_end_matches(',')
                .trim_end_matches('，')
                .trim();
            // 忽略更新时间元数据分组（如 `🕘️更新时间,#genre#`）
            if !genre_name.is_empty() && !is_junk_channel_name(genre_name) {
                current_group = Some(genre_name.to_string());
            } else {
                current_group = None;
            }
            continue;
        }

        // 频道与流地址：以逗号切分（支持英文逗号与中文逗号）
        let parts = line
            .find(',')
            .map(|idx| (&line[..idx], &line[idx + 1..]))
            .or_else(|| {
                line.find('，')
                    .map(|idx| (&line[..idx], &line[idx + '，'.len_utf8()..]))
            });

        if let Some((name_part, url_part)) = parts {
            let name = name_part.trim();
            let url = url_part.trim();
            if name.is_empty() || url.is_empty() {
                continue;
            }
            if is_junk_channel_name(name) {
                continue;
            }
            if !is_valid_stream_url(url) {
                continue;
            }

            out.push(ChannelDraft {
                name: name.to_string(),
                stream_url: url.to_string(),
                logo_url: None,
                group_title: current_group.clone(),
                epg_channel_id: None,
                number: None,
                sort_order: out.len() as i64,
            });
        }
    }

    if out.is_empty() {
        return Err(LiveError::EmptyPlaylist);
    }
    Ok(out)
}

/// 解析播放列表文本。自适应 M3U 与 TXT 格式。
pub fn parse_playlist(text: &str) -> Result<Vec<ChannelDraft>, LiveError> {
    let body = text.trim_start_matches('\u{feff}');
    if body.trim_start().starts_with("#EXTM3U") {
        parse_m3u(text)
    } else {
        // 如果包含 #genre# 或频道逗号行，尝试作为 TXT 解析
        parse_txt(text).map_err(|e| match e {
            LiveError::EmptyPlaylist => LiveError::NotAPlaylist,
            other => other,
        })
    }
}

/// 解析 `#EXTINF:` 之后的那一段。
///
/// 形如：`-1 tvg-id="cctv1" tvg-logo="http://x/y.png" group-title="央视",CCTV-1 综合`
fn extinf_to_draft(rest: &str) -> ChannelDraft {
    // 显示名在**第一个不在引号内的逗号**之后。
    // 直接 `split(',')` 会在"显示名里带逗号"或"属性值里带逗号"时错位。
    let (attr_part, display) = split_display_name(rest);

    let mut draft = ChannelDraft {
        name: String::new(),
        stream_url: String::new(),
        logo_url: None,
        group_title: None,
        epg_channel_id: None,
        number: None,
        sort_order: 0,
    };

    for (k, v) in attrs(attr_part) {
        match k.as_str() {
            "tvg-id" | "id" => draft.epg_channel_id = Some(v),
            "tvg-logo" | "logo" => draft.logo_url = Some(v),
            "group-title" | "group" => draft.group_title = Some(v),
            "tvg-chno" | "chno" => draft.number = v.trim().parse::<i64>().ok(),
            // tvg-name 是"备选名"：只有逗号后没写名字时才用它。
            "tvg-name" | "name" if draft.name.is_empty() => draft.name = v,
            _ => {}
        }
    }

    let display = display.trim();
    if !display.is_empty() {
        draft.name = display.to_string();
    }
    draft
}

/// 切出属性段与显示名。返回 `(属性段, 逗号之后的显示名)`。
fn split_display_name(s: &str) -> (&str, &str) {
    let bytes = s.as_bytes();
    let mut in_quote = false;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' => in_quote = !in_quote,
            b',' if !in_quote => return (&s[..i], &s[i + 1..]),
            _ => {}
        }
    }
    (s, "")
}

/// 扫出 `key="value"` 对。属性之间用空白分隔，值可以带空格（在引号内）。
fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // 跳过分隔空白
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // key 直到 '='
        let start = i;
        while i < bytes.len() && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            // 没有 '=' 的裸词（比如 `-1` 那个时长）。整段吃掉，继续。
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            continue;
        }
        let key = s[start..i].trim().to_ascii_lowercase();
        i += 1; // 过 '='

        let value = if i < bytes.len() && bytes[i] == b'"' {
            i += 1;
            let start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            let v = s[start..i.min(s.len())].to_string();
            i += 1; // 过收尾引号
            v
        } else {
            let start = i;
            while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            s[start..i].to_string()
        };
        out.push((key, value));
    }
    out
}

/// 没有 `#EXTINF` 时，从 URL 推一个能看的名字。
fn name_from_url(url: &str) -> String {
    let last = url.rsplit('/').next().unwrap_or(url);
    let last = last.split('?').next().unwrap_or(last);
    let stem = match last.rsplit_once('.') {
        Some((stem, ext)) if ext.len() <= 5 && !stem.is_empty() => stem,
        _ => last,
    };
    let decoded = percent_decode(stem);
    let t = decoded.trim();
    if t.is_empty() {
        "未命名频道".to_string()
    } else {
        t.to_string()
    }
}

/// 只解 `%XX`，不做 `+` → 空格（URL 路径里 `+` 就是加号）。
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"#EXTM3U
#EXTINF:-1 tvg-id="cctv1" tvg-name="CCTV-1" tvg-logo="http://logo/cctv1.png" group-title="央视",CCTV-1 综合
http://example.com/cctv1.ts
#EXTINF:-1 tvg-id="hunan" tvg-logo="http://logo/hunan.png" group-title="卫视",湖南卫视
http://example.com/hunan.ts
"#;

    #[test]
    fn parses_standard_playlist() {
        let ch = parse_m3u(SAMPLE).unwrap();
        assert_eq!(ch.len(), 2);

        assert_eq!(ch[0].name, "CCTV-1 综合");
        assert_eq!(ch[0].stream_url, "http://example.com/cctv1.ts");
        assert_eq!(ch[0].group_title.as_deref(), Some("央视"));
        assert_eq!(ch[0].logo_url.as_deref(), Some("http://logo/cctv1.png"));
        assert_eq!(ch[0].epg_channel_id.as_deref(), Some("cctv1"));
        assert_eq!(ch[0].sort_order, 0);

        assert_eq!(ch[1].name, "湖南卫视");
        assert_eq!(ch[1].sort_order, 1);
    }

    #[test]
    fn bom_and_crlf_are_tolerated() {
        let raw = "\u{feff}#EXTM3U\r\n#EXTINF:-1 group-title=\"G\",A\r\nhttp://a.ts\r\n";
        let ch = parse_m3u(raw).unwrap();
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].name, "A");
        assert_eq!(ch[0].stream_url, "http://a.ts");
    }

    #[test]
    fn attribute_aliases_normalize() {
        // 有的订阅写 logo= / group= 而不是 tvg-logo= / group-title=
        let raw = "#EXTM3U\n#EXTINF:0 logo=\"L\" group=\"G\" id=\"i1\" chno=\"7\",N\nhttp://u\n";
        let ch = parse_m3u(raw).unwrap();
        assert_eq!(ch[0].logo_url.as_deref(), Some("L"));
        assert_eq!(ch[0].group_title.as_deref(), Some("G"));
        assert_eq!(ch[0].epg_channel_id.as_deref(), Some("i1"));
        assert_eq!(ch[0].number, Some(7));
    }

    #[test]
    fn bare_urls_still_become_channels() {
        let raw = "#EXTM3U\nhttp://a.com/one.ts\nhttp://b.com/two.m3u8?token=x\n";
        let ch = parse_m3u(raw).unwrap();
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0].name, "one");
        assert_eq!(ch[1].name, "two");
        assert_eq!(ch[1].stream_url, "http://b.com/two.m3u8?token=x");
    }

    #[test]
    fn directives_between_extinf_and_url_are_skipped() {
        let raw = "#EXTM3U\n#EXTINF:-1 group-title=\"G\",N\n#KODIPROP:inputstream=1\n#EXTVLCOPT:x=y\nhttp://u.ts\n";
        let ch = parse_m3u(raw).unwrap();
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].name, "N");
    }

    #[test]
    fn comma_inside_display_name_is_kept() {
        let raw = "#EXTM3U\n#EXTINF:-1 group-title=\"G\",电影, 频道A\nhttp://u.ts\n";
        let ch = parse_m3u(raw).unwrap();
        assert_eq!(ch[0].name, "电影, 频道A");
    }

    #[test]
    fn missing_header_is_rejected() {
        let e = parse_m3u("http://a.ts\nhttp://b.ts\n").unwrap_err();
        assert!(matches!(e, LiveError::NotAPlaylist));
    }

    #[test]
    fn header_without_channels_is_empty_not_ok() {
        // "同步成功，0 个频道" 是最糟的反馈：用户以为配对了。
        let e = parse_m3u("#EXTM3U\n#EXTINF:-1,N\n").unwrap_err();
        assert!(matches!(e, LiveError::EmptyPlaylist));
    }

    #[test]
    fn tvg_name_used_only_when_display_is_empty() {
        let raw = "#EXTM3U\n#EXTINF:-1 tvg-name=\"备用名\",\nhttp://u.ts\n";
        let ch = parse_m3u(raw).unwrap();
        assert_eq!(ch[0].name, "备用名");
    }

    #[test]
    fn percent_encoded_names_are_readable() {
        let raw = "#EXTM3U\nhttp://a/%E4%B8%AD%E5%A4%AE.m3u8\n";
        let ch = parse_m3u(raw).unwrap();
        assert_eq!(ch[0].name, "中央");
    }

    #[test]
    fn parses_txt_playlist_with_genres() {
        let raw = r#"
🕘️更新时间,#genre#
2026-10-12 11:25:50,http://example.com/cctv1hd.m3u8

📺央视频道,#genre#
CCTV-1,http://example.com/cctv1_line1.m3u8
CCTV-1,http://example.com/cctv1_line2.m3u8
CCTV-2,http://example.com/cctv2.m3u8

📡卫视频道,#genre#
广东卫视,http://example.com/gd.m3u8
"#;
        let list = parse_txt(raw).unwrap();
        assert_eq!(list.len(), 4);

        assert_eq!(list[0].name, "CCTV-1");
        assert_eq!(list[0].stream_url, "http://example.com/cctv1_line1.m3u8");
        assert_eq!(list[0].group_title.as_deref(), Some("📺央视频道"));

        assert_eq!(list[1].name, "CCTV-1");
        assert_eq!(list[1].stream_url, "http://example.com/cctv1_line2.m3u8");
        assert_eq!(list[1].group_title.as_deref(), Some("📺央视频道"));

        assert_eq!(list[2].name, "CCTV-2");
        assert_eq!(list[2].group_title.as_deref(), Some("📺央视频道"));

        assert_eq!(list[3].name, "广东卫视");
        assert_eq!(list[3].group_title.as_deref(), Some("📡卫视频道"));
    }

    #[test]
    fn adaptive_playlist_detects_m3u_and_txt() {
        let m3u_sample = "#EXTM3U\n#EXTINF:-1,测试\nhttp://test.m3u8\n";
        let res1 = parse_playlist(m3u_sample).unwrap();
        assert_eq!(res1.len(), 1);
        assert_eq!(res1[0].name, "测试");

        let txt_sample = "央视频道,#genre#\nCCTV-1,http://test.m3u8\n";
        let res2 = parse_playlist(txt_sample).unwrap();
        assert_eq!(res2.len(), 1);
        assert_eq!(res2[0].name, "CCTV-1");
        assert_eq!(res2[0].group_title.as_deref(), Some("央视频道"));
    }

    #[test]
    fn filters_out_invalid_urls_and_junk_channels() {
        let raw = r#"#EXTM3U
#EXTINF:-1,广告招租联系客服
http://my-live.com/ad.m3u8
#EXTINF:-1,合法频道A
http://my-live.com/live1.m3u8
#EXTINF:-1,非法协议频道
javascript:alert(1)
#EXTINF:-1,非媒体文件
http://my-live.com/player.apk
#EXTINF:-1,回环占位
http://127.0.0.1/test.m3u8
#EXTINF:-1,合法频道B
rtmp://my-live.com/live/stream
"#;
        let list = parse_m3u(raw).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "合法频道A");
        assert_eq!(list[1].name, "合法频道B");
    }
}
