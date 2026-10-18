//! XMLTV（EPG 节目单）解析。
//!
//! L2 用得上，但放在 L0 一起写是因为它和 M3U 一样是"纯文本 → 草稿"的活，
//! 而且**节目单必须能离线测**：抓一次真实 EPG 是几十 MB，不可能拿来当语料。
//!
//! 只取用得上的字段：`channel.id` / `display-name` / `icon`，
//! `programme` 的 `channel` / `start` / `stop` / `title` / `sub-title` / `desc`。
//! XMLTV 还有 `<category>` `<episode-num>` `<rating>` 等一大堆，
//! 现阶段不落库，也就**不解析** —— 解析了没有消费者等于死代码。

use crate::model::ProgramDraft;
use crate::LiveError;

/// XMLTV 里描述的频道（用于给已有频道补名字与图标）。
#[derive(Debug, Clone)]
pub struct ChannelMeta {
    /// 与 `tvg-id` 对应的那个 id
    pub id: String,
    pub display_name: Option<String>,
    pub icon_url: Option<String>,
}

/// 解析一份 XMLTV。返回 `(频道描述, 节目)`。
///
/// 任一条 `programme` 缺 `start`/`stop`/`channel` 就**跳过**而不是整体失败：
/// EPG 脏数据是常态，一个坏节目不该让整份节目单作废。
pub fn parse_xmltv(xml: &str) -> Result<(Vec<ChannelMeta>, Vec<ProgramDraft>), LiveError> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| LiveError::BadXml(e.to_string()))?;

    let mut channels = Vec::new();
    let mut programs = Vec::new();

    for node in doc.descendants().filter(|n| n.has_tag_name("channel")) {
        let Some(id) = node.attribute("id") else {
            continue;
        };
        let mut meta = ChannelMeta {
            id: id.to_string(),
            display_name: None,
            icon_url: None,
        };
        for c in node.children() {
            if c.has_tag_name("display-name") {
                if let Some(t) = c.text() {
                    let t = t.trim();
                    if !t.is_empty() {
                        meta.display_name = Some(t.to_string());
                    }
                }
            } else if c.has_tag_name("icon") {
                if let Some(src) = c.attribute("src") {
                    meta.icon_url = Some(src.to_string());
                }
            }
        }
        channels.push(meta);
    }

    for node in doc.descendants().filter(|n| n.has_tag_name("programme")) {
        let Some(chan) = node.attribute("channel") else {
            continue;
        };
        let Some(start) = node.attribute("start").and_then(xmltv_time) else {
            continue;
        };
        let Some(end) = node
            .attribute("stop")
            .and_then(xmltv_time)
            .or_else(|| node.attribute("end").and_then(xmltv_time))
        else {
            continue;
        };
        let title = node
            .children()
            .find(|c| c.has_tag_name("title"))
            .and_then(|c| c.text())
            .unwrap_or_default()
            .trim();
        if title.is_empty() {
            continue;
        }

        programs.push(ProgramDraft {
            epg_channel_id: chan.to_string(),
            start_at: start,
            end_at: end,
            title: title.to_string(),
            subtitle: node
                .children()
                .find(|c| c.has_tag_name("sub-title"))
                .and_then(|c| c.text())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            description: node
                .children()
                .find(|c| c.has_tag_name("desc"))
                .and_then(|c| c.text())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        });
    }

    Ok((channels, programs))
}

