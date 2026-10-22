//! 采集源 Provider 抽象与实现。

use anyhow::Result;
use async_trait::async_trait;

use crate::model::{RawVodItem, VodSourceConfig};

pub mod maccms;
pub use maccms::MacCmsProvider;

#[async_trait]
pub trait VodProvider: Send + Sync {
    /// 唯一标识
    fn id(&self) -> &str;
    /// 友好名称
    fn name(&self) -> &str;
    /// 获取当前配置
    fn config(&self) -> &VodSourceConfig;
    /// 搜索接口
    async fn search(&self, keyword: &str, timeout_ms: u64) -> Result<Vec<RawVodItem>>;
    /// 详情查询接口
    async fn get_detail(&self, vod_id: &str, timeout_ms: u64) -> Result<Option<RawVodItem>>;
}
