use thiserror::Error;

pub type Result<T> = std::result::Result<T, MosaicError>;

#[derive(Debug, Error)]
pub enum MosaicError {
    #[error("storage error [{provider}]: {message}")]
    Storage { provider: String, message: String },

    /// 路径穿越 / 符号链接逃逸 / 越权访问。永远不要降级为普通 NotFound。
    #[error("path rejected: {message}")]
    PathRejected { message: String },

    #[error("not found: {entity} {id}")]
    NotFound { entity: &'static str, id: String },

    #[error("metadata provider error [{provider}]: {message}")]
    Metadata { provider: String, message: String },

    #[error("ffmpeg error: {message}")]
    Ffmpeg { message: String },

    #[error("playback unsupported: {reason}")]
    PlaybackUnsupported { reason: String },

    #[error("task cancelled")]
    Cancelled,

    #[error("unauthorized: {message}")]
    Unauthorized { message: String },

    /// AI 越权。Tool 层拒绝执行危险操作时返回。
    #[error("ai tool denied: {tool} reason: {reason}")]
    AiDenied { tool: String, reason: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("internal: {0}")]
    Internal(String),
}
