//! 启动参数构造。
//!
//! **参数顺序即正确性**，这里每一条都对应一个真实的踩坑点。

use serde_json::json;
use std::path::{Path, PathBuf};

use crate::ipc::IpcPath;

/// 要挂上去的字幕。外挂字幕必须在启动参数里给，
/// 启动后再 `sub-add` 会有一段"字幕还没出来"的空窗。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleAttach {
    pub path: PathBuf,
    /// 语言标签，用于 mpv 的 `sid` 自动选择。
    pub lang: Option<String>,
    pub forced: bool,
}

/// 一次启动请求的全部输入。用结构体而不是 7 个位置参数 ——
/// `build_args(url, None, false, 0.0, None, false)` 没人看得懂。
#[derive(Debug, Clone)]
pub struct LaunchRequest {
    /// 播放地址。可以是本地路径，也可以是 http(s)。
    pub url: String,
    /// 续播起点（秒）。0 表示从头。
    pub start_seconds: f64,
    pub ipc: IpcPath,
    pub subtitles: Vec<SubtitleAttach>,
    /// 硬件解码模式。None 表示让 mpv 自己决定（等价于不传 --hwdec）。
    pub hwdec: Option<String>,
    /// 是否开独立窗口。桌面端总是开；这个开关留给无界面场景（比如只取信息）。
    pub window: bool,
    /// 窗口标题。多开时能分清哪个是哪个。
    pub title: Option<String>,
}

/// 构造 mpv 的 argv。
///
/// 顺序要点：
///   · `--input-ipc-server` 必须在**启动时**给，事后改不了；
///   · `--sub-file` 也必须在启动时给（见 `SubtitleAttach` 的注释）；
///   · `--start` 是启动参数，不是 IPC 命令 —— 用它而不是起来后再 seek，
///     能省掉一次解码器重启。
pub fn build_args(req: &LaunchRequest) -> Vec<String> {
    let mut a = Vec::new();

    // 无终端：我们是 GUI 应用，mpv 往 stdout 打进度条只会污染日志。
    a.push("--no-terminal".to_string());
    a.push("--idle".to_string());
    // 不让它自己退出：进程活着才能让 IPC 控制。播完停在最后一帧。
    a.push("--keep-open=always".to_string());

    a.push(format!("--input-ipc-server={}", req.ipc.as_mpv_arg()));

    if let Some(hw) = &req.hwdec {
        if !hw.is_empty() {
            a.push(format!("--hwdec={hw}"));
        }
    }

    if req.window {
        a.push("--force-window=yes".to_string());
    }
    if let Some(t) = &req.title {
        if !t.is_empty() {
            a.push(format!("--title={t}"));
        }
    }

    for s in &req.subtitles {
        a.push(format!("--sub-file={}", s.path.display()));
        // 外挂字幕默认不选中（--sub-auto 只管同目录自动发现），
        // 这里显式带上语言，让 mpv 的 sid 选择能命中它。
        if let Some(l) = &s.lang {
            if !l.is_empty() {
                a.push(format!("--sub-file-lang={l}"));
            }
        }
        // forced 字幕（只有外语对白的部分字幕）不该自动显示。
        if s.forced {
            a.push("--sub-forced-only".to_string());
        }
    }

    if req.start_seconds > 0.0 {
        a.push(format!("--start={:.3}", req.start_seconds));
    }

    a.push(req.url.clone());
    a
}

/// 把一次"设置属性"变成 IPC 命令文本。
///
/// 单独抽出来是因为 UI 里 90% 的控制都是 set_property，
/// 没必要每次都手写 `json!` 数组。
pub fn set_property_cmd(name: &str, value: serde_json::Value) -> String {
    crate::ipc::encode_command(&[json!("set_property"), json!(name), value])
}

/// 取一次属性值。返回的是**异步命令**，需要 request_id 才能把响应对上。
pub fn get_property_cmd(name: &str, request_id: u32) -> String {
    crate::ipc::encode_async(&[json!("get_property"), json!(name)], request_id)
}

/// seek。`absolute+exact`：进度条拖到哪就是哪。
///
/// 不用默认的 `relative+keyframes` —— 那会让拖动后落在实际关键帧上，
/// 用户拖到 12:34 结果停在 12:29，看起来像 bug。
pub fn seek_cmd(seconds: f64) -> String {
    crate::ipc::encode_command(&[json!("seek"), json!(seconds), json!("absolute+exact")])
}

