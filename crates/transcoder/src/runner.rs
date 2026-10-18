//! FFmpeg 进程生命周期：启动 / 超时 / 取消 / 收尸。
//!
//! 三条不变量：
//!   1. **取消必须先 kill 再 wait。** 只 kill 不等会留下僵尸进程；只 wait 不 kill 会永远等下去。
//!   2. **取消不是错误。** 用户关掉播放器是正常操作，返回值要能区分
//!      "被取消" 与 "ffmpeg 报错"，否则日志里全是假故障。
//!   3. **没有 ffmpeg 时给出可操作的错误**，而不是 `Os error 2`。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use whatmedia_core::{Result, TaskPriority, WhatMediaError};

use crate::args::{build_args, AccelArgs, StreamRequest};
use crate::process::{BoxRead, ChildProc, CommandRunner, TokioRunner};

/// 硬件后端的首帧判定窗口。
///
/// 10 秒的依据：ffmpeg 初始化硬件失败几乎立刻退出（设备打不开），
/// 而 4K 片源在慢盘上开解第一帧也要好几秒。定太短会把"慢"误判成"失败"，
/// 定太长会让用户在黑屏里等。
pub const FIRST_FRAME_WINDOW: Duration = Duration::from_secs(10);

/// `spawn_with_fallback` 的结果。
pub struct FallbackOutcome {
    /// 活着的进程（若整条链都失败，则是最后一次那个已退出的进程）。
    pub process: RunningProcess,
    /// 实际用上的候选下标。
    pub used: usize,
    /// 总共尝试了几次。
    pub attempts: usize,
    /// 被放弃的尝试：`(候选下标, stderr)`。诊断"为什么没走硬件"全靠它。
    pub dropped: Vec<(usize, String)>,
}

