//! 字幕语言与标记识别。
//!
//! 外挂字幕的信息**几乎全部藏在文件名里**（`Movie.zh-Hans.forced.srt`），
//! 没有别的来源。所以解析规则必须显式、可测，且对"猜不出来"保持诚实（返回 None）。

/// 3 字母 → 2 字母的常用映射。ISO 639-1 是客户端普遍认识的那一套。
const BCP47_3_TO_2: &[(&str, &str)] = &[
    ("eng", "en"),
    ("chi", "zh"),
    ("zho", "zh"),
    ("jpn", "ja"),
    ("kor", "ko"),
    ("fre", "fr"),
    ("fra", "fr"),
    ("ger", "de"),
    ("deu", "de"),
    ("spa", "es"),
    ("rus", "ru"),
    ("ita", "it"),
    ("por", "pt"),
    ("nld", "nl"),
    ("dut", "nl"),
    ("swe", "sv"),
    ("dan", "da"),
    ("nor", "no"),
    ("fin", "fi"),
    ("pol", "pl"),
    ("tur", "tr"),
    ("ara", "ar"),
    ("heb", "he"),
    ("hin", "hi"),
    ("tha", "th"),
    ("vie", "vi"),
    ("ind", "id"),
    ("may", "ms"),
    ("msa", "ms"),
    ("ces", "cs"),
    ("cze", "cs"),
    // 简体/繁体的三字母写法（chs / cht）在中文圈的字幕里非常常见。
    ("chs", "zh"),
    ("cht", "zh"),
    ("hun", "hu"),
    ("ron", "ro"),
    ("rum", "ro"),
    ("ell", "el"),
    ("gre", "el"),
    ("ukr", "uk"),
    ("cat", "ca"),
];

/// 从文件名片段猜语言标签。猜不出来返回 None —— **不要瞎猜成 und**。
pub fn normalize_lang(token: &str) -> Option<String> {
    let t = token.trim().to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }

    // 已经是 2 字母或带地区的 BCP-47（zh-Hans / pt-BR / en-US）。
    if let Some((head, rest)) = t.split_once('-') {
        let base = normalize_lang(head)?;
        // 地区部分统一写成首字母大写的规范形式（zh-hans → zh-Hans）。
        let region = match rest {
            "hans" | "cn" | "chs" | "simplified" => "Hans".to_string(),
            "hant" | "tw" | "hk" | "cht" | "traditional" => "Hant".to_string(),
            // 两位地区码一律大写（pt-br → pt-BR），其余只首字母大写。
            other if other.len() == 2 => other.to_ascii_uppercase(),
            other => capitalize(other),
        };
        return Some(format!("{base}-{region}"));
    }

    if t.len() == 2 {
        return Some(t);
    }
    if t.len() == 3 {
        if let Some((_, two)) = BCP47_3_TO_2.iter().find(|(a, _)| *a == t.as_str()) {
            return Some((*two).to_string());
        }
        return None;
    }

    // 中文文件名里常见的写法。
    match t.as_str() {
        "chinese" | "zhongwen" | "中文" | "简体" | "繁體" | "繁体" => {
            Some("zh".to_string())
        }
        "english" => Some("en".to_string()),
        "japanese" => Some("ja".to_string()),
        "korean" => Some("ko".to_string()),
        _ => None,
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// 文件名里的标记词。这些词**不是语言**，必须单独挑出来，
/// 否则 `Movie.forced.srt` 会被当成语言 "forced"。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SubtitleFlags {
    pub forced: bool,
    pub default: bool,
    /// SDH / CC：听障字幕。不算 forced，但 UI 上要能区分。
    pub sdh: bool,
}

