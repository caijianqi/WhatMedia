//! FFmpeg 进程生命周期：启动 / 超时 / 取消 / 收尸。
//!
//! 三条不变量：
//!   1. **取消必须先 kill 再 wait。** 只 kill 不等会留下僵尸进程；只 wait 不 kill 会永远等下去。
//!   2. **取消不是错误。** 用户关掉播放器是正常操作，返回值要能区分
//!      "被取消" 与 "ffmpeg 报错"，否则日志里全是假故障。
//!   3. **没有 ffmpeg 时给出可操作的错误**，而不是 `Os error 2`。

use whatmedia_core::{WhatMediaError, Result, TaskPriority};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::process::{BoxRead, ChildProc, CommandRunner, TokioRunner};

/// 进程结局。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Exit {
    /// 正常跑完。
    Completed,
    /// 被主动取消。
    Cancelled,
    /// 超时（视为失败，调用方可以重试）。
    Timeout,
    /// ffmpeg 非 0 退出。
    Failed,
}

/// 运行中的 ffmpeg。
pub struct RunningProcess {
    proc: Box<dyn ChildProc>,
    cancel: Arc<AtomicBool>,
    started_at: Instant,
    /// 已采集到的 stderr。失败原因全在里面。
    pub stderr: String,
}

impl RunningProcess {
    pub fn pid(&self) -> Option<u32> {
        self.proc.pid()
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// 请求取消。幂等。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// 拿到取消标志，方便挂到别的地方（比如会话关闭时）。
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// 取走 stdout。流式输出直接接到 HTTP body 上，不要读进内存。
    pub fn take_stdout(&mut self) -> Option<BoxRead> {
        self.proc.take_stdout()
    }

    /// 取走 stderr，交给 `ProgressParser` 增量解析。
    pub fn take_stderr(&mut self) -> Option<BoxRead> {
        self.proc.take_stderr()
    }

    /// 干等到结束。
    pub async fn wait(&mut self) -> Result<Exit> {
        let ok = self.proc.wait().await?;
        Ok(if ok { Exit::Completed } else { Exit::Failed })
    }

    /// 带超时与取消地等待。
    ///
    /// 轮询而不是 `select!`：进程句柄要被 `&mut` 独占，
    /// 而 `select!` 取消分支会留下一个"正在 wait 中"的 future，无法安全重试。
    pub async fn wait_with_timeout(&mut self, timeout: Option<Duration>) -> Result<Exit> {
        let poll = Duration::from_millis(50);
        let deadline = timeout.map(|t| Instant::now() + t);

        loop {
            if self.cancel.load(Ordering::SeqCst) {
                // 不变量 ①：先 kill 再 wait。
                self.proc.kill()?;
                self.proc.wait().await?;
                return Ok(Exit::Cancelled);
            }
            if let Some(d) = deadline {
                if Instant::now() >= d {
                    self.proc.kill()?;
                    self.proc.wait().await?;
                    return Ok(Exit::Timeout);
                }
            }
            match self.proc.try_wait()? {
                Some(true) => return Ok(Exit::Completed),
                Some(false) => return Ok(Exit::Failed),
                None => tokio::time::sleep(poll).await,
            }
        }
    }

    /// 把 stderr 全读进来（用于失败诊断）。会**消耗**掉 stderr 句柄。
    pub async fn collect_stderr(&mut self) -> Result<String> {
        let Some(r) = self.take_stderr() else {
            return Ok(String::new());
        };
        let s = crate::process::read_to_string(r).await?;
        self.stderr = s.clone();
        Ok(s)
    }
}

/// FFmpeg 启动器。可注入 `CommandRunner` 以便离线测试。
pub struct FfmpegRunner {
    bin: String,
    runner: Arc<dyn CommandRunner>,
    default_timeout: Option<Duration>,
}

impl FfmpegRunner {
    pub fn new(bin: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            runner: Arc::new(TokioRunner),
            default_timeout: None,
        }
    }

    /// 注入自定义 runner（测试用）。
    pub fn with_runner(bin: impl Into<String>, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            bin: bin.into(),
            runner,
            default_timeout: None,
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.default_timeout = Some(t);
        self
    }

    pub fn bin(&self) -> &str {
        &self.bin
    }

    /// 启动一个进程。参数由 `crate::args::build_args` 产出。
    pub fn spawn(&self, args: Vec<String>) -> Result<RunningProcess> {
        let proc = self.runner.spawn(&self.bin, &args)?;
        Ok(RunningProcess {
            proc,
            cancel: Arc::new(AtomicBool::new(false)),
            started_at: Instant::now(),
            stderr: String::new(),
        })
    }

    /// 跑完整个进程并收集 stderr。缩略图、字幕转换这类短任务用。
    pub async fn run_to_completion(
        &self,
        args: Vec<String>,
        timeout: Option<Duration>,
    ) -> Result<(Exit, String)> {
        let mut p = self.spawn(args)?;
        let exit = p
            .wait_with_timeout(timeout.or(self.default_timeout))
            .await?;
        let stderr = p.collect_stderr().await.unwrap_or_default();
        Ok((exit, stderr))
    }

