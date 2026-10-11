//! 字幕的中间表示与四种文本格式互转。
//!
//! **为什么要有一个中间表示**：SRT / ASS / VTT / 内嵌轨 之间两两转换要写 12 个函数，
//! 有了 `Cue` 之后只需要"各格式 → Cue"与"Cue → 各格式"共 8 个。
//!
//! 代价是 ASS 的样式（字体、位置、卡拉OK）会丢。这是**故意的**：
//! 我们不做 OCR，也不做样式保真，转换的目标是"字幕文字能看懂"。

use std::fmt::Write;

/// 一条字幕。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub start_ms: i64,
    pub end_ms: i64,
    /// 多行文本，行内以 `\n` 分隔。
    pub text: String,
}

impl Cue {
    pub fn new(start_ms: i64, end_ms: i64, text: impl Into<String>) -> Self {
        Self {
            start_ms,
            end_ms,
            text: text.into(),
        }
    }

    /// 丢弃"结束早于开始"的坏字幕。损坏的字幕文件里这种行很常见，
    /// 整文件解析失败比丢几条坏行糟糕得多。
    pub fn is_valid(&self) -> bool {
        self.end_ms >= self.start_ms && self.start_ms >= 0 && !self.text.trim().is_empty()
    }
}

/// 统一换行并去掉 BOM。
///
/// 字幕文件的换行是**三种混着来的**：Unix 的 LF、Windows 的 CRLF、老 Mac 的 CR。
/// 不先归一化，按空行切块在 CRLF 上根本切不开 —— 整个文件会被当成一大块。
fn normalize_newlines(input: &str) -> String {
    input
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

// ─────────────────────────────────────────────────────────────
// 时间码
// ─────────────────────────────────────────────────────────────

/// 解析 `HH:MM:SS,mmm` / `HH:MM:SS.mmm` / `MM:SS.mmm` / `H:MM:SS.cc`（ASS）。
///
/// ASS 的小数位是**百分秒**而不是毫秒，这里统一按两位处理。
pub fn parse_timestamp(s: &str) -> Option<i64> {
    let s = s.trim();
    let (hms, frac) = match s.split_once(['.', ',']) {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };

    let parts: Vec<&str> = hms.split(':').collect();
    let (h, m, sec) = match parts.len() {
        3 => (
            parts[0].parse::<i64>().ok()?,
            parts[1].parse::<i64>().ok()?,
            parts[2].parse::<i64>().ok()?,
        ),
        2 => (
            0,
            parts[0].parse::<i64>().ok()?,
            parts[1].parse::<i64>().ok()?,
        ),
        1 => (0, 0, parts[0].parse::<i64>().ok()?),
        _ => return None,
    };

    // 小数位补齐到毫秒：`.5` → 500ms，`.25` → 250ms，`.123` → 123ms。
    let ms = match frac {
        Some(f) => {
            let digits: String = f.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                0
            } else if digits.len() >= 3 {
                digits[..3].parse::<i64>().unwrap_or(0)
            } else {
                let v: i64 = digits.parse().unwrap_or(0);
                v * 10_i64.pow(3 - digits.len() as u32)
            }
        }
        None => 0,
    };

    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

/// 毫秒 → `HH:MM:SS,mmm`（SRT 用逗号）。
pub fn format_srt_time(ms: i64) -> String {
    format_time(ms, ",")
}

/// 毫秒 → `HH:MM:SS.mmm`（VTT 用点）。
pub fn format_vtt_time(ms: i64) -> String {
    format_time(ms, ".")
}

fn format_time(ms: i64, sep: &str) -> String {
    let ms = ms.max(0);
    let h = ms / 3_600_000;
    let m = (ms % 3_600_000) / 60_000;
    let s = (ms % 60_000) / 1000;
    let rest = ms % 1000;
    format!("{h:02}:{m:02}:{s:02}{sep}{rest:03}")
}

// ─────────────────────────────────────────────────────────────
// SRT
// ─────────────────────────────────────────────────────────────

/// 解析 SRT。容错优先：坏块跳过而不是整文件失败。
pub fn parse_srt(input: &str) -> Vec<Cue> {
    let input = normalize_newlines(input);
    let mut cues = Vec::new();

    for block in input.split("\n\n") {
        let mut lines = block
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.trim().is_empty());
        let Some(first) = lines.next() else { continue };

        // 第一行可能是序号，也可能直接就是时间行（有些生成器不写序号）。
        let time_line = if first.contains("-->") {
            first
        } else {
            match lines.next() {
                Some(l) if l.contains("-->") => l,
                _ => continue,
            }
        };

        let Some((start_s, end_s)) = time_line.split_once("-->") else {
            continue;
        };
        let Some(start) = parse_timestamp(start_s) else {
            continue;
        };
        // 结束时间缺失时给一个 2 秒的默认时长，比丢掉整条好。
        let end = parse_timestamp(end_s).unwrap_or(start + 2000);

        let text: Vec<String> = lines.collect();
        if text.is_empty() {
            continue;
        }
        let cue = Cue::new(start, end, text.join("\n"));
        if cue.is_valid() {
            cues.push(cue);
        }
    }
    cues
}

