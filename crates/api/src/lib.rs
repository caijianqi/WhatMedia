//! Axum 路由层 —— 系统唯一对外出口。
//!
//! 约定（见 DEVELOPMENT-PLAN.md §七）：
//!   · 前缀 /api/v1，全 JSON
//!   · 错误统一 { error: { code, message, trace_id } }
//!   · 分页 { data: [], page: { total, page, page_size } }

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::Utc;
use mosaic_config::AppConfig;
use serde::Serialize;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub started_at: chrono::DateTime<Utc>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config: Arc::new(config),
            started_at: Utc::now(),
        }
    }
}

/// 统一错误信封。所有 4xx/5xx 都必须用它，不得裸返回字符串。
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    pub code: &'static str,
    pub message: String,
    pub trace_id: Option<String>,
}

pub fn error_response(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
) -> Response {
    let body = ErrorBody {
        error: ErrorDetail {
            code,
            message: message.into(),
            trace_id: None,
        },
    };
    (status, Json(body)).into_response()
}

async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let report = mosaic_diagnostics::collect(env!("CARGO_PKG_VERSION"), state.started_at);
    (StatusCode::OK, Json(report))
}

/// P0 阶段未实现的端点统一返回 501，避免前端拿到 404 后误判为"接口不存在"。
async fn not_implemented() -> Response {
    error_response(
        StatusCode::NOT_IMPLEMENTED,
        "NOT_IMPLEMENTED",
        "该端点在当前阶段尚未实现",
    )
}

async fn fallback() -> Response {
    error_response(StatusCode::NOT_FOUND, "NOT_FOUND", "资源不存在")
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // 健康检查：容器 HEALTHCHECK 与 CI 冒烟都打这个
        .route("/healthz", get(healthz))
        .route("/api/v1/health", get(healthz))
        // v1 端点占位（P1 起逐个实现）
        .route("/api/v1/libraries", get(not_implemented))
        .route("/api/v1/media", get(not_implemented))
        .route("/api/v1/movies", get(not_implemented))
        .route("/api/v1/series", get(not_implemented))
        .route("/api/v1/episodes", get(not_implemented))
        .route("/api/v1/search", get(not_implemented))
        .route("/api/v1/tasks", get(not_implemented))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
        .fallback(fallback)
}

/// 供集成测试使用：无需绑定端口即可发起请求。
#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use serde_json::json;
    use tower::ServiceExt;

    fn test_router() -> Router {
        let mut cfg = AppConfig::default();
        cfg.server.port = 0;
        build_router(AppState::new(cfg))
    }

    #[tokio::test]
    async fn healthz_returns_ok() {
        let res = test_router()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_route_returns_error_envelope() {
        let res = test_router()
            .oneshot(Request::builder().uri("/nope").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn error_body_shape_is_stable() {
        let body = ErrorBody {
            error: ErrorDetail {
                code: "X",
                message: "y".into(),
                trace_id: None,
            },
        };
        assert_eq!(json!(body)["error"]["code"], "X");
    }
}
