//! 分页 —— 全 API 统一的分页信封。
//!
//! 约定（见 DEVELOPMENT-PLAN.md §七）：列表接口一律返回
//! `{ data: [...], page: { total, page, page_size } }`。
//! 前端只认这一种形状，不允许某个接口自己发明分页格式。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PageInfo {
    pub total: i64,
    pub page: u32,
    pub page_size: u32,
}

impl PageInfo {
    pub fn new(total: i64, page: u32, page_size: u32) -> Self {
        Self {
            total,
            page,
            page_size,
        }
    }

    pub fn page_count(&self) -> u32 {
        if self.page_size == 0 {
            return 0;
        }
        ((self.total + self.page_size as i64 - 1) / self.page_size as i64) as u32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Page<T> {
    pub data: Vec<T>,
    pub page: PageInfo,
}

impl<T> Page<T> {
    pub fn new(data: Vec<T>, total: i64, page: u32, page_size: u32) -> Self {
        Self {
            data,
            page: PageInfo::new(total, page, page_size),
        }
    }

    pub fn empty(page_size: u32) -> Self {
        Self::new(Vec::new(), 0, 1, page_size)
    }

    /// 保住元素类型不变的映射。分页信息原样透传。
    pub fn map<U, F: FnMut(T) -> U>(self, mut f: F) -> Page<U> {
        Page {
            data: self.data.into_iter().map(&mut f).collect(),
            page: self.page,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_count_rounds_up() {
        assert_eq!(PageInfo::new(0, 1, 20).page_count(), 0);
        assert_eq!(PageInfo::new(1, 1, 20).page_count(), 1);
        assert_eq!(PageInfo::new(21, 1, 20).page_count(), 2);
    }
}
