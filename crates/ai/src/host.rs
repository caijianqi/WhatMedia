//! AI 触达业务能力的**唯一入口**。
//!
//! 设计要点（ADR-0006 §1）：`crates/ai` 不依赖 `whatmedia-database`。
//! 业务能力由宿主注入这个 trait，于是：
//!
//!   · AI 侧的每一条逻辑都能离线测（`InMemoryHost` 喂假数据）
//!   · "AI 碰不到 SQL / Shell / FS" 从口头约定变成可静态检查的事实
//!   · 真实实现（`DbHost`）写在 API 层，AI 侧不需要知道表结构
//!
//! 方法分成两组，与工具的风险分级一一对应：
//! 查询组（`search_media` … `capability`）对应只读工具；
//! 动作组（`scan_library` / `identify_media`）对应危险工具，**只能经确认后调用**。

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use whatmedia_core::{Id, MediaKind};

use crate::error::AiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingKind {
    Poster,
    Subtitle,
    Metadata,
}

impl MissingKind {
    pub fn label(&self) -> &'static str {
        match self {
            MissingKind::Poster => "海报",
            MissingKind::Subtitle => "字幕",
            MissingKind::Metadata => "元数据",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchState {
    /// 一次都没播过。
    Unwatched,
    /// 播过但没看完（进度 < 95%）。
    Partial,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    SizeDesc,
    AddedDesc,
    RatingDesc,
    TitleAsc,
}

/// 结构化查询。**这是"数据库负责真正查询"那一半**（需求稿 §72）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaQuery {
    pub text: Option<String>,
    pub kind: Option<MediaKind>,
    pub genres: Vec<String>,
    pub region: Option<String>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    /// 缺什么（海报 / 字幕 / 元数据）。
    pub missing: Vec<MissingKind>,
    pub added_within_days: Option<u32>,
    pub runtime_min_ms: Option<i64>,
    pub runtime_max_ms: Option<i64>,
    pub duplicates_only: bool,
    pub watch: Option<WatchState>,
    pub sort: Option<SortKey>,
    /// 上限。**必须有上限**：模型很容易要"全部"，而"全部"在十万条库上是一次全表扫。
    pub limit: u32,
}

impl MediaQuery {
    /// 给日志与审计用的一句话摘要。
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(k) = self.kind {
            parts.push(format!("kind={k:?}"));
        }
        for g in &self.genres {
            parts.push(format!("genre={g}"));
        }
        for m in &self.missing {
            parts.push(format!("missing={}", m.label()));
        }
        if let Some(d) = self.added_within_days {
            parts.push(format!("added<={d}d"));
        }
        if let Some(ms) = self.runtime_max_ms {
            parts.push(format!("runtime<={}", human_runtime(ms)));
        }
        if self.duplicates_only {
            parts.push("duplicates".into());
        }
        if let Some(w) = self.watch {
            parts.push(format!("watch={w:?}"));
        }
        if let Some(s) = self.sort {
            parts.push(format!("sort={s:?}"));
        }
        if parts.is_empty() {
            "无条件".into()
        } else {
            parts.join(" ")
        }
    }
}

/// 一次查询结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaHit {
    pub id: Id,
    pub title: String,
    pub kind: MediaKind,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub rating: Option<f64>,
    pub runtime_ms: Option<i64>,
    pub size_bytes: Option<i64>,
    pub added_at: Option<DateTime<Utc>>,
    pub has_poster: bool,
    pub has_subtitle: bool,
    pub has_metadata: bool,
    /// 播放进度 0.0–1.0。`None` 表示从未播放。
    pub progress: Option<f32>,
    pub path: Option<String>,
}

impl MediaHit {
    pub fn watch_state(&self) -> WatchState {
        match self.progress {
            None => WatchState::Unwatched,
            Some(p) if p >= 0.95 => WatchState::Finished,
            Some(_) => WatchState::Partial,
        }
    }
}

/// 一组重复条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateGroup {
    /// 归一化后的标题（去空格、去标点、小写）。
    pub key: String,
    pub title: String,
    pub year: Option<i32>,
    pub hits: Vec<MediaHit>,
}

