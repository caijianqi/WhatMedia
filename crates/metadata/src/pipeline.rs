//! 刮削编排：队列 → 搜索 → 打分 → 确认 → 落库 → 取图。
//!
//! 三条硬约束：
//!   1. **可断点续跑** —— 1,000 部片刮一次要几十分钟，崩了从头再来不可接受
//!   2. **不得覆盖用户改过的字段** —— 见 `merge` 模块
//!   3. **限流** —— 不得打爆 Provider（见 `ratelimit`）

use anyhow::Result;
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::task::JoinSet;

use whatmedia_core::{Id, MetadataProvider, MetadataQuery, MetadataRecord};
use whatmedia_database::{repo, Db};

use crate::images::ImageCache;
use crate::matcher::{pick_auto, score_candidates, Confidence, MatchThresholds, ScoredCandidate};
use crate::merge::{parse_locked, serialize_locked, ItemMeta, MergeContext};

#[derive(Debug, Clone)]
pub struct ScrapeConfig {
    pub concurrency: usize,
    pub thresholds: MatchThresholds,
    /// 下载并缓存图片。关掉它刮削会快很多，用于压测匹配率。
    pub fetch_images: bool,
    /// 要抓的图片类型。默认只要 poster —— backdrop 体积大、命中率低。
    pub image_kinds: Vec<String>,
    /// 关掉后所有结果都进人工确认队列。新库第一次刮建议关掉。
    pub auto_accept: bool,
    /// 剧集是否逐集抓单集信息（每集多一个请求）。
    pub scrape_episode_level: bool,
    /// 同一个 item 最多重跑几次。
    pub max_attempts: u32,
}

