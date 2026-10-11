//! 文件名解析器。
//!
//! 媒体文件的命名没有任何强制标准，实际世界里至少有四套并存：
//!   · Scene 规则    `The.Matrix.1999.1080p.BluRay.x264.DTS-HD.MA.5.1-FGT`
//!   · 常规剧集      `Breaking.Bad.S01E01.1080p.BluRay.x265-RARBG`
//!   · 动漫发布组    `[SubsPlease] Frieren - 01 (1080p) [HEVC x265 10bit]`
//!   · 中文习惯      `我的团长我的团.第01集.mp4`
//!
//! 所以这里**不是**一个正则能解决的事，而是一条有序规则链：
//!   ① 剥离方括号标签 → ② 季/集 → ③ 年份 → ④ 发布组 → ⑤ 发行信息 → ⑥ 余下即标题
//!
//! 顺序不能随意调换：先抽季集，是因为 `S01E01` 里的数字会干扰年份与分辨率判断。

use whatmedia_core::HdrType;
use regex::Regex;
use std::sync::LazyLock;

/// 解析结果。所有字段都是"尽力而为"，缺了就 None，绝不猜。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedName {
    pub title: String,
    pub year: Option<i32>,
    pub season: Option<i32>,
    pub episode: Option<i32>,
    /// 合并集（`S01E01-E03`）的结束集号。
    pub episode_end: Option<i32>,
    /// 单集标题（`S01E02.Day.of.Days` 里的 `Day of Days`）。
    /// 只有拿到所在目录名才能可靠地切分出来，见 `apply_series_hint`。
    pub episode_title: Option<String>,
    pub resolution: Option<String>,
    pub source: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub bit_depth: Option<i32>,
    pub hdr: Option<HdrType>,
    pub release_group: Option<String>,
}

impl ParsedName {
    pub fn is_episode(&self) -> bool {
        self.episode.is_some() || self.season.is_some()
    }
}

// ─────────────────────────────────────────────────────────────
// 正则表。集中放一处，便于对照真实语料逐条修。
// ─────────────────────────────────────────────────────────────

/// `S01E01` / `S01E01-E03` / `S01.E01`
static RE_SE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[ ._\-])s(?P<s>\d{1,2})[ ._\-]*e(?P<e>\d{1,3})(?:[ ._\-]*(?:e|-)[ ._\-]*(?P<e2>\d{1,3}))?").unwrap()
});

/// `1x01` / `01x02`
static RE_X: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[ ._\-])(?P<s>\d{1,2})x(?P<e>\d{2,3})(?:[ ._\-]|$)").unwrap()
});

/// `Season 1 Episode 2`
static RE_SEASON_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)season[ ._\-]*(?P<s>\d{1,2})[ ._\-]*episode[ ._\-]*(?P<e>\d{1,3})").unwrap()
});

/// 只有季：`Show.S01.1080p...`
static RE_SEASON_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[ ._\-])s(?P<s>\d{1,3})(?:[ ._\-]|$)").unwrap());

/// 只有集：`E01` / `EP12`
static RE_EPISODE_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[ ._\-])ep?(?P<e>\d{1,3})(?:[ ._\-]|$)").unwrap());

/// 中文：`第01集` / `第01话`
static RE_CN_EPISODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"第\s*(?P<e>\d{1,3})\s*[集话]").unwrap());

/// 结尾纯数字 → 动漫绝对集号：`One.Piece.1080`（第 1080 集）。
///
/// 必须**在年份抽取之后**才用：否则 `Movie.2019` 会被当成第 2019 集。
/// 也必须要求数字后面没有 `p`/`i`，否则 `2160p` 会被当成集号。
static RE_TRAILING_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[ ._\-])(?P<e>\d{2,4})$").unwrap());

/// 动漫：`Frieren - 01 (1080p)`、`Show - 12v2`
static RE_ANIME_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[ ._\-])-[ ._\-]*(?P<e>\d{2,3})(?:v\d)?(?:[ ._\-]|$)").unwrap()
});

