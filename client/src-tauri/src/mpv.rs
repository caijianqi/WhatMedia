//! mpv 进程桥接。
//!
//! 这一层刻意很薄：启动参数与协议编解码全在 `whatmedia-mpv` 里（那边有测试），
//! 这里只负责"把进程拉起来、把流接上、把事件转发出去"。
//!
//! 用 `std::process` 而不是 tokio：mpv 是个独立 GUI 进程，
//! 不需要异步运行时，加一个 tokio 只为这件事不划算。

use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use whatmedia_mpv::{
    build_args, detect_with_override, ipc_path, LaunchRequest, Os, RealMpvEnv, SubtitleAttach,
};
use whatmedia_mpv::ipc::EventReader;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Runtime};

/// 一次会话的运行时句柄。
struct Session {
    child: std::process::Child,
    /// Unix domain socket 是全双工的，可复用同一个写端；
    /// Windows 同步命名管道在 ReadFile 阻塞时会与同句柄 WriteFile 死锁，因此 Windows 每次发命令使用独立短连接。
    #[cfg(unix)]
    tx: Arc<Mutex<Box<dyn Write + Send>>>,
    cancel: Arc<AtomicBool>,
    ipc: whatmedia_mpv::IpcPath,
}

/// 全局状态。桌面端同一时刻只播一个，所以是 `Option` 而不是 map。
#[derive(Default)]
pub struct MpvState {
    inner: Arc<Mutex<Option<Session>>>,
    next_id: AtomicU32,
}

/// 探测结果。前端据此决定"要不要显示 mpv 选项"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MpvStatus {
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// 不可用时的原因。写进设置页，别让用户自己去猜。
    pub reason: Option<String>,
}

/// 启动参数（来自前端）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartArgs {
    pub url: String,
    #[serde(default)]
    pub start_seconds: f64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub subtitles: Vec<SubtitleArg>,
    #[serde(default)]
    pub hwdec: Option<String>,
    /// 显式指定 mpv 路径。留空则走探测。
    #[serde(default)]
    pub mpv_path: Option<String>,
    /// 目标嵌入窗口句柄（HWND/XID）。若留空，在 Windows 下自动尝试绑定当前 Tauri 主窗口以防止弹窗。
    #[serde(default)]
    pub wid: Option<i64>,
    /// 自定义请求头
    #[serde(default)]
    pub headers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleArg {
    pub path: String,
    #[serde(default)]
    pub lang: Option<String>,
    #[serde(default)]
    pub forced: bool,
}

#[tauri::command]
pub fn mpv_status(mpv_path: Option<String>) -> MpvStatus {
    let env = RealMpvEnv;
    match detect_with_override(&env, mpv_path.as_deref()) {
        Some(b) => MpvStatus {
            available: true,
            path: Some(b.path.to_string_lossy().to_string()),
            version: Some(b.version_line),
            reason: None,
        },
        None => MpvStatus {
            available: false,
            path: None,
            version: None,
            reason: Some(match mpv_path {
                Some(p) => format!("指定路径不可用：{p}"),
                None => "没有找到 mpv（PATH 与常见安装位置都没找到，或版本低于 0.34）".to_string(),
            }),
        },
    }
}

