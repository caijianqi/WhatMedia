//! IPC 通道与协议编解码。
//!
//! mpv 的 JSON IPC 是**换行分隔的 JSON**：一条一行，半包会真的发生
//! （TCP/管道都不保证一次 read 就是一整行），所以必须自己缓冲。
//! 这一层全是纯函数，因此可以离线测那些在真机上偶发、测不到的情况。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// IPC 通道地址。Windows 是命名管道，其它平台是 Unix socket。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcPath {
    /// Windows：`\\.\pipe\<name>`。传给 mpv 时要带前缀。
    Windows(String),
    /// Unix：`/tmp/<name>.sock`。
    Unix(String),
}

impl IpcPath {
    /// 给 `--input-ipc-server=` 用的形式。
    pub fn as_mpv_arg(&self) -> String {
        match self {
            IpcPath::Windows(name) => format!("\\\\.\\pipe\\{name}"),
            IpcPath::Unix(p) => p.clone(),
        }
    }
}

/// 按平台生成一个唯一的 IPC 通道名。
///
/// 名字里带 pid 而不是uuid：桌面端一般只开一两个 mpv，
/// pid 足够唯一且排障时能直接对上进程。
pub fn ipc_path(os: crate::detect::Os, session: u32) -> IpcPath {
    match os {
        crate::detect::Os::Windows => IpcPath::Windows(format!("whatmedia-mpv-{session}")),
        _ => IpcPath::Unix(format!("/tmp/whatmedia-mpv-{session}.sock")),
    }
}

/// 把一条命令编成 IPC 文本。
///
/// mpv 要求 `{"command": [...]}` 且**以换行结束**；
/// 少了换行 mpv 会一直等着，表现为"命令发出去没反应"。
pub fn encode_command(cmd: &[Value]) -> String {
    encode_with_id(cmd, None)
}

/// 带 request_id 的命令。响应里会回同一个 id，用来对号。
///
/// **request_id 是命令对象的顶层字段，不是命令数组的元素** ——
/// mpv 的 `input/ipc.c` 原样把它回填进响应，塞进 command 数组里会被当成参数，
/// 于是 `get_property` 变成"多了一个参数"，响应永远对不上号。
pub fn encode_async(cmd: &[Value], request_id: u32) -> String {
    encode_with_id(cmd, Some(request_id))
}

fn encode_with_id(cmd: &[Value], request_id: Option<u32>) -> String {
    let obj = match request_id {
        Some(id) => serde_json::json!({ "command": cmd, "request_id": id }),
        None => serde_json::json!({ "command": cmd }),
    };
    let mut s = serde_json::to_string(&obj).unwrap_or_else(|_| "{\"command\":[]}".to_string());
    s.push('\n');
    s
}

/// mpv 推过来的一条事件。
///
/// 带 `Serialize` 是因为桌面端直接把它转发给前端（`emit("mpv://event", &ev)`），
/// 在 Rust 侧另建一份镜像结构会立刻开始漂移。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MpvEvent {
    /// 事件名。`property-change` / `file-loaded` / `end-file` / `shutdown` ...
    pub name: String,
    /// `property-change` 时的属性名。
    pub property: Option<String>,
    /// 属性新值（也可能是 null）。
    pub data: Option<Value>,
    /// 异步命令的响应 id。
    pub request_id: Option<u32>,
    /// 命令结果。`success` 之外都是失败。
    pub error: Option<String>,
    /// `end-file` 的结束原因：`eof` / `error` / `quit` / `stop`。
    ///
    /// 单独存一个字段而不是塞进 `data`：mpv 的 end-file 用的是 `reason` 键，
    /// 不是 `data`，复用 data 会把它静默丢掉 —— 于是"播完了"和"解不动了"
    /// 在上层看起来一模一样。
    pub reason: Option<String>,
}

impl MpvEvent {
    /// 这是"某个属性变了"吗？是的话返回 (属性名, 新值)。
    pub fn as_property_change(&self) -> Option<(&str, Option<&Value>)> {
        if self.name == "property-change" {
            self.property.as_deref().map(|p| (p, self.data.as_ref()))
        } else {
            None
        }
    }

    /// 播放结束。`end-file` 里的 `reason` 能区分播完 / 出错 / 被替换。
    pub fn is_end(&self) -> bool {
        self.name == "end-file" || self.name == "shutdown"
    }
}