/// 年份。取候选里**最后一个**落在 1900..=2030 的，理由见 `pick_year`。
///
/// 注意：regex crate 不支持前后断言，所以"后面不能紧跟字母/数字"这条约束
/// 用消费型分组 `(?:[^0-9A-Za-z]|$)` 表达 —— 它能挡掉 `1920x1080` 这种分辨率。
static RE_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"((?:19|20)\d{2})(?:[^0-9A-Za-z]|$)").unwrap());

/// 方括号标签：`[...]` `【...】`
static RE_BRACKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\[【]([^\]】]*)[\]】]").unwrap());

/// 结尾发布组：`...-FGT`
static RE_GROUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-(?P<g>[A-Za-z0-9][A-Za-z0-9_.]{1,23})\s*$").unwrap());

/// 第一个"发行信息"关键字。它左边的一切都是标题。
static RE_RELEASE_CUT: LazyLock<Regex> = LazyLock::new(|| {
    // 用 \b 而不是"分隔符 + 关键字 + 分隔符"：后者会把分隔符**消费掉**，
    // 于是 `HEVC x265` 里只有第一个能被匹配到。零宽边界没有这个问题。
    Regex::new(
        r"(?ix)\b
        (2160p|4320p|1080p|1080i|720p|576p|480p|360p|4k|8k
        |blu-?ray|bdrip|brrip|bdremux|bd|remux|web-?dl|web-?rip|webrip|web|hdtv|hddvd|dvdrip|dvd|cam
        |x264|x265|h\.?264|h\.?265|hevc|avc|av1|vp9|xvid|divx|mpeg-?2|vc-?1
        |10-?bit|8-?bit|hi10p
        |hdr10\+|hdr10|dolby.?vision|dolbyvision|hlg|hdr|dv|sdr
        |truehd|atmos|dts-?hd|dts-?es|dts|ddp|dd\+|eac3|ac3|aac|flac|opus|mp3|lpcm|pcm
        |5\.1|7\.1|2\.0|6ch|2ch|8ch
        |proper|repack|internal|limited|unrated|extended|remastered|imax|directors.?cut)
        \b",
    )
    .unwrap()
});

static RE_RESOLUTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(2160p|4320p|1080p|1080i|720p|576p|480p|360p|4k|8k)\b").unwrap()
});

static RE_SOURCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(blu-?ray|bdrip|brrip|bdremux|remux|web-?dl|web-?rip|webrip|hdtv|hddvd|dvdrip|dvd|cam)\b",
    )
    .unwrap()
});

static RE_VIDEO_CODEC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(x264|x265|h\.?264|h\.?265|hevc|avc|av1|vp9|xvid|divx|mpeg-?2|vc-?1)\b")
        .unwrap()
});

static RE_AUDIO_CODEC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(truehd(?:\.atmos)?|atmos|dts-?hd(?:\.ma)?|dts-?es|dts|ddp|dd\+|eac3|ac3|aac|flac|opus|mp3|lpcm|pcm)\b")
        .unwrap()
});

static RE_BIT_DEPTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b((?:10|8)-?bit|hi10p)\b").unwrap());

/// HDR 关键字。**只在开头加词边界** —— `HDR10+` 的 `+` 后面不是词边界，
/// 加尾部 \b 会让 HDR10+ 退化成 HDR10。这是踩过的坑，别改回去。
static RE_HDR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(hdr10\+|hdr10|dolby.?vision|dolbyvision|hlg|hdr)").unwrap()
});

/// `DV` 必须两侧都有边界，否则会被 `dvd` 命中。
static RE_DV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bdv\b").unwrap());

static RE_SDR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bsdr\b").unwrap());

// ─────────────────────────────────────────────────────────────

