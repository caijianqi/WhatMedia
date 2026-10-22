//! 滑动窗口熔断器。
//!
//! 针对不可靠的三方采集源进行自适应保护：
//! - 在最近 N 次调用（滑动窗口）中统计超时/失败率。
//! - 当失败率达到阈值且样本数足够时，进入 Open 熔断状态，快速拒绝后续请求。
//! - 冷却期过后进入 HalfOpen 试探状态，一次成功则重置为 Closed，再次失败则延长熔断。

use std::collections::VecDeque;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// 正常流通
    Closed,
    /// 熔断拦截
    Open,
    /// 半开试探
    HalfOpen,
}

#[derive(Debug)]
pub struct CircuitBreaker {
    state: CircuitState,
    history: VecDeque<(Instant, bool)>,
    window_size: usize,
    failure_rate_threshold: f64,
    min_samples: usize,
    open_until: Option<Instant>,
    cooldown_duration: Duration,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new(10, 0.5, 4, Duration::from_secs(900)) // 默认 15 分钟冷却
    }
}

impl CircuitBreaker {
    pub fn new(
        window_size: usize,
        failure_rate_threshold: f64,
        min_samples: usize,
        cooldown_duration: Duration,
    ) -> Self {
        Self {
            state: CircuitState::Closed,
            history: VecDeque::with_capacity(window_size),
            window_size,
            failure_rate_threshold,
            min_samples,
            open_until: None,
            cooldown_duration,
        }
    }

    /// 检查当前是否允许发起请求
    pub fn can_execute(&mut self) -> bool {
        let now = Instant::now();
        match self.state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                if let Some(until) = self.open_until {
                    if now >= until {
                        // 冷却时间已到，转入半开试探
                        self.state = CircuitState::HalfOpen;
                        self.open_until = None;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    /// 记录请求成功
    pub fn record_success(&mut self) {
        let now = Instant::now();
        if self.state == CircuitState::HalfOpen {
            // 半开试探成功，彻底恢复
            self.state = CircuitState::Closed;
            self.history.clear();
            return;
        }

        self.push_history(now, true);
    }

    /// 记录请求失败或超时
    pub fn record_failure(&mut self) {
        let now = Instant::now();
        if self.state == CircuitState::HalfOpen {
            // 半开试探失败，立即重新熔断
            self.trip(now);
            return;
        }

        self.push_history(now, false);
        self.evaluate(now);
    }

    /// 当前状态
    pub fn state(&self) -> CircuitState {
        self.state
    }

    fn push_history(&mut self, now: Instant, success: bool) {
        if self.history.len() >= self.window_size {
            self.history.pop_front();
        }
        self.history.push_back((now, success));
    }

    fn evaluate(&mut self, now: Instant) {
        if self.history.len() < self.min_samples {
            return;
        }

        let total = self.history.len();
        let failures = self.history.iter().filter(|(_, ok)| !*ok).count();
        let failure_rate = failures as f64 / total as f64;

        if failure_rate >= self.failure_rate_threshold {
            self.trip(now);
        }
    }

    fn trip(&mut self, now: Instant) {
        self.state = CircuitState::Open;
        self.open_until = Some(now + self.cooldown_duration);
        self.history.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_transitions() {
        let mut cb = CircuitBreaker::new(5, 0.5, 3, Duration::from_millis(50));
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.can_execute());

        // 连续 2 次失败，但未达到 min_samples = 3
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);

        // 第 3 次失败，总 3 次全失败，达到阈值，触发熔断
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());

        // 等待冷却结束
        std::thread::sleep(Duration::from_millis(60));

        // 应当转入 HalfOpen
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // 试探成功，恢复为 Closed
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.can_execute());
    }

    #[test]
    fn test_half_open_failure_re_trips() {
        let mut cb = CircuitBreaker::new(5, 0.5, 2, Duration::from_millis(20));
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(30));
        assert!(cb.can_execute());
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        // 试探失败，立即重新 Open
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(!cb.can_execute());
    }
}