impl FallbackOutcome {
    /// 是否发生了降级。日志与诊断页用。
    pub fn degraded(&self) -> bool {
        self.used > 0
    }
}

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

    /// 等进程退出，最多等 `window`。
    ///
    /// 返回 `None` 表示"窗口内没退出"——对流式转码来说这等于**跑起来了**，
    /// 后面再出问题就是片源的事，不该再换后端。
    ///
    /// 这是硬件降级判定的时间边界：ffmpeg 初始化硬件失败几乎都在启动后几秒内退出
    /// （`Failed to initialise VAAPI` / `Device creation failed` 之类），
    /// 跑过窗口还没退，说明硬件这条路是通的。
    pub async fn wait_for_exit_or(&mut self, window: Duration) -> Result<Option<Exit>> {
        let poll = Duration::from_millis(50);
        let deadline = Instant::now() + window;

        loop {
            if self.cancel.load(Ordering::SeqCst) {
                self.proc.kill()?;
                self.proc.wait().await?;
                return Ok(Some(Exit::Cancelled));
            }
            match self.proc.try_wait()? {
                Some(true) => return Ok(Some(Exit::Completed)),
                Some(false) => return Ok(Some(Exit::Failed)),
                None => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    tokio::time::sleep(poll).await;
                }
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

    /// 按后端候选链依次尝试，硬件挂了就换下一个 —— **用户无感知**。
    ///
    /// 判定规则（P5 DoD：硬件转码失败时自动静默降级到软件转码）：
    ///   · 只有在**首帧窗口内**失败才换后端。跑过窗口才失败的，
    ///     换后端也救不回来，反而会把"片源损坏"伪装成"硬件不行"。
    ///   · `Cancelled` 不触发降级。用户关掉播放器不是故障。
    ///   · 链尾必须是软件档（`accel_chain` 保证），所以最后一次尝试一定能跑纯 CPU。
    ///
    /// 返回值里的 `process` 可能是已经退出的失败进程（链全试完仍失败时），
    /// 调用方照常走失败处理，stderr 也在里面，不会丢诊断信息。
    pub async fn spawn_with_fallback(
        &self,
        req: &StreamRequest,
        chain: &[AccelArgs],
        window: Option<Duration>,
    ) -> Result<FallbackOutcome> {
        let window = window.unwrap_or(FIRST_FRAME_WINDOW);
        let mut dropped: Vec<(usize, String)> = Vec::new();

        // 链为空：按请求自带的 accel 跑一次，不引入降级语义。
        let attempts: Vec<Option<AccelArgs>> = if chain.is_empty() {
            vec![req.accel.clone()]
        } else {
            chain.iter().map(|a| Some(a.clone())).collect()
        };

        let mut last: Option<(RunningProcess, usize)> = None;

        for (i, accel) in attempts.iter().enumerate() {
            let mut attempt = req.clone();
            attempt.accel = accel.clone();
            let mut proc = self.spawn(build_args(&attempt))?;

            match proc.wait_for_exit_or(window).await? {
                Some(Exit::Completed) | None => {
                    return Ok(FallbackOutcome {
                        process: proc,
                        used: i,
                        attempts: i + 1,
                        dropped,
                    });
                }
                Some(Exit::Cancelled) => {
                    return Ok(FallbackOutcome {
                        process: proc,
                        used: i,
                        attempts: i + 1,
                        dropped,
                    });
                }
                Some(exit) => {
                    // 已经退出的进程，stderr 可以放心拿走做诊断。
                    let stderr = proc.collect_stderr().await.unwrap_or_default();
                    tracing::warn!(
                        accel = %accel.as_ref().map(|a| a.id.clone()).unwrap_or_default(),
                        "转码后端 {:?} 在首帧窗口内失败：{}",
                        exit,
                        stderr.lines().last().unwrap_or("").trim()
                    );
                    dropped.push((i, stderr));
                    if i + 1 < attempts.len() {
                        continue;
                    }
                    last = Some((proc, i));
                }
            }
        }

        let (process, used) = last.expect("attempts 非空");
        Ok(FallbackOutcome {
            process,
            used,
            attempts: attempts.len(),
            dropped,
        })
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

    // ── 硬件 → 软件 静默降级（P5 DoD）─────────────────────────

    fn chain() -> Vec<AccelArgs> {
        vec![
            AccelArgs {
                id: "qsv".into(),
                decoder: vec!["-hwaccel".into(), "qsv".into()],
                encoder: vec!["-c:v".into(), "h264_qsv".into()],
                scale: None,
                tonemap: None,
            },
            AccelArgs {
                id: "software".into(),
                decoder: Vec::new(),
                encoder: Vec::new(),
                scale: None,
                tonemap: None,
            },
        ]
    }

    fn transcode_req() -> StreamRequest {
        StreamRequest {
            input: "/media/a.mkv".into(),
            seek_ms: 0,
            video: crate::args::VideoAction::Encode {
                codec: whatmedia_core::VideoCodec::H264,
                max_width: None,
                max_height: None,
                bitrate_kbps: None,
                tonemap_to: None,
            },
            audio: crate::args::AudioAction::Copy { index: None },
            burn: None,
            output: crate::args::OutputSpec::Hls {
                dir: "/cache/hls/j1".into(),
                seg_seconds: 6,
            },
            accel: None,
            live: false,
        }
    }

    #[tokio::test]
    async fn hw_failure_silently_falls_back_to_software() {
        // 第一次（qsv）立刻失败，第二次（software）跑起来不退出。
        let r = Arc::new(MockRunner::new().with_script(vec![
            MockChild::fail(b"Device creation failed".to_vec()),
            MockChild::long_running(5_000, Vec::new()),
        ]));
        let f = FfmpegRunner::with_runner("ffmpeg", r.clone());

        let out = f
            .spawn_with_fallback(&transcode_req(), &chain(), Some(Duration::from_millis(150)))
            .await
            .unwrap();

        assert_eq!(out.used, 1, "应当换到软件档");
        assert_eq!(out.attempts, 2);
        assert!(out.degraded());
        // 被放弃的那次要把 stderr 留下来 —— 否则"为什么没走硬件"永远查不出来。
        assert!(out.dropped[0].1.contains("Device creation failed"));

        // 两次 spawn 的参数必须真的不同，否则降级是假的。
        let calls = r.recorded();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1.contains(&"h264_qsv".to_string()));
        assert!(!calls[1].1.contains(&"h264_qsv".to_string()));
    }

    #[tokio::test]
    async fn hw_success_does_not_touch_software() {
        let r = Arc::new(
            MockRunner::new().with_script(vec![MockChild::long_running(5_000, Vec::new())]),
        );
        let f = FfmpegRunner::with_runner("ffmpeg", r.clone());
        let out = f
            .spawn_with_fallback(&transcode_req(), &chain(), Some(Duration::from_millis(150)))
            .await
            .unwrap();
        assert_eq!(out.used, 0);
        assert!(!out.degraded());
        assert_eq!(r.recorded().len(), 1, "硬件能跑就不该多起一次进程");
    }

    #[tokio::test]
    async fn failure_after_window_is_not_retried() {
        // 跑过首帧窗口才失败的，换后端救不回来 —— 重试只会把片源问题伪装成硬件问题。
        let r =
            Arc::new(MockRunner::new().with_script(vec![MockChild::long_running(300, Vec::new())]));
        let f = FfmpegRunner::with_runner("ffmpeg", r.clone());
        let out = f
            .spawn_with_fallback(&transcode_req(), &chain(), Some(Duration::from_millis(100)))
            .await
            .unwrap();
        assert_eq!(out.used, 0, "窗口内没退出 → 视为跑起来了，不降级");
        assert_eq!(r.recorded().len(), 1);
    }

    #[tokio::test]
    async fn all_candidates_failing_returns_last_process() {
        let r = Arc::new(MockRunner::new().with_script(vec![
            MockChild::fail(b"no device".to_vec()),
            MockChild::fail(b"still broken".to_vec()),
        ]));
        let f = FfmpegRunner::with_runner("ffmpeg", r.clone());
        let mut out = f
            .spawn_with_fallback(&transcode_req(), &chain(), None)
            .await
            .unwrap();
        assert_eq!(out.used, 1);
        assert_eq!(out.dropped.len(), 2);
        // 调用方照常能拿到失败状态，诊断信息不丢。
        assert_eq!(out.process.wait().await.unwrap(), Exit::Failed);
    }

    #[tokio::test]
    async fn cancellation_is_not_a_hardware_failure() {
        // 用户关掉播放器不是故障，不该触发降级重试。
        let r = Arc::new(
            MockRunner::new().with_script(vec![MockChild::long_running(5_000, Vec::new())]),
        );
        let f = FfmpegRunner::with_runner("ffmpeg", r.clone());
        let mut out = f
            .spawn_with_fallback(&transcode_req(), &chain(), Some(Duration::from_millis(150)))
            .await
            .unwrap();
        out.process.cancel();
        assert_eq!(
            out.process.wait_with_timeout(None).await.unwrap(),
            Exit::Cancelled
        );
        assert_eq!(r.recorded().len(), 1);
    }

    #[tokio::test]
    async fn empty_chain_runs_request_as_is() {
        let r = Arc::new(MockRunner::new().with_child(MockChild::ok(Vec::new(), Vec::new())));
        let f = FfmpegRunner::with_runner("ffmpeg", r.clone());
        let out = f
            .spawn_with_fallback(&transcode_req(), &[], Some(Duration::from_millis(150)))
            .await
            .unwrap();
        assert_eq!(out.used, 0);
        assert_eq!(out.attempts, 1);
    }
}