/// 去掉空的括号残留。`Movie.(2020)` 抽掉年份后会剩下 `Movie.()`。
fn strip_empty_brackets(s: &str) -> String {
    // 逐字符删掉空括号对（含"括号里只有空白"）。比正则直观，也不引入新依赖。
    let mut buf = String::with_capacity(s.len());
    let mut chars: Vec<char> = s.chars().collect();
    loop {
        buf.clear();
        let mut i = 0usize;
        let mut changed = false;
        while i < chars.len() {
            if i + 1 < chars.len() && is_open_bracket(chars[i]) && is_close_bracket(chars[i + 1]) {
                changed = true;
                i += 2;
                continue;
            }
            // 括号里只有空白也算空
            if is_open_bracket(chars[i]) {
                let mut j = i + 1;
                let mut only_space = true;
                while j < chars.len() && !is_close_bracket(chars[j]) {
                    if !chars[j].is_whitespace() {
                        only_space = false;
                    }
                    j += 1;
                }
                if only_space && j < chars.len() {
                    changed = true;
                    i = j + 1;
                    continue;
                }
            }
            buf.push(chars[i]);
            i += 1;
        }
        chars = buf.chars().collect();
        if !changed {
            break;
        }
    }
    chars.into_iter().collect()
}

fn is_open_bracket(c: char) -> bool {
    matches!(c, '(' | '[' | '{' | '（' | '【')
}

fn is_close_bracket(c: char) -> bool {
    matches!(c, ')' | ']' | '}' | '）' | '】')
}

/// 用所在目录名修正剧集标题。
///
/// 单独看文件名无法区分 `Breaking Bad` 和它的集名：
/// `Band.of.Brothers.S01E02.Day.of.Days.1080p.mkv` 里，`Day of Days` 是集名不是剧名。
/// 目录名是唯一可靠的信号 —— 所以它作为参数传进来，而不是让解析器去猜。
pub fn apply_series_hint(parsed: &mut ParsedName, dir_name: &str) {
    if !parsed.is_episode() || dir_name.trim().is_empty() {
        return;
    }
    let dir_title = parse_stem(dir_name).title;
    if dir_title.is_empty() {
        return;
    }

    let want: Vec<char> = dir_title.to_lowercase().chars().collect();
    let have: Vec<char> = parsed.title.to_lowercase().chars().collect();
    if have.len() <= want.len() || have[..want.len()] != want[..] {
        return;
    }

    let rest: String = parsed.title.chars().skip(want.len()).collect();
    let rest = rest.trim().to_string();
    if rest.is_empty() {
        return;
    }
    parsed.episode_title = Some(rest);
    parsed.title = dir_title;
}

/// 解析文件名，并用所在目录名修正剧集标题。扫描器应当用这个而不是 `parse_filename`。
pub fn parse_file_in_dir(file_name: &str, dir_name: &str) -> ParsedName {
    let mut p = parse_filename(file_name);
    apply_series_hint(&mut p, dir_name);
    p
}

/// 解析一个文件名（带扩展名或纯文件名均可）。
pub fn parse_filename(name: &str) -> ParsedName {
    let stem = file_stem(name);
    parse_stem(&stem)
}

