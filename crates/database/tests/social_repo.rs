//! 收藏 / 通知 / 设置的持久化测试。
//!
//! 这里守的是三件**出错了很难被发现**的事：
//!
//!   1. **通知的"已读"是每人一份的**。批量扇出如果写成了广播行，
//!      一个人点"全部已读"会把全家人的通知都清掉 ——
//!      这种 bug 在单人开发库里永远测不出来。
//!   2. **收藏是幂等的**。重复收藏必须是 no-op 而不是报错，
//!      否则界面上双击心形按钮会弹一个红条。
//!   3. **设置覆盖的上限是一行**。重复写同一个键必须覆盖而不是追加，
//!      否则库里存着两个值，"到底哪个生效"就成了玄学。

use std::path::{Path, PathBuf};
use whatmedia_core::{Id, MediaItem, MediaKind};
use whatmedia_database::repo::*;
use whatmedia_database::Db;

fn migrations_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .join("migrations")
}

async fn db() -> Db {
    let db = Db::open_in_memory().await.unwrap();
    db.migrate(&migrations_dir()).await.unwrap();
    db
}

async fn user(db: &Db, username: &str, role: &str) -> Id {
    insert_user(
        db,
        NewUser {
            username,
            display_name: Some(username),
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0c2E$hash",
            role,
            parent_id: None,
            max_rating: None,
        },
    )
    .await
    .unwrap()
}

async fn library(db: &Db, name: &str) -> Id {
    insert_library(
        db,
        name,
        MediaKind::Movie,
        "local",
        &format!("/media/{name}"),
        &[],
    )
    .await
    .unwrap()
    .id
}

async fn item(db: &Db, library_id: Id, title: &str) -> Id {
    let it = MediaItem {
        id: Id::new_v4(),
        library_id,
        kind: MediaKind::Movie,
        title: title.into(),
        original_title: None,
        sort_title: None,
        year: Some(2021),
        overview: None,
        runtime_ms: None,
        rating: None,
        locked_fields: Vec::new(),
        provider: None,
        provider_id: None,
        added_at: chrono::Utc::now(),
    };
    upsert_item(db, &it).await.unwrap();
    it.id
}

fn note(title: &str) -> NewNotification {
    NewNotification {
        kind: "scan_finished",
        level: NotifyLevel::Info,
        title: title.into(),
        body: Some("正文".into()),
        link: None,
    }
}

// ─────────────────────────────────────────────────────────────
// 收藏
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn favoriting_twice_is_a_noop_not_an_error() {
    let db = db().await;
    let u = user(&db, "a", "user").await;
    let lib = library(&db, "影").await;
    let i = item(&db, lib, "电影一").await;

    add_favorite(&db, u, i).await.unwrap();
    // 第二次必须成功 —— 双击心形按钮不该弹红条。
    add_favorite(&db, u, i).await.unwrap();

    assert!(is_favorited(&db, u, i).await.unwrap());
    assert_eq!(favorite_count_for_item(&db, i).await.unwrap(), 1);
}

#[tokio::test]
async fn unfavoriting_reports_whether_it_actually_removed_something() {
    let db = db().await;
    let u = user(&db, "a", "user").await;
    let lib = library(&db, "影").await;
    let i = item(&db, lib, "电影一").await;

    // 从来没收藏过 —— 返回 false，让服务端能区分 204 与 404。
    assert!(!remove_favorite(&db, u, i).await.unwrap());

    add_favorite(&db, u, i).await.unwrap();
    assert!(remove_favorite(&db, u, i).await.unwrap());
    assert!(!is_favorited(&db, u, i).await.unwrap());
}

#[tokio::test]
async fn favorites_are_per_user_and_listed_newest_first() {
    let db = db().await;
    let a = user(&db, "a", "user").await;
    let b = user(&db, "b", "user").await;
    let lib = library(&db, "影").await;
    let i1 = item(&db, lib, "先收藏的").await;
    let i2 = item(&db, lib, "后收藏的").await;

    add_favorite(&db, a, i1).await.unwrap();
    // 让两条 created_at 一定不同：同一毫秒内写两条的话顺序就无从判断了。
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    add_favorite(&db, a, i2).await.unwrap();

    let (rows, total) = list_favorites(&db, a, 1, 50).await.unwrap();
    assert_eq!(total, 2);
    assert_eq!(rows[0].title, "后收藏的", "新收藏的排前面");
    assert_eq!(rows[1].title, "先收藏的");

    // b 没收藏过任何东西。
    let (rows_b, total_b) = list_favorites(&db, b, 1, 50).await.unwrap();
    assert_eq!(total_b, 0);
    assert!(rows_b.is_empty());

    // 同一个条目被两个人收藏，计数是 2。
    add_favorite(&db, b, i1).await.unwrap();
    assert_eq!(favorite_count_for_item(&db, i1).await.unwrap(), 2);
}

