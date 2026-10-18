//! 鉴权持久化层集成测试。
//!
//! 跑真实的 SQLite（迁移后的内存库），因为这里要守的东西恰恰是**SQL 与表结构之间
//! 的契约**，mock 掉数据库等于把被测对象换掉：
//!
//!   1. **0006 迁移能叠加应用**。老库里已经有 user（role 是老值）的情况要能升级。
//!   2. **登出所有设备**必须真的把同一用户的全部会话打上吊销标记，
//!      而且**不能**碰别人的会话 —— 这就是差集。
//!   3. **删除用户要级联**。会话表挂着 FK，删不掉说明迁移里的级联没写对，
//!      于是"删了账户但登录态还在"这种最坏的情况会发生。
//!   4. **停用账户 = 立刻踢下线**。只把 `user.disabled` 置起来是不够的，
//!      在线会话必须同时失效，否则"禁用"只是下次登录登不上而已。

use std::path::{Path, PathBuf};
use whatmedia_core::Id;
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

struct New<'a> {
    username: &'a str,
    role: &'a str,
}

async fn seed(db: &Db, u: New<'_>) -> Id {
    insert_user(
        db,
        NewUser {
            username: u.username,
            display_name: None,
            password_hash: "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0c2E$hash",
            role: u.role,
            parent_id: None,
            max_rating: None,
        },
    )
    .await
    .unwrap()
}

async fn session(db: &Db, user: Id, token_hash: &str) -> Id {
    insert_session(
        db,
        NewAuthSession {
            user_id: user,
            device_id: None,
            token_hash,
            kind: "web",
            ip: None,
            user_agent: None,
            expires_at: "2999-01-01T00:00:00Z",
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn empty_db_has_no_users() {
    let db = db().await;
    assert_eq!(count_users(&db).await.unwrap(), 0);
}

#[tokio::test]
async fn user_roundtrip_hides_nothing_from_the_auth_row() {
    // 类型名叫 AuthRow —— 它**故意带着散列**。选型时不要再目标是在这里。
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "alice",
            role: "admin",
        },
    )
    .await;
    let row = get_user_by_id(&db, id).await.unwrap().unwrap();
    assert_eq!(row.username, "alice");
    assert_eq!(row.role, "admin");
    assert!(row.password_hash.is_some());
    assert_eq!(row.failed_logins, 0);
    assert!(row.locked_until.is_none());
    assert!(row.last_login_at.is_none());
}

#[tokio::test]
async fn user_view_strips_secrets() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "bob",
            role: "user",
        },
    )
    .await;
    let row = get_user_by_id(&db, id).await.unwrap().unwrap();
    let view = UserView::from_auth(&row).unwrap();
    let json = serde_json::to_value(&view).unwrap();
    let flat = json.to_string();
    assert!(!flat.contains("password_hash"), "{flat}");
    assert!(!flat.contains("pin"), "{flat}");
    assert_eq!(json["username"], "bob");
    assert_eq!(json["role"], "user");
    assert_eq!(json["disabled"], false);
}

#[tokio::test]
async fn username_lookup_and_existence() {
    let db = db().await;
    seed(
        &db,
        New {
            username: "carol",
            role: "user",
        },
    )
    .await;
    assert!(username_exists(&db, "carol").await.unwrap());
    assert!(!username_exists(&db, "nobody").await.unwrap());
    assert!(get_user_by_username(&db, "carol").await.unwrap().is_some());
    assert!(get_user_by_username(&db, "nobody").await.unwrap().is_none());
}