/// 播放能力报告（对应 `get_playback_capability`）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityReport {
    pub item_id: Id,
    pub title: String,
    /// `direct_play` / `remux` / `transcode` / `unsupported`。
    pub strategy: String,
    /// 为什么走这条路。用户问"为什么我的 4K 片在转码"时要答得出来。
    pub reason: String,
    pub container: Option<String>,
    pub video: Option<String>,
    pub audio: Vec<String>,
    pub subtitles: Vec<String>,
    pub warnings: Vec<String>,
}

/// 触发一次扫描的回执（危险动作的执行结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanTicket {
    pub task_id: String,
    pub library: String,
}

/// 重新识别的回执。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentifyOutcome {
    pub item_id: Id,
    pub title: String,
    pub provider: Option<String>,
    pub provider_id: Option<String>,
    /// 实际被改动的字段。预览里承诺了什么，执行后就得报什么。
    pub changed_fields: Vec<String>,
}

/// 全网聚合流媒体条目摘要（AI 搜索结果）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedVodHit {
    pub title: String,
    pub category: Option<String>,
    pub year: Option<i32>,
    pub route_count: usize,
    pub total_episodes: usize,
    pub latest_note: Option<String>,
}

#[async_trait]
pub trait AiHost: Send + Sync {
    async fn search_media(&self, q: &MediaQuery) -> Result<Vec<MediaHit>, AiError>;
    async fn get_media(&self, id: Id) -> Result<Option<MediaHit>, AiError>;
    async fn duplicates(&self, limit: u32) -> Result<Vec<DuplicateGroup>, AiError>;
    async fn capability(
        &self,
        item_id: Id,
        client_id: Option<&str>,
    ) -> Result<CapabilityReport, AiError>;
    /// 推荐。`seed` 是可选的种子条目（"和这部差不多的"）。
    async fn recommend(&self, spec: &RecommendSpec) -> Result<Vec<MediaHit>, AiError>;
    /// 全网多源流媒体聚合搜索
    async fn search_vod(&self, _query: &str) -> Result<Vec<AggregatedVodHit>, AiError> {
        Ok(Vec::new())
    }