    /// 检查 ffmpeg 是否可用。`ffmpeg -version` 能跑通即认为可用。
    pub async fn is_available(&self) -> bool {
        let Ok((exit, _)) = self
            .run_to_completion(vec!["-version".to_string()], Some(Duration::from_secs(5)))
            .await
        else {
            return false;
        };
        exit == Exit::Completed
    }
}

/// 把 ffmpeg 的失败 stderr 变成可操作的错误。
///
/// 用户看到 `No such file or directory` 不会知道该怎么办；
/// 看到"ffmpeg 不存在，请安装"才知道下一步。
pub fn describe_failure(exit: Exit, stderr: &str) -> WhatMediaError {
    let tail: String = stderr
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" | ");

    match exit {
        Exit::Timeout => WhatMediaError::Ffmpeg {
            message: format!("ffmpeg 超时，已终止：{tail}"),
        },
        Exit::Failed if stderr.contains("Invalid data") => WhatMediaError::Ffmpeg {
            message: format!("片源可能损坏或编码不被支持：{tail}"),
        },
        Exit::Failed => WhatMediaError::Ffmpeg {
            message: if tail.is_empty() {
                "ffmpeg 以非 0 退出码结束".to_string()
            } else {
                tail
            },
        },
        other => WhatMediaError::Ffmpeg {
            message: format!("ffmpeg 未正常完成（{other:?}）"),
        },
    }
}

/// 转码任务的默认超时。
///
/// 实时转码（HLS 长任务）不该有超时；只有缩略图 / 字幕转换这类短任务要卡死线。
pub fn timeout_for_priority(p: TaskPriority) -> Option<Duration> {
    match p {
        // 用户在等：给一个宽松但有限的上限（首帧超时由调用方另算）。
        TaskPriority::Critical | TaskPriority::High => Some(Duration::from_secs(30)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{MockChild, MockRunner};

    fn runner_with(child: MockChild) -> (FfmpegRunner, Arc<MockRunner>) {
        let r = Arc::new(MockRunner::new().with_child(child));
        let f = FfmpegRunner::with_runner("ffmpeg", r.clone());
        (f, r)
    }

    #[tokio::test]
    async fn spawn_passes_args_through() {
        let (f, r) = runner_with(MockChild::ok(Vec::new(), Vec::new()));
        let mut p = f.spawn(vec!["-i".into(), "a.mkv".into()]).unwrap();
        assert_eq!(p.wait().await.unwrap(), Exit::Completed);
        let calls = r.recorded();
        assert_eq!(calls[0].1, vec!["-i".to_string(), "a.mkv".to_string()]);
    }

    #[tokio::test]
    async fn missing_ffmpeg_is_a_clear_error() {
        let r = Arc::new(MockRunner::new().with_spawn_failure());
        let f = FfmpegRunner::with_runner("ffmpeg", r);
        let err = match f.spawn(vec![]) {
            Ok(_) => panic!("spawn 应当失败"),
            Err(e) => e,
        };
        let msg = err.to_string();
        assert!(msg.contains("ffmpeg"), "错误信息里要点名 ffmpeg：{msg}");
    }

    #[tokio::test]
    async fn cancel_kills_and_reports_cancelled() {
        // 这条测试守护 DoD 的"转码任务可取消"。
        let (f, _) = runner_with(MockChild::long_running(10_000, Vec::new()));
        let mut p = f.spawn(vec![]).unwrap();

        let token = p.cancel_token();
        let handle = tokio::spawn(async move { p.wait_with_timeout(None).await });

        tokio::time::sleep(Duration::from_millis(30)).await;
        token.store(true, Ordering::SeqCst);

        let exit = tokio::time::timeout(Duration::from_secs(3), handle)
            .await
            .expect("取消后必须很快返回")
            .unwrap()
            .unwrap();
        assert_eq!(exit, Exit::Cancelled, "取消必须被区分出来，不能报成 Failed");
    }

    #[tokio::test]
    async fn timeout_kills_and_reports_timeout() {
        let (f, _) = runner_with(MockChild::long_running(10_000, Vec::new()));
        let mut p = f.spawn(vec![]).unwrap();
        let exit = p
            .wait_with_timeout(Some(Duration::from_millis(80)))
            .await
            .unwrap();
        assert_eq!(exit, Exit::Timeout);
    }

    #[tokio::test]
    async fn failure_collects_stderr() {
        let (f, _) = runner_with(MockChild::fail(
            b"Invalid data found when processing input".to_vec(),
        ));
        let (exit, stderr) = f.run_to_completion(vec![], None).await.unwrap();
        assert_eq!(exit, Exit::Failed);
        assert!(stderr.contains("Invalid data"));
        let e = describe_failure(exit, &stderr);
        assert!(e.to_string().contains("片源可能损坏"));
    }

    #[test]
    fn timeouts_only_for_interactive_priorities() {
        // 后台任务（扫描/刮削触发的转码）不该被超时打断。
        assert!(timeout_for_priority(TaskPriority::Critical).is_some());
        assert!(timeout_for_priority(TaskPriority::Low).is_none());
    }

    #[test]
    fn describe_failure_is_actionable() {
        let e = describe_failure(Exit::Timeout, "x");
        assert!(e.to_string().contains("超时"));
        let e = describe_failure(Exit::Failed, "");
        assert!(!e.to_string().is_empty());
    }
}