/// 把字节流切成事件。**有状态**：残行要留到下一批。
#[derive(Debug, Default)]
pub struct EventReader {
    buf: String,
}

impl EventReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂进一批新数据，返回这一批里凑齐的**完整**事件。
    ///
    /// 半包留着；**坏行直接丢**而不是 panic —— mpv 偶尔会打一行
    /// 非 JSON 的告警，为它把整个桌面端崩掉不值得。
    pub fn feed(&mut self, chunk: &str) -> Vec<MpvEvent> {
        self.buf.push_str(chunk);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let line: String = self.buf.drain(..=pos).collect();
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(line) {
                Ok(v) => {
                    if let Some(e) = parse_event(&v) {
                        out.push(e);
                    }
                }
                Err(_) => {
                    // 故意忽略：见上面的注释。真要看，调用方自己开日志。
                }
            }
        }
        out
    }

    /// 还剩多少没凑齐。测试与诊断用。
    pub fn pending(&self) -> &str {
        &self.buf
    }
}

fn parse_event(v: &Value) -> Option<MpvEvent> {
    // 命令响应：{"error":"success","request_id":3}
    if v.get("error").is_some() {
        return Some(MpvEvent {
            name: "response".to_string(),
            property: None,
            data: v.get("data").cloned(),
            request_id: v
                .get("request_id")
                .and_then(|x| x.as_u64())
                .map(|x| x as u32),
            error: v.get("error").and_then(|x| x.as_str()).map(str::to_string),
            reason: v.get("reason").and_then(|x| x.as_str()).map(str::to_string),
        });
    }
    let name = v.get("event").and_then(|x| x.as_str())?.to_string();
    Some(MpvEvent {
        name,
        property: v.get("name").and_then(|x| x.as_str()).map(str::to_string),
        data: v.get("data").cloned(),
        request_id: v
            .get("request_id")
            .and_then(|x| x.as_u64())
            .map(|x| x as u32),
        error: None,
        reason: v.get("reason").and_then(|x| x.as_str()).map(str::to_string),
    })
}

/// 观察这些属性。少了 `time-pos` 就没有进度条，少了 `duration` 进度条就是 0 长度。
///
/// **一条属性一个命令**，而不是 mpv 那条"一次观察多个属性"的批量形式：
/// 批量形式是后加的、各版本支持程度不一致，而逐条发在所有 ≥0.34 的版本上都成立。
/// 代价只是多几行文本，启动时一次性发完，不影响任何热路径。
///
/// id 从 `id` 开始逐个 +1，方便排障时看事件里的 id 对回属性。
pub fn observe_properties(props: &[&str], id: u32) -> String {
    let mut out = String::new();
    for (i, p) in props.iter().enumerate() {
        out.push_str(&encode_command(&[
            Value::from("observe_property"),
            Value::from(id + i as u32),
            Value::from(*p),
        ]));
    }
    out
}