/// 解析已去掉扩展名的文件名。扫描器拿到目录项后直接调这个。
pub fn parse_stem(stem: &str) -> ParsedName {
    let mut out = ParsedName::default();

    // ① 剥离方括号标签，内容留作后续扫描（动漫的编码信息常在这里）
    let mut tags: Vec<String> = Vec::new();
    let mut work = String::new();
    let mut last = 0usize;
    for m in RE_BRACKET.find_iter(stem) {
        work.push_str(&stem[last..m.start()]);
        tags.push(stem[m.start() + 1..m.end() - 1].to_string());
        last = m.end();
    }
    work.push_str(&stem[last..]);
    work = work.trim().to_string();

    // ② 季 / 集
    if let Some(c) = RE_SE.captures(&work) {
        out.season = cap_i32(&c, "s");
        out.episode = cap_i32(&c, "e");
        out.episode_end = cap_i32(&c, "e2");
        work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
    } else if let Some(c) = RE_SEASON_EPISODE.captures(&work) {
        out.season = cap_i32(&c, "s");
        out.episode = cap_i32(&c, "e");
        work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
    } else if let Some(c) = RE_X.captures(&work) {
        out.season = cap_i32(&c, "s");
        out.episode = cap_i32(&c, "e");
        work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
    } else if let Some(c) = RE_CN_EPISODE.captures(&work) {
        out.episode = cap_i32(&c, "e");
        work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
    } else if let Some(c) = RE_EPISODE_ONLY.captures(&work) {
        // 只在没有季号的场合才认"纯集号"，避免 `EP` 误伤。
        out.episode = cap_i32(&c, "e");
        work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
    } else if let Some(c) = RE_ANIME_EPISODE.captures(&work) {
        out.episode = cap_i32(&c, "e");
        work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
    } else if let Some(c) = RE_SEASON_ONLY.captures(&work) {
        out.season = cap_i32(&c, "s");
        work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
    }

    // ③ 年份
    if let Some((start, end, year)) = pick_year(&work) {
        out.year = Some(year);
        work = remove_range(&work, start, end);
    }

    // ③-b 动漫绝对集号。放在年份之后，避免 `Movie.2019` 被误判。
    if out.episode.is_none() && out.season.is_none() {
        if let Some(c) = RE_TRAILING_NUMBER.captures(&work) {
            out.episode = cap_i32(&c, "e");
            work = remove_range(&work, c.get(0).unwrap().start(), c.get(0).unwrap().end());
        }
    }

    // ④ 发布组（结尾 `-XXX`）
    if let Some(c) = RE_GROUP.captures(&work) {
        let m = c.get(0).unwrap();
        out.release_group = c.name("g").map(|g| g.as_str().to_string());
        work = remove_range(&work, m.start(), m.end());
    }

    // ⑤ 发行信息（标题到这里为止）
    let (title_part, info_part) = split_title(&work);
    let scan = format!("{title_part} {info_part} {}", tags.join(" "));

    out.resolution = pick_resolution(&scan);
    out.source = first_cap(&RE_SOURCE, &scan).map(normalize_source);
    out.video_codec = pick_video_codec(&scan);
    out.audio_codec = first_cap(&RE_AUDIO_CODEC, &scan).map(normalize_audio);
    out.hdr = pick_hdr(&scan);
    out.bit_depth = RE_BIT_DEPTH
        .captures(&scan)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
        .and_then(parse_bit_depth);

    // ⑥ 标题
    out.title = clean_title(&strip_empty_brackets(&title_part));
    if out.title.is_empty() {
        out.title = clean_title(&work);
    }
    if out.release_group.is_none() {
        out.release_group = guess_group_from_tags(&tags);
    }

    out
}

// ─────────────────────────────────────────────────────────────
// 内部工具
// ─────────────────────────────────────────────────────────────

fn file_stem(name: &str) -> String {
    let p = std::path::Path::new(name);
    p.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string())
}

fn cap_i32(c: &regex::Captures, name: &str) -> Option<i32> {
    c.name(name).and_then(|m| m.as_str().parse().ok())
}

fn remove_range(s: &str, start: usize, end: usize) -> String {
    let mut out = String::with_capacity(s.len());
    out.push_str(&s[..start]);
    out.push(' ');
    out.push_str(&s[end..]);
    out
}

fn first_cap(re: &Regex, s: &str) -> Option<String> {
    re.captures(s)
        .and_then(|c| c.get(1).map(|m| m.as_str().to_string()))
}

/// 年份候选取**最后一个**。
///
/// 反例驱动的结论：`1917 (2019)`、`Blade Runner 2049 (2017)`、`2012 (2009)`
/// —— 片名里的数字几乎总在前面，真实年份总在后面。取第一个会全错。
fn pick_year(s: &str) -> Option<(usize, usize, i32)> {
    let mut hit: Option<(usize, usize, i32)> = None;
    for c in RE_YEAR.captures_iter(s) {
        let m = c.get(1)?;
        if let Ok(y) = m.as_str().parse::<i32>() {
            if (1900..=2030).contains(&y) {
                hit = Some((m.start(), m.end(), y));
            }
        }
    }
    hit
}