#[tokio::test]
async fn deleting_an_item_takes_its_favorites_with_it() {
    let db = db().await;
    let u = user(&db, "a", "user").await;
    let lib = library(&db, "影").await;
    let i = item(&db, lib, "电影一").await;
    add_favorite(&db, u, i).await.unwrap();

    sqlx::query("DELETE FROM media_item WHERE id = ?")
        .bind(i.to_string())
        .execute(db.pool())
        .await
        .unwrap();

    assert_eq!(favorite_count_for_item(&db, i).await.unwrap(), 0);
}

// ─────────────────────────────────────────────────────────────
// 通知
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn one_persons_reading_does_not_mark_anyone_elses() {
    let db = db().await;
    let a = user(&db, "a", "admin").await;
    let b = user(&db, "b", "admin").await;

    notify_admins(&db, &note("扫描完成")).await.unwrap();
    assert_eq!(count_unread(&db, a).await.unwrap(), 1);
    assert_eq!(count_unread(&db, b).await.unwrap(), 1);

    // a 全部已读。
    assert_eq!(mark_all_read(&db, a).await.unwrap(), 1);
    assert_eq!(count_unread(&db, a).await.unwrap(), 0);
    // b 的不受影响 —— 这就是"每人一行"而不是一条广播的意义。
    assert_eq!(count_unread(&db, b).await.unwrap(), 1);
}

#[tokio::test]
async fn marking_read_is_idempotent_and_scoped_to_the_owner() {
    let db = db().await;
    let a = user(&db, "a", "admin").await;
    let b = user(&db, "b", "admin").await;
    notify_admins(&db, &note("扫描完成")).await.unwrap();

    let mine = list_notifications(&db, a, 20, false).await.unwrap();
    let id = mine[0].id;

    assert!(mark_notification_read(&db, a, id).await.unwrap());
    // 第二次返回 false：已经读过了。
    assert!(!mark_notification_read(&db, a, id).await.unwrap());
    // b 拿着 a 的通知 id 也改不动。
    assert!(!mark_notification_read(&db, b, id).await.unwrap());
}

#[tokio::test]
async fn unread_filter_and_delete_are_scoped_to_the_owner() {
    let db = db().await;
    let a = user(&db, "a", "admin").await;
    notify_admins(&db, &note("一")).await.unwrap();
    notify_admins(&db, &note("二")).await.unwrap();

    let all = list_notifications(&db, a, 20, false).await.unwrap();
    assert_eq!(all.len(), 2);
    // 按时间倒序：最后写的最先出现。
    assert_eq!(all[0].title, "二");

    mark_all_read(&db, a).await.unwrap();
    assert!(list_notifications(&db, a, 20, true)
        .await
        .unwrap()
        .is_empty());

    // 删别人的通知删不掉：b 拿着 a 的通知 id 只能拿到 false。
    let b = user(&db, "b", "admin").await;
    assert!(!delete_notification(&db, b, all[0].id).await.unwrap());
    // 本人的能删掉。
    assert!(delete_notification(&db, a, all[0].id).await.unwrap());
}

#[tokio::test]
async fn notifying_nobody_is_not_an_error() {
    let db = db().await;
    // 库里还没有管理员。发通知不该 panic —— 扫描结束时没人可通知是正常情况。
    assert_eq!(notify_admins(&db, &note("扫描完成")).await.unwrap(), 0);
}

#[tokio::test]
async fn prune_only_touches_old_notifications() {
    let db = db().await;
    let a = user(&db, "a", "admin").await;
    notify_admins(&db, &note("新")).await.unwrap();

    // 把这条改成 100 天前。
    sqlx::query("UPDATE notification SET created_at = ?")
        .bind((chrono::Utc::now() - chrono::Duration::days(100)).to_rfc3339())
        .execute(db.pool())
        .await
        .unwrap();
    assert_eq!(prune_notifications(&db, 90).await.unwrap(), 1);

    notify_admins(&db, &note("新的")).await.unwrap();
    assert_eq!(prune_notifications(&db, 90).await.unwrap(), 0);
    assert_eq!(count_unread(&db, a).await.unwrap(), 1);
}

