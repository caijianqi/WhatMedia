//! Mosaic 领域模型与核心抽象契约。
//!
//! 本 crate 是依赖分层的根节点：不依赖任何其他 mosaic crate。
//! 这里的 trait 签名在 P0 冻结（见 docs/adr/0000-abstractions.md），
//! 之后只允许新增带默认实现的方法或新增类型，不允许修改既有签名。

pub mod error;
pub mod model;
pub mod provider;

pub use error::{MosaicError, Result};
pub use model::{
    AudioCodec, ByteRange, Container, Episode, HdrType, Id, Library, MediaFile, MediaItem,
    MediaKind, MediaStream, MediaVersion, PlaybackSession, PlaybackSource, PlaybackStrategy,
    Season, Series, StreamType, SubtitleCodec, Task, TaskKind, TaskPriority, TaskState,
    TranscodeJob, TranscodeState, VideoCodec,
};
pub use provider::{
    AiTool, HardwareAccelerationProvider, HardwareKind, HardwareProfile, MediaPlayer,
    MetadataProvider, MetadataQuery, MetadataRecord, ModelProvider, PlayerEvent, PlayerState,
    StorageEntry, StorageProvider, ToolContext, ToolRisk,
};