impl Default for ScrapeConfig {
    fn default() -> Self {
        Self {
            concurrency: 4,
            thresholds: MatchThresholds::default(),
            fetch_images: true,
            image_kinds: vec!["poster".into(), "backdrop".into()],
            auto_accept: true,
            scrape_episode_level: true,
            max_attempts: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Outcome {
    Matched,
    Review,
    #[default]
    Skipped,
    Failed,
}

impl Outcome {
    pub fn job_status(self) -> &'static str {
        match self {
            Outcome::Matched => "done",
            Outcome::Review => "review",
            Outcome::Skipped => "skipped",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ScrapeStats {
    pub matched: u64,
    pub review: u64,
    pub failed: u64,
    pub skipped: u64,
    pub images: u64,
}

impl ScrapeStats {
    /// 命中率 = 确认 + 待确认（都算"找对了"），除以处理总数。
    ///
    /// 待确认算命中：它说明"找到了可信候选"，只是没自动写入。
    /// 真正没找到的是 `skipped`。
    pub fn hit_rate(&self) -> f64 {
        let processed = self.matched + self.review + self.failed + self.skipped;
        if processed == 0 {
            return 0.0;
        }
        (self.matched + self.review) as f64 / processed as f64
    }
}

pub struct Scraper {
    db: Db,
    providers: Vec<Arc<dyn MetadataProvider>>,
    cache: Option<Arc<ImageCache>>,
    cfg: ScrapeConfig,
}

impl Scraper {
    pub fn new(db: Db, providers: Vec<Arc<dyn MetadataProvider>>, cfg: ScrapeConfig) -> Self {
        Self {
            db,
            providers,
            cache: None,
            cfg,
        }
    }

    pub fn with_cache(mut self, cache: Arc<ImageCache>) -> Self {
        self.cache = Some(cache);
        self
    }

    pub fn providers(&self) -> &[Arc<dyn MetadataProvider>] {
        &self.providers
    }

    pub async fn enqueue(&self, library_id: Id) -> Result<u64> {
        repo::metadata::enqueue_library(&self.db, library_id).await
    }

    pub async fn requeue_failed(&self, library_id: Id) -> Result<u64> {
        repo::metadata::requeue_failed(&self.db, library_id).await
    }

    pub async fn stats(&self, library_id: Option<Id>) -> Result<repo::JobStats> {
        repo::metadata::job_stats(&self.db, library_id).await
    }

    /// 跑完整个队列。可重复调用 —— 已完成的任务不会被重跑，这就是断点续跑。
    pub async fn run(&self, library_id: Option<Id>, cancel: Cancel) -> Result<ScrapeStats> {
        let stats = Stats::default();
        let mut set: JoinSet<()> = JoinSet::new();

        loop {
            if cancel.is_cancelled() {
                break;
            }
            let jobs =
                repo::metadata::claim_jobs(&self.db, library_id, self.cfg.concurrency as u32)
                    .await?;
            if jobs.is_empty() {
                break;
            }

            for job in jobs {
                if cancel.is_cancelled() {
                    break;
                }
                // 把 running 的任务在下次启动时捞回来：进程崩溃时它们会卡在 running。
                let db = self.db.clone();
                let providers = self.providers.clone();
                let cache = self.cache.clone();
                let cfg = self.cfg.clone();
                let s = stats.clone();
                set.spawn(async move {
                    let worker = Self {
                        db,
                        providers,
                        cache,
                        cfg,
                    };
                    let outcome = worker.process_one(job.item_id).await;
                    s.record(outcome.0);
                    let _ = repo::metadata::finish_job(
                        &worker.db,
                        job.item_id,
                        outcome.0.job_status(),
                        outcome.1.as_deref(),
                        outcome.2.as_deref(),
                        outcome.3.as_deref(),
                    )
                    .await;
                });
            }

            while set.join_next().await.is_some() {}
        }

        Ok(stats.snapshot())
    }

    /// 恢复崩溃时卡在 `running` 的任务。
    ///
    /// **这一步必须有**：否则一次 OOM 会让几百个 item 永久卡在 running，
    /// 用户再点"重新刮削"也不会处理它们。
    pub async fn reap_running(&self) -> Result<u64> {
        let now = chrono::Utc::now().to_rfc3339();
        repo::metadata::reap_running(&self.db, &now).await
    }

    /// 处理单个 item。返回 (结果, provider, provider_id, 错误)。
    pub async fn process_one(
        &self,
        item_id: Id,
    ) -> (Outcome, Option<String>, Option<String>, Option<String>) {
        match self.scrape_item(item_id).await {
            Ok(r) => r,
            Err(e) => (
                Outcome::Failed,
                None,
                None,
                Some(truncate(&e.to_string(), 500)),
            ),
        }
    }

    async fn scrape_item(
        &self,
        item_id: Id,
    ) -> Result<(Outcome, Option<String>, Option<String>, Option<String>)> {
        let target = match repo::metadata::scrape_target(&self.db, item_id).await? {
            Some(t) => t,
            None => return Ok((Outcome::Skipped, None, None, None)),
        };

        let is_episode = target.kind == "episode" && target.series_id.is_some();
        let query = MetadataQuery {
            // 用 match_title（文件名派生）而不是 title：用户把标题改成
            // "黑客帝国" 之后，拿它去 TMDB 搜是搜不到的。
            title: if is_episode {
                target
                    .series_match_title
                    .clone()
                    .or_else(|| target.series_title.clone())
                    .unwrap_or_else(|| target.match_title.clone())
            } else {
                target.match_title.clone()
            },
            year: target.year,
            language: None,
            episode_title: None,
        };

        let mut last_error: Option<String> = None;
        let mut best: Option<(ScoredCandidate, Arc<dyn MetadataProvider>)> = None;

        for p in &self.providers {
            let records = if is_episode {
                p.search_series(&query).await
            } else {
                p.search_movie(&query).await
            };
            match records {
                Ok(list) => {
                    let scored = score_candidates(&query, list, &self.cfg.thresholds);
                    if let Some(c) = scored.first() {
                        if c.confidence != Confidence::Reject
                            && best
                                .as_ref()
                                .map(|(b, _)| c.score > b.score)
                                .unwrap_or(true)
                        {
                            best = Some((c.clone(), p.clone()));
                        }
                    }
                }
                Err(e) => last_error = Some(truncate(&e.to_string(), 300)),
            }
        }

        let (candidate, provider) = match best {
            Some(b) => b,
            None => {
                return Ok((Outcome::Skipped, None, None, last_error));
            }
        };

        if !self.cfg.auto_accept || pick_auto(std::slice::from_ref(&candidate)).is_none() {
            return Ok((
                Outcome::Review,
                provider.id().to_string().into(),
                candidate.record.provider_id.clone().into(),
                None,
            ));
        }

        // 命中之后拉详情（搜索结果里通常没有演职员和完整图片）
        let record = if is_episode {
            provider
                .get_series(&candidate.record.provider_id)
                .await
                .unwrap_or_else(|_| candidate.record.clone())
        } else {
            provider
                .get_movie(&candidate.record.provider_id)
                .await
                .unwrap_or_else(|_| candidate.record.clone())
        };

        self.apply_record(&target, &record, is_episode).await?;

        Ok((
            Outcome::Matched,
            Some(record.provider.clone()),
            Some(record.provider_id.clone()),
            None,
        ))
    }

    /// 人工确认：直接按 provider_id 采用，跳过打分。
    pub async fn identify(
        &self,
        item_id: Id,
        provider_name: &str,
        provider_id: &str,
    ) -> Result<()> {
        let target = repo::metadata::scrape_target(&self.db, item_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("条目不存在：{item_id}"))?;
        let p = self
            .providers
            .iter()
            .find(|p| p.id() == provider_name)
            .ok_or_else(|| anyhow::anyhow!("未配置的 Provider：{provider_name}"))?;

        let is_episode = target.kind == "episode" && target.series_id.is_some();
        let record = if is_episode {
            p.get_series(provider_id).await?
        } else {
            p.get_movie(provider_id).await?
        };

        self.apply_record(&target, &record, is_episode).await?;
        repo::metadata::finish_job(
            &self.db,
            item_id,
            "done",
            Some(&record.provider),
            Some(&record.provider_id),
            None,
        )
        .await?;
        Ok(())
    }

    /// 给 UI 用的候选列表。人工确认界面就靠它。
    pub async fn candidates(
        &self,
        item_id: Id,
        title_override: Option<String>,
        year_override: Option<i32>,
    ) -> Result<Vec<CandidateView>> {
        let target = repo::metadata::scrape_target(&self.db, item_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("条目不存在：{item_id}"))?;
        let is_episode = target.kind == "episode" && target.series_id.is_some();

        let query = MetadataQuery {
            title: title_override.unwrap_or_else(|| {
                if is_episode {
                    target
                        .series_match_title
                        .clone()
                        .or_else(|| target.series_title.clone())
                        .unwrap_or_else(|| target.match_title.clone())
                } else {
                    target.match_title.clone()
                }
            }),
            year: year_override.or(target.year),
            language: None,
            episode_title: None,
        };

        let mut out = Vec::new();
        for p in &self.providers {
            let list = if is_episode {
                p.search_series(&query).await
            } else {
                p.search_movie(&query).await
            };
            if let Ok(list) = list {
                for c in score_candidates(&query, list, &self.cfg.thresholds) {
                    // 不过滤 Reject：人工确认界面要能看到"最接近的几部"，
                    // 哪怕分数很低 —— 空白列表比低分候选更让人无从下手。
                    out.push(CandidateView {
                        provider: c.record.provider.clone(),
                        provider_id: c.record.provider_id.clone(),
                        title: c.record.title.clone(),
                        original_title: c.record.original_title.clone(),
                        year: c.record.year,
                        overview: c.record.overview.clone(),
                        poster: c
                            .record
                            .images
                            .iter()
                            .find(|i| i.kind == "poster")
                            .map(|i| i.url.clone()),
                        score: c.score,
                        confidence: format!("{:?}", c.confidence),
                    });
                }
            }
        }
        out.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(out)
    }

    /// 把一条记录写进库。NFO 优先链与字段锁定都在这里汇合。
    async fn apply_record(
        &self,
        target: &repo::ScrapeTarget,
        record: &MetadataRecord,
        is_episode: bool,
    ) -> Result<()> {
        let locked: HashSet<String> = parse_locked(&target.locked_fields);
        let mut ctx = MergeContext::new(locked);

        let mut meta = ItemMeta {
            title: Some(target.title.clone()),
            original_title: target.original_title.clone(),
            year: target.year,
            ..Default::default()
        };

        // 剧集：剧集级信息写 series，单集信息写 item
        if is_episode {
            if let Some(series_id) = target.series_id {
                let mut s_meta = ItemMeta {
                    title: target.series_title.clone(),
                    year: target.year,
                    ..Default::default()
                };
                let mut s_ctx =
                    MergeContext::new(parse_locked(&series_locked_of(&self.db, series_id).await));
                crate::merge::merge_record(&mut s_meta, record, &mut s_ctx, false);
                repo::metadata::update_series_meta(
                    &self.db,
                    series_id,
                    &repo::metadata::SeriesMetaUpdate {
                        title: s_meta.title,
                        original_title: s_meta.original_title,
                        year: s_meta.year,
                        overview: s_meta.overview,
                        provider: Some(record.provider.clone()),
                        provider_id: Some(record.provider_id.clone()),
                        ..Default::default()
                    },
                )
                .await?;
            }

            // 单集信息：每集一次请求，但可以关掉
            if self.cfg.scrape_episode_level {
                if let (Some(sid), Some(s), Some(e)) = (
                    target.series_provider_id.clone(),
                    target.season_number,
                    target.episode_number,
                ) {
                    if let Some(p) = self.providers.iter().find(|p| p.id() == record.provider) {
                        if let Ok(ep) = p.get_episode(&sid, s, e).await {
                            if !ep.title.is_empty() {
                                crate::merge::merge_record(&mut meta, &ep, &mut ctx, false);
                            }
                            self.cache_images(target.id, &ep).await?;
                        }
                    }
                }
            }
        } else {
            crate::merge::merge_record(&mut meta, record, &mut ctx, false);
        }

        repo::metadata::update_item_meta(
            &self.db,
            target.id,
            &repo::metadata::ItemMetaUpdate {
                title: meta.title,
                original_title: meta.original_title,
                sort_title: meta.sort_title,
                year: meta.year,
                tagline: meta.tagline,
                overview: meta.overview,
                rating: meta.rating,
                runtime_ms: meta.runtime_ms,
                locked_fields: Some(serialize_locked(&ctx.locked)),
                provider: Some(record.provider.clone()),
                provider_id: Some(record.provider_id.clone()),
            },
        )
        .await?;

        if !is_episode {
            repo::metadata::set_genres(&self.db, target.id, &record.genres).await?;
            repo::metadata::set_credits(&self.db, target.id, &record.provider, &record.credits)
                .await?;
            self.cache_images(target.id, record).await?;
        } else {
            // 剧集的类型/演职员挂在剧集上，这里借 item 行存一份便于海报墙查询
            repo::metadata::set_genres(&self.db, target.id, &record.genres).await?;
        }

        Ok(())
    }

    async fn cache_images(&self, item_id: Id, record: &MetadataRecord) -> Result<u64> {
        if !self.cfg.fetch_images {
            return Ok(0);
        }

        let mut n = 0u64;
        for img in &record.images {
            if !self.cfg.image_kinds.contains(&img.kind) {
                continue;
            }
            let key = ImageCache::key_for(&img.url, &img.kind);

            // 缓存里已有就顺手填上 cached_path；没有就只记 URL，
            // 等 UI 请求 /images 时再按需拉取 —— 全量预下载会让
            // 1,000 部片的首次刮削多花十几分钟。
            let path = match &self.cache {
                Some(c) => c.get(&key).await,
                None => None,
            };

            let cached = path.as_ref().map(|p| p.to_string_lossy().to_string());
            repo::image::bind(
                &self.db,
                &repo::image::ImageBind {
                    item_id,
                    kind: &img.kind,
                    url: Some(&img.url),
                    cache_key: path.as_ref().map(|_| key.as_str()),
                    provider: Some(&record.provider),
                    width: img.width,
                    height: img.height,
                    cached_path: cached.as_deref(),
                },
            )
            .await?;
            n += 1;
        }
        Ok(n)
    }
}

async fn series_locked_of(db: &Db, series_id: Id) -> String {
    repo::metadata::locked_fields_of_series(db, series_id)
        .await
        .unwrap_or_else(|_| "[]".into())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let head: String = s.chars().take(n).collect();
    format!("{head}…")
}

/// 取消令牌。扫描和刮削共用同一套语义。
#[derive(Debug, Clone, Default)]
pub struct Cancel {
    inner: Arc<std::sync::atomic::AtomicBool>,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.inner.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.inner.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CandidateView {
    pub provider: String,
    pub provider_id: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub overview: Option<String>,
    pub poster: Option<String>,
    pub score: f64,
    pub confidence: String,
}

#[derive(Clone, Default)]
struct Stats {
    matched: Arc<AtomicU64>,
    review: Arc<AtomicU64>,
    failed: Arc<AtomicU64>,
    skipped: Arc<AtomicU64>,
}

impl Stats {
    fn record(&self, o: Outcome) {
        let slot = match o {
            Outcome::Matched => &self.matched,
            Outcome::Review => &self.review,
            Outcome::Failed => &self.failed,
            Outcome::Skipped => &self.skipped,
        };
        slot.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> ScrapeStats {
        ScrapeStats {
            matched: self.matched.load(Ordering::Relaxed),
            review: self.review.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
            images: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_rate_counts_review_as_hit() {
        let s = ScrapeStats {
            matched: 90,
            review: 8,
            failed: 1,
            skipped: 1,
            images: 0,
        };
        assert!((s.hit_rate() - 0.98).abs() < 1e-9);
    }

    #[test]
    fn empty_stats_hit_rate_is_zero() {
        assert_eq!(ScrapeStats::default().hit_rate(), 0.0);
    }

    #[test]
    fn outcome_maps_to_status() {
        assert_eq!(Outcome::Matched.job_status(), "done");
        assert_eq!(Outcome::Review.job_status(), "review");
        assert_eq!(Outcome::Skipped.job_status(), "skipped");
        assert_eq!(Outcome::Failed.job_status(), "failed");
    }

    #[test]
    fn cancel_sticks() {
        let c = Cancel::new();
        assert!(!c.is_cancelled());
        c.cancel();
        assert!(c.is_cancelled());
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("abc", 10), "abc");
        assert_eq!(truncate(&"x".repeat(20), 5), "xxxxx…");
    }
}
