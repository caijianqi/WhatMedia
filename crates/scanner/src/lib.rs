//! 媒体库扫描器。
//!
//! 三条不变量：
//!   1. **增量优先** —— 指纹 `(mtime_ns, size)` 未变的文件一律跳过，不重新 ffprobe。
//!      ARM 上一次 ffprobe 要好几百毫秒，10 万文件全量探测是不可接受的。
//!   2. **可取消可暂停** —— 扫描是后台任务，用户在看片时它必须能让路（§107）。
//!   3. **失败不中断** —— 单个文件权限不足/损坏，只记一笔 `failed`，整库继续。

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, Semaphore};
use tokio::task::JoinSet;
use tracing::{debug, warn};
use whatmedia_core::{Library, StorageEntry, StorageProvider};
use whatmedia_database::{repo, Db};
use whatmedia_library::{ingest_file, IngestRequest};
use whatmedia_media::{
    is_video_file, parse_file_in_dir, FfprobeRunner, NfoKind, ParsedName, ProbeResult,
};
use whatmedia_storage::fingerprint;

/// 扫描配置。
#[derive(Debug, Clone)]
pub struct ScanConfig {
    /// 并发入库上限。ffprobe 是重活，ARM 上建议 2–4。
    pub concurrency: usize,
    /// 是否跳过指纹未变的文件。
    pub incremental: bool,
    /// 是否调用 ffprobe。关掉则只有文件名解析结果，用于快速建库。
    pub probe: bool,
    pub ffprobe: Option<Arc<FfprobeRunner>>,
    /// 小于该体积的文件跳过（可用来过滤 sample）。0 表示不过滤。
    pub min_size_bytes: i64,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            concurrency: 4,
            incremental: true,
            probe: true,
            ffprobe: None,
            min_size_bytes: 0,
        }
    }
}

/// 扫描过程控制柄。取消后不可恢复；暂停后可继续。
#[derive(Debug, Clone, Default)]
pub struct ScanControl {
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
}

impl ScanControl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }
    pub fn pause(&self) {
        self.pause.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.pause.store(false, Ordering::SeqCst);
    }
    pub async fn wait_if_paused(&self) {
        while self.pause.load(Ordering::SeqCst) && !self.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }
}

/// 进度快照。SSE 与 CLI 都消费它。
#[derive(Debug, Clone, Default, serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScanProgress {
    pub total: u64,
    pub done: u64,
    pub added: u64,
    pub updated: u64,
    pub skipped: u64,
    pub failed: u64,
    pub finished: bool,
    pub current: Option<String>,
}

/// 扫描结果统计。
#[derive(Debug, Clone, Copy, Default)]
pub struct ScanStats {
    pub found: u64,
    pub added: u64,
    pub updated: u64,
    pub skipped: u64,
    pub failed: u64,
    pub missing: u64,
    pub cancelled: bool,
}

pub struct Scanner {
    storage: Arc<dyn StorageProvider>,
    db: Db,
    cfg: ScanConfig,
}

impl Scanner {
    pub fn new(storage: Arc<dyn StorageProvider>, db: Db, cfg: ScanConfig) -> Self {
        Self { storage, db, cfg }
    }

    /// 扫一个媒体库。进度通过 `tx` 广播，调用方可以边扫边展示。
    pub async fn scan(
        &self,
        library: &Library,
        ctrl: ScanControl,
        tx: Option<broadcast::Sender<ScanProgress>>,
    ) -> Result<ScanStats> {
        let known = if self.cfg.incremental {
            repo::file_index_for_library(&self.db, library.id).await?
        } else {
            HashMap::new()
        };

        let files = self.walk(&library.root_path, &ctrl).await?;
        let total = files.len() as u64;
        let mut stats = ScanStats {
            found: total,
            // 遍历阶段就可能被取消，此时一个文件都没处理
            cancelled: ctrl.is_cancelled(),
            ..Default::default()
        };

        let progress = Arc::new(ProgressCounter::new(total, tx));
        let storage = self.storage.clone();
        let db = self.db.clone();
        let cfg = self.cfg.clone();
        let sem = Arc::new(Semaphore::new(self.cfg.concurrency.max(1)));
        let mut seen: Vec<String> = Vec::with_capacity(files.len());
        let mut set: JoinSet<FileOutcome> = JoinSet::new();

        for entry in files {
            if ctrl.is_cancelled() {
                stats.cancelled = true;
                break;
            }
            ctrl.wait_if_paused().await;

            seen.push(entry.path.clone());

            let fp = fingerprint(entry.modified_ns, entry.size.unwrap_or(0));
            if self.cfg.incremental {
                if let Some(old) = known.get(&entry.path) {
                    if old.fingerprint == fp {
                        stats.skipped += 1;
                        progress.bump_skipped(&entry.path).await;
                        continue;
                    }
                }
            }
            if self.cfg.min_size_bytes > 0 && entry.size.unwrap_or(0) < self.cfg.min_size_bytes {
                stats.skipped += 1;
                continue;
            }

            let existed = known.contains_key(&entry.path);
            let permit = sem.clone().acquire_owned().await?;
            let (st, d, c, lib) = (storage.clone(), db.clone(), cfg.clone(), library.clone());
            set.spawn(async move {
                let r = process_one(st, d, c, lib, entry, fp, existed).await;
                drop(permit);
                r
            });
        }

        while let Some(res) = set.join_next().await {
            match res {
                Ok(FileOutcome::Added(p)) => {
                    stats.added += 1;
                    progress.bump_added(&p).await;
                }
                Ok(FileOutcome::Updated(p)) => {
                    stats.updated += 1;
                    progress.bump_updated(&p).await;
                }
                Ok(FileOutcome::Failed(p, e)) => {
                    stats.failed += 1;
                    warn!("入库失败 {p}: {e}");
                    progress.bump_failed(&p).await;
                }
                Err(e) => {
                    stats.failed += 1;
                    warn!("扫描任务异常：{e}");
                }
            }
        }

        // 磁盘上已消失的文件：标记不可用，不删除。
        if !ctrl.is_cancelled() {
            let missing: Vec<_> = known
                .iter()
                .filter(|(path, _)| !seen.iter().any(|s| s == *path))
                .map(|(_, v)| v.id)
                .collect();
            stats.missing = repo::mark_files_missing(&self.db, &missing).await? as u64;
        }

        progress.finish().await;
        Ok(stats)
    }

