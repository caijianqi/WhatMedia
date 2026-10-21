//! 播放决策与会话。
//!
//! 模块分层：
//!
//! ```text
//! decision   策略：Client + Media + Server + Network → 四条路径之一
//!   ├─ capability  能力事实（客户端 / 片源 / 服务端）
//!   └─ session     播放会话与续播点
//! ```
//!
//! 本 crate **不碰 ffmpeg**（那是 `whatmedia-transcoder` 的事），
//! 也不写 SQL（那是 `whatmedia-database` 的事）。它只回答"走哪条路"。

pub mod capability;
pub mod decision;
pub mod session;

pub use capability::{
    AudioTrack, ClientCapability, MediaCapability, ServerCapability, VideoTrack,
};
pub use decision::{decide, DecisionInput, PlaybackDecision, Reason};
pub use session::{SessionManager, StartRequest};
