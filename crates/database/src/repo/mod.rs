//! 数据访问函数。**全仓库只有这里允许出现 SQL。**
//!
//! 调用方拿到的都是 `whatmedia_core` 的领域模型，不是 Row。
//! 这样"SQLite → PostgreSQL"的切换只需要重写本目录。

pub mod auth;
pub mod image;
pub mod library;
pub mod live;
pub mod media;
pub mod metadata;
pub mod playback;
pub mod scan;
pub mod search;
pub mod aggregator;
pub mod social;
pub mod storage;
pub mod tv;

pub use aggregator::{
    cleanup_expired_cache, delete_vod_source, get_search_cache, get_vod_source,
    list_active_vod_sources, list_vod_sources, set_search_cache, upsert_vod_source,
};

pub use storage::{
    delete_storage_account, get_storage_account, insert_storage_account, list_storage_accounts,
    update_storage_account, CreateStorageAccountInput, UpdateStorageAccountInput,
};

pub use auth::{
    count_users, decide_approval, delete_device, delete_user, find_device_by_name, get_approval,
    get_device, get_session_by_hash, get_user_by_id, get_user_by_username, insert_approval,
    insert_device, insert_session, insert_user, list_pending_approvals, list_sessions,
    list_user_devices, list_users, pending_approval_for_user, prune_sessions, record_login_failure,
    record_login_success, revoke_session, revoke_user_sessions, set_device_approved,
    set_user_disabled, set_user_limits, set_user_password, set_user_pin, set_user_role,
    touch_device, touch_session, username_exists, ApprovalRow, DeviceRow, NewApproval, NewDevice,
    NewSession as NewAuthSession, NewUser, SessionRow, SessionWithUser, UserAuthRow, UserView,
};
pub use image::ItemImage;
pub use library::{find_library_by_root, get_library, insert_library, list_libraries};
pub use live::{
    bump_live_session, current_program, delete_live_session, delete_live_source,
    find_channel_by_epg_id, get_live_channel, get_live_source, insert_live_source,
    list_channels_by_source, list_dead_live_sessions, list_live_channels, list_live_sources,
    live_ref_count, release_live_session, replace_channels, replace_programs, set_channel_enabled,
    set_channels_disabled, set_source_sync_result, upcoming_programs, LiveChannelRow,
    LiveSourceRow, ProgramRow,
};
pub use media::{
    count_items, get_item, kind_counts, list_items, replace_streams, upsert_file, upsert_item,
    upsert_version, FileWithStreams, ItemDetail, KindCount, VersionWithFiles,
};
pub use metadata::{job_stats, requeue_all, series_item_ids, CreditRow, JobStats, ScrapeTarget};
pub use playback::{
    create_session, delete_stream_session, file_target, files_for_item, force_position,
    get_session, get_stream_session, get_transcode_job, heartbeat, insert_stream_session,
    insert_transcode_job, list_idle_stream_sessions, resume_points, set_stream_state,
    set_transcode_state, stop_session, touch_stream_session, FileTarget, NewSession,
    NewStreamSession, NewTranscodeJob, ResumePoint, StreamSession,
};
pub use scan::{file_index_for_library, mark_files_missing};
pub use search::search_items;
pub use social::{
    add_favorite, count_unread, delete_notification, delete_server_setting, delete_user_setting,
    favorite_count_for_item, is_favorited, library_storage, list_favorites, list_notifications,
    list_server_settings, list_user_settings, mark_all_read, mark_notification_read, notify_admins,
    notify_users, prune_notifications, remove_favorite, set_server_setting, set_user_setting,
    FavoriteRow, LibraryStorage, NewNotification, NotificationRow, NotifyLevel, SettingRow,
};
pub use tv::{
    list_episodes, list_seasons, list_series, series_of_item, EpisodeSummary, SeasonSummary,
    SeriesRef, SeriesSummary,
};

use serde::{Deserialize, Serialize};
use whatmedia_core::{Id, MediaKind, Page};

/// 排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    Desc,
    Asc,
}

impl SortOrder {
    /// 只输出白名单里的 SQL 关键字 —— 排序字段绝不能来自未校验的用户输入。
    pub fn sql(self) -> &'static str {
        match self {
            SortOrder::Asc => "ASC",
            SortOrder::Desc => "DESC",
        }
    }
}

/// 允许的排序维度。同样是白名单，杜绝 SQL 注入。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ItemSort {
    #[default]
    AddedAt,
    Title,
    Year,
    Rating,
}

impl ItemSort {
    pub fn sql(self) -> &'static str {
        match self {
            ItemSort::AddedAt => "added_at",
            ItemSort::Title => "title",
            ItemSort::Year => "year",
            ItemSort::Rating => "rating",
        }
    }
}

/// 列表查询参数。分页 / 排序 / 过滤的统一入口。
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ItemQuery {
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub library_id: Option<Id>,
    pub kind: Option<MediaKind>,
    #[serde(default, deserialize_with = "deserialize_opt_i32_lenient")]
    pub year: Option<i32>,
    #[serde(default)]
    pub sort: ItemSort,
    #[serde(default)]
    pub order: SortOrder,
    #[serde(default = "default_page", deserialize_with = "deserialize_page_lenient")]
    pub page: u32,
    #[serde(
        default = "default_page_size",
        deserialize_with = "deserialize_page_size_lenient"
    )]
    pub page_size: u32,
    #[serde(default, deserialize_with = "deserialize_opt_bool_lenient")]
    pub aggregate_series: Option<bool>,
}

