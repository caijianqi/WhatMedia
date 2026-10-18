//! 收藏 / 通知 / 设置 / 存储统计的 HTTP 边界。
//!
//! ## 为什么单独成一个模块
//!
//! 这四个功能在原型里各占一块界面，但**它们没有一个是"凭空画出来的"** ——
//! 这一版把它们从"假按钮"补成真的。放在同一个文件里，是为了让
//! "界面上这块东西背后到底有没有数据"这件事可以被一次性审计完。
//!
//! ## 关于存储统计不报磁盘容量
//!
//! 原型那张卡片画的是"已用 / 总量"的进度条。这里**只报已用的量，不报总量**，
//! 因为跨平台拿磁盘容量要引入平台相关代码（Windows 的 `GetDiskFreeSpaceExW`、
//! Unix 的 `statvfs`），而它换来的是一个"还剩多少"的数字 ——
//! 对影音库真正有用的是**哪个库最占地方**，那个用数据库里的 `size_bytes` 就能算，
//! 而且分库精确。报不出来的一律不报，好过报一个假的 100%。

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use whatmedia_config::{
    setting_spec, validate_setting, SettingKind, SettingScope, SETTING_SPECS,
};
use whatmedia_core::Id;

use crate::auth::Current;
use crate::{bad_request, internal, AppState};

fn deny(code: &'static str, msg: impl Into<String>, status: StatusCode) -> Response {
    crate::error_response(
        status,
        code,
        msg,
    )
}

fn db<'a>(state: &'a AppState) -> Result<whatmedia_database::Db, Response> {
    match state.db_or_503() {
        Ok(d) => Ok(d),
        Err(r) => Err(*r),
    }
}

// ─────────────────────────────────────────────────────────────
// 收藏
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FavoriteListParams {
    #[serde(default = "one")]
    pub page: u32,
    #[serde(default = "fifty")]
    pub page_size: u32,
}

fn one() -> u32 {
    1
}
fn fifty() -> u32 {
    50
}

