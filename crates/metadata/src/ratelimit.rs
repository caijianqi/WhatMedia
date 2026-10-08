//! 限流与退避重试。
//!
//! 刮削最容易犯的错是把 TMDB 打爆然后被封 IP。
//! 1,000 部电影 × (1 次搜索 + 1 次详情) = 2,000 个请求，不限流几分钟就能触发限流。
//!
//! 这里刻意做成**进程内单点**而不是 per-provider：多个 Provider 共用一个桶更保守，
//! 而且实现简单得多。真需要 per-provider 配额时再拆。

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::transport::HttpError;

/// 最小间隔限流器。令牌桶的简化版。
///
/// 用最小间隔而不是"每秒 N 个"，是因为各家 API 的配额描述本来就是
/// "每秒 50 个"这种形式，换算成最小间隔最直观，也不会有突发打满的问题。
pub struct RateLimiter {
    min_interval: Duration,
    last: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn new(min_interval: Duration) -> Self {
        Self {
            min_interval,
            last: Mutex::new(None),
        }
    }

    /// 每秒最多 `n` 个请求。n=0 视为不限流（直接放行）。
    pub fn per_second(n: f64) -> Self {
        if n <= 0.0 {
            Self::new(Duration::ZERO)
        } else {
            Self::new(Duration::from_secs_f64(1.0 / n))
        }
    }

    pub async fn acquire(&self) {
        let wait = {
            let last = self.last.lock().unwrap();
            match *last {
                Some(t) => {
                    let elapsed = t.elapsed();
                    if elapsed >= self.min_interval {
                        Duration::ZERO
                    } else {
                        self.min_interval - elapsed
                    }
                }
                None => Duration::ZERO,
            }
        };

        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        *self.last.lock().unwrap() = Some(Instant::now());
    }
}

/// 退避重试。只对 `HttpStatus::Retryable` 重试，`Fatal` 直接返回。
///
/// 退避序列 200ms / 400ms / 800ms…，上限 8s。
/// 不做抖动：刮削是后台任务，惊群效应不明显，加抖动反而让测试不确定。
pub async fn retry<F, Fut, T>(max_attempts: u32, mut f: F) -> Result<T, HttpError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, HttpError>>,
{
    let mut last_err: Option<HttpError> = None;
    for attempt in 0..max_attempts.max(1) {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if e.status != crate::transport::HttpStatus::Retryable {
                    return Err(e);
                }
                if attempt + 1 < max_attempts {
                    let backoff = Duration::from_millis(200 * (1 << attempt.min(5)));
                    tokio::time::sleep(backoff.min(Duration::from_secs(8))).await;
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("max_attempts >= 1，循环至少执行一次"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn per_second_zero_means_unlimited() {
        let l = RateLimiter::per_second(0.0);
        assert_eq!(l.min_interval, Duration::ZERO);
    }

    /// 限流器不能让第一个请求就等 —— 那会让小任务的延迟凭空多一个间隔。
    #[tokio::test]
    async fn first_call_is_immediate() {
        let l = RateLimiter::new(Duration::from_millis(300));
        let start = Instant::now();
        l.acquire().await;
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let attempts = AtomicU32::new(0);
        let r = retry(4, || {
            let n = attempts.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < 2 {
                    Err(HttpError::fatal(503, "服务暂时不可用"))
                } else {
                    Ok(n)
                }
            }
        })
        .await;
        assert_eq!(r.unwrap(), 2);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    /// 401 重试一万次也是 401，不该浪费时间。
    #[tokio::test]
    async fn fatal_is_not_retried() {
        let attempts = AtomicU32::new(0);
        let r: Result<(), _> = retry(5, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            async { Err(HttpError::fatal(401, "API Key 无效")) }
        })
        .await;
        assert!(r.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
