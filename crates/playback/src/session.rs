//! 播放会话：创建 / 心跳 / 关闭 / 续播点。
//!
//! 两条现实约束决定了这里的设计：
//!
//! 1. **心跳是高频写**（5 秒一次）。所以用 UPDATE 而不是 INSERT，
//!    一次两小时的播放只产生一行数据，不是 1440 行。
//! 2. **客户端会异常退出**（断电、断网、崩溃）。所以不能只靠客户端上报
//!    "我关了"，必须有 `reap_idle` 兜底，否则"继续观看"里全是僵尸会话。

use anyhow::Result;
use chrono::Utc;
use whatmedia_core::{Id, PlaybackSession, PlaybackStrategy};
use whatmedia_database::repo::{
    create_session, delete_session, force_position, get_session, heartbeat, resume_points,
    stop_session, NewSession, ResumePoint,
};
use whatmedia_database::Db;

/// 开播请求。
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub item_id: Id,
    pub file_id: Id,
    pub strategy: PlaybackStrategy,
    pub client_id: Option<String>,
    pub device_id: Option<String>,
    pub reason: Option<String>,
    pub position_ms: i64,
    pub duration_ms: Option<i64>,
    pub user_id: Option<Id>,
}

/// 会话管理器。无状态，可随时 Clone 到别的任务里。
#[derive(Debug, Clone)]
pub struct SessionManager {
    db: Db,
    /// 多久没心跳就认为客户端已经走了（秒）。
    idle_timeout_secs: i64,
}

