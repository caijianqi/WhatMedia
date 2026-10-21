//! 存储源账号持久化测试。

use std::path::{Path, PathBuf};
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

#[tokio::test]
async fn storage_account_crud_lifecycle() {
    let db = db().await;

    // 1. 列表初始为空
    let list = list_storage_accounts(&db).await.unwrap();
    assert!(list.is_empty());

    // 2. 插入一个 WebDAV 存储源（挂载夸克网盘）
    let created = insert_storage_account(
        &db,
        CreateStorageAccountInput {
            name: "我的夸克网盘".into(),
            driver: "webdav".into(),
            endpoint: "http://127.0.0.1:5244/dav/quark".into(),
            auth_type: "basic".into(),
            username: Some("admin".into()),
            password: Some("123456".into()),
            root_path: "/".into(),
            direct_stream: true,
            extra_headers: serde_json::json!({ "User-Agent": "WhatMedia/1.0" }),
        },
    )
    .await
    .unwrap();

    assert_eq!(created.name, "我的夸克网盘");
    assert_eq!(created.driver, "webdav");
    assert!(created.direct_stream);

    // 3. 详情查询
    let fetched = get_storage_account(&db, created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.endpoint, "http://127.0.0.1:5244/dav/quark");

    // 4. 更新配置
    let updated = update_storage_account(
        &db,
        created.id,
        UpdateStorageAccountInput {
            name: Some("家庭影视夸克盘".into()),
            endpoint: None,
            auth_type: None,
            username: None,
            password: None,
            root_path: Some("/4K电影".into()),
            direct_stream: Some(false),
            extra_headers: None,
        },
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(updated.name, "家庭影视夸克盘");
    assert_eq!(updated.root_path, "/4K电影");
    assert!(!updated.direct_stream);

    // 5. 删除账号
    let deleted = delete_storage_account(&db, created.id).await.unwrap();
    assert!(deleted);

    let after_delete = get_storage_account(&db, created.id).await.unwrap();
    assert!(after_delete.is_none());
}