#[tokio::test]
async fn login_failure_counts_and_lock_can_be_applied_and_cleared() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "dave",
            role: "user",
        },
    )
    .await;

    record_login_failure(&db, id, None).await.unwrap();
    record_login_failure(&db, id, None).await.unwrap();
    let row = get_user_by_id(&db, id).await.unwrap().unwrap();
    assert_eq!(row.failed_logins, 2);
    assert!(row.locked_until.is_none());

    record_login_failure(&db, id, Some("2999-01-01T00:00:00Z"))
        .await
        .unwrap();
    let row = get_user_by_id(&db, id).await.unwrap().unwrap();
    assert_eq!(row.locked_until.as_deref(), Some("2999-01-01T00:00:00Z"));

    // 登录成功要**同时**清计数与清锁定 —— 只做一半等于永久封号。
    record_login_success(&db, id).await.unwrap();
    let row = get_user_by_id(&db, id).await.unwrap().unwrap();
    assert_eq!(row.failed_logins, 0);
    assert!(row.locked_until.is_none());
    assert!(row.last_login_at.is_some());
}

#[tokio::test]
async fn role_change_keeps_is_child_in_sync() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "kid",
            role: "user",
        },
    )
    .await;
    set_user_role(&db, id, "child").await.unwrap();
    let raw: (String, i32) = sqlx::query_as("SELECT role, is_child FROM user WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(raw.0, "child");
    assert_eq!(raw.1, 1, "role=child 必须同步 is_child=1");

    set_user_role(&db, id, "user").await.unwrap();
    let raw: (String, i32) = sqlx::query_as("SELECT role, is_child FROM user WHERE id = ?")
        .bind(id.to_string())
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert_eq!(raw.1, 0);
}

#[tokio::test]
async fn session_lookup_joins_user_fields() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "erin",
            role: "admin",
        },
    )
    .await;
    let sid = session(&db, id, "hash-abc").await;

    let s = get_session_by_hash(&db, "hash-abc").await.unwrap().unwrap();
    assert_eq!(s.id, sid.to_string());
    assert_eq!(s.username, "erin");
    assert_eq!(s.role, "admin");
    assert_eq!(s.user_disabled, 0);
    assert!(s.revoked_at.is_none());

    assert!(get_session_by_hash(&db, "hash-nope")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn revoke_all_sessions_only_touches_that_user() {
    let db = db().await;
    let a = seed(
        &db,
        New {
            username: "a",
            role: "user",
        },
    )
    .await;
    let b = seed(
        &db,
        New {
            username: "b",
            role: "user",
        },
    )
    .await;
    session(&db, a, "h1").await;
    session(&db, a, "h2").await;
    session(&db, b, "h3").await;

    let n = revoke_user_sessions(&db, a).await.unwrap();
    assert_eq!(n, 2, "只应吊销 a 的两条");

    assert!(get_session_by_hash(&db, "h1")
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(get_session_by_hash(&db, "h2")
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
    assert!(
        get_session_by_hash(&db, "h3")
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none(),
        "别人的会话不能被连坐"
    );

    // 重复调用不该再计数 —— 幂等才能让"登出"按钮随便点。
    assert_eq!(revoke_user_sessions(&db, a).await.unwrap(), 0);
}

#[tokio::test]
async fn disabling_a_user_kicks_their_sessions() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "frank",
            role: "user",
        },
    )
    .await;
    session(&db, id, "f1").await;

    set_user_disabled(&db, id, true).await.unwrap();

    let row = get_user_by_id(&db, id).await.unwrap().unwrap();
    assert_eq!(row.disabled, 1);
    assert!(
        get_session_by_hash(&db, "f1")
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some(),
        "停用必须立刻吊销在线会话"
    );

    // 反向：解除停用不应该帮他把会话复活。
    set_user_disabled(&db, id, false).await.unwrap();
    assert!(get_session_by_hash(&db, "f1")
        .await
        .unwrap()
        .unwrap()
        .revoked_at
        .is_some());
}

#[tokio::test]
async fn revoking_twice_reports_false_the_second_time() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "gina",
            role: "user",
        },
    )
    .await;
    let sid = session(&db, id, "g1").await;
    assert!(revoke_session(&db, sid).await.unwrap());
    assert!(!revoke_session(&db, sid).await.unwrap());
}