/// 序列化成 SRT。
pub fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (i, c) in cues.iter().enumerate() {
        let _ = writeln!(out, "{}", i + 1);
        let _ = writeln!(
            out,
            "{} --> {}",
            format_srt_time(c.start_ms),
            format_srt_time(c.end_ms)
        );
        let _ = writeln!(out, "{}", c.text);
        let _ = writeln!(out);
    }
    out
}

// ─────────────────────────────────────────────────────────────
// WebVTT
// ─────────────────────────────────────────────────────────────

/// 解析 WebVTT。要跳过 `NOTE` / `STYLE` / `REGION` 块。
pub fn parse_vtt(input: &str) -> Vec<Cue> {
    let input = normalize_newlines(input);
    let mut cues = Vec::new();

    for block in input.split("\n\n") {
        let lines: Vec<&str> = block
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.trim().is_empty())
            .collect();
        if lines.is_empty() {
            continue;
        }
        // 头部与注释块：整块跳过。
        if lines[0].starts_with("WEBVTT")
            || lines[0].starts_with("NOTE")
            || lines[0].starts_with("STYLE")
            || lines[0].starts_with("REGION")
        {
            continue;
        }

        let mut idx = 0;
        // 可选的 cue id（不含 `-->` 的标识行）。
        if !lines[0].contains("-->") {
            idx = 1;
            if lines.len() < 2 {
                continue;
            }
        }
        let time_line = lines[idx];
        let Some((start_s, end_s)) = time_line.split_once("-->") else {
            continue;
        };
        // VTT 时间行后面可能带 `align:start position:10%` 之类的设置。
        let end_s = end_s.split_whitespace().next().unwrap_or("");
        let Some(start) = parse_timestamp(start_s) else {
            continue;
        };
        let end = parse_timestamp(end_s).unwrap_or(start + 2000);

        let text = lines[idx + 1..].join("\n");
        let cue = Cue::new(start, end, text);
        if cue.is_valid() {
            cues.push(cue);
        }
    }
    cues
}

/// 序列化成 WebVTT。HLS 的字幕轨只认 VTT。
pub fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for (i, c) in cues.iter().enumerate() {
        let _ = writeln!(out, "{}", i + 1);
        let _ = writeln!(
            out,
            "{} --> {}",
            format_vtt_time(c.start_ms),
            format_vtt_time(c.end_ms)
        );
        let _ = writeln!(out, "{}", c.text);
        let _ = writeln!(out);
    }
    out
}

// ─────────────────────────────────────────────────────────────
// ASS / SSA
// ─────────────────────────────────────────────────────────────

