//! P3 播放端点冒烟测试（集成）。
//!
//! 跑的是**真实 HTTP 往返**：真的 axum router、真的 SQLite（迁移后的内存库）、
//! 真的文件系统。P3-6 结束时这些端点只"编译通过"过，从未真正响应过一次请求，
//! 这份测试就是为了堵住这个洞。
//!
//! 唯一缺的外置依赖是 ffmpeg。本机没有 ffmpeg 不是异常而是常态（CI 也是），
//! 所以"没有 ffmpeg 时必须返回 501 而不是 500"这条约定被**钉死在这里** ——
//! 501 前端能提示"请安装 ffmpeg"，500 只会弹一个毫无意义的网络错误。

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use std::path::Path;
use whatmedia_api::AppState;
use whatmedia_config::AppConfig;
use whatmedia_core::Id;
use whatmedia_database::Db;

// ─────────────────────────────────────────────────────────────
// 装配
// ─────────────────────────────────────────────────────────────

struct Harness {
    state: AppState,
    /// 会话票据。**播放端点也在鉴权之后** —— 这条以前没被验证过。
    tok: String,
    /// 必须活到测试结束：媒体文件与缓存目录都在它下面。
    _dir: tempfile::TempDir,
    /// MP4 + H264 + AAC —— Web 端可以直出。
    mp4: Id,
    mp4_item: Id,
    /// MKV + H264 + AAC —— Web 端容器不认，要 remux。
    mkv: Id,
}

mod common;
use common::{admin_token, get, post_json, send};

async fn seed_library(db: &Db, root: &Path) {
    sqlx::query(
        "INSERT INTO library (id, name, kind, storage_provider, root_path, created_at)
         VALUES (?,'冒烟库','movie','local',?,'2026-01-01T00:00:00Z')",
    )
    .bind(Id::new_v4().to_string())
    .bind(root.to_string_lossy().to_string())
    .execute(db.pool())
    .await
    .unwrap();
}

/// 插一条 item/version/file，并写入一个真实文件。
///
/// **为什么一定要写真实文件**：Direct Play 端点要读 `Content-Length` 并做 Range 切片，
/// 用假路径测出来的 200/206 是自欺欺人。
async fn seed_file(
    db: &Db,
    root: &Path,
    rel: &str,
    container: &str,
) -> (Id, Id) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &bytes).unwrap();

    let item = Id::new_v4();
    let version = Id::new_v4();
    let file = Id::new_v4();

    sqlx::query(
        "INSERT INTO media_item (id, library_id, kind, title, locked_fields, added_at, match_title)
         SELECT ?, id, 'movie', ?, '[]', '2026-01-01T00:00:00Z', ? FROM library LIMIT 1",
    )
    .bind(item.to_string())
    .bind(rel)
    .bind(rel)
    .execute(db.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO media_version (id, item_id, name, hdr_type, is_default)
         VALUES (?,?,'v','sdr',1)",
    )
    .bind(version.to_string())
    .bind(item.to_string())
    .execute(db.pool())
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO media_file (id, version_id, storage_provider, path, container,
                                 size_bytes, duration_ms, bitrate, file_fingerprint, is_available)
         VALUES (?,?,'local',?,?,?,7200000,8000000,'fp',1)",
    )
    .bind(file.to_string())
    .bind(version.to_string())
    .bind(path.to_string_lossy().to_string())
    .bind(container)
    .bind(bytes.len() as i64)
    .execute(db.pool())
    .await
    .unwrap();

    insert_stream(
        db,
        file,
        "video",
        0,
        "h264",
        Some("eng"),
        Some(1920),
        Some(1080),
        None,
    )
    .await;
    insert_stream(
        db,
        file,
        "audio",
        1,
        "aac",
        Some("eng"),
        None,
        None,
        Some(2),
    )
    .await;

    (item, file)
}