fn default_page() -> u32 {
    1
}

fn default_page_size() -> u32 {
    50
}

fn deserialize_page_lenient<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = u32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u32 integer or string containing a u32")
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            u32::try_from(v).map_err(serde::de::Error::custom)
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            u32::try_from(v).map_err(serde::de::Error::custom)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                Ok(default_page())
            } else {
                trimmed.parse::<u32>().map_err(serde::de::Error::custom)
            }
        }
    }

    deserializer.deserialize_any(Visitor)
}

fn deserialize_page_size_lenient<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = u32;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a u32 integer or string containing a u32")
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            u32::try_from(v).map_err(serde::de::Error::custom)
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            u32::try_from(v).map_err(serde::de::Error::custom)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                Ok(default_page_size())
            } else {
                trimmed.parse::<u32>().map_err(serde::de::Error::custom)
            }
        }
    }

    deserializer.deserialize_any(Visitor)
}

fn deserialize_opt_i32_lenient<'de, D>(deserializer: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Option<i32>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an optional i32 integer or string")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
        where
            D2: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(Visitor)
        }

        fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            i32::try_from(v).map(Some).map_err(serde::de::Error::custom)
        }

        fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            i32::try_from(v).map(Some).map_err(serde::de::Error::custom)
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                trimmed.parse::<i32>().map(Some).map_err(serde::de::Error::custom)
            }
        }
    }

    deserializer.deserialize_any(Visitor)
}

fn deserialize_opt_bool_lenient<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Option<bool>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("an optional boolean or string")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D2>(self, deserializer: D2) -> Result<Self::Value, D2::Error>
        where
            D2: serde::Deserializer<'de>,
        {
            deserializer.deserialize_any(Visitor)
        }

        fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(v))
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            let s = v.trim().to_ascii_lowercase();
            match s.as_str() {
                "" => Ok(None),
                "true" | "1" | "yes" => Ok(Some(true)),
                "false" | "0" | "no" => Ok(Some(false)),
                _ => Err(serde::de::Error::custom(format!("invalid boolean: {v}"))),
            }
        }
    }

    deserializer.deserialize_any(Visitor)
}

impl Default for ItemQuery {
    fn default() -> Self {
        Self {
            library_id: None,
            kind: None,
            year: None,
            sort: ItemSort::default(),
            order: SortOrder::default(),
            page: 1,
            page_size: 50,
            aggregate_series: None,
        }
    }
}

impl ItemQuery {
    /// 页码从 1 开始；page_size 上限 200，防止一次性拉爆内存。
    pub fn clamp(&self) -> (u32, u32) {
        let page = self.page.max(1);
        let size = self.page_size.clamp(1, 200);
        (page, size)
    }

    pub fn offset(&self) -> i64 {
        let (page, size) = self.clamp();
        ((page - 1) as i64) * size as i64
    }
}

/// 列表项视图：海报墙只需要这些字段，不该把 overview 也拉出来。
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ItemSummary {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub id: Id,
    pub kind: MediaKind,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
    pub rating: Option<f64>,
    pub runtime_ms: Option<i64>,
    pub added_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub episode_count: Option<i64>,
}

impl ItemSummary {
    pub fn from_row(item: &whatmedia_core::MediaItem) -> Self {
        Self {
            id: item.id,
            kind: item.kind,
            title: item.title.clone(),
            original_title: item.original_title.clone(),
            year: item.year,
            rating: item.rating,
            runtime_ms: item.runtime_ms,
            added_at: item.added_at.to_rfc3339(),
            episode_count: None,
        }
    }
}

/// 把 `Vec<T>` 与总数包装成统一分页信封。
pub fn page_of<T>(data: Vec<T>, total: i64, query: &ItemQuery) -> Page<T> {
    let (page, size) = query.clamp();
    Page::new(data, total, page, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize)]
    struct MockSearch {
        pub q: String,
        #[serde(flatten)]
        pub page: ItemQuery,
    }

    #[test]
    fn item_query_deserializes_flattened_strings_from_query_params() {
        let json_str = r#"{"q":"狂飙","page_size":"6","page":"2","year":"2023","aggregate_series":"true"}"#;
        let s: MockSearch = serde_json::from_str(json_str).unwrap();
        assert_eq!(s.q, "狂飙");
        assert_eq!(s.page.page_size, 6);
        assert_eq!(s.page.page, 2);
        assert_eq!(s.page.year, Some(2023));
        assert_eq!(s.page.aggregate_series, Some(true));
    }

    #[test]
    fn item_query_deserializes_defaults_when_empty() {
        let json_str = r#"{"q":"狂飙","page_size":"","page":""}"#;
        let s: MockSearch = serde_json::from_str(json_str).unwrap();
        assert_eq!(s.page.page_size, 50);
        assert_eq!(s.page.page, 1);
        assert_eq!(s.page.year, None);
    }
}
