//! 进程抽象的**可注入层**。
//!
//! 本机不一定装了 FFmpeg，CI（尤其自托管 arm64 runner）也不一定。
//! 但"转码任务可取消、取消后进程与临时文件 100% 清理"是 P3 的 DoD，
//! 这条不能靠"等我接上机器再测"。
//!
//! 所以把 `Command` 收进 `CommandRunner` trait：真实环境跑 `tokio::process`，
//! 测试里跑 `MockRunner`，进程生命周期逻辑两边共用同一份代码。

use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt};
use whatmedia_core::{Result, WhatMediaError};

/// stdout / stderr 的统一形态。
///
/// 用 trait object 而不是 `ChildStdout`，是为了让 mock 能塞一个内存 buffer 进来。
pub type BoxRead = Box<dyn AsyncRead + Send + Unpin>;

/// 一个已经启动的子进程。
#[async_trait]
pub trait ChildProc: Send {
    fn pid(&self) -> Option<u32>;

    /// 取走 stdout。只能取一次；流式输出直接接 HTTP body。
    fn take_stdout(&mut self) -> Option<BoxRead>;

    /// 取走 stderr。ffmpeg 的进度全在这上面。
    fn take_stderr(&mut self) -> Option<BoxRead>;

    /// 请求终止。不保证立刻退出，调用方要自己 wait。
    fn kill(&mut self) -> Result<()>;

    /// 非阻塞地看一眼是否已退出。返回 Some(是否成功)。
    fn try_wait(&mut self) -> Result<Option<bool>>;

    /// 等待退出。被 kill 的进程返回 `Ok(false)`，不视为错误。
    async fn wait(&mut self) -> Result<bool>;
}

/// 启动器。
pub trait CommandRunner: Send + Sync {
    fn spawn(&self, bin: &str, args: &[String]) -> Result<Box<dyn ChildProc>>;
}

// ─────────────────────────────────────────────────────────────
// 真实实现
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct TokioRunner;

impl CommandRunner for TokioRunner {
    fn spawn(&self, bin: &str, args: &[String]) -> Result<Box<dyn ChildProc>> {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        // 让子进程自成进程组，kill 时不会把信号误发给调用方。
        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let child = cmd.spawn().map_err(|e| WhatMediaError::Ffmpeg {
            message: format!("无法启动 {bin}：{e}"),
        })?;
        Ok(Box::new(TokioChild { child }))
    }
}

struct TokioChild {
    child: tokio::process::Child,
}

#[async_trait]
impl ChildProc for TokioChild {
    fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    fn take_stdout(&mut self) -> Option<BoxRead> {
        self.child.stdout.take().map(|s| Box::new(s) as BoxRead)
    }

    fn take_stderr(&mut self) -> Option<BoxRead> {
        self.child.stderr.take().map(|s| Box::new(s) as BoxRead)
    }

    fn kill(&mut self) -> Result<()> {
        self.child.start_kill().map_err(|e| WhatMediaError::Ffmpeg {
            message: format!("终止进程失败：{e}"),
        })
    }

    fn try_wait(&mut self) -> Result<Option<bool>> {
        match self.child.try_wait() {
            Ok(Some(st)) => Ok(Some(st.success())),
            Ok(None) => Ok(None),
            Err(e) => Err(WhatMediaError::Ffmpeg {
                message: format!("等待进程失败：{e}"),
            }),
        }
    }

    async fn wait(&mut self) -> Result<bool> {
        let st = self
            .child
            .wait()
            .await
            .map_err(|e| WhatMediaError::Ffmpeg {
                message: format!("等待进程失败：{e}"),
            })?;
        Ok(st.success())
    }
}

// ─────────────────────────────────────────────────────────────
// Mock 实现
// ─────────────────────────────────────────────────────────────

/// 可编程的假进程。
///
/// `run_ms` 模拟"要跑一会儿"：取消必须发生在进程还没结束的时候才有意义，
/// 一个瞬间退出的假进程测不出取消路径。
pub struct MockChild {
    pid: u32,
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    success: bool,
    run_ms: u64,
    killed: Arc<AtomicBool>,
    started: Arc<AtomicBool>,
}