/// 判断一个路径能不能安全地交给 mpv。
///
/// **这是安全边界**：URL 来自服务端决策结果，也可能是用户手输的。
/// 本地路径不做限制（桌面端的意义就是直读本地文件），
/// 但协议必须白名单 —— 否则 `av://lavfi:...` 这类可以被拿来读任意东西。
pub fn url_allowed(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return true;
    }
    if lower.starts_with('/') || lower.contains(':') && Path::new(&lower).is_absolute() {
        // 绝对路径（含 Windows 的 C:\）
        return true;
    }
    if lower.starts_with("\\\\") {
        // UNC 路径
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcPath;

    fn unix_ipc() -> IpcPath {
        IpcPath::Unix("/tmp/whatmedia-mpv-1.sock".to_string())
    }

    fn base() -> LaunchRequest {
        LaunchRequest {
            url: "http://127.0.0.1:8080/api/v1/stream/abc".to_string(),
            start_seconds: 0.0,
            ipc: unix_ipc(),
            subtitles: vec![],
            hwdec: None,
            window: true,
            title: None,
        }
    }

    #[test]
    fn ipc_server_is_always_present() {
        let a = build_args(&base());
        assert!(
            a.iter()
                .any(|x| x == "--input-ipc-server=/tmp/whatmedia-mpv-1.sock"),
            "IPC 必须在启动时就给：{a:?}"
        );
    }

    #[test]
    fn url_is_the_last_argument() {
        let a = build_args(&base());
        assert_eq!(a.last().unwrap(), "http://127.0.0.1:8080/api/v1/stream/abc");
    }

    #[test]
    fn start_is_only_added_when_resuming() {
        let a = build_args(&base());
        assert!(!a.iter().any(|x| x.starts_with("--start=")));

        let mut r = base();
        r.start_seconds = 612.345;
        let a = build_args(&r);
        assert!(a.iter().any(|x| x == "--start=612.345"), "{a:?}");
    }

    #[test]
    fn hwdec_is_omitted_when_unset() {
        let a = build_args(&base());
        assert!(!a.iter().any(|x| x.starts_with("--hwdec=")));
    }

    #[test]
    fn subtitles_are_attached_at_launch() {
        let mut r = base();
        r.subtitles = vec![SubtitleAttach {
            path: PathBuf::from("D:\\media\\a.srt"),
            lang: Some("chi".to_string()),
            forced: false,
        }];
        let a = build_args(&r);
        assert!(a.iter().any(|x| x == "--sub-file=D:\\media\\a.srt"));
        assert!(a.iter().any(|x| x == "--sub-file-lang=chi"));
        // 不是 forced 就不该开 --sub-forced-only，否则正片字幕全没了。
        assert!(!a.iter().any(|x| x == "--sub-forced-only"));
    }

    #[test]
    fn forced_subtitle_enables_forced_only() {
        let mut r = base();
        r.subtitles = vec![SubtitleAttach {
            path: PathBuf::from("/tmp/a.srt"),
            lang: None,
            forced: true,
        }];
        let a = build_args(&r);
        assert!(a.iter().any(|x| x == "--sub-forced-only"));
    }

    #[test]
    fn windows_ipc_uses_named_pipe_form() {
        let mut r = base();
        r.ipc = IpcPath::Windows("whatmedia-mpv-9".to_string());
        let a = build_args(&r);
        assert!(
            a.iter()
                .any(|x| x == "--input-ipc-server=\\\\.\\pipe\\whatmedia-mpv-9"),
            "{a:?}"
        );
    }

    #[test]
    fn seek_is_absolute_and_exact() {
        let s = seek_cmd(754.2);
        assert!(s.contains("\"seek\""));
        assert!(s.contains("absolute+exact"), "{s}");
    }

    #[test]
    fn get_property_carries_a_top_level_request_id() {
        // request_id 曾经被塞进 command 数组，那样 mpv 会把它当参数，
        // get_property 变成"多参数"，响应永远对不上号 —— 轨道列表就永远读不出来。
        let s = get_property_cmd("track-list", 7);
        assert_eq!(
            s,
            "{\"command\":[\"get_property\",\"track-list\"],\"request_id\":7}\n"
        );
    }

    #[test]
    fn commands_are_newline_terminated() {
        assert!(set_property_cmd("pause", json!(true)).ends_with('\n'));
        assert!(seek_cmd(1.0).ends_with('\n'));
    }

    #[test]
    fn url_whitelist_rejects_lavfi_and_ffmpeg_schemes() {
        assert!(url_allowed("http://x/a.mkv"));
        assert!(url_allowed("https://x/a.mkv"));
        assert!(url_allowed("/media/a.mkv"));
        assert!(url_allowed("C:\\media\\a.mkv"));
        assert!(url_allowed("\\\\NAS\\media\\a.mkv"));
        // 这几个都能让 mpv 去读/解不该读的东西
        assert!(!url_allowed("av://lavfi:color"));
        assert!(!url_allowed("lavfi://testsrc"));
        assert!(!url_allowed("bd://"));
    }

    #[test]
    fn relative_path_is_rejected() {
        // 相对路径会随 mpv 的 cwd 漂移，不接受。
        assert!(!url_allowed("media/a.mkv"));
    }
}
