//! NFO 读取（Kodi / Emby / Jellyfin 通用格式）。
//!
//! NFO 在优先级链里排第一（§32：NFO > 手动锁定 > 在线 Provider），
//! 因为它是用户**显式表达**的意图 —— 在线刮削永远不该覆盖它。

use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct NfoMovie {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub sort_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    /// 分钟。入库时转毫秒。
    pub runtime_min: Option<i32>,
    pub premiered: Option<String>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NfoSeries {
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct NfoEpisode {
    pub title: Option<String>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    pub aired: Option<String>,
}

/// 解析 movie.nfo。根元素必须是 `<movie>`。
pub fn parse_movie_nfo(xml: &str) -> Option<NfoMovie> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "movie" {
        return None;
    }
    Some(NfoMovie {
        title: child_text(&root, "title"),
        original_title: child_text(&root, "originaltitle"),
        sort_title: child_text(&root, "sorttitle"),
        year: child_i32(&root, "year"),
        overview: child_text(&root, "plot").or_else(|| child_text(&root, "overview")),
        rating: child_f64(&root, "rating"),
        runtime_min: child_i32(&root, "runtime"),
        premiered: child_text(&root, "premiered"),
        tmdb_id: unique_id(&root, "tmdb"),
        imdb_id: unique_id(&root, "imdb").or_else(|| child_text(&root, "imdbid")),
    })
}

/// 解析 tvshow.nfo。根元素必须是 `<tvshow>`。
pub fn parse_series_nfo(xml: &str) -> Option<NfoSeries> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "tvshow" {
        return None;
    }
    Some(NfoSeries {
        title: child_text(&root, "title"),
        original_title: child_text(&root, "originaltitle"),
        year: child_i32(&root, "year"),
        overview: child_text(&root, "plot").or_else(|| child_text(&root, "overview")),
        rating: child_f64(&root, "rating"),
        tmdb_id: unique_id(&root, "tmdb"),
        imdb_id: unique_id(&root, "imdb").or_else(|| child_text(&root, "imdbid")),
    })
}

/// 解析 episode.nfo。根元素必须是 `<episodedetails>`。
pub fn parse_episode_nfo(xml: &str) -> Option<NfoEpisode> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    if root.tag_name().name() != "episodedetails" {
        return None;
    }
    Some(NfoEpisode {
        title: child_text(&root, "title"),
        season: child_i32(&root, "season"),
        episode: child_i32(&root, "episode"),
        overview: child_text(&root, "plot").or_else(|| child_text(&root, "overview")),
        rating: child_f64(&root, "rating"),
        aired: child_text(&root, "aired"),
    })
}

/// 自动识别类型：按根元素分派。扫描器拿到任意 .nfo 时用它。
pub enum NfoKind {
    Movie(NfoMovie),
    Series(NfoSeries),
    Episode(NfoEpisode),
}

pub fn parse_nfo(xml: &str) -> Option<NfoKind> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    match doc.root_element().tag_name().name() {
        "movie" => parse_movie_nfo(xml).map(NfoKind::Movie),
        "tvshow" => parse_series_nfo(xml).map(NfoKind::Series),
        "episodedetails" => parse_episode_nfo(xml).map(NfoKind::Episode),
        _ => None,
    }
}

fn child_text(node: &roxmltree::Node, name: &str) -> Option<String> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn child_i32(node: &roxmltree::Node, name: &str) -> Option<i32> {
    child_text(node, name).and_then(|s| s.parse().ok())
}

fn child_f64(node: &roxmltree::Node, name: &str) -> Option<f64> {
    child_text(node, name).and_then(|s| s.parse().ok())
}

/// `<uniqueid type="tmdb">603</uniqueid>`
fn unique_id(node: &roxmltree::Node, kind: &str) -> Option<String> {
    node.children()
        .find(|n| {
            n.is_element() && n.tag_name().name() == "uniqueid" && n.attribute("type") == Some(kind)
        })
        .and_then(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOVIE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<movie>
  <title>黑客帝国</title>
  <originaltitle>The Matrix</originaltitle>
  <sorttitle>Matrix, The</sorttitle>
  <year>1999</year>
  <plot>一名程序员发现世界是模拟的。</plot>
  <runtime>136</runtime>
  <rating>8.7</rating>
  <premiered>1999-03-31</premiered>
  <uniqueid type="tmdb">603</uniqueid>
  <uniqueid type="imdb">tt0133093</uniqueid>
</movie>"#;

    const EPISODE: &str = r#"<episodedetails>
  <title>Pilot</title>
  <season>1</season>
  <episode>1</episode>
  <aired>2008-01-20</aired>
</episodedetails>"#;

    #[test]
    fn movie_nfo() {
        let m = parse_movie_nfo(MOVIE).unwrap();
        assert_eq!(m.title.as_deref(), Some("黑客帝国"));
        assert_eq!(m.original_title.as_deref(), Some("The Matrix"));
        assert_eq!(m.year, Some(1999));
        assert_eq!(m.runtime_min, Some(136));
        assert_eq!(m.tmdb_id.as_deref(), Some("603"));
        assert_eq!(m.imdb_id.as_deref(), Some("tt0133093"));
    }

    #[test]
    fn episode_nfo() {
        let e = parse_episode_nfo(EPISODE).unwrap();
        assert_eq!(e.season, Some(1));
        assert_eq!(e.episode, Some(1));
        assert_eq!(e.title.as_deref(), Some("Pilot"));
    }

    #[test]
    fn wrong_root_is_none() {
        assert!(parse_movie_nfo(EPISODE).is_none());
    }

    #[test]
    fn dispatch_by_root_element() {
        assert!(matches!(parse_nfo(MOVIE), Some(NfoKind::Movie(_))));
        assert!(matches!(parse_nfo(EPISODE), Some(NfoKind::Episode(_))));
        assert!(parse_nfo("<nope/>").is_none());
    }

    #[test]
    fn malformed_xml_is_none_not_panic() {
        assert!(parse_movie_nfo("<movie><title>x</movie").is_none());
    }
}
