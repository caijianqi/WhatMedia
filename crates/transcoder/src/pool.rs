//! 并发上限与优先级。
//!
//! **为什么不是"先来后到"的普通信号量**：用户点播放时（Critical）
//! 后台可能正在跑批量转码（Low）。让他排在队尾等 3 个槽位空出来，
//! 就是"点播放卡十秒"的直接原因。
//!
//! 做法：主信号量 + **一个只为 Critical 保留的备用槽**。
//! Critical 同时在两条队列上等，谁先给就用谁 —— 播放永远有路可走。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use whatmedia_core::TaskPriority;

/// 转码并发池。
#[derive(Debug, Clone)]
pub struct TranscodePool {
    main: Arc<Semaphore>,
    /// 保留槽。只有 Critical 能用。
    reserve: Arc<Semaphore>,
    capacity: usize,
    active: Arc<AtomicUsize>,
    /// 因为满而被拒绝的次数。诊断页要看这个指标。
    rejected: Arc<AtomicUsize>,
}

impl TranscodePool {
    /// `capacity` 至少为 1。传 0 会被抬到 1 —— 完全不允许转码等于"不能播"。
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            main: Arc::new(Semaphore::new(capacity)),
            reserve: Arc::new(Semaphore::new(1)),
            capacity,
            active: Arc::new(AtomicUsize::new(0)),
            rejected: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::SeqCst)
    }

    /// 还有没有空位（不含保留槽）。
    pub fn available(&self) -> usize {
        self.capacity.saturating_sub(self.active())
    }

    /// 申请一个执行槽。返回的 permit 离开作用域即释放。
    pub async fn acquire(&self, priority: TaskPriority) -> Slot {
        let permit = match priority {
            TaskPriority::Critical => {
                // 双队列竞争：主槽与保留槽谁先到用谁，没被选中的那个 future
                // 会被 drop，不会泄漏 permit。
                let main = self.main.clone();
                let reserve = self.reserve.clone();
                tokio::select! {
                    p = main.acquire_owned() => p.expect("信号量不会被关闭"),
                    p = reserve.acquire_owned() => p.expect("信号量不会被关闭"),
                }
            }
            _ => self
                .main
                .clone()
                .acquire_owned()
                .await
                .expect("信号量不会被关闭"),
        };

        self.active.fetch_add(1, Ordering::SeqCst);
        Slot {
            _permit: permit,
            active: self.active.clone(),
        }
    }

    /// 尝试立即拿到槽。拿不到返回 None（调用方据此降级而不是排队）。
    pub fn try_acquire(&self) -> Option<Slot> {
        let permit = self.main.clone().try_acquire_owned().ok()?;
        self.active.fetch_add(1, Ordering::SeqCst);
        Some(Slot {
            _permit: permit,
            active: self.active.clone(),
        })
    }

    /// 无槽可用时计数。用于"最近 N 次播放被排队了"这类告警。
    pub fn note_rejected(&self) {
        self.rejected.fetch_add(1, Ordering::SeqCst);
    }
}

/// 一个已经拿到的执行槽。Drop 即归还。
pub struct Slot {
    _permit: OwnedSemaphorePermit,
    active: Arc<AtomicUsize>,
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capacity_is_at_least_one() {
        let p = TranscodePool::new(0);
        assert_eq!(p.capacity(), 1);
    }

    #[tokio::test]
    async fn slots_are_released_on_drop() {
        let p = TranscodePool::new(2);
        {
            let _a = p.acquire(TaskPriority::Normal).await;
            assert_eq!(p.active(), 1);
            let _b = p.acquire(TaskPriority::Normal).await;
            assert_eq!(p.active(), 2);
            assert_eq!(p.available(), 0);
        }
        assert_eq!(p.active(), 0);
    }

    #[tokio::test]
    async fn normal_priority_blocks_when_full() {
        let p = TranscodePool::new(1);
        let _held = p.acquire(TaskPriority::Normal).await;
        // 主槽已满，Normal 申请会挂住。
        let got = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            p.acquire(TaskPriority::Normal),
        )
        .await;
        assert!(got.is_err(), "Normal 在池满时应当等待");
    }

    #[tokio::test]
    async fn critical_uses_reserved_slot() {
        // 这条测试守护"播放抢占"：池满时用户点播放也要能立刻开始。
        let p = TranscodePool::new(1);
        let _held = p.acquire(TaskPriority::Normal).await;

        let got = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            p.acquire(TaskPriority::Critical),
        )
        .await;
        assert!(got.is_ok(), "Critical 必须能通过保留槽立刻拿到执行权");
        assert_eq!(p.active(), 2, "保留槽让并发短暂超过容量上限，这是故意的");
    }

    #[tokio::test]
    async fn try_acquire_does_not_block() {
        let p = TranscodePool::new(1);
        let _held = p.acquire(TaskPriority::Normal).await;
        assert!(p.try_acquire().is_none());
        drop(_held);
        assert!(p.try_acquire().is_some());
    }

    #[tokio::test]
    async fn rejected_counter_increments() {
        let p = TranscodePool::new(1);
        p.note_rejected();
        p.note_rejected();
        assert_eq!(p.rejected(), 2);
    }
}
