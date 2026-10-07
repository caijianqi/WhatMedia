//! SQLx 数据访问层。
//!
//! 分层铁律：**只有本 crate 能写 SQL**。其他 crate 一律通过 `repo::*` 访问数据，
//! 违反这条会让"SQLite → PostgreSQL"的后期切换变成全仓库地毯式修改。

pub mod db;
pub mod repo;
pub mod rows;

pub use db::{migration_files, Db};
pub use rows::{to_hdr, to_video_codec};