impl MockChild {
    pub fn ok(stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self {
            pid: 1,
            stdout: Some(stdout),
            stderr: Some(stderr),
            success: true,
            run_ms: 0,
            killed: Arc::new(AtomicBool::new(false)),
            started: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 一个"跑很久"的进程，方便测取消。
    pub fn long_running(run_ms: u64, stderr: Vec<u8>) -> Self {
        Self {
            run_ms,
            stderr: Some(stderr),
            ..Self::ok(Vec::new(), Vec::new())
        }
    }

    pub fn fail(stderr: Vec<u8>) -> Self {
        Self {
            success: false,
            stderr: Some(stderr),
            ..Self::ok(Vec::new(), Vec::new())
        }
    }

    pub fn killed_flag(&self) -> Arc<AtomicBool> {
        self.killed.clone()
    }
}

#[async_trait]
impl ChildProc for MockChild {
    fn pid(&self) -> Option<u32> {
        Some(self.pid)
    }

    fn take_stdout(&mut self) -> Option<BoxRead> {
        self.stdout
            .take()
            .map(|b| Box::new(Cursor::new(b)) as BoxRead)
    }

    fn take_stderr(&mut self) -> Option<BoxRead> {
        self.stderr
            .take()
            .map(|b| Box::new(Cursor::new(b)) as BoxRead)
    }

    fn kill(&mut self) -> Result<()> {
        self.killed.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn try_wait(&mut self) -> Result<Option<bool>> {
        if self.killed.load(Ordering::SeqCst) {
            return Ok(Some(false));
        }
        if self.run_ms == 0 {
            Ok(Some(self.success))
        } else {
            Ok(None)
        }
    }

    async fn wait(&mut self) -> Result<bool> {
        self.started.store(true, Ordering::SeqCst);
        // 分片睡眠，这样 kill 能在中途生效。
        let step = 10u64;
        let mut waited = 0u64;
        while waited < self.run_ms {
            tokio::time::sleep(std::time::Duration::from_millis(step)).await;
            waited += step;
            if self.killed.load(Ordering::SeqCst) {
                return Ok(false);
            }
        }
        Ok(self.success)
    }
}

/// 内存_reader。`std::io::Cursor` 包一层以满足 `AsyncRead`。
struct Cursor {
    data: std::io::Cursor<Vec<u8>>,
}

impl Cursor {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data: std::io::Cursor::new(data),
        }
    }
}

impl AsyncRead for Cursor {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.data).poll_read(cx, buf)
    }
}

/// 假启动器。记录下每次 spawn 的参数，供断言使用。
pub struct MockRunner {
    pub calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
    pub outcome: std::sync::Mutex<Option<Box<dyn FnOnce() -> MockChild + Send>>>,
    /// 按顺序供进程。降级链要 spawn 多次，靠这个编排"第一次失败、第二次成功"。
    pub script: std::sync::Mutex<std::collections::VecDeque<MockChild>>,
    pub fail_spawn: bool,
}

impl MockRunner {
    pub fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
            outcome: std::sync::Mutex::new(None),
            script: std::sync::Mutex::new(std::collections::VecDeque::new()),
            fail_spawn: false,
        }
    }

    pub fn with_child(self, child: MockChild) -> Self {
        *self.outcome.lock().unwrap() = Some(Box::new(move || child));
        self
    }

    /// 依次给出多个进程。用完后再 spawn 就退回默认（成功、立即退出）。
    pub fn with_script(self, children: Vec<MockChild>) -> Self {
        *self.script.lock().unwrap() = children.into();
        self
    }

    /// spawn 直接失败。用来测"ffmpeg 不存在"这条路径。
    pub fn with_spawn_failure(self) -> Self {
        Self {
            fail_spawn: true,
            ..self
        }
    }

    pub fn recorded(&self) -> Vec<(String, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl Default for MockRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRunner for MockRunner {
    fn spawn(&self, bin: &str, args: &[String]) -> Result<Box<dyn ChildProc>> {
        self.calls
            .lock()
            .unwrap()
            .push((bin.to_string(), args.to_vec()));
        if self.fail_spawn {
            return Err(WhatMediaError::Ffmpeg {
                message: format!("无法启动 {bin}"),
            });
        }
        let child = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .or_else(|| self.outcome.lock().unwrap().take().map(|f| f()))
            .unwrap_or_else(|| MockChild::ok(Vec::new(), Vec::new()));
        Ok(Box::new(child))
    }
}

/// 把 stderr 全部读成字符串。日志采集用。
pub async fn read_to_string(mut r: BoxRead) -> Result<String> {
    let mut buf = Vec::new();
    r.read_to_end(&mut buf).await.map_err(WhatMediaError::Io)?;
    Ok(String::from_utf8_lossy(&buf).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_child_returns_stdout() {
        let mut c = MockChild::ok(b"hello".to_vec(), b"warn".to_vec());
        let out = read_to_string(c.take_stdout().unwrap()).await.unwrap();
        assert_eq!(out, "hello");
        assert!(c.wait().await.unwrap());
    }

    #[tokio::test]
    async fn killing_long_running_child_makes_wait_fail_fast() {
        let mut c = MockChild::long_running(5_000, Vec::new());
        let killed = c.killed_flag();
        let handle = tokio::spawn(async move { c.wait().await });

        // 等它真的开始跑再 kill，否则测的是"还没启动就结束"。
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        killed.store(true, Ordering::SeqCst);

        let ok = tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("wait 应在 kill 后很快返回")
            .unwrap()
            .unwrap();
        assert!(!ok, "被 kill 的进程不应视为成功");
    }

    #[tokio::test]
    async fn mock_runner_records_args_and_can_fail() {
        let r = MockRunner::new();
        let args = vec!["-i".to_string(), "a.mkv".to_string()];
        let mut c = r.spawn("ffmpeg", &args).unwrap();
        assert!(c.wait().await.unwrap());
        let calls = r.recorded();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "ffmpeg");
        assert_eq!(calls[0].1, args);

        let r2 = MockRunner::new().with_spawn_failure();
        assert!(r2.spawn("ffmpeg", &[]).is_err());
    }

    #[tokio::test]
    async fn failing_child_reports_false() {
        let mut c = MockChild::fail(b"Invalid data".to_vec());
        assert!(!c.wait().await.unwrap());
        let err = read_to_string(c.take_stderr().unwrap()).await.unwrap();
        assert!(err.contains("Invalid data"));
    }
}