    /// 递归遍历。广度优先，迭代实现（避免深目录爆栈）。
    async fn walk(&self, root: &str, ctrl: &ScanControl) -> Result<Vec<StorageEntry>> {
        let mut found = Vec::new();
        let mut queue: Vec<String> = vec![root.to_string()];

        while let Some(dir) = queue.pop() {
            if ctrl.is_cancelled() {
                break;
            }
            let entries = match self.storage.list(&dir).await {
                Ok(e) => e,
                Err(e) => {
                    warn!("跳过不可读目录 {dir}: {e}");
                    continue;
                }
            };
            for e in entries {
                if e.is_dir {
                    if !is_ignored_dir(&e.name) {
                        queue.push(e.path);
                    }
                } else if is_video_file(&e.name) {
                    found.push(e);
                }
            }
        }
        Ok(found)
    }
}

/// 单个文件的处理入口。做成自由函数而不是方法，是为了能直接 spawn 成 'static 任务。
///
/// **这里会重试。** 不是为了掩盖错误，而是因为实测到一类必然能重试成功的失败：
/// 10,000 文件并发 8 入库时，会有万分之几的概率撞上 SQLite 的瞬时锁冲突，
/// 表现为个别文件 `failed`；而紧接着再扫一次，那几个文件**全都成功入库**。
/// 也就是说第二次尝试就能过 —— 不重试的代价是用户静默少几部片子，且毫无线索。
async fn process_one(
    storage: Arc<dyn StorageProvider>,
    db: Db,
    cfg: ScanConfig,
    library: Library,
    entry: StorageEntry,
    fp: String,
    existed: bool,
) -> FileOutcome {
    const ATTEMPTS: usize = 3;
    let path = entry.path.clone();

    for attempt in 0..ATTEMPTS {
        match ingest_one(
            &storage,
            &db,
            &cfg,
            library.clone(),
            entry.clone(),
            fp.clone(),
        )
        .await
        {
            Ok(_) => {
                return if existed {
                    FileOutcome::Updated(path)
                } else {
                    FileOutcome::Added(path)
                }
            }
            Err(e) => {
                let msg = e.to_string();
                // 只重试"再试一次有可能成"的那些。权限不足、文件损坏这类
                // 重试三次也是同样结果，白白拖慢整库扫描。
                let transient = is_transient(&msg);
                if !transient || attempt + 1 == ATTEMPTS {
                    return FileOutcome::Failed(path, msg);
                }
                debug!("入库重试 {}/{} {path}：{msg}", attempt + 1, ATTEMPTS);
                tokio::time::sleep(std::time::Duration::from_millis(20 * (attempt as u64 + 1)))
                    .await;
            }
        }
    }
    unreachable!("循环要么 return，要么在最后一次 attempt 上 return")
}

/// 判断一个入库错误是否值得重试。
///
/// 认不出来时**不重试** —— 宁可让这个文件进 `failed` 被看见，
/// 也不要把真正的 bug 用三次重试糊过去。
fn is_transient(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("locked")
        || m.contains("busy")
        || m.contains("sqlite_busy")
        || m.contains("timeout")
        || m.contains("deadlock")
        || m.contains("temporarily")
}