    // ── 危险动作：只能经 Plan→Preview→Confirm→Execute 调用 ──
    async fn scan_library(&self, library: Option<Id>) -> Result<ScanTicket, AiError>;
    async fn identify_media(&self, item_id: Id) -> Result<IdentifyOutcome, AiError>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecommendSpec {
    pub genres: Vec<String>,
    pub max_runtime_ms: Option<i64>,
    pub kind: Option<MediaKind>,
    pub exclude_watched: bool,
    pub limit: u32,
}

// ─────────────────────────────────────────────────────────────
// 测试实现
// ─────────────────────────────────────────────────────────────

/// 内存宿主：让工具层的全部逻辑离线可测。
///
/// 额外记录危险动作是否真的被调用过 —— 这是"未经确认不得执行"
/// 这条约束最直接的证据。
pub struct InMemoryHost {
    pub hits: Vec<MediaHit>,
    pub vod_hits: Vec<AggregatedVodHit>,
    /// 固定"现在"，否则 `added_within_days` 的测试会随时间漂移。
    pub now: DateTime<Utc>,
    pub scans: std::sync::Mutex<Vec<Option<Id>>>,
    pub identifies: std::sync::Mutex<Vec<Id>>,
}

impl InMemoryHost {
    pub fn new(hits: Vec<MediaHit>) -> Self {
        Self {
            hits,
            vod_hits: Vec::new(),
            now: Utc::now(),
            scans: std::sync::Mutex::new(Vec::new()),
            identifies: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn with_vod_hits(mut self, vod_hits: Vec<AggregatedVodHit>) -> Self {
        self.vod_hits = vod_hits;
        self
    }

    pub fn with_now(mut self, now: DateTime<Utc>) -> Self {
        self.now = now;
        self
    }

    pub fn scan_calls(&self) -> Vec<Option<Id>> {
        self.scans.lock().unwrap().clone()
    }

    pub fn identify_calls(&self) -> Vec<Id> {
        self.identifies.lock().unwrap().clone()
    }
}

fn matches(hit: &MediaHit, q: &MediaQuery, now: DateTime<Utc>) -> bool {
    if let Some(k) = q.kind {
        if hit.kind != k {
            return false;
        }
    }
    if let Some(text) = &q.text {
        let t = text.to_lowercase();
        if !hit.title.to_lowercase().contains(&t) {
            return false;
        }
    }
    for g in &q.genres {
        if !hit.genres.iter().any(|x| x.eq_ignore_ascii_case(g)) {
            return false;
        }
    }
    if let Some(from) = q.year_from {
        if hit.year.unwrap_or(0) < from {
            return false;
        }
    }
    if let Some(to) = q.year_to {
        if hit.year.unwrap_or(i32::MAX) > to {
            return false;
        }
    }
    for m in &q.missing {
        let ok = match m {
            MissingKind::Poster => !hit.has_poster,
            MissingKind::Subtitle => !hit.has_subtitle,
            MissingKind::Metadata => !hit.has_metadata,
        };
        if !ok {
            return false;
        }
    }
    if let Some(days) = q.added_within_days {
        match hit.added_at {
            Some(at) => {
                if at < now - Duration::days(days as i64) {
                    return false;
                }
            }
            // 没有添加时间就当它不满足"最近添加" —— 猜成满足会漏掉真正的老条目。
            None => return false,
        }
    }
    if let Some(min) = q.runtime_min_ms {
        if hit.runtime_ms.unwrap_or(0) < min {
            return false;
        }
    }
    if let Some(max) = q.runtime_max_ms {
        if hit.runtime_ms.unwrap_or(i64::MAX) > max {
            return false;
        }
    }
    if let Some(w) = q.watch {
        if hit.watch_state() != w {
            return false;
        }
    }
    true
}

/// 命中结果的排序。**唯一一份**，对外导出（`crates/api` 的 `DbHost` 也用它）。
///
/// `Option` 字段一律先 `unwrap_or` 再比：`None` 参与 `Reverse` 排序会跑到序列两端，
/// 表现得像"这些条目没有时长/评分就活该垫底"，而实际语义是"未知"。
pub fn sort_hits(hits: &mut [MediaHit], sort: Option<SortKey>) {
    match sort {
        Some(SortKey::SizeDesc) => {
            hits.sort_by_key(|h| std::cmp::Reverse(h.size_bytes.unwrap_or(0)))
        }
        Some(SortKey::AddedDesc) => hits.sort_by(|a, b| {
            b.added_at
                .unwrap_or(DateTime::<Utc>::MIN_UTC)
                .cmp(&a.added_at.unwrap_or(DateTime::<Utc>::MIN_UTC))
        }),
        Some(SortKey::RatingDesc) => hits.sort_by(|a, b| {
            b.rating
                .unwrap_or(0.0)
                .partial_cmp(&a.rating.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        Some(SortKey::TitleAsc) | None => hits.sort_by(|a, b| a.title.cmp(&b.title)),
    }
}

/// 重复判定键：标题归一化 + 年份。
///
/// 只用标题会把"同名不同年"的两部片并成一组；
/// 加上年份之后，2008 年的《钢铁侠》和 2020 年的《钢铁侠》才不会误判。
pub fn duplicate_key(title: &str, year: Option<i32>) -> String {
    let norm: String = title
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '.' | '-' | '_' | '（' | '）' | '(' | ')'))
        .collect();
    format!(
        "{}#{}",
        norm,
        year.map(|y| y.to_string()).unwrap_or_default()
    )
}

#[async_trait]
impl AiHost for InMemoryHost {
    async fn search_media(&self, q: &MediaQuery) -> Result<Vec<MediaHit>, AiError> {
        let mut out: Vec<MediaHit> = self
            .hits
            .iter()
            .filter(|h| matches(h, q, self.now))
            .cloned()
            .collect();
        sort_hits(&mut out, q.sort);
        let limit = if q.limit == 0 { 20 } else { q.limit.min(200) };
        out.truncate(limit as usize);
        Ok(out)
    }

    async fn get_media(&self, id: Id) -> Result<Option<MediaHit>, AiError> {
        Ok(self.hits.iter().find(|h| h.id == id).cloned())
    }

    async fn duplicates(&self, limit: u32) -> Result<Vec<DuplicateGroup>, AiError> {
        use std::collections::BTreeMap;
        let mut groups: BTreeMap<String, Vec<MediaHit>> = BTreeMap::new();
        for h in &self.hits {
            groups
                .entry(duplicate_key(&h.title, h.year))
                .or_default()
                .push(h.clone());
        }
        let mut out: Vec<DuplicateGroup> = groups
            .into_iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|(key, mut hits)| {
                hits.sort_by_key(|h| std::cmp::Reverse(h.size_bytes.unwrap_or(0)));
                DuplicateGroup {
                    key,
                    title: hits[0].title.clone(),
                    year: hits[0].year,
                    hits,
                }
            })
            .collect();
        out.truncate(limit.max(1) as usize);
        Ok(out)
    }

    async fn capability(
        &self,
        item_id: Id,
        _client_id: Option<&str>,
    ) -> Result<CapabilityReport, AiError> {
        let hit = self
            .hits
            .iter()
            .find(|h| h.id == item_id)
            .ok_or_else(|| AiError::bad_args(format!("条目不存在：{item_id}")))?;
        // 内存宿主没有真实流信息，给一条明确的"未知"而不是编一个假的编码。
        Ok(CapabilityReport {
            item_id,
            title: hit.title.clone(),
            strategy: "unknown".into(),
            reason: "内存宿主没有流信息；真实实现由播放决策给出".into(),
            container: None,
            video: None,
            audio: Vec::new(),
            subtitles: Vec::new(),
            warnings: Vec::new(),
        })
    }

    async fn recommend(&self, spec: &RecommendSpec) -> Result<Vec<MediaHit>, AiError> {
        let mut out: Vec<MediaHit> = self
            .hits
            .iter()
            .filter(|h| {
                if let Some(k) = spec.kind {
                    if h.kind != k {
                        return false;
                    }
                }
                if let Some(max) = spec.max_runtime_ms {
                    if h.runtime_ms.unwrap_or(i64::MAX) > max {
                        return false;
                    }
                }
                if spec.exclude_watched && h.progress.is_some() {
                    return false;
                }
                if !spec.genres.is_empty()
                    && !h
                        .genres
                        .iter()
                        .any(|g| spec.genres.iter().any(|s| s.eq_ignore_ascii_case(g)))
                {
                    return false;
                }
                true
            })
            .cloned()
            .collect();
        out.sort_by(|a, b| {
            b.rating
                .unwrap_or(0.0)
                .partial_cmp(&a.rating.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        out.truncate(spec.limit.max(1) as usize);
        Ok(out)
    }

    async fn search_vod(&self, query: &str) -> Result<Vec<AggregatedVodHit>, AiError> {
        let q = query.trim().to_lowercase();
        let out = self
            .vod_hits
            .iter()
            .filter(|v| v.title.to_lowercase().contains(&q))
            .cloned()
            .collect();
        Ok(out)
    }

    async fn scan_library(&self, library: Option<Id>) -> Result<ScanTicket, AiError> {
        self.scans.lock().unwrap().push(library);
        Ok(ScanTicket {
            task_id: "task-1".into(),
            library: library
                .map(|i| i.to_string())
                .unwrap_or_else(|| "全部媒体库".into()),
        })
    }

    async fn identify_media(&self, item_id: Id) -> Result<IdentifyOutcome, AiError> {
        let hit = self
            .hits
            .iter()
            .find(|h| h.id == item_id)
            .ok_or_else(|| AiError::bad_args(format!("条目不存在：{item_id}")))?;
        self.identifies.lock().unwrap().push(item_id);
        Ok(IdentifyOutcome {
            item_id,
            title: hit.title.clone(),
            provider: Some("tmdb".into()),
            provider_id: Some("12345".into()),
            changed_fields: vec!["overview".into(), "poster".into()],
        })
    }
}

// ─────────────────────────────────────────────────────────────
// 人类可读格式化
// ─────────────────────────────────────────────────────────────

pub fn human_size(bytes: i64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.0} MB", b / MB)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

pub fn human_runtime(ms: i64) -> String {
    let mins = ms / 60_000;
    if mins >= 60 {
        let h = mins / 60;
        let m = mins % 60;
        // 整小时不写"0分" —— 谁也不想看到"2小时0分"。
        if m == 0 {
            format!("{h}小时")
        } else {
            format!("{h}小时{m}分")
        }
    } else {
        format!("{mins}分钟")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(id: &str, title: &str, kind: MediaKind) -> MediaHit {
        MediaHit {
            id: Id::parse_str(id).unwrap(),
            title: title.into(),
            kind,
            year: Some(2014),
            genres: vec!["Sci-Fi".into()],
            rating: Some(8.5),
            runtime_ms: Some(10_140_000),
            size_bytes: Some(20_000_000_000),
            added_at: Some(Utc::now()),
            has_poster: true,
            has_subtitle: true,
            has_metadata: true,
            progress: None,
            path: Some(format!("/media/{title}.mkv")),
        }
    }

    #[tokio::test]
    async fn missing_poster_filter() {
        let now = Utc::now();
        let mut a = hit(
            "00000000-0000-0000-0000-000000000001",
            "A",
            MediaKind::Movie,
        );
        a.has_poster = false;
        let host = InMemoryHost::new(vec![
            a,
            hit(
                "00000000-0000-0000-0000-000000000002",
                "B",
                MediaKind::Movie,
            ),
        ])
        .with_now(now);
        let out = host
            .search_media(&MediaQuery {
                missing: vec![MissingKind::Poster],
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "A");
    }

    #[tokio::test]
    async fn added_within_days_uses_host_clock() {
        let now = Utc::now();
        let mut old = hit(
            "00000000-0000-0000-0000-000000000001",
            "老片",
            MediaKind::Movie,
        );
        old.added_at = Some(now - Duration::days(10));
        let mut new = hit(
            "00000000-0000-0000-0000-000000000002",
            "新片",
            MediaKind::Movie,
        );
        new.added_at = Some(now - Duration::hours(2));
        let host = InMemoryHost::new(vec![old, new]).with_now(now);

        let out = host
            .search_media(&MediaQuery {
                added_within_days: Some(1),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].title, "新片");
    }

    #[tokio::test]
    async fn duplicates_group_by_normalized_title_and_year() {
        let a = hit(
            "00000000-0000-0000-0000-000000000001",
            "The.Matrix",
            MediaKind::Movie,
        );
        let mut b = a.clone();
        b.id = Id::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
        b.title = "The Matrix".into(); // 只差一个点
        let host = InMemoryHost::new(vec![a, b]);
        let groups = host.duplicates(10).await.unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].hits.len(), 2);
    }

    #[test]
    fn duplicate_key_keeps_years_apart() {
        assert_ne!(
            duplicate_key("钢铁侠", Some(2008)),
            duplicate_key("钢铁侠", Some(2020))
        );
        assert_eq!(
            duplicate_key("The.Matrix", None),
            duplicate_key("the matrix", None)
        );
    }

    #[tokio::test]
    async fn limit_is_always_bounded() {
        let hits: Vec<MediaHit> = (1..=10)
            .map(|i| {
                hit(
                    &format!("00000000-0000-0000-0000-{:012}", i),
                    &format!("M{i}"),
                    MediaKind::Movie,
                )
            })
            .collect();
        let host = InMemoryHost::new(hits);
        // limit=0 也要有上限，不能返回全部 —— 那在十万条库上是一次全表扫。
        let out = host.search_media(&MediaQuery::default()).await.unwrap();
        assert!(out.len() <= 20);
        let out = host
            .search_media(&MediaQuery {
                limit: 3,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn human_formatters() {
        assert_eq!(human_size(2_500_000_000), "2.3 GB");
        assert_eq!(human_runtime(7_200_000), "2小时");
        assert_eq!(human_runtime(5_400_000), "1小时30分");
        assert_eq!(human_runtime(45 * 60_000), "45分钟");
    }
}