/// 我收藏的条目。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/me/favorites",
        params(("page" = Option<u32>, Query), ("page_size" = Option<u32>, Query)),
        responses(
            (status = 200, description = "收藏列表", body = FavoritePage),
            (status = 401, description = "未认证", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn list_my_favorites(
    State(state): State<AppState>,
    Current(p): Current,
    Query(q): Query<FavoriteListParams>,
) -> Response {
    use whatmedia_database::repo::list_favorites;
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    match list_favorites(&d, p.user_id, q.page, q.page_size).await {
        Ok((data, total)) => Json(FavoritePage { total, data }).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FavoritePage {
    pub total: i64,
    pub data: Vec<whatmedia_database::repo::FavoriteRow>,
}

/// 收藏 / 取消收藏一个条目。
///
/// 用 PUT + 显式 `favorite` 布尔，而不是 POST/DELETE 两条动词：
/// 详情页那颗心形按钮是**幂等的切换**，"设为已收藏"重复点两次结果一样，
/// 而 POST 语义上会产生第二条记录。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        put,
        path = "/api/v1/items/{id}/favorite",
        params(("id" = uuid::Uuid, Path)),
        request_body = SetFavoriteInput,
        responses(
            (status = 200, description = "已更新", body = FavoriteState),
            (status = 401, description = "未认证", body = crate::ErrorBody),
            (status = 404, description = "条目不存在", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn set_favorite(
    State(state): State<AppState>,
    Current(p): Current,
    Path(id): Path<Id>,
    Json(body): Json<SetFavoriteInput>,
) -> Response {
    use whatmedia_database::repo::{add_favorite, favorite_count_for_item, remove_favorite};
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let r = if body.favorite {
        add_favorite(&d, p.user_id, id).await
    } else {
        remove_favorite(&d, p.user_id, id).await.map(|_| ())
    };
    if let Err(e) = r {
        return internal(e);
    }
    match favorite_count_for_item(&d, id).await {
        Ok(count) => Json(FavoriteState {
            item_id: id,
            favorite: body.favorite,
            count,
        })
        .into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetFavoriteInput {
    pub favorite: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct FavoriteState {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub item_id: Id,
    pub favorite: bool,
    /// 全站有多少人收藏了它。儿童账户也能看到 —— 这不是隐私，是"家里人都爱看"。
    pub count: i64,
}

/// 这个条目的收藏状态（含我自己收没收）。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/items/{id}/favorite",
        params(("id" = uuid::Uuid, Path)),
        responses(
            (status = 200, description = "收藏状态", body = FavoriteState),
            (status = 401, description = "未认证", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn get_favorite(
    State(state): State<AppState>,
    Current(p): Current,
    Path(id): Path<Id>,
) -> Response {
    use whatmedia_database::repo::{favorite_count_for_item, is_favorited};
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let (mine, count) = match tokio::try_join!(
        is_favorited(&d, p.user_id, id),
        favorite_count_for_item(&d, id)
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };
    Json(FavoriteState {
        item_id: id,
        favorite: mine,
        count,
    })
    .into_response()
}

// ─────────────────────────────────────────────────────────────
// 通知
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotificationParams {
    #[serde(default = "twenty")]
    pub limit: u32,
    #[serde(default)]
    pub unread_only: bool,
}

fn twenty() -> u32 {
    20
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct NotificationList {
    pub unread: i64,
    pub data: Vec<whatmedia_database::repo::NotificationRow>,
    /// 待家长审批的设备数。**只对管理员有意义** —— 其余人恒为 0。
    ///
    /// 它是从 `approval_request` 现算的，不是通知表里的一条记录：
    /// 审批有状态（批了就没了），塞进通知表就得回头去删，多一份可能不一致的数据。
    pub pending_approvals: i64,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/notifications",
        params(("limit" = Option<u32>, Query), ("unread_only" = Option<bool>, Query)),
        responses(
            (status = 200, description = "通知列表", body = NotificationList),
            (status = 401, description = "未认证", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn list_notifications(
    State(state): State<AppState>,
    Current(p): Current,
    Query(q): Query<NotificationParams>,
) -> Response {
    use whatmedia_auth::Ability;
    use whatmedia_database::repo::{count_unread, list_notifications, list_pending_approvals};
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let (unread, data) = match tokio::try_join!(
        count_unread(&d, p.user_id),
        list_notifications(&d, p.user_id, q.limit, q.unread_only)
    ) {
        Ok(v) => v,
        Err(e) => return internal(e),
    };

    // 待审批只对能管人的人可见：孩子不该看到"有台设备等你批"这种提示。
    let pending = if p.can(Ability::ManageUsers) {
        list_pending_approvals(&d, None).await.map(|v| v.len() as i64).unwrap_or(0)
    } else {
        0
    };

    Json(NotificationList {
        unread,
        data,
        pending_approvals: pending,
    })
    .into_response()
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/notifications/{id}/read",
        params(("id" = uuid::Uuid, Path)),
        responses(
            (status = 204, description = "已标记"),
            (status = 401, description = "未认证", body = crate::ErrorBody),
            (status = 404, description = "不是你的通知或已读", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn read_notification(
    State(state): State<AppState>,
    Current(p): Current,
    Path(id): Path<Id>,
) -> Response {
    use whatmedia_database::repo::mark_notification_read;
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    match mark_notification_read(&d, p.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => deny("NOT_FOUND", "通知不存在或已读", StatusCode::NOT_FOUND),
        Err(e) => internal(e),
    }
}

/// 全部标为已读。返回实际更新的条数 —— 前端要显示"已清空 N 条"。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        post,
        path = "/api/v1/notifications/read-all",
        responses(
            (status = 200, description = "已全部标记", body = ReadAllResult),
            (status = 401, description = "未认证", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn read_all_notifications(
    State(state): State<AppState>,
    Current(p): Current,
) -> Response {
    use whatmedia_database::repo::mark_all_read;
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    match mark_all_read(&d, p.user_id).await {
        Ok(updated) => Json(ReadAllResult { updated }).into_response(),
        Err(e) => internal(e),
    }
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReadAllResult {
    pub updated: u64,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        delete,
        path = "/api/v1/notifications/{id}",
        params(("id" = uuid::Uuid, Path)),
        responses(
            (status = 204, description = "已删除"),
            (status = 401, description = "未认证", body = crate::ErrorBody),
            (status = 404, description = "不是你的通知", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn delete_notification(
    State(state): State<AppState>,
    Current(p): Current,
    Path(id): Path<Id>,
) -> Response {
    use whatmedia_database::repo::delete_notification;
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    match delete_notification(&d, p.user_id, id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => deny("NOT_FOUND", "通知不存在", StatusCode::NOT_FOUND),
        Err(e) => internal(e),
    }
}

// ─────────────────────────────────────────────────────────────
// 设置
// ─────────────────────────────────────────────────────────────

/// 界面渲染设置表单所需的全部信息。
///
/// `value` 是**当前生效值**，`default` 是 TOML/环境变量给的值 ——
/// 两者不同就说明这条被改过，界面上要显示"已改动"和一个"恢复默认"按钮。
/// 少了 `default` 的话用户根本不知道自己改没改过。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingEntry {
    pub key: String,
    pub label: String,
    pub hint: String,
    pub scope: String,
    pub kind: String,
    pub choices: Vec<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub value: String,
    pub default: String,
    pub overridden: bool,
    /// 改完要不要重启服务才生效。
    ///
    /// **当前 server 级全部是 true**：刮削器、图片缓存、转码运行时都是在
    /// 启动时按配置构造好的，改配置没法热替换它们。
    /// 宁可如实写着"要重启"，也不要假装即时生效。
    pub restart_required: bool,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SettingsView {
    pub entries: Vec<SettingEntry>,
    /// 启动时被拒掉的覆盖项。正常情况下是空的；
    /// 有内容说明有人在库里写了不合法的值，界面上要露出来而不是静默忽略。
    pub rejected: Vec<String>,
}

/// 读全部可写设置。
///
/// 用户级设置按**当前登录者**返回；管理员额外拿到 server 级那一批。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/settings",
        responses(
            (status = 200, description = "设置项", body = SettingsView),
            (status = 401, description = "未认证", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn get_settings(State(state): State<AppState>, Current(p): Current) -> Response {
    use whatmedia_auth::Ability;
    use whatmedia_database::repo::{list_server_settings, list_user_settings};
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };

    let overrides: std::collections::HashMap<String, String> = if p.can(Ability::ManageSettings) {
        list_server_settings(&d)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| (r.key, r.value))
            .collect()
    } else {
        std::collections::HashMap::new()
    };
    let mine: std::collections::HashMap<String, String> = list_user_settings(&d, p.user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| (r.key, r.value))
        .collect();

    let mut entries = Vec::with_capacity(SETTING_SPECS.len());
    for spec in SETTING_SPECS {
        let (map, is_admin_only) = match spec.scope {
            SettingScope::Server => (&overrides, true),
            SettingScope::User => (&mine, false),
        };
        if is_admin_only && !p.can(Ability::ManageSettings) {
            continue;
        }
        let default = state
            .config
            .get_setting(spec.key)
            .unwrap_or_else(|| default_for(spec));
        let (value, overridden) = match map.get(spec.key) {
            Some(v) => (v.clone(), true),
            None => (default.clone(), false),
        };
        entries.push(SettingEntry {
            key: spec.key.to_string(),
            label: spec.label.to_string(),
            hint: spec.hint.to_string(),
            scope: match spec.scope {
                SettingScope::Server => "server".to_string(),
                SettingScope::User => "user".to_string(),
            },
            kind: match spec.kind {
                SettingKind::Bool => "bool",
                SettingKind::Int => "int",
                SettingKind::Float => "float",
                SettingKind::Text => "text",
                SettingKind::Enum => "enum",
            }
            .to_string(),
            choices: spec.choices.iter().map(|c| c.to_string()).collect(),
            min: spec.min,
            max: spec.max,
            value,
            default,
            overridden,
            restart_required: spec.scope == SettingScope::Server,
        });
    }

    Json(SettingsView {
        entries,
        rejected: state.rejected_settings(),
    })
    .into_response()
}

fn default_for(spec: &whatmedia_config::SettingSpec) -> String {
    match spec.kind {
        SettingKind::Bool => "false".to_string(),
        SettingKind::Enum => spec.choices.first().unwrap_or(&"").to_string(),
        _ => String::new(),
    }
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetSettingInput {
    pub key: String,
    pub value: String,
}

/// 改一条设置。
///
/// **先过白名单再过校验，最后才落库。** 顺序反过来的话，
/// 一个非法值会在"看起来写成功了"之后在启动时被拒掉 ——
/// 那时界面已经打开不了，用户无法自己改回来。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        put,
        path = "/api/v1/settings",
        request_body = SetSettingInput,
        responses(
            (status = 200, description = "已保存", body = SettingEntry),
            (status = 400, description = "键不在白名单内或值不合法", body = crate::ErrorBody),
            (status = 401, description = "未认证", body = crate::ErrorBody),
            (status = 403, description = "无权改 server 级设置", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn put_setting(
    State(state): State<AppState>,
    Current(p): Current,
    Json(body): Json<SetSettingInput>,
) -> Response {
    use whatmedia_auth::Ability;
    use whatmedia_database::repo::{set_server_setting, set_user_setting};
    let Some(spec) = setting_spec(&body.key) else {
        return bad_request(format!(
            "「{}」不在可写设置白名单内",
            body.key
        ));
    };
    if let Err(e) = validate_setting(spec, &body.value) {
        return bad_request(format!("{}：{}", spec.label, e));
    }
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };

    match spec.scope {
        SettingScope::Server => {
            if !p.can(Ability::ManageSettings) {
                return deny(
                    "FORBIDDEN",
                    "只有管理员能改全服设置",
                    StatusCode::FORBIDDEN,
                );
            }
            if let Err(e) = set_server_setting(&d, spec.key, &body.value, p.user_id).await {
                return internal(e);
            }
        }
        SettingScope::User => {
            if let Err(e) = set_user_setting(&d, p.user_id, spec.key, &body.value).await {
                return internal(e);
            }
        }
    }

    Json(SettingEntry {
        key: spec.key.to_string(),
        label: spec.label.to_string(),
        hint: spec.hint.to_string(),
        scope: match spec.scope {
            SettingScope::Server => "server".to_string(),
            SettingScope::User => "user".to_string(),
        },
        kind: match spec.kind {
            SettingKind::Bool => "bool",
            SettingKind::Int => "int",
            SettingKind::Float => "float",
            SettingKind::Text => "text",
            SettingKind::Enum => "enum",
        }
        .to_string(),
        choices: spec.choices.iter().map(|c| c.to_string()).collect(),
        min: spec.min,
        max: spec.max,
        value: body.value,
        default: state
            .config
            .get_setting(spec.key)
            .unwrap_or_else(|| default_for(spec)),
        overridden: true,
        restart_required: spec.scope == SettingScope::Server,
    })
    .into_response()
}

/// 恢复默认 = 删掉覆盖行。
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        delete,
        path = "/api/v1/settings/{key}",
        params(("key" = String, Path)),
        responses(
            (status = 204, description = "已恢复默认"),
            (status = 400, description = "键不在白名单内", body = crate::ErrorBody),
            (status = 403, description = "无权改 server 级设置", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn reset_setting(
    State(state): State<AppState>,
    Current(p): Current,
    Path(key): Path<String>,
) -> Response {
    use whatmedia_auth::Ability;
    use whatmedia_database::repo::{delete_server_setting, delete_user_setting};
    let Some(spec) = setting_spec(&key) else {
        return bad_request(format!("「{key}」不在可写设置白名单内"));
    };
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let r = match spec.scope {
        SettingScope::Server => {
            if !p.can(Ability::ManageSettings) {
                return deny(
                    "FORBIDDEN",
                    "只有管理员能改全服设置",
                    StatusCode::FORBIDDEN,
                );
            }
            delete_server_setting(&d, &key).await
        }
        SettingScope::User => delete_user_setting(&d, p.user_id, &key).await,
    };
    // 删不掉（数据库挂了）不该表现为"看起来恢复成功了"。
    if let Err(e) = r {
        return internal(e);
    }
    StatusCode::NO_CONTENT.into_response()
}

// ─────────────────────────────────────────────────────────────
// 存储统计
// ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StorageView {
    pub libraries: Vec<whatmedia_database::repo::LibraryStorage>,
    /// 缓存目录（图片 + HLS 分片）的占用。
    ///
    /// **这个是真的扫盘**：图片缓存不在数据库里有大小记录，
    /// 而 HLS 分片更是短命的临时文件。走一遍目录是唯一准的办法，
    /// 所以调用频率必须低 —— 前端只在设置页打开时拉一次。
    pub cache: CacheUsage,
    pub total_bytes: i64,
}

#[derive(Debug, Serialize, Default)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CacheUsage {
    pub bytes: i64,
    pub entries: i64,
    /// 目录不存在或读不出来时 true。界面据此显示"暂无缓存"而不是 0B。
    pub unavailable: bool,
}

#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/api/v1/system/storage",
        responses(
            (status = 200, description = "存储占用", body = StorageView),
            (status = 401, description = "未认证", body = crate::ErrorBody),
        ),
        tag = "social"
    )
)]
pub async fn storage_stats(State(state): State<AppState>, Current(_p): Current) -> Response {
    use whatmedia_database::repo::library_storage;
    let d = match db(&state) {
        Ok(d) => d,
        Err(r) => return r,
    };
    let libraries = match library_storage(&d).await {
        Ok(v) => v,
        Err(e) => return internal(e),
    };

    // 扫盘是同步阻塞的 IO，扔到阻塞线程池里，别占着 tokio 的 worker。
    let images_dir = state.config.images_dir();
    let hls_dir = state.config.paths.cache_dir.join("hls");
    let cache = tokio::task::spawn_blocking(move || {
        let a = dir_usage(&images_dir);
        let b = dir_usage(&hls_dir);
        CacheUsage {
            bytes: a.0 + b.0,
            entries: a.1 + b.1,
            unavailable: a.2 && b.2,
        }
    })
    .await
    .unwrap_or(CacheUsage {
        unavailable: true,
        ..Default::default()
    });

    let total_bytes = libraries.iter().map(|l| l.bytes).sum::<i64>() + cache.bytes;
    Json(StorageView {
        libraries,
        cache,
        total_bytes,
    })
    .into_response()
}

/// 遍历目录统计 (字节, 文件数, 是否不可用)。
const WALK_LIMIT: usize = 50_000;

fn dir_usage(dir: &std::path::Path) -> (i64, i64, bool) {
    if !dir.exists() {
        return (0, 0, true);
    }
    let mut bytes = 0i64;
    let mut entries = 0i64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(p) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        for e in rd.flatten() {
            // 上限是护栏：一个符号链接环或异常深的目录不该把请求挂死。
            if entries >= WALK_LIMIT as i64 {
                return (bytes, entries, false);
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                stack.push(e.path());
            } else if meta.is_file() {
                bytes += meta.len() as i64;
                entries += 1;
            }
        }
    }
    (bytes, entries, false)
}