#[tauri::command]
pub async fn mpv_start<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MpvState>,
    args: StartArgs,
) -> Result<String, String> {
    // 安全边界：URL 来自服务端，也可能是用户手输的。
    if !whatmedia_mpv::url_allowed(&args.url) {
        return Err(format!("不允许的播放地址：{}", args.url));
    }

    // 嵌入式无弹窗保障：在 Windows 下先在主/调用线程拿到 HWND，再把阻塞的拉起/等管道动作交给后台线程池，
    // 彻底避免阻塞 WebView2 UI 线程或与 mpv --wid 产生 Win32 消息死锁。
    #[cfg(windows)]
    let target_wid = args.wid.or_else(|| {
        use tauri::Manager;
        app.get_webview_window("main")
            .or_else(|| app.webview_windows().into_values().next())
            .and_then(|w| w.hwnd().ok())
            .map(|h| h.0 as i64)
    });
    #[cfg(not(windows))]
    let target_wid = args.wid;

    let inner = state.inner.clone();
    let id = state.next_id.fetch_add(1, Ordering::SeqCst);

    tauri::async_runtime::spawn_blocking(move || {
        let env = RealMpvEnv;
        let bin = detect_with_override(&env, args.mpv_path.as_deref())
            .ok_or_else(|| "mpv 不可用".to_string())?;

        let ipc = ipc_path(Os::from_target(), id);

        // 上一次没退干净就先退掉。
        stop_session_sync(&inner);

        let req = LaunchRequest {
            url: args.url.clone(),
            start_seconds: args.start_seconds,
            ipc: ipc.clone(),
            subtitles: args
                .subtitles
                .iter()
                .map(|s| SubtitleAttach {
                    path: std::path::PathBuf::from(&s.path),
                    lang: s.lang.clone(),
                    forced: s.forced,
                })
                .collect(),
            hwdec: args.hwdec.clone(),
            window: true,
            title: args.title.clone(),
            wid: target_wid,
            headers: args.headers.clone(),
        };

        let mut argv = build_args(&req);
        let log_path = std::env::temp_dir().join("whatmedia-mpv.log");
        argv.push(format!("--log-file={}", log_path.display()));

        let mut cmd = std::process::Command::new(&bin.path);
        cmd.args(&argv);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let child = cmd.spawn().map_err(|e| format!("启动 mpv 失败：{e}"))?;

        let mut stream = connect_with_retry(&ipc, 40, 100)
            .ok_or_else(|| "mpv 已启动但连不上它的 IPC 通道".to_string())?;

        let observe = [
            whatmedia_mpv::set_property_cmd("pause", serde_json::json!(false)),
            whatmedia_mpv::ipc::observe_properties(
                &["time-pos", "duration", "pause", "eof-reached"],
                1,
            ),
        ]
        .join("");

        // 先在启动事件泵之前把初始 observe 写进订阅通道，避免 Windows 同句柄读写竞争。
        stream
            .write_all(observe.as_bytes())
            .map_err(|e| format!("写初始 IPC 失败：{e}"))?;
        stream
            .flush()
            .map_err(|e| format!("刷新初始 IPC 失败：{e}"))?;

        #[cfg(unix)]
        let tx: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(
            stream.try_clone().map_err(|e| e.to_string())?,
        )));

        let rx = BufReader::new(stream);
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_cancel = cancel.clone();
        let app2 = app.clone();
        std::thread::spawn(move || {
            pump(app2, rx, thread_cancel);
        });

        {
            let mut g = inner.lock().unwrap();
            *g = Some(Session {
                child,
                #[cfg(unix)]
                tx,
                cancel,
                ipc,
            });
        }

        Ok(format!("mpv-{id}"))
    })
    .await
    .map_err(|e| format!("mpv 启动线程异常：{e}"))?
}

/// 发一条原始 IPC 命令。前端传的是命令数组（JSON），
/// 这样加新控制不用改 Rust。
///
/// `request_id` 有值时走异步形式：响应里会回同一个 id，
/// 前端靠它把"取轨道列表"这类请求和对面对上号。
#[tauri::command]
pub async fn mpv_send<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, MpvState>,
    command: Vec<serde_json::Value>,
    request_id: Option<u32>,
) -> Result<(), String> {
    let line = match request_id {
        Some(id) => whatmedia_mpv::encode_async(&command, id),
        None => whatmedia_mpv::encode_command(&command),
    };
    let inner = state.inner.clone();
    tauri::async_runtime::spawn_blocking(move || {
        send_command_sync(&app, &inner, &line, request_id)
    })
    .await
    .map_err(|e| format!("mpv 命令线程异常：{e}"))?
}

#[tauri::command]
pub async fn mpv_stop(state: tauri::State<'_, MpvState>) -> Result<(), String> {
    let inner = state.inner.clone();
    tauri::async_runtime::spawn_blocking(move || {
        stop_session_sync(&inner);
    })
    .await
    .map_err(|e| format!("mpv 停止线程异常：{e}"))?;
    Ok(())
}