/// 便于构造命令的小工具：`cmd!["set_property", "pause", true]`。
#[macro_export]
macro_rules! cmd {
    ($($arg:expr),+ $(,)?) => {{
        let parts: Vec<serde_json::Value> = vec![$(serde_json::to_value($arg).unwrap()),+];
        $crate::ipc::encode_command(&parts)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Os;

    #[test]
    fn windows_pipe_gets_the_double_backslash_prefix() {
        let p = IpcPath::Windows("whatmedia-mpv-1".to_string());
        assert_eq!(p.as_mpv_arg(), "\\\\.\\pipe\\whatmedia-mpv-1");
    }

    #[test]
    fn unix_socket_is_used_as_is() {
        let p = IpcPath::Unix("/tmp/x.sock".to_string());
        assert_eq!(p.as_mpv_arg(), "/tmp/x.sock");
    }

    #[test]
    fn ipc_path_differs_by_os() {
        assert!(matches!(ipc_path(Os::Windows, 1), IpcPath::Windows(_)));
        assert!(matches!(ipc_path(Os::Linux, 1), IpcPath::Unix(_)));
        assert!(matches!(ipc_path(Os::Macos, 1), IpcPath::Unix(_)));
    }

    #[test]
    fn command_is_json_and_newline_terminated() {
        let s = encode_command(&[Value::from("set_property"), Value::from("pause")]);
        assert_eq!(s, "{\"command\":[\"set_property\",\"pause\"]}\n");
    }

    #[test]
    fn async_command_puts_request_id_at_top_level() {
        // 关键回归：request_id 曾一度被追加进 command 数组，
        // 那样 mpv 会把它当参数，响应里永远带不回同一个 id。
        let s = encode_async(&[Value::from("get_property"), Value::from("time-pos")], 42);
        assert_eq!(
            s,
            "{\"command\":[\"get_property\",\"time-pos\"],\"request_id\":42}\n"
        );
    }

    #[test]
    fn plain_command_has_no_request_id() {
        let s = encode_command(&[Value::from("set_property"), Value::from("pause")]);
        assert!(!s.contains("request_id"), "{s}");
    }

    #[test]
    fn half_packet_is_buffered_until_complete() {
        let mut r = EventReader::new();
        let a = r.feed("{\"event\":\"file-load");
        assert!(a.is_empty(), "半包不该产出事件");
        assert_eq!(r.pending(), "{\"event\":\"file-load");

        let b = r.feed("ed\"}\n");
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].name, "file-loaded");
        assert_eq!(r.pending(), "");
    }

    #[test]
    fn multiple_events_in_one_chunk_are_all_returned() {
        let mut r = EventReader::new();
        let evs = r.feed("{\"event\":\"a\"}\n{\"event\":\"b\"}\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].name, "a");
        assert_eq!(evs[1].name, "b");
    }

    #[test]
    fn garbage_line_is_skipped_not_fatal() {
        let mut r = EventReader::new();
        let evs = r.feed("[warning] something\n{\"event\":\"ok\"}\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].name, "ok");
    }

    #[test]
    fn property_change_is_decoded() {
        let mut r = EventReader::new();
        let evs = r.feed("{\"event\":\"property-change\",\"name\":\"time-pos\",\"data\":12.5}\n");
        assert_eq!(evs.len(), 1);
        let (name, val) = evs[0].as_property_change().expect("应当识别为属性变化");
        assert_eq!(name, "time-pos");
        assert_eq!(val.and_then(|v| v.as_f64()), Some(12.5));
    }

    #[test]
    fn null_property_value_is_preserved() {
        // duration 在元数据没到手前是 null。丢掉它进度条会一直显示 0。
        let mut r = EventReader::new();
        let evs = r.feed("{\"event\":\"property-change\",\"name\":\"duration\",\"data\":null}\n");
        let (_, val) = evs[0].as_property_change().unwrap();
        assert!(val.is_some(), "null 也要原样带出来，不能当成缺失");
    }

    #[test]
    fn end_file_is_recognized_as_end() {
        let mut r = EventReader::new();
        let evs = r.feed("{\"event\":\"end-file\",\"reason\":\"eof\"}\n");
        assert!(evs[0].is_end());
    }

    #[test]
    fn end_file_reason_is_kept_separately() {
        // mpv 的 end-file 用 "reason" 而不是 "data"。
        // 丢了它就无法区分"播完了"和"解码失败"。
        let mut r = EventReader::new();
        let evs = r.feed("{\"event\":\"end-file\",\"reason\":\"error\"}\n");
        assert_eq!(evs[0].reason.as_deref(), Some("error"));
        let evs = r.feed("{\"event\":\"end-file\",\"reason\":\"eof\"}\n");
        assert_eq!(evs[0].reason.as_deref(), Some("eof"));
    }

    #[test]
    fn response_carries_error_and_request_id() {
        let mut r = EventReader::new();
        let evs = r.feed("{\"error\":\"success\",\"request_id\":7}\n");
        assert_eq!(evs[0].error.as_deref(), Some("success"));
        assert_eq!(evs[0].request_id, Some(7));
    }

    #[test]
    fn observe_emits_one_command_per_property() {
        let s = observe_properties(&["time-pos", "pause"], 1);
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2, "逐条发，不能合并成一条：{s}");
        assert!(lines[0].contains("\"time-pos\""));
        assert!(lines[1].contains("\"pause\""));
        // 每条都必须是完整的一行（以 \n 结束），否则 mpv 会一直等着
        assert!(s.ends_with('\n'));
    }
}