impl SessionManager {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            idle_timeout_secs: 120,
        }
    }

    pub fn with_idle_timeout(mut self, secs: i64) -> Self {
        self.idle_timeout_secs = secs;
        self
    }

    /// 开播参数。合成一个结构体而不是 9 个位置参数：
    /// 位置参数一多，调用点就会开始出现"传反了"的 bug。
    pub async fn start(&self, req: StartRequest) -> Result<PlaybackSession> {
        create_session(
            &self.db,
            NewSession {
                item_id: req.item_id,
                file_id: req.file_id,
                strategy: req.strategy,
                user_id: req.user_id,
                device_id: req.device_id,
                client_id: req.client_id,
                strategy_reason: req.reason,
                position_ms: req.position_ms,
                duration_ms: req.duration_ms,
            },
        )
        .await
    }

    pub async fn heartbeat(
        &self,
        id: Id,
        position_ms: i64,
        duration_ms: Option<i64>,
    ) -> Result<bool> {
        heartbeat(&self.db, id, position_ms, duration_ms).await
    }

    /// 关闭会话。位置以最后一次上报为准（不传则不改）。
    pub async fn stop(&self, id: Id, position_ms: Option<i64>) -> Result<()> {
        if let Some(p) = position_ms {
            force_position(&self.db, id, p).await?;
        }
        stop_session(&self.db, id, position_ms).await
    }

    /// 删除会话（例如用户主动从“继续观看”列表中移除）。
    pub async fn delete(&self, id: Id) -> Result<()> {
        delete_session(&self.db, id).await
    }

    pub async fn get(&self, id: Id) -> Result<Option<PlaybackSession>> {
        get_session(&self.db, id).await
    }

    pub async fn resume(&self, user_id: Option<Id>, limit: u32) -> Result<Vec<ResumePoint>> {
        resume_points(&self.db, user_id, limit).await
    }

    /// 把"很久没心跳"的会话标记为已停止。
    ///
    /// 客户端崩溃时不会发 stop，没有这一步，"继续观看"会一直挂着
    /// 几十条实际上早就没人看的会话。
    pub async fn reap_idle(&self) -> Result<usize> {
        self.reap_idle_with(self.idle_timeout_secs).await
    }

    pub async fn reap_idle_with(&self, idle_secs: i64) -> Result<usize> {
        let cutoff = (Utc::now() - chrono::Duration::seconds(idle_secs)).to_rfc3339();
        let r = sqlx::query(
            "UPDATE playback_session SET stopped_at = ?, updated_at = ?
              WHERE stopped_at IS NULL AND updated_at < ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .bind(cutoff)
        .execute(self.db.pool())
        .await?;
        Ok(r.rows_affected() as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use whatmedia_core::MediaKind;

    async fn db() -> Db {
        let d = Db::open_in_memory().await.unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .join("migrations");
        d.migrate(&dir).await.unwrap();
        d
    }

    async fn seed(d: &Db) -> (Id, Id) {
        let library = Id::new_v4();
        sqlx::query(
            "INSERT INTO library (id,name,kind,storage_provider,root_path,created_at)
             VALUES (?,'lib','mixed','local','/m','2026-01-01T00:00:00Z')",
        )
        .bind(library.to_string())
        .execute(d.pool())
        .await
        .unwrap();
        let item = Id::new_v4();
        let version = Id::new_v4();
        let file = Id::new_v4();
        sqlx::query(
            "INSERT INTO media_item (id,library_id,kind,title,locked_fields,added_at,match_title)
             VALUES (?,?,'movie','Arrival','[]','2026-01-01T00:00:00Z','Arrival')",
        )
        .bind(item.to_string())
        .bind(library.to_string())
        .execute(d.pool())
        .await
        .unwrap();
        sqlx::query("INSERT INTO media_version (id,item_id,name,hdr_type,is_default) VALUES (?,?,'v','sdr',1)")
            .bind(version.to_string())
            .bind(item.to_string())
            .execute(d.pool())
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO media_file (id,version_id,storage_provider,path,size_bytes,file_fingerprint,is_available)
             VALUES (?,?,'local','/m/a.mkv',1,'f',1)",
        )
        .bind(file.to_string())
        .bind(version.to_string())
        .execute(d.pool())
        .await
        .unwrap();
        let _ = MediaKind::Movie;
        (item, file)
    }

    #[tokio::test]
    async fn session_lifecycle() {
        let d = db().await;
        let (item, file) = seed(&d).await;
        let m = SessionManager::new(d.clone());

        let s = m
            .start(StartRequest {
                item_id: item,
                file_id: file,
                strategy: PlaybackStrategy::DirectPlay,
                client_id: Some("web".into()),
                device_id: Some("dev-1".into()),
                reason: None,
                position_ms: 0,
                duration_ms: Some(7_200_000),
                user_id: None,
            })
            .await
            .unwrap();

        assert!(m.heartbeat(s.id, 1_200_000, None).await.unwrap());
        let got = m.get(s.id).await.unwrap().unwrap();
        assert_eq!(got.position_ms, 1_200_000);

        m.stop(s.id, Some(1_500_000)).await.unwrap();
        let got = m.get(s.id).await.unwrap().unwrap();
        assert!(got.stopped_at.is_some());
        assert_eq!(got.position_ms, 1_500_000);
    }

    #[tokio::test]
    async fn idle_sessions_are_reaped() {
        let d = db().await;
        let (item, file) = seed(&d).await;
        let m = SessionManager::new(d.clone());
        let s = m
            .start(StartRequest {
                item_id: item,
                file_id: file,
                strategy: PlaybackStrategy::Remux,
                client_id: None,
                device_id: None,
                reason: Some("容器不支持".into()),
                position_ms: 600_000,
                duration_ms: Some(7_200_000),
                user_id: None,
            })
            .await
            .unwrap();

        // 还没超时 → 不动。
        assert_eq!(m.reap_idle().await.unwrap(), 0);
        // 阈值设为 -1（cutoff 在未来）→ 一定命中。
        assert_eq!(m.reap_idle_with(-1).await.unwrap(), 1);
        assert!(m.get(s.id).await.unwrap().unwrap().stopped_at.is_some());
    }

    #[tokio::test]
    async fn resume_list_returns_latest_per_item() {
        let d = db().await;
        let (item, file) = seed(&d).await;
        let m = SessionManager::new(d.clone());
        m.start(StartRequest {
            item_id: item,
            file_id: file,
            strategy: PlaybackStrategy::DirectPlay,
            client_id: None,
            device_id: None,
            reason: None,
            position_ms: 900_000,
            duration_ms: Some(7_200_000),
            user_id: None,
        })
        .await
        .unwrap();
        let pts = m.resume(None, 10).await.unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].title, "Arrival");
    }
}