/// 标记词 → 置位函数。用类型别名是为了不让 clippy 的 type_complexity 吵。
type FlagSetters = [(&'static str, fn(&mut SubtitleFlags)); 6];

const FLAG_TOKENS: FlagSetters = [
    ("forced", |f| f.forced = true),
    ("default", |f| f.default = true),
    ("sdh", |f| f.sdh = true),
    ("cc", |f| f.sdh = true),
    ("hearing", |f| f.sdh = true),
    ("impaired", |f| f.sdh = true),
];

/// 解析 `Foo.zh-Hans.forced` 这类后缀串。
///
/// 返回 `(语言, 标记)`。多个语言 token 时取**第一个能识别的**
/// （`Foo.zh.en.srt` 这种命名是有的，取第一个最符合直觉）。
pub fn parse_suffix(tokens: &[&str]) -> (Option<String>, SubtitleFlags) {
    let mut flags = SubtitleFlags::default();
    let mut lang = None;

    for t in tokens {
        let lower = t.to_ascii_lowercase();
        if let Some((_, apply)) = FLAG_TOKENS.iter().find(|(name, _)| *name == lower.as_str()) {
            apply(&mut flags);
            continue;
        }
        if lang.is_none() {
            lang = normalize_lang(t);
        }
    }
    (lang, flags)
}

/// 按扩展名判断字幕格式。未知扩展名返回 None（不当成字幕文件）。
pub fn codec_from_ext(ext: &str) -> Option<whatmedia_core::SubtitleCodec> {
    use whatmedia_core::SubtitleCodec as C;
    match ext.to_ascii_lowercase().as_str() {
        "srt" => Some(C::Srt),
        "ass" => Some(C::Ass),
        "ssa" => Some(C::Ssa),
        "vtt" => Some(C::Vtt),
        "pgs" | "sup" => Some(C::Pgs),
        "sub" => Some(C::DvdSub),
        "dvbsub" => Some(C::DvbSub),
        _ => None,
    }
}

/// 位图字幕。客户端普遍不支持，服务端必须烧录或丢弃。
pub fn is_bitmap(c: &whatmedia_core::SubtitleCodec) -> bool {
    use whatmedia_core::SubtitleCodec as C;
    matches!(c, C::Pgs | C::DvdSub | C::DvbSub)
}

/// 文本字幕。可以无损（近似无损）地互相转换。
pub fn is_text(c: &whatmedia_core::SubtitleCodec) -> bool {
    use whatmedia_core::SubtitleCodec as C;
    matches!(c, C::Srt | C::Ass | C::Ssa | C::Vtt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatmedia_core::SubtitleCodec as C;

    #[test]
    fn three_letter_codes_map_to_two() {
        assert_eq!(normalize_lang("eng").as_deref(), Some("en"));
        assert_eq!(normalize_lang("chi").as_deref(), Some("zh"));
        assert_eq!(normalize_lang("jpn").as_deref(), Some("ja"));
        assert_eq!(normalize_lang("deu").as_deref(), Some("de"));
    }

    #[test]
    fn region_variants_normalize() {
        assert_eq!(normalize_lang("zh-Hans").as_deref(), Some("zh-Hans"));
        assert_eq!(normalize_lang("zh-hant").as_deref(), Some("zh-Hant"));
        assert_eq!(normalize_lang("pt-BR").as_deref(), Some("pt-BR"));
        assert_eq!(normalize_lang("chs").as_deref(), Some("zh"));
    }

    #[test]
    fn unknown_tokens_stay_unknown() {
        // 宁可返回 None 也不要编一个语言出来。
        assert_eq!(normalize_lang("xyz"), None);
        assert_eq!(normalize_lang(""), None);
        assert_eq!(normalize_lang("forced"), None);
    }

    #[test]
    fn flags_are_not_language() {
        let (lang, flags) = parse_suffix(&["forced"]);
        assert_eq!(lang, None);
        assert!(flags.forced);
        assert!(!flags.default);
    }

    #[test]
    fn language_and_flags_coexist() {
        let (lang, flags) = parse_suffix(&["zh-Hans", "forced"]);
        assert_eq!(lang.as_deref(), Some("zh-Hans"));
        assert!(flags.forced);

        let (lang, flags) = parse_suffix(&["eng", "sdh"]);
        assert_eq!(lang.as_deref(), Some("en"));
        assert!(flags.sdh);
        assert!(!flags.forced);
    }

    #[test]
    fn first_recognized_language_wins() {
        let (lang, _) = parse_suffix(&["zh", "en"]);
        assert_eq!(lang.as_deref(), Some("zh"));
    }

    #[test]
    fn extension_maps_to_codec() {
        assert_eq!(codec_from_ext("srt"), Some(C::Srt));
        assert_eq!(codec_from_ext("ASS"), Some(C::Ass));
        assert_eq!(codec_from_ext("sup"), Some(C::Pgs));
        assert_eq!(codec_from_ext("mkv"), None);
    }

    #[test]
    fn bitmap_classification() {
        assert!(is_bitmap(&C::Pgs));
        assert!(is_bitmap(&C::DvdSub));
        assert!(!is_bitmap(&C::Ass));
        assert!(is_text(&C::Vtt));
        assert!(!is_text(&C::Pgs));
    }
}