// ─────────────────────────────────────────────────────────────
// 设置
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn writing_the_same_key_twice_overwrites() {
    let db = db().await;
    let admin = user(&db, "admin", "admin").await;

    set_server_setting(&db, "metadata.language", "zh-CN", admin)
        .await
        .unwrap();
    set_server_setting(&db, "metadata.language", "en-US", admin)
        .await
        .unwrap();

    let rows = list_server_settings(&db).await.unwrap();
    assert_eq!(rows.len(), 1, "同一个键只能有一行");
    assert_eq!(rows[0].value, "en-US");
}

#[tokio::test]
async fn resetting_a_setting_removes_the_override_row() {
    let db = db().await;
    let admin = user(&db, "admin", "admin").await;
    set_server_setting(&db, "ai.model", "gpt-4o-mini", admin)
        .await
        .unwrap();

    assert!(delete_server_setting(&db, "ai.model").await.unwrap());
    // 删一个从来没覆盖过的键返回 false —— 界面据此区分"已恢复"与"本来就是默认"。
    assert!(!delete_server_setting(&db, "ai.model").await.unwrap());
    assert!(list_server_settings(&db).await.unwrap().is_empty());
}

#[tokio::test]
async fn user_settings_never_bleed_into_another_user() {
    let db = db().await;
    let a = user(&db, "a", "user").await;
    let b = user(&db, "b", "user").await;

    set_user_setting(&db, a, "player.subtitle_lang", "zho")
        .await
        .unwrap();
    set_user_setting(&db, b, "player.subtitle_lang", "eng")
        .await
        .unwrap();

    let ra: Vec<_> = list_user_settings(&db, a).await.unwrap();
    assert_eq!(ra.len(), 1);
    assert_eq!(ra[0].value, "zho");

    let rb: Vec<_> = list_user_settings(&db, b).await.unwrap();
    assert_eq!(rb[0].value, "eng");

    // server 级的列表不该看到 user 级的行。
    assert!(list_server_settings(&db).await.unwrap().is_empty());
}

#[tokio::test]
async fn deleting_a_user_takes_their_settings_and_notifications_with_them() {
    let db = db().await;
    let a = user(&db, "a", "admin").await;
    notify_admins(&db, &note("一")).await.unwrap();
    set_user_setting(&db, a, "player.autoplay_next", "true")
        .await
        .unwrap();

    delete_user(&db, a).await.unwrap();
    assert!(list_user_settings(&db, a).await.unwrap().is_empty());
    assert_eq!(count_unread(&db, a).await.unwrap(), 0);
}

// ─────────────────────────────────────────────────────────────
// 存储统计
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn storage_reports_per_library_sizes_from_the_database() {
    let db = db().await;
    let lib = library(&db, "影").await;
    let i = item(&db, lib, "电影一").await;

    // 造一个版本 + 文件，size_bytes 是真实来源（不扫盘）。
    let v = whatmedia_core::MediaVersion {
        id: Id::new_v4(),
        item_id: i,
        name: "default".into(),
        source: None,
        resolution: Some("1080p".into()),
        video_codec: None,
        hdr_type: whatmedia_core::HdrType::Sdr,
        is_default: true,
    };
    upsert_version(&db, &v).await.unwrap();
    let f = whatmedia_core::MediaFile {
        id: Id::new_v4(),
        version_id: v.id,
        storage_provider: "local".into(),
        path: "/media/影/a.mkv".into(),
        container: None,
        size_bytes: 4_294_967_296,
        duration_ms: None,
        bitrate: None,
        file_fingerprint: "fp1".into(),
        probe_json: None,
        is_available: true,
    };
    upsert_file(&db, &f).await.unwrap();

    let rows = library_storage(&db).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "影");
    assert_eq!(rows[0].item_count, 1);
    assert_eq!(rows[0].file_count, 1);
    assert_eq!(rows[0].bytes, 4_294_967_296);
    // 这个目录在磁盘上不存在 —— 必须如实报告，界面要靠它显示"盘没挂载"。
    assert!(!rows[0].reachable);
}

#[tokio::test]
async fn storage_is_empty_before_any_library_exists() {
    let db = db().await;
    assert!(library_storage(&db).await.unwrap().is_empty());
}
