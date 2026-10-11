//! 健康检查与系统诊断。
//! P0 只提供最小健康报告；完整的存储连通性 / FFmpeg 能力 / HardwareProfile
//! 诊断在 P5（多来源存储）与 P6（硬件加速）补齐。

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HealthReport {
    pub status: &'static str,
    pub version: String,
    pub arch: &'static str,
    pub os: &'static str,
    pub started_at: DateTime<Utc>,
    pub uptime_secs: i64,
}

/// 进程启动时刻。用 once 保证全进程一致。
static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn mark_started() {
    START.get_or_init(Instant::now);
}

pub fn uptime_secs() -> i64 {
    START
        .get()
        .map(|s| s.elapsed().as_secs() as i64)
        .unwrap_or(0)
}

pub fn collect(version: &str, started_at: DateTime<Utc>) -> HealthReport {
    HealthReport {
        status: "ok",
        version: version.to_string(),
        arch: std::env::consts::ARCH,
        os: std::env::consts::OS,
        started_at,
        uptime_secs: uptime_secs(),
    }
}