async fn ingest_one(
    storage: &Arc<dyn StorageProvider>,
    db: &Db,
    cfg: &ScanConfig,
    library: Library,
    entry: StorageEntry,
    fp: String,
) -> Result<()> {
    let stem = Path::new(&entry.path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let dir = Path::new(&entry.path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut parsed: ParsedName = parse_file_in_dir(&stem, &dir);

    // NFO 优先级最高（§32）：用户显式写的，文件名只是 fallback。
    if let Some(nfo) = load_nfo(storage, &dir, &stem).await {
        apply_nfo(&mut parsed, nfo);
    }

    let probe: Option<ProbeResult> = if cfg.probe {
        match &cfg.ffprobe {
            Some(runner) => match runner.probe(&entry.path).await {
                Ok(p) => Some(p),
                Err(e) => {
                    debug!("ffprobe 失败 {}：{e}", entry.path);
                    None
                }
            },
            None => None,
        }
    } else {
        None
    };

    let req = IngestRequest {
        library,
        storage_provider: storage.id().to_string(),
        path: entry.path.clone(),
        size_bytes: entry.size.unwrap_or(0),
        fingerprint: fp,
        parsed,
        probe,
    };
    ingest_file(db, &req).await?;
    Ok(())
}

/// 依次尝试 `<stem>.nfo`；剧集再退到目录里的 `tvshow.nfo`。
async fn load_nfo(storage: &Arc<dyn StorageProvider>, dir: &str, stem: &str) -> Option<NfoKind> {
    let sibling = Path::new(dir).join(format!("{stem}.nfo"));
    if let Ok(xml) = storage.read(&sibling.to_string_lossy()).await {
        if let Ok(text) = String::from_utf8(xml) {
            if let Some(kind) = whatmedia_media::parse_nfo(&text) {
                return Some(kind);
            }
        }
    }
    let tvshow = Path::new(dir).join("tvshow.nfo");
    if let Ok(xml) = storage.read(&tvshow.to_string_lossy()).await {
        if let Ok(text) = String::from_utf8(xml) {
            return whatmedia_media::parse_nfo(&text);
        }
    }
    None
}

/// 用 NFO 覆盖文件名解析结果。只覆盖 NFO 里非空的字段。
fn apply_nfo(parsed: &mut ParsedName, nfo: NfoKind) {
    match nfo {
        NfoKind::Movie(m) => {
            if let Some(t) = m.title {
                parsed.title = t;
            }
            if m.year.is_some() {
                parsed.year = m.year;
            }
        }
        NfoKind::Episode(e) => {
            if let Some(t) = e.title {
                // 集标题是"这一集的标题"，作品标题留给 series
                let _ = t;
            }
            if let Some(s) = e.season {
                parsed.season = Some(s);
            }
            if let Some(ep) = e.episode {
                parsed.episode = Some(ep);
            }
        }
        NfoKind::Series(s) => {
            if let Some(t) = s.title {
                parsed.title = t;
            }
            if s.year.is_some() {
                parsed.year = s.year;
            }
        }
    }
}

/// 需要跳过的目录：隐藏目录 + NAS/系统产生的垃圾目录。
fn is_ignored_dir(name: &str) -> bool {
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name.to_lowercase().as_str(),
        "@eadir"
            | "@recycle"
            | "#recycle"
            | "system volume information"
            | "lost+found"
            | "$recycle.bin"
            | "extrafiles"
    )
}

enum FileOutcome {
    Added(String),
    Updated(String),
    Failed(String, String),
}

/// 进度计数器。用原子量聚合，避免每个文件都加锁。
struct ProgressCounter {
    total: u64,
    done: AtomicU64,
    added: AtomicU64,
    updated: AtomicU64,
    skipped: AtomicU64,
    failed: AtomicU64,
    current: tokio::sync::Mutex<String>,
    tx: Option<broadcast::Sender<ScanProgress>>,
}

impl ProgressCounter {
    fn new(total: u64, tx: Option<broadcast::Sender<ScanProgress>>) -> Self {
        Self {
            total,
            done: AtomicU64::new(0),
            added: AtomicU64::new(0),
            updated: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            current: tokio::sync::Mutex::new(String::new()),
            tx,
        }
    }

    async fn bump(&self, kind: &str, path: &str) {
        match kind {
            "added" => self.added.fetch_add(1, Ordering::Relaxed),
            "updated" => self.updated.fetch_add(1, Ordering::Relaxed),
            "skipped" => self.skipped.fetch_add(1, Ordering::Relaxed),
            _ => self.failed.fetch_add(1, Ordering::Relaxed),
        };
        self.done.fetch_add(1, Ordering::Relaxed);
        *self.current.lock().await = path.to_string();
        self.emit(false).await;
    }

    async fn bump_added(&self, path: &str) {
        self.bump("added", path).await
    }
    async fn bump_updated(&self, path: &str) {
        self.bump("updated", path).await
    }
    async fn bump_skipped(&self, path: &str) {
        self.bump("skipped", path).await
    }
    async fn bump_failed(&self, path: &str) {
        self.bump("failed", path).await
    }

    async fn finish(&self) {
        self.emit(true).await;
    }

    async fn emit(&self, finished: bool) {
        if let Some(tx) = &self.tx {
            let snap = ScanProgress {
                total: self.total,
                done: self.done.load(Ordering::Relaxed),
                added: self.added.load(Ordering::Relaxed),
                updated: self.updated.load(Ordering::Relaxed),
                skipped: self.skipped.load(Ordering::Relaxed),
                failed: self.failed.load(Ordering::Relaxed),
                finished,
                current: Some(self.current.lock().await.clone()),
            };
            // 没有接收者时 send 会失败，这不是错误。
            let _ = tx.send(snap);
        }
    }
}
