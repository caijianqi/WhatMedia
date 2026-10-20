//! 媒体文件"读懂"层：文件名解析 / ffprobe 探测 / NFO 读取。
//!
//! 本 crate 不碰数据库，只把"一个文件"变成"结构化信息"。
//! 入库是 `whatmedia-library` 的事。

pub mod naming;
pub mod nfo;
pub mod probe;

pub use naming::{
    apply_series_hint, extract_path_hint, parse_file_in_dir, parse_filename, parse_season_dir,
    parse_stem, ParsedName, PathHint,
};
pub use nfo::{parse_nfo, NfoEpisode, NfoKind, NfoMovie, NfoSeries};
pub use probe::{parse_probe_json, FfprobeRunner, ProbeResult, ProbeStream, DEFAULT_TIMEOUT};

/// 视频扩展名白名单。扫描器用它过滤，避免把字幕/图片/NFO 当媒体入库。
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "mov", "avi", "wmv", "flv", "webm", "mpg", "mpeg", "m2ts", "ts", "rmvb",
    "rm", "vob", "iso", "ogm", "divx", "xvid",
];

pub fn is_video_file(name: &str) -> bool {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}