fn stop_session_sync(inner: &Arc<Mutex<Option<Session>>>) {
    let mut g = inner.lock().unwrap();
    let Some(mut s) = g.take() else {
        return;
    };
    drop(g);

    s.cancel.store(true, Ordering::SeqCst);

    // 先礼后兵：quit 让 mpv 自己收尾（写完缓存、关窗口），
    // 不行再 kill。直接 kill 会留下没清干净的状态。
    let quit = whatmedia_mpv::encode_command(&[serde_json::json!("quit")]);
    #[cfg(unix)]
    if let Ok(mut tx) = s.tx.lock() {
        let _ = tx.write_all(quit.as_bytes());
        let _ = tx.flush();
    }
    #[cfg(windows)]
    if let Some(mut tx) = connect(&s.ipc) {
        let _ = tx.write_all(quit.as_bytes());
        let _ = tx.flush();
    }
    std::thread::sleep(std::time::Duration::from_millis(150));
    let _ = s.child.kill();
    let _ = s.child.wait();

    // Unix socket 文件是 mpv 建的，也应该由它删；
    // 但进程被 kill 时删不掉，这里兜一下，否则下次同 id 会连不上。
    #[cfg(unix)]
    if let whatmedia_mpv::IpcPath::Unix(p) = &s.ipc {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(unix)]
fn send_command_sync<R: Runtime>(
    _app: &AppHandle<R>,
    inner: &Arc<Mutex<Option<Session>>>,
    line: &str,
    _request_id: Option<u32>,
) -> Result<(), String> {
    let tx = {
        let g = inner.lock().unwrap();
        let Some(s) = g.as_ref() else {
            return Err("没有在播的 mpv 会话".to_string());
        };
        s.tx.clone()
    };
    let mut tx = tx.lock().map_err(|e| format!("IPC 写端被锁住：{e}"))?;
    tx.write_all(line.as_bytes())
        .map_err(|e| format!("写 IPC 失败：{e}"))?;
    tx.flush().map_err(|e| format!("刷新 IPC 失败：{e}"))
}

#[cfg(windows)]
fn send_command_sync<R: Runtime>(
    app: &AppHandle<R>,
    inner: &Arc<Mutex<Option<Session>>>,
    line: &str,
    request_id: Option<u32>,
) -> Result<(), String> {
    let ipc = {
        let g = inner.lock().unwrap();
        let Some(s) = g.as_ref() else {
            return Err("没有在播的 mpv 会话".to_string());
        };
        s.ipc.clone()
    };

    // Windows 同步命名管道每次命令使用独立短连接，彻底避免与 pump 线程的 ReadFile 发生内核级读写死锁。
    let mut stream = connect_with_retry(&ipc, 5, 20)
        .ok_or_else(|| "无法连接到 mpv 命令通道".to_string())?;
    stream
        .write_all(line.as_bytes())
        .map_err(|e| format!("写 IPC 失败：{e}"))?;
    stream
        .flush()
        .map_err(|e| format!("刷新 IPC 失败：{e}"))?;

    // 带 request_id 的查询命令（如 track-list）从同一短连接读取响应并转发给前端。
    if let Some(target_id) = request_id {
        let mut reader = BufReader::new(stream);
        let mut events = EventReader::new();
        let mut resp_line = String::new();
        for _ in 0..32 {
            resp_line.clear();
            match reader.read_line(&mut resp_line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    for ev in events.feed(&resp_line) {
                        if ev.name == "response" && ev.request_id == Some(target_id) {
                            let _ = app.emit("mpv://event", &ev);
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// 把 mpv 的事件流读出来，原样转发给前端。
///
/// 事件不在这里解释 —— 解释逻辑放在 TypeScript 里，
/// 那边的 `MediaPlayer` 实现要同时能处理 Web 与 mpv 两条来源。
fn pump<R: Runtime, T: std::io::Read>(
    app: AppHandle<R>,
    mut reader: BufReader<T>,
    cancel: Arc<AtomicBool>,
) {
    let mut events = EventReader::new();
    let mut line = String::new();
    while !cancel.load(Ordering::SeqCst) {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // 进程关了
            Ok(_) => {
                for ev in events.feed(&line) {
                    let _ = app.emit("mpv://event", &ev);
                }
            }
            Err(_) => break,
        }
    }
    let _ = app.emit("mpv://closed", ());
}

// ── 平台相关的连接 ───────────────────────────────────────────

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;

#[cfg(windows)]
type Stream = std::fs::File;

#[cfg(unix)]
fn connect(path: &whatmedia_mpv::IpcPath) -> Option<Stream> {
    match path {
        whatmedia_mpv::IpcPath::Unix(p) => std::os::unix::net::UnixStream::connect(p).ok(),
        whatmedia_mpv::IpcPath::Windows(_) => None,
    }
}

#[cfg(windows)]
fn connect(path: &whatmedia_mpv::IpcPath) -> Option<Stream> {
    use std::fs::OpenOptions;
    match path {
        whatmedia_mpv::IpcPath::Windows(name) => {
            let p = format!("\\\\.\\pipe\\{name}");
            OpenOptions::new().read(true).write(true).open(p).ok()
        }
        whatmedia_mpv::IpcPath::Unix(_) => None,
    }
}

/// mpv 起来后要一会儿才建好 IPC 端点，所以这里重试。
fn connect_with_retry(path: &whatmedia_mpv::IpcPath, attempts: u32, delay_ms: u64) -> Option<Stream> {
    for _ in 0..attempts {
        if let Some(s) = connect(path) {
            return Some(s);
        }
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
    }
    None
}