/// 解析 ASS / SSA 的 Dialogue 行。
///
/// 只取 `Start / End / Text` 三列，其余（样式、边距、特效）丢弃。
/// `Format:` 行决定了列顺序，不同生成器的列顺序是不一样的。
pub fn parse_ass(input: &str) -> Vec<Cue> {
    let input = normalize_newlines(input);
    let mut start_col = 1usize;
    let mut end_col = 2usize;
    let mut text_col = 9usize;
    let mut in_events = false;
    let mut cues = Vec::new();

    for raw in input.lines() {
        let line = raw.trim_end_matches('\r').trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with("!") {
            continue;
        }
        if line.starts_with('[') {
            in_events = line.eq_ignore_ascii_case("[Events]");
            continue;
        }
        if !in_events {
            continue;
        }

        if let Some(rest) = line
            .strip_prefix("Format:")
            .or_else(|| line.strip_prefix("format:"))
        {
            let cols: Vec<String> = rest
                .split(',')
                .map(|c| c.trim().to_ascii_lowercase())
                .collect();
            if let Some(i) = cols.iter().position(|c| c == "start") {
                start_col = i;
            }
            if let Some(i) = cols.iter().position(|c| c.as_str() == "end") {
                end_col = i;
            }
            if let Some(i) = cols.iter().position(|c| c.as_str() == "text") {
                text_col = i;
            }
            continue;
        }

        let Some(rest) = line
            .strip_prefix("Dialogue:")
            .or_else(|| line.strip_prefix("dialogue:"))
            .map(|r| r.trim_start())
        else {
            continue;
        };

        // 逗号分隔，但**最后一列里允许有逗号**，所以必须限制切分次数：
        // 切成 (最大列号 + 1) 段，最后一段吞掉剩余内容。
        // 注意 Text 未必是最后一列（有些生成器把它放最前面），
        // 所以这里的次数取三列的最大值，而不是固定 9。
        let n = start_col.max(end_col).max(text_col);
        let parts: Vec<&str> = rest.splitn(n + 1, ',').collect();

        let get = |i: usize| parts.get(i).copied().unwrap_or("").trim();
        let Some(start) = parse_timestamp(get(start_col)) else {
            continue;
        };
        let end = parse_timestamp(get(end_col)).unwrap_or(start + 2000);
        let text = clean_ass_text(get(text_col));
        let cue = Cue::new(start, end, text);
        if cue.is_valid() {
            cues.push(cue);
        }
    }
    cues
}

/// 清掉 ASS 的样式覆盖与排版指令，只留下能读的字。
fn clean_ass_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_override = false;
    let mut chars = raw.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '{' => in_override = true,
            '}' => in_override = false,
            '\\' => {
                // \N \n → 换行；\h → 空格；其余转义指令整条跳过。
                match chars.peek() {
                    Some('N') | Some('n') => {
                        chars.next();
                        out.push('\n');
                    }
                    Some('h') => {
                        chars.next();
                        out.push(' ');
                    }
                    _ => {
                        // 跳过 `\xxx(...)` 指令名，直到遇到空白、下一个指令或覆盖块结束。
                        while let Some(&n) = chars.peek() {
                            if n.is_whitespace() || n == '\\' || n == '{' || n == '}' {
                                break;
                            }
                            chars.next();
                        }
                    }
                }
            }
            _ if !in_override => out.push(c),
            _ => {}
        }
    }

    // 折叠多余空行，避免转换出一堆空白字幕。
    let cleaned: Vec<&str> = out
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();
    cleaned.join("\n")
}

