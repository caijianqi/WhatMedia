//! 优先级任务队列。
//!
//! **为什么 P1 之前就要有**：扫描、刮削、转码、AI 四条异步链路共用同一套调度。
//! 等 P6 再补会引发大规模返工。
//!
//! 核心不变量：**播放抢占**。用户正在等（Critical）时，扫描/刮削（Low）必须让路。

use whatmedia_core::{Id, Task, TaskKind, TaskPriority, TaskState};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Mutex;

/// 最小入队参数。
#[derive(Debug, Clone)]
pub struct NewTask {
    pub kind: TaskKind,
    pub priority: TaskPriority,
    pub payload: Option<String>,
}

impl NewTask {
    pub fn new(kind: TaskKind, priority: TaskPriority) -> Self {
        Self {
            kind,
            priority,
            payload: None,
        }
    }
}

#[derive(Debug)]
struct Queued {
    task: Task,
}

impl Ord for Queued {
    fn cmp(&self, other: &Self) -> Ordering {
        // 优先级高者先出；同级时创建更早者先出（FIFO 语义）。
        self.task
            .priority
            .cmp(&other.task.priority)
            .then_with(|| other.task.created_at.cmp(&self.task.created_at))
    }
}

impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Queued {}

/// 线程安全的优先级队列。P1 会接上 Tokio worker 池与持久化。
#[derive(Debug, Default)]
pub struct TaskQueue {
    inner: Mutex<BinaryHeap<Queued>>,
}

impl TaskQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&self, new: NewTask) -> Id {
        let task = Task {
            id: Id::new_v4(),
            kind: new.kind,
            priority: new.priority,
            state: TaskState::Pending,
            payload: new.payload,
            progress: 0.0,
            error: None,
            created_at: chrono::Utc::now(),
        };
        let id = task.id;
        self.inner.lock().unwrap().push(Queued { task });
        id
    }

    /// 取出当前优先级最高的任务；空则返回 None。
    pub fn pop(&self) -> Option<Task> {
        self.inner.lock().unwrap().pop().map(|q| q.task)
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_priority_pops_first() {
        let q = TaskQueue::new();
        q.push(NewTask::new(TaskKind::Scan, TaskPriority::Low));
        q.push(NewTask::new(TaskKind::Transcode, TaskPriority::Critical));

        let t = q.pop().unwrap();
        assert_eq!(t.kind, TaskKind::Transcode);
        assert_eq!(t.priority, TaskPriority::Critical);
    }

    #[test]
    fn same_priority_is_fifo() {
        let q = TaskQueue::new();
        let a = q.push(NewTask::new(TaskKind::Scan, TaskPriority::Normal));
        let b = q.push(NewTask::new(TaskKind::Metadata, TaskPriority::Normal));

        assert_eq!(q.pop().unwrap().id, a);
        assert_eq!(q.pop().unwrap().id, b);
        assert!(q.is_empty());
    }

    #[test]
    fn playback_preempts_background_scan() {
        // 这条测试守护"扫描/刮削不影响播放"（§107）的调度前提。
        let q = TaskQueue::new();
        q.push(NewTask::new(TaskKind::Scan, TaskPriority::Low));
        q.push(NewTask::new(TaskKind::Metadata, TaskPriority::Low));
        q.push(NewTask::new(TaskKind::Transcode, TaskPriority::Critical));

        assert_eq!(q.pop().unwrap().priority, TaskPriority::Critical);
        assert_eq!(q.len(), 2);
    }
}