#[tokio::test]
async fn pruning_only_removes_expired_sessions() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "hank",
            role: "user",
        },
    )
    .await;
    session(&db, id, "h-keep").await;
    insert_session(
        &db,
        NewAuthSession {
            user_id: id,
            device_id: None,
            token_hash: "h-old",
            kind: "device",
            ip: None,
            user_agent: None,
            expires_at: "2000-01-01T00:00:00Z",
        },
    )
    .await
    .unwrap();

    let removed = prune_sessions(&db, 0).await.unwrap();
    assert_eq!(removed, 1, "只该删掉过期那条");
    assert!(get_session_by_hash(&db, "h-keep").await.unwrap().is_some());
    assert!(get_session_by_hash(&db, "h-old").await.unwrap().is_none());
}

#[tokio::test]
async fn listing_sessions_hides_revoked_ones() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "ivy",
            role: "user",
        },
    )
    .await;
    let s1 = session(&db, id, "i1").await;
    session(&db, id, "i2").await;
    revoke_session(&db, s1).await.unwrap();
    assert_eq!(list_sessions(&db, id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn devices_are_deduplicated_by_name_per_user() {
    let db = db().await;
    let a = seed(
        &db,
        New {
            username: "jo",
            role: "user",
        },
    )
    .await;
    let b = seed(
        &db,
        New {
            username: "ke",
            role: "user",
        },
    )
    .await;
    let da = insert_device(
        &db,
        NewDevice {
            user_id: a,
            name: "客厅电视",
            kind: Some("tv"),
            platform: Some("android"),
            app_version: Some("0.1.0"),
            approved: true,
        },
    )
    .await
    .unwrap();
    insert_device(
        &db,
        NewDevice {
            user_id: b,
            name: "客厅电视",
            kind: Some("tv"),
            platform: Some("android"),
            app_version: None,
            approved: false,
        },
    )
    .await
    .unwrap();

    // 同名设备在不同用户下是两台。
    assert_eq!(
        find_device_by_name(&db, a, "客厅电视")
            .await
            .unwrap()
            .unwrap()
            .id,
        da.to_string()
    );
    assert_eq!(list_user_devices(&db, a).await.unwrap().len(), 1);
    assert_eq!(list_user_devices(&db, b).await.unwrap().len(), 1);
}

#[tokio::test]
async fn device_approval_flag_and_deletion() {
    let db = db().await;
    let u = seed(
        &db,
        New {
            username: "kid",
            role: "child",
        },
    )
    .await;
    let d = insert_device(
        &db,
        NewDevice {
            user_id: u,
            name: "平板",
            kind: Some("mobile"),
            platform: Some("ios"),
            app_version: None,
            approved: false,
        },
    )
    .await
    .unwrap();
    assert_eq!(get_device(&db, d).await.unwrap().unwrap().approved, 0);

    let ap = insert_approval(
        &db,
        NewApproval {
            user_id: u,
            device_id: Some(d),
            kind: "device",
            title: "平板 请求登录",
            detail: None,
        },
    )
    .await
    .unwrap();

    assert!(set_device_approved(&db, d, true, Some(ap)).await.unwrap());
    let dev = get_device(&db, d).await.unwrap().unwrap();
    assert_eq!(dev.approved, 1);
    assert_eq!(dev.approval_id.as_deref(), Some(&*ap.to_string()));

    touch_device(&db, d, Some("192.168.1.9")).await.unwrap();
    assert_eq!(
        get_device(&db, d)
            .await
            .unwrap()
            .unwrap()
            .last_ip
            .as_deref(),
        Some("192.168.1.9")
    );

    assert!(delete_device(&db, d).await.unwrap());
    assert!(!delete_device(&db, d).await.unwrap());
}

#[tokio::test]
async fn approvals_are_pending_then_decided_once() {
    let db = db().await;
    let parent = seed(
        &db,
        New {
            username: "mom",
            role: "admin",
        },
    )
    .await;
    let child = seed(
        &db,
        New {
            username: "kid",
            role: "child",
        },
    )
    .await;

    let id = insert_approval(
        &db,
        NewApproval {
            user_id: child,
            device_id: None,
            kind: "device",
            title: "请求在新电视上登录",
            detail: Some("设备：卧室电视"),
        },
    )
    .await
    .unwrap();

    assert_eq!(list_pending_approvals(&db, None).await.unwrap().len(), 1);
    assert_eq!(
        list_pending_approvals(&db, Some(child))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(list_pending_approvals(&db, Some(parent))
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        pending_approval_for_user(&db, child)
            .await
            .unwrap()
            .unwrap()
            .id,
        id.to_string()
    );

    assert!(decide_approval(&db, id, true, parent, Some("可以"))
        .await
        .unwrap());
    // **只能决一次** —— 否则两个家长同时点会出现互相覆盖的" decide 历史"。
    assert!(!decide_approval(&db, id, false, parent, None).await.unwrap());

    let row = get_approval(&db, id).await.unwrap().unwrap();
    assert_eq!(row.status, "approved");
    assert_eq!(row.decided_by.as_deref(), Some(&*parent.to_string()));
    assert_eq!(row.note.as_deref(), Some("可以"));
    assert!(list_pending_approvals(&db, None).await.unwrap().is_empty());
    assert!(pending_approval_for_user(&db, child)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn deleting_a_user_cascades_sessions_and_devices() {
    let db = db().await;
    let id = seed(
        &db,
        New {
            username: "temp",
            role: "user",
        },
    )
    .await;
    let s = session(&db, id, "t1").await;
    let d = insert_device(
        &db,
        NewDevice {
            user_id: id,
            name: "临时",
            kind: Some("web"),
            platform: Some("web"),
            app_version: None,
            approved: true,
        },
    )
    .await
    .unwrap();

    assert!(delete_user(&db, id).await.unwrap());
    assert!(!delete_user(&db, id).await.unwrap());

    // FK + ON DELETE CASCADE：会话、设备一并消失。
    assert!(get_session_by_hash(&db, "t1").await.unwrap().is_none());
    assert!(session_is_gone(&db, &s.to_string()).await);
    assert!(get_device(&db, d).await.unwrap().is_none());
}

async fn session_is_gone(db: &Db, id: &str) -> bool {
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM session WHERE id = ?")
        .bind(id)
        .fetch_one(db.pool())
        .await
        .unwrap();
    n == 0
}

#[tokio::test]
async fn user_limits_and_pin_are_updatable() {
    let db = db().await;
    let parent = seed(
        &db,
        New {
            username: "parent",
            role: "admin",
        },
    )
    .await;
    let child = seed(
        &db,
        New {
            username: "child",
            role: "child",
        },
    )
    .await;

    set_user_limits(&db, child, Some("PG-13"), Some(parent))
        .await
        .unwrap();
    set_user_pin(&db, parent, Some("$argon2id$pin"))
        .await
        .unwrap();
    set_user_password(&db, parent, "$argon2id$new")
        .await
        .unwrap();

    let row = get_user_by_id(&db, child).await.unwrap().unwrap();
    assert_eq!(row.max_rating.as_deref(), Some("PG-13"));
    assert_eq!(row.parent_id.as_deref(), Some(&*parent.to_string()));

    let row = get_user_by_id(&db, parent).await.unwrap().unwrap();
    assert_eq!(row.pin_hash.as_deref(), Some("$argon2id$pin"));
    assert_eq!(row.password_hash.as_deref(), Some("$argon2id$new"));
}

#[tokio::test]
async fn listing_users_keeps_insertion_order() {
    let db = db().await;
    seed(
        &db,
        New {
            username: "z1",
            role: "admin",
        },
    )
    .await;
    seed(
        &db,
        New {
            username: "z2",
            role: "user",
        },
    )
    .await;
    seed(
        &db,
        New {
            username: "z3",
            role: "child",
        },
    )
    .await;
    let all = list_users(&db).await.unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].username, "z1");
    assert_eq!(all[2].role, "child");
}