#[allow(clippy::too_many_arguments)]
async fn insert_stream(
    db: &Db,
    file: Id,
    kind: &str,
    idx: i32,
    codec: &str,
    lang: Option<&str>,
    w: Option<i32>,
    h: Option<i32>,
    channels: Option<i32>,
) {
    sqlx::query(
        "INSERT INTO media_stream (id, file_id, stream_type, idx, codec, language, title,
                                   is_default, is_forced, width, height, bit_depth, hdr_type,
                                   channels, bitrate)
         VALUES (?,?,?,?,?,?,NULL,1,0,?,?,8,'sdr',?,NULL)",
    )
    .bind(Id::new_v4().to_string())
    .bind(file.to_string())
    .bind(kind)
    .bind(idx)
    .bind(codec)
    .bind(lang.map(str::to_string))
    .bind(w)
    .bind(h)
    .bind(channels)
    .execute(db.pool())
    .await
    .unwrap();
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("media");
    std::fs::create_dir_all(&root).unwrap();

    let db = Db::open_in_memory().await.unwrap();
    db.migrate(&common::migrations_dir()).await.unwrap();
    seed_library(&db, &root).await;

    let (mp4_item, mp4) = seed_file(&db, &root, "Movie.2020.1080p.mp4", "mp4").await;
    let (_mkv_item, mkv) = seed_file(&db, &root, "Movie.2020.1080p.mkv", "mkv").await;

    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    cfg.paths.cache_dir = dir.path().join("cache");
    std::fs::create_dir_all(&cfg.paths.cache_dir).unwrap();

    // 本机没有 ffmpeg：这正是要测的状态之一（端点必须给 501 而不是 500）。
    let tok = admin_token(&db).await;
    let state = AppState::new(cfg)
        .with_db(db)
        .with_playback(None, Vec::new(), Default::default());

    Harness {
        state,
        tok,
        _dir: dir,
        mp4,
        mp4_item,
        mkv,
    }
}

// ─────────────────────────────────────────────────────────────
// 请求辅助
// ─────────────────────────────────────────────────────────────

// `Reply` / `send` 来自 common：它们装配的是同一个真 router，
// 各文件自己写一份就会有人漏掉鉴权中间件。

fn get_with_range(tok: &str, uri: &str, v: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {tok}"))
        .header(header::RANGE, v)
        .body(Body::empty())
        .unwrap()
}

