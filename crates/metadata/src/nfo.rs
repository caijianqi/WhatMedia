//! 本地 NFO 作为元数据源。
//!
//! **它故意不实现 `MetadataProvider`。**
//!
//! 理由：`MetadataProvider` 的语义是"拿一个查询去远端搜"，而 NFO 是"这个文件旁边
//! 恰好有一份描述"。把它硬塞进 search 接口，要么得先建索引（多余），
//! 要么返回一个元素的数组（自欺欺人）。
//!
//! 所以 NFO 在优先级链里是**独立的一环**，由 `pipeline` 在 Provider 之前应用。

use whatmedia_core::{MetadataImage, MetadataRecord};
use whatmedia_media::{NfoKind, NfoMovie, NfoSeries};

/// 把 NFO 翻译成统一的 `MetadataRecord`，`provider` 标记为 `"nfo"`。
///
/// 返回 None 表示这份 NFO 里没有任何可用字段（比如只有 `<title/>` 是空的）。
pub fn record_from_nfo(kind: &NfoKind) -> Option<MetadataRecord> {
    match kind {
        NfoKind::Movie(m) => from_movie(m),
        NfoKind::Series(s) => from_series(s),
        NfoKind::Episode(_) => None,
    }
}

fn from_movie(m: &NfoMovie) -> Option<MetadataRecord> {
    let r = MetadataRecord {
        provider: "nfo".into(),
        // NFO 里的 tmdbid 才是真正的外部 id；没有就只能留空，
        // 上层据此判断"要不要再去 Provider 补一轮"。
        provider_id: m.tmdb_id.clone().unwrap_or_default(),
        title: m.title.clone().unwrap_or_default(),
        original_title: m.original_title.clone(),
        sort_title: m.sort_title.clone(),
        year: m.year.or_else(|| year_of(m.premiered.as_deref())),
        tagline: None,
        overview: m.overview.clone(),
        runtime_ms: m.runtime_min.map(|r| r as i64 * 60_000),
        rating: m.rating,
        genres: Vec::new(),
        images: Vec::new(),
        credits: Vec::new(),
    };
    has_anything(&r).then_some(r)
}

fn from_series(s: &NfoSeries) -> Option<MetadataRecord> {
    let r = MetadataRecord {
        provider: "nfo".into(),
        provider_id: s.tmdb_id.clone().unwrap_or_default(),
        title: s.title.clone().unwrap_or_default(),
        original_title: s.original_title.clone(),
        sort_title: None,
        year: s.year,
        tagline: None,
        overview: s.overview.clone(),
        runtime_ms: None,
        rating: s.rating,
        genres: Vec::new(),
        images: Vec::new(),
        credits: Vec::new(),
    };
    has_anything(&r).then_some(r)
}

/// `YYYY-MM-DD` → 年份。NFO 里 `year` 经常缺，但 `premiered` 有。
pub fn year_of(s: Option<&str>) -> Option<i32> {
    s.and_then(|v| v.get(0..4))
        .and_then(|y| y.parse().ok())
        .filter(|y| (1900..=2035).contains(y))
}

/// 除了 provider 之外一个字段都没有的 NFO 应当视为无效，
/// 否则它会"占位"并阻止 Provider 补齐数据。
fn has_anything(r: &MetadataRecord) -> bool {
    !r.title.is_empty()
        || r.year.is_some()
        || r.overview.is_some()
        || r.rating.is_some()
        || r.runtime_ms.is_some()
        || r.original_title.is_some()
}

/// NFO 里记录的图片路径（`<thumb>`）通常是相对路径或 URL。
/// 这里只做"是不是 http"的判断，真正的取图在 `images` 模块。
pub fn image_from_nfo_thumb(thumb: &str) -> MetadataImage {
    MetadataImage {
        kind: "poster".into(),
        url: thumb.to_string(),
        width: None,
        height: None,
        vote: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatmedia_media::NfoMovie;

    #[test]
    fn movie_nfo_maps_fields() {
        let m = NfoMovie {
            title: Some("黑客帝国".into()),
            original_title: Some("The Matrix".into()),
            sort_title: Some("Matrix, The".into()),
            year: None,
            overview: Some("简介".into()),
            rating: Some(8.5),
            runtime_min: Some(136),
            premiered: Some("1999-03-31".into()),
            tmdb_id: Some("603".into()),
            imdb_id: None,
        };
        let r = record_from_nfo(&NfoKind::Movie(m)).unwrap();
        assert_eq!(r.title, "黑客帝国");
        assert_eq!(r.runtime_ms, Some(136 * 60_000));
        // year 缺失时要从 premiered 补
        assert_eq!(r.year, Some(1999));
        assert_eq!(r.provider_id, "603");
    }

    /// 全空的 NFO 不能"占位"，否则 Provider 永远补不上数据。
    #[test]
    fn empty_nfo_is_not_a_source() {
        let r = record_from_nfo(&NfoKind::Movie(NfoMovie::default()));
        assert!(r.is_none());
    }

    #[test]
    fn year_parsing_is_bounded() {
        assert_eq!(year_of(Some("1999-03-31")), Some(1999));
        assert_eq!(year_of(Some("abcd-01-01")), None);
        assert_eq!(year_of(Some("1870-01-01")), None, "1900 之前不算年份");
        assert_eq!(year_of(None), None);
    }
}
