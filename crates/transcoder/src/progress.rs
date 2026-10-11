//! FFmpeg 进度解析。
//!
//! ffmpeg 的进度只写在 stderr 上，而且**换行符是 `\r` 不是 `\n`**
//! （同一行不断被覆写）。直接按行缓冲会永远解析不出东西。
//! 这里把 `\r` 也当分隔符，并保留最后一段不完整的残行。

/// 一次进度采样。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub position_ms: i64,
    /// `1.2x` 里的 1.2。实时倍速是判断"能不能追得上播放"的关键指标。
    pub speed: Option<f64>,
    /// 0~1。有总时长时才有值。
    pub percent: Option<f32>,
}

/// 增量式解析器。喂 stderr 分片，吐出这一片里能解析出的所有进度点。
#[derive(Debug, Default)]
pub struct ProgressParser {
    buf: String,
    duration_ms: Option<i64>,
}

impl ProgressParser {
    pub fn new(duration_ms: Option<i64>) -> Self {
        Self {
            buf: String::new(),
            duration_ms,
        }
    }

    /// 喂入一段 stderr，返回这一段里解析出的进度。
    pub fn feed(&mut self, chunk: &str) -> Vec<Progress> {
        self.buf.push_str(chunk);

        // 最后一个分隔符之后的内容可能还不完整，留在 buffer 里等下一片。
        let last_sep = self
            .buf
            .char_indices()
            .rev()
            .find(|(_, c)| *c == '\n' || *c == '\r')
            .map(|(i, _)| i);

        let (complete, rest) = match last_sep {
            Some(i) => {
                let complete: String = self.buf[..i].replace('\r', "\n");
                let rest = self.buf[i + 1..].to_string();
                (complete, rest)
            }
            None => (String::new(), self.buf.clone()),
        };
        self.buf = rest;

        complete
            .lines()
            .filter_map(|line| parse_line(line, self.duration_ms))
            .collect()
    }
}

/// 从一行 stderr 里抠出进度。
///
/// 同时认两种写法：
///   · 默认统计行：`frame= 123 fps=45 time=00:01:23.45 bitrate=... speed=1.2x`
///   · `-progress` 输出：`out_time_ms=83450000` / `out_time_us=83450000`
pub fn parse_line(line: &str, duration_ms: Option<i64>) -> Option<Progress> {
    let position_ms = parse_time_field(line)
        .or_else(|| parse_numeric_field(line, "out_time_ms="))
        .or_else(|| parse_numeric_field(line, "out_time_us=").map(|v| v / 1000))?;

    let speed = parse_speed(line);
    let percent = duration_ms
        .filter(|d| *d > 0)
        .map(|d| (position_ms as f64 / d as f64).clamp(0.0, 1.0) as f32);

    Some(Progress {
        position_ms,
        speed,
        percent,
    })
}

fn parse_time_field(line: &str) -> Option<i64> {
    let rest = line.split_once("time=")?.1;
    let token = rest.split_whitespace().next()?;
    // ffmpeg 在还没产出第一帧时会写 `time=N/A`。
    if token == "N/A" {
        return None;
    }
    timestamp_to_ms(token)
}

/// `00:01:23.45` / `01:23.45` → 毫秒。
///
/// 与字幕时间码唯一的区别是 ffmpeg 也可能给出 `N/A`，这里只处理数字形式。
pub(crate) fn timestamp_to_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    let (hms, frac) = match s.split_once('.') {
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
    let ms = match frac {
        Some(f) => {
            let digits: String = f.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 3 {
                digits[..3].parse::<i64>().unwrap_or(0)
            } else if digits.is_empty() {
                0
            } else {
                digits.parse::<i64>().unwrap_or(0) * 10_i64.pow(3 - digits.len() as u32)
            }
        }
        None => 0,
    };
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms)
}

fn parse_numeric_field(line: &str, key: &str) -> Option<i64> {
    let rest = line.split_once(key)?.1;
    let token = rest.split_whitespace().next()?;
    token.parse::<i64>().ok()
}

fn parse_speed(line: &str) -> Option<f64> {
    let rest = line.split_once("speed=")?.1;
    let token = rest.split_whitespace().next()?;
    token.trim_end_matches('x').parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default_stats_line() {
        let p = parse_line(
            "frame= 1234 fps=48 q=28.0 size=   10240kB time=00:01:23.45 bitrate=1000.0kbits/s speed=1.23x",
            Some(600_000),
        )
        .unwrap();
        assert_eq!(p.position_ms, 83_450);
        assert_eq!(p.speed, Some(1.23));
        assert!((p.percent.unwrap() - 0.139).abs() < 0.001);
    }

    #[test]
    fn carriage_returns_are_line_separators() {
        // ffmpeg 覆写同一行：分片喂进来时必须能在 \r 处切开。
        let mut p = ProgressParser::new(None);
        let a =
            p.feed("frame=1 time=00:00:01.00 speed=1.0x\rframe=2 time=00:00:02.00 speed=1.1x\r");
        assert_eq!(a.len(), 2);
        assert_eq!(a[1].position_ms, 2000);
    }

    #[test]
    fn partial_lines_are_buffered() {
        let mut p = ProgressParser::new(None);
        assert!(p.feed("frame=1 time=00:00:01").is_empty());
        let a = p.feed(".00 speed=1.0x\n");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].position_ms, 1000);
    }

    #[test]
    fn na_time_is_not_progress() {
        assert!(parse_line("frame=0 time=N/A speed=N/A", None).is_none());
    }

    #[test]
    fn progress_style_fields_work() {
        let p = parse_line("out_time_us=83450000\nspeed=2.0x", None).unwrap();
        assert_eq!(p.position_ms, 83_450);
        let p2 = parse_line("out_time_ms=83450", None).unwrap();
        assert_eq!(p2.position_ms, 83_450);
    }

    #[test]
    fn percent_is_clamped() {
        let p = parse_line("time=00:10:00.00", Some(60_000)).unwrap();
        assert_eq!(p.percent, Some(1.0));
        assert!(parse_line("time=00:01:00.00", None)
            .unwrap()
            .percent
            .is_none());
    }
}