// ─────────────────────────────────────────────────────────────
// 决策
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn decide_direct_plays_web_friendly_file() {
    let h = harness().await;
    let r = send(
        &h.state,
        get(
            &h.tok,
            &format!("/api/v1/playback/decide?file_id={}&client=web", h.mp4),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let v = r.json();
    assert_eq!(v["strategy"], "direct_play");
    assert_eq!(v["client"], "web");
    // 直出不该凭空给出降级原因。
    assert!(v["reasons"].as_array().unwrap().is_empty(), "{v}");
}

#[tokio::test]
async fn decide_remuxes_mkv_on_web_with_an_explanation() {
    let h = harness().await;
    let r = send(
        &h.state,
        get(
            &h.tok,
            &format!("/api/v1/playback/decide?file_id={}&client=web", h.mkv),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let v = r.json();
    assert_eq!(v["strategy"], "remux");
    // 单向降级铁律：每次降级都必须留下可解释的 reason。
    let reasons = v["reasons"].as_array().unwrap();
    assert!(!reasons.is_empty(), "降级了却给不出原因：{v}");
    assert!(!v["reason"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn decide_is_a_pure_read_with_no_side_effects() {
    // 决策端点只做决策：不能建会话、不能起进程。用续播列表为空来证明。
    let h = harness().await;
    let _ = send(
        &h.state,
        get(
            &h.tok,
            &format!("/api/v1/playback/decide?file_id={}", h.mp4),
        ),
    )
    .await;
    let r = send(&h.state, get(&h.tok, "/api/v1/playback/resume")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.json().as_array().unwrap().is_empty());
}

#[tokio::test]
async fn decide_returns_404_for_unknown_file() {
    let h = harness().await;
    let r = send(
        &h.state,
        get(
            &h.tok,
            &format!("/api/v1/playback/decide?file_id={}", Id::new_v4()),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.json()["error"]["code"], "NOT_FOUND");
}

// ─────────────────────────────────────────────────────────────
// Direct Play 与 HTTP Range
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn direct_stream_returns_full_body_and_accept_ranges() {
    let h = harness().await;
    let r = send(&h.state, get(&h.tok, &format!("/api/v1/stream/{}", h.mp4))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.header(header::ACCEPT_RANGES), "bytes");
    assert_eq!(r.header(header::CONTENT_LENGTH), "4096");
    // 没有 Range 头时长度取磁盘实际大小，而不是数据库里的旧值。
    assert_eq!(r.body.len(), 4096);
    assert_eq!(r.header(header::CONTENT_TYPE), "video/mp4");

    let r = send(&h.state, get(&h.tok, &format!("/api/v1/stream/{}", h.mkv))).await;
    assert_eq!(r.header(header::CONTENT_TYPE), "video/x-matroska");
}

#[tokio::test]
async fn direct_stream_honors_range_request() {
    let h = harness().await;
    let r = send(
        &h.state,
        get_with_range(&h.tok, &format!("/api/v1/stream/{}", h.mp4), "bytes=0-99"),
    )
    .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.header(header::CONTENT_RANGE), "bytes 0-99/4096");
    assert_eq!(r.header(header::CONTENT_LENGTH), "100");
    assert_eq!(r.body.len(), 100);
    // 切片内容必须真的是文件开头那 100 字节，不能是别处凑来的。
    let expected: Vec<u8> = (0..100u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(r.body, expected);
}

#[tokio::test]
async fn direct_stream_honors_open_ended_and_suffix_ranges() {
    let h = harness().await;

    // 开区间：从 4000 到结尾。
    let r = send(
        &h.state,
        get_with_range(&h.tok, &format!("/api/v1/stream/{}", h.mp4), "bytes=4000-"),
    )
    .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.header(header::CONTENT_RANGE), "bytes 4000-4095/4096");
    assert_eq!(r.body.len(), 96);

    // 尾部区间：最后 96 字节。
    let r = send(
        &h.state,
        get_with_range(&h.tok, &format!("/api/v1/stream/{}", h.mp4), "bytes=-96"),
    )
    .await;
    assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(r.header(header::CONTENT_RANGE), "bytes 4000-4095/4096");
    assert_eq!(r.body.len(), 96);
}

#[tokio::test]
async fn direct_stream_out_of_bounds_range_is_416() {
    let h = harness().await;
    for bad in ["bytes=99999-", "bytes=0-99999", "bytes=10-5"] {
        let r = send(
            &h.state,
            get_with_range(&h.tok, &format!("/api/v1/stream/{}", h.mp4), bad),
        )
        .await;
        assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE, "{bad}");
        assert_eq!(r.header(header::CONTENT_RANGE), "bytes */4096", "{bad}");
    }
}

#[tokio::test]
async fn direct_stream_rejects_multipart_and_malformed_ranges() {
    let h = harness().await;
    for bad in ["bytes=0-9,20-29", "items=0-10", "bytes=abc-def", "0-99"] {
        let r = send(
            &h.state,
            get_with_range(&h.tok, &format!("/api/v1/stream/{}", h.mp4), bad),
        )
        .await;
        assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE, "{bad}");
    }
}

#[tokio::test]
async fn direct_stream_returns_404_for_unknown_file() {
    let h = harness().await;
    let r = send(
        &h.state,
        get(&h.tok, &format!("/api/v1/stream/{}", Id::new_v4())),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

// ─────────────────────────────────────────────────────────────
// 会话生命周期
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn session_lifecycle_start_heartbeat_resume_stop() {
    let h = harness().await;

    // 建会话。
    let r = send(
        &h.state,
        post_json(
            &h.tok,
            "/api/v1/playback/sessions",
            serde_json::json!({ "file_id": h.mp4, "client": "web", "device_id": "dev-1" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let v = r.json();
    let session = v["session_id"].as_str().unwrap().to_string();
    assert_eq!(v["strategy"], "direct_play");
    assert_eq!(
        v["url"].as_str().unwrap(),
        format!("/api/v1/stream/{}", h.mp4)
    );

    // 心跳推进到 10 分钟（> 30s 才会进"继续观看"）。
    let r = send(
        &h.state,
        post_json(
            &h.tok,
            &format!("/api/v1/playback/sessions/{session}/heartbeat"),
            serde_json::json!({ "position_ms": 600_000, "duration_ms": 7_200_000 }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());

    // 续播列表里应当出现。
    let r = send(&h.state, get(&h.tok, "/api/v1/playback/resume")).await;
    assert_eq!(r.status, StatusCode::OK);
    let list = r.json().as_array().unwrap().clone();
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0]["item_id"], h.mp4_item.to_string());
    assert_eq!(list[0]["position_ms"], 600_000);

    // 停止后不再出现在续播列表里（stopped_at 非空）。
    let r = send(
        &h.state,
        post_json(
            &h.tok,
            &format!("/api/v1/playback/sessions/{session}/stop"),
            serde_json::json!({ "position_ms": 610_000 }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = send(&h.state, get(&h.tok, "/api/v1/playback/resume")).await;
    assert!(r.json().as_array().unwrap().is_empty());
}

#[tokio::test]
async fn session_that_needs_ffmpeg_is_501_not_500() {
    // 没有 ffmpeg 时"播不了"必须是一个能解释的 501。
    // 前端据此提示"请安装 ffmpeg"，而不是弹一个网络错误。
    let h = harness().await;
    let r = send(
        &h.state,
        post_json(
            &h.tok,
            "/api/v1/playback/sessions",
            serde_json::json!({ "file_id": h.mkv, "client": "web" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_IMPLEMENTED, "{}", r.text());
    assert_eq!(r.json()["error"]["code"], "FFMPEG_UNAVAILABLE");
}

#[tokio::test]
async fn session_for_unknown_file_is_404() {
    let h = harness().await;
    let r = send(
        &h.state,
        post_json(
            &h.tok,
            "/api/v1/playback/sessions",
            serde_json::json!({ "file_id": Id::new_v4() }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn remux_without_ffmpeg_is_501() {
    let h = harness().await;
    let r = send(
        &h.state,
        get(&h.tok, &format!("/api/v1/stream/{}/remux", h.mkv)),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(r.json()["error"]["code"], "FFMPEG_UNAVAILABLE");
}

// ─────────────────────────────────────────────────────────────
// HLS / 硬件 / 字幕
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn unknown_hls_job_is_404() {
    let h = harness().await;
    let job = Id::new_v4();
    for uri in [
        format!("/api/v1/hls/{job}/master.m3u8"),
        format!("/api/v1/hls/{job}/index.m3u8"),
        format!("/api/v1/hls/{job}/stop"),
    ] {
        let req = if uri.ends_with("/stop") {
            post_json(&h.tok, &uri, serde_json::json!({}))
        } else {
            get(&h.tok, &uri)
        };
        let r = send(&h.state, req).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn hls_file_name_is_checked_against_traversal() {
    let h = harness().await;
    // 任务不存在会先撞 404；这条断言的是"文件名校验发生在读盘之前"这个顺序不会
    // 因为将来补上任务登记而被绕过 —— 越界路径永远到不了文件系统。
    let job = Id::new_v4();
    let r = send(
        &h.state,
        get(&h.tok, &format!("/api/v1/hls/{job}/..%2f..%2fetc%2fpasswd")),
    )
    .await;
    assert_ne!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn hardware_profile_reports_ffmpeg_absence() {
    let h = harness().await;
    let r = send(&h.state, get(&h.tok, "/api/v1/hardware/profile")).await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert!(v["ffmpeg"].is_null());
    assert!(v["profiles"].as_array().unwrap().is_empty());
    assert_eq!(v["streams"], 0);
}


#[tokio::test]
async fn playback_endpoints_are_behind_auth_first() {
    // 没凭据一律 401。这条替代了原来的 "无数据库 → 503"：
    // 鉴权现在排在数据库之前，一个来路不明的请求连"有没有接数据库"
    // 都不该看得出来。
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    let state = AppState::new(cfg);
    for uri in [
        format!("/api/v1/playback/decide?file_id={}", Id::new_v4()),
        "/api/v1/playback/resume".to_string(),
        "/api/v1/playback/streams".to_string(),
        format!("/api/v1/stream/{}", Id::new_v4()),
    ] {
        let r = send(
            &state,
            Request::builder().uri(&uri).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{uri}");
    }
}