/// `20261016090000 +0800` → `2026-10-16T09:00:00+08:00`。
///
/// 手动拼而不过 `DateTime::parse_from_str` 再格式化，是因为 XMLTV 的时区写法
/// 五花八门（`+0800` / `+08:00` / 干脆没有），而**缺时区时的默认必须显式**：
/// 规范上缺省应视为 UTC，悄悄按本地时区解释会让节目单整体平移几小时。
fn xmltv_time(s: &str) -> Option<String> {
    let s = s.trim();
    // 至少要有 14 位 YYYYMMDDHHMMSS
    if s.len() < 14 {
        return None;
    }
    let (date, rest) = s.split_at(14);
    let y = &date[0..4];
    let mo = &date[4..6];
    let d = &date[6..8];
    let h = &date[8..10];
    let mi = &date[10..12];
    let se = &date[12..14];

    let tz = rest.trim();
    let tz = if tz.is_empty() {
        // 没有时区：按 UTC，且不省略 —— 写清楚 "+00:00" 才不会让下游猜。
        "+00:00".to_string()
    } else {
        // "+0800" → "+08:00"；"+08:00" 原样
        let b = tz.as_bytes();
        if b.len() == 5 && (b[0] == b'+' || b[0] == b'-') {
            format!("{}{}:{}", &tz[0..1], &tz[1..3], &tz[3..5])
        } else {
            tz.to_string()
        }
    };

    Some(format!("{y}-{mo}-{d}T{h}:{mi}:{se}{tz}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tv>
  <channel id="cctv1">
    <display-name>CCTV-1 综合</display-name>
    <icon src="http://logo/1.png"/>
  </channel>
  <programme channel="cctv1" start="20261016190000 +0800" stop="20261016193000 +0800">
    <title>新闻联播</title>
    <sub-title>第 1 期</sub-title>
    <desc>今日要闻</desc>
  </programme>
  <programme channel="cctv1" start="20261016193000 +0800" stop="20261016200000 +0800">
    <title>天气预报</title>
  </programme>
</tv>
"#;

    #[test]
    fn parses_channels_and_programmes() {
        let (ch, pr) = parse_xmltv(SAMPLE).unwrap();
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].id, "cctv1");
        assert_eq!(ch[0].display_name.as_deref(), Some("CCTV-1 综合"));
        assert_eq!(ch[0].icon_url.as_deref(), Some("http://logo/1.png"));

        assert_eq!(pr.len(), 2);
        assert_eq!(pr[0].title, "新闻联播");
        assert_eq!(pr[0].start_at, "2026-10-16T19:00:00+08:00");
        assert_eq!(pr[0].end_at, "2026-10-16T19:30:00+08:00");
        assert_eq!(pr[0].subtitle.as_deref(), Some("第 1 期"));
        assert_eq!(pr[0].description.as_deref(), Some("今日要闻"));
        assert_eq!(pr[1].subtitle, None);
    }

    #[test]
    fn missing_timezone_is_utc_and_says_so() {
        // 缺省时区若悄悄按本地解释，节目单会整体平移几小时。
        let xml = r#"<tv><programme channel="c" start="20261016090000" stop="20261016100000"><title>T</title></programme></tv>"#;
        let (_, pr) = parse_xmltv(xml).unwrap();
        assert_eq!(pr[0].start_at, "2026-10-16T09:00:00+00:00");
    }

    #[test]
    fn dirty_entries_are_skipped_not_fatal() {
        let xml = r#"<tv>
          <programme channel="c" start="20261016090000 +0800"><title>没有结束时间</title></programme>
          <programme channel="c" start="20261016090000 +0800" stop="20261016100000 +0800"><title>好的</title></programme>
          <programme start="20261016090000 +0800" stop="20261016100000 +0800"><title>没有频道</title></programme>
        </tv>"#;
        let (_, pr) = parse_xmltv(xml).unwrap();
        assert_eq!(pr.len(), 1);
        assert_eq!(pr[0].title, "好的");
    }

    #[test]
    fn broken_xml_is_an_error() {
        let e = parse_xmltv("<tv><programme>").unwrap_err();
        assert!(matches!(e, LiveError::BadXml(_)));
    }

    #[test]
    fn colon_timezone_passes_through() {
        let xml = r#"<tv><programme channel="c" start="20261016090000 +08:00" stop="20261016100000 +08:00"><title>T</title></programme></tv>"#;
        let (_, pr) = parse_xmltv(xml).unwrap();
        assert_eq!(pr[0].start_at, "2026-10-16T09:00:00+08:00");
    }
}