/// 在第一个"发行信息"关键字处切断；左边是标题，右边是发行信息。
fn split_title(work: &str) -> (String, String) {
    match RE_RELEASE_CUT.find(work) {
        // 正则的第一个分组是"分隔符"，所以它不属于标题，直接切在它前面。
        Some(m) => (work[..m.start()].to_string(), work[m.start()..].to_string()),
        None => (work.to_string(), String::new()),
    }
}

fn clean_title(s: &str) -> String {
    s.chars()
        .map(|c| if matches!(c, '.' | '_' | '+') { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|c: char| matches!(c, '-' | ' ' | '.' | '_' | '+' | '(' | '['))
        .to_string()
}

fn normalize_source(s: String) -> String {
    let l = s.to_lowercase();
    match l.as_str() {
        "blu-ray" | "bluray" | "bdrip" | "brrip" | "bd" | "bdremux" => "BluRay".into(),
        "web-dl" | "webdl" => "WEB-DL".into(),
        "web-rip" | "webrip" => "WEBRip".into(),
        "web" => "WEB".into(),
        "dvd" | "dvdrip" => "DVD".into(),
        "hddvd" => "HDDVD".into(),
        "hdtv" => "HDTV".into(),
        "cam" => "CAM".into(),
        other => other.to_uppercase(),
    }
}

/// 一个名字里可能同时出现 `HEVC` 和 `x265`（动漫发布组最爱这么写）。
/// `x264/x265` 指名了具体编码器，比 `HEVC/AVC` 这种大类更有信息量，优先取它。
/// `10bit` / `10-bit` / `hi10p` → 10。取匹配串开头的数字，别直接 parse 整个串。
fn parse_bit_depth(s: String) -> Option<i32> {
    if s.eq_ignore_ascii_case("hi10p") {
        return Some(10);
    }
    s.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// `A.Movie.4K.2160p` 里两个都能命中；`2160p` 是精确值，`4K` 是俗称，取前者。
fn pick_resolution(s: &str) -> Option<String> {
    let all: Vec<String> = RE_RESOLUTION
        .captures_iter(s)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_lowercase()))
        .collect();
    all.iter()
        .find(|r| r.ends_with('p') || r.ends_with('i'))
        .or_else(|| all.first())
        .cloned()
}

fn pick_video_codec(s: &str) -> Option<String> {
    let found: Vec<String> = RE_VIDEO_CODEC
        .captures_iter(s)
        .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
        .collect();
    if found.is_empty() {
        return None;
    }
    found
        .iter()
        .find(|c| {
            let l = c.to_lowercase();
            l == "x264" || l == "x265"
        })
        .or_else(|| found.first())
        .map(|c| normalize_codec(c.clone()))
}

fn normalize_codec(s: String) -> String {
    let l = s.to_lowercase();
    match l.as_str() {
        "h264" | "h.264" | "avc" => "H.264".into(),
        "h265" | "h.265" | "hevc" => "HEVC".into(),
        "x264" => "x264".into(),
        "x265" => "x265".into(),
        "mpeg2" | "mpeg-2" => "MPEG-2".into(),
        "vc1" | "vc-1" => "VC-1".into(),
        other => other.to_uppercase(),
    }
}

fn normalize_audio(s: String) -> String {
    let l = s.to_lowercase();
    match l.as_str() {
        "truehd" | "truehd.atmos" => "TrueHD".into(),
        "atmos" => "Atmos".into(),
        "dtshd" | "dts-hd" | "dtshd.ma" | "dts-hd.ma" => "DTS-HD".into(),
        "dtses" | "dts-es" => "DTS-ES".into(),
        "dts" => "DTS".into(),
        "ddp" | "dd+" | "eac3" => "EAC3".into(),
        "ac3" => "AC3".into(),
        "aac" => "AAC".into(),
        "flac" => "FLAC".into(),
        "opus" => "Opus".into(),
        "mp3" => "MP3".into(),
        "lpcm" | "pcm" => "PCM".into(),
        other => other.to_uppercase(),
    }
}