/// 把任意文本字幕统一转成 SRT。目标是"服务端外挂投递"这一条路径。
pub fn to_srt_from(cues: &[Cue]) -> String {
    to_srt(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\r\n00:00:01,000 --> 00:00:04,000\r\nHello\r\nworld\r\n\r\n2\r\n00:00:05,500 --> 00:00:07,000\r\nBye\r\n";

    #[test]
    fn srt_roundtrip() {
        let cues = parse_srt(SRT);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[0].end_ms, 4000);
        assert_eq!(cues[0].text, "Hello\nworld");
        assert_eq!(cues[1].start_ms, 5500);

        let out = to_srt(&cues);
        assert!(out.contains("00:00:01,000 --> 00:00:04,000"));
        let again = parse_srt(&out);
        assert_eq!(again, cues);
    }

    #[test]
    fn srt_without_index_line_still_parses() {
        let cues = parse_srt("00:00:01,000 --> 00:00:02,000\nHi\n");
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "Hi");
    }

    #[test]
    fn srt_missing_end_time_gets_default_duration() {
        // 有些生成器只写开始时间。丢掉整条不如给个 2 秒。
        let cues = parse_srt("1\n00:00:03,000 -->\nHi\n");
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].end_ms, 5000);
    }

    #[test]
    fn srt_broken_blocks_are_skipped() {
        let cues =
            parse_srt("garbage\n\n1\nnot-a-time\ntext\n\n1\n00:00:01,000 --> 00:00:02,000\nok\n");
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "ok");
    }

    #[test]
    fn bom_is_tolerated() {
        let cues = parse_srt("\u{feff}1\n00:00:01,000 --> 00:00:02,000\nok\n");
        assert_eq!(cues.len(), 1);
    }

    #[test]
    fn vtt_parses_and_skips_notes() {
        let vtt = "WEBVTT\n\nNOTE this is a note\n\n1\n00:00:01.000 --> 00:00:03.000 align:start\nHi\n\n2\n00:00:04.000 --> 00:00:05.000\nBye\n";
        let cues = parse_vtt(vtt);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[0].text, "Hi");
        assert_eq!(cues[1].start_ms, 4000);
    }

    #[test]
    fn vtt_roundtrip_uses_dot_separator() {
        let cues = vec![Cue::new(61_000, 63_500, "one minute")];
        let out = to_vtt(&cues);
        assert!(out.starts_with("WEBVTT"));
        assert!(out.contains("00:01:01.000 --> 00:01:03.500"));
        assert_eq!(parse_vtt(&out), cues);
    }

    #[test]
    fn srt_to_vtt_conversion() {
        let cues = parse_srt(SRT);
        let vtt = to_vtt(&cues);
        assert!(vtt.contains("00:00:05.500"));
        assert!(!vtt.contains("00:00:05,500"));
    }

    const ASS: &str = r#"[Script Info]
Title: x

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
Dialogue: 0,0:00:01.00,0:00:04.50,Default,,0,0,0,,Hello\Nworld
Dialogue: 0,0:00:06.00,0:00:08.00,Default,,0,0,0,,{\i1}italic{\i0} and {\pos(1,2)}placed
"#;

    #[test]
    fn ass_dialogue_lines_become_cues() {
        let cues = parse_ass(ASS);
        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].start_ms, 1000);
        // ASS 的 `.00` 是百分秒 → 4.50s = 4500ms
        assert_eq!(cues[0].end_ms, 4500);
        assert_eq!(cues[0].text, "Hello\nworld");
    }

    #[test]
    fn ass_style_overrides_are_stripped() {
        let cues = parse_ass(ASS);
        assert_eq!(cues[1].text, "italic and placed");
    }

    #[test]
    fn ass_text_with_commas_is_not_split() {
        // Text 是最后一列，里面可以随便带逗号。
        let ass = "[Events]\nFormat: Start, End, Text\nDialogue: 0:00:01.00,0:00:02.00,a,b,c\n";
        let cues = parse_ass(ass);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].text, "a,b,c");
    }

    #[test]
    fn ass_custom_column_order_is_respected() {
        let ass = "[Events]\nFormat: Text, Start, End\nDialogue: hi,0:00:01.00,0:00:02.00\n";
        let cues = parse_ass(ass);
        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start_ms, 1000);
        assert_eq!(cues[0].text, "hi");
    }

    #[test]
    fn ass_h_becomes_space() {
        let ass = "[Events]\nFormat: Start, End, Text\nDialogue: 0:00:01.00,0:00:02.00,a\\hb\n";
        assert_eq!(parse_ass(ass)[0].text, "a b");
    }

    #[test]
    fn timestamp_edge_cases() {
        assert_eq!(parse_timestamp("00:00:01,000"), Some(1000));
        assert_eq!(parse_timestamp("01:02:03.250"), Some(3_723_250));
        assert_eq!(parse_timestamp("02:03.5"), Some(123_500));
        assert_eq!(parse_timestamp("0:00:01.00"), Some(1000));
        assert_eq!(parse_timestamp("nope"), None);
    }

    #[test]
    fn formatting_clamps_negative() {
        assert_eq!(format_srt_time(-5), "00:00:00,000");
        assert_eq!(format_vtt_time(3_661_007), "01:01:01.007");
    }

    #[test]
    fn invalid_cues_are_filtered() {
        assert!(!Cue::new(5000, 1000, "x").is_valid());
        assert!(!Cue::new(0, 1000, "   ").is_valid());
        assert!(Cue::new(0, 1000, "x").is_valid());
    }
}