fn pick_hdr(s: &str) -> Option<HdrType> {
    if let Some(m) = RE_HDR.find(s) {
        return Some(match m.as_str().to_lowercase().as_str() {
            "hdr10+" => HdrType::Hdr10Plus,
            "hdr10" => HdrType::Hdr10,
            "hlg" => HdrType::Hlg,
            "dolbyvision" | "dolby vision" => HdrType::DolbyVision,
            _ => HdrType::Hdr10,
        });
    }
    if RE_DV.is_match(s) {
        return Some(HdrType::DolbyVision);
    }
    if RE_SDR.is_match(s) {
        return Some(HdrType::Sdr);
    }
    None
}

/// 动漫场景：发布组写在方括号里（`[SubsPlease]`），且它不像编码信息。
fn guess_group_from_tags(tags: &[String]) -> Option<String> {
    tags.iter()
        .find(|t| {
            !t.contains(' ')
                && !t.is_empty()
                && !RE_VIDEO_CODEC.is_match(t)
                && !RE_RESOLUTION.is_match(t)
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_movie() {
        let p = parse_filename("The.Matrix.1999.1080p.BluRay.x264.DTS-HD.MA.5.1-FGT.mkv");
        assert_eq!(p.title, "The Matrix");
        assert_eq!(p.year, Some(1999));
        assert_eq!(p.resolution.as_deref(), Some("1080p"));
        assert_eq!(p.source.as_deref(), Some("BluRay"));
        assert_eq!(p.video_codec.as_deref(), Some("x264"));
        assert_eq!(p.release_group.as_deref(), Some("FGT"));
        assert!(!p.is_episode());
    }

    #[test]
    fn scene_episode() {
        let p = parse_filename("Breaking.Bad.S01E01.1080p.BluRay.x265-RARBG.mkv");
        assert_eq!(p.title, "Breaking Bad");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(1));
    }

    #[test]
    fn multi_episode_range() {
        let p = parse_filename("Some.Show.S02E05-E07.1080p.WEB-DL.mkv");
        assert_eq!(p.episode, Some(5));
        assert_eq!(p.episode_end, Some(7));
    }

    #[test]
    fn year_in_title_does_not_win() {
        // 取"最后一个"年份候选的直接理由
        let p = parse_filename("1917.2019.1080p.BluRay.x264.mkv");
        assert_eq!(p.year, Some(2019));
        assert_eq!(p.title, "1917");
    }

    #[test]
    fn chinese_episode() {
        let p = parse_filename("我的团长我的团.第01集.mp4");
        assert_eq!(p.episode, Some(1));
        assert!(p.title.contains("我的团长我的团"));
    }

    #[test]
    fn anime_release() {
        let p = parse_filename("[SubsPlease] Frieren - 01 (1080p) [HEVC x265 10bit].mkv");
        assert_eq!(p.episode, Some(1));
        assert_eq!(p.title, "Frieren");
        assert_eq!(p.bit_depth, Some(10));
        assert_eq!(p.video_codec.as_deref(), Some("x265"));
    }

    #[test]
    fn hdr10plus_remux() {
        let p = parse_filename(
            "Movie.Title.2020.2160p.UHD.BluRay.REMUX.HDR10+.TrueHD.Atmos.7.1-DON.mkv",
        );
        assert_eq!(p.year, Some(2020));
        assert_eq!(p.resolution.as_deref(), Some("2160p"));
        assert_eq!(p.hdr, Some(HdrType::Hdr10Plus));
        assert_eq!(p.source.as_deref(), Some("BluRay"));
    }

    #[test]
    fn empty_title_falls_back() {
        // 极端输入也不能 panic 或产生空标题
        let p = parse_filename("1080p.mkv");
        assert!(!p.title.is_empty());
    }
}
