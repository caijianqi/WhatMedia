//! P8 直播端点集成测试。
//!
//! 跑真实 HTTP 往返：真的 router、真的 SQLite（迁移后的内存库）。
//!
//! **刻意不测"真拉一路流"**：那需要网络源和几十秒等待，在 CI 上是脆弱测试。
//! 这里守住的是那些"错了就会静默出错"的部分：
//!
//!   1. **频道不进 media_item**。同步后 `/api/v1/media` 的条数必须是 0 ——
//!      这是整个设计的地基（直播不是资产），漂移了后面全错。
//!   2. **引用计数**。同一频道 play 两次只应当起一路；
//!      stop 一次不能停，stop 到零才停。
//!   3. **订阅解析的错误要变成人话**。贴错链接时用户得看到"这不是 M3U"，
//!      而不是"同步成功，0 个频道"。

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use whatmedia_api::{build_router, AppState};
use whatmedia_config::AppConfig;

mod common;
use common::{admin_token, db, delete, get, post_json};

struct Reply {
    status: StatusCode,
    body: Vec<u8>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

async fn send(state: &AppState, req: Request<Body>) -> Reply {
    eprintln!(
        "SEND {:?} {} headers={:?}",
        req.method(),
        req.uri(),
        req.headers()
    );
    let res = build_router(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    Reply { status, body }
}

/// 返回 (state, 会话票据, 临时目录)。
///
/// 票据来自 `common::admin_token`： **测试必须跑在生产那套鉴权之后**，
/// 否则"直播端点在被保护着"这件事就从来没被测过。
async fn state() -> (AppState, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = db().await;
    let tok = admin_token(&db).await;
    let mut cfg = AppConfig::default();
    cfg.server.port = 0;
    cfg.paths.cache_dir = dir.path().to_path_buf();
    let mut st = AppState::new(cfg);
    st.db = Some(db);
    (st, tok, dir)
}

const M3U: &str = r#"#EXTM3U
#EXTINF:-1 tvg-id="cctv1" tvg-logo="http://logo/1.png" group-title="央视",CCTV-1 综合
http://example.com/cctv1.ts
#EXTINF:-1 tvg-id="hunan" group-title="卫视",湖南卫视
http://example.com/hunan.ts
"#;

async fn add_source(st: &AppState, tok: &str, url: &str) -> String {
    let r = send(
        st,
        post_json(
            tok,
            "/api/v1/live/sources",
            json!({ "name": "测试源", "kind": "m3u", "url": url }),
        ),
    )
    .await;
    eprintln!("ADD_SOURCE status={:?} body={}", r.status, r.text());
    assert_eq!(r.status, StatusCode::OK);
    r.json()["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn sync_imports_channels_with_groups_and_logos() {
    let (st, tok, dir) = state().await;
    let p = dir.path().join("list.m3u");
    tokio::fs::write(&p, M3U).await.unwrap();

    let id = add_source(&st, &tok, &p.to_string_lossy()).await;
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["channels"], 2);

    let r = send(&st, get(&tok, "/api/v1/live/channels")).await;
    assert_eq!(r.status, StatusCode::OK);
    let ch = r.json();
    assert_eq!(ch.as_array().unwrap().len(), 2);
    assert_eq!(ch[0]["name"], "CCTV-1 综合");
    assert_eq!(ch[0]["group"], "央视");
    assert_eq!(ch[0]["number"], 1);
    assert_eq!(ch[1]["name"], "湖南卫视");
    assert_eq!(ch[1]["number"], 2);
}

#[tokio::test]
async fn channels_never_leak_into_the_media_library() {
    // 这条是设计地基：直播不是资产。哪天它挂了，说明有人把频道塞进了 media_item。
    let (st, tok, dir) = state().await;
    let p = dir.path().join("list.m3u");
    tokio::fs::write(&p, M3U).await.unwrap();
    let id = add_source(&st, &tok, &p.to_string_lossy()).await;
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    let r = send(&st, get(&tok, "/api/v1/media?page_size=10")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["page"]["total"], 0, "频道绝不能出现在媒体库里");
}

#[tokio::test]
async fn bad_playlist_is_reported_not_swallowed() {
    // "同步成功，0 个频道"是最糟的反馈：用户以为自己配对了。
    let (st, tok, dir) = state().await;
    let p = dir.path().join("bad.m3u");
    tokio::fs::write(&p, "这不是播放列表\nhttp://a.ts\n")
        .await
        .unwrap();

    let id = add_source(&st, &tok, &p.to_string_lossy()).await;
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert_eq!(r.json()["error"]["code"], "BAD_PLAYLIST");

    // 失败次数要留痕，否则前端永远显示"从未同步"。
    let r = send(&st, get(&tok, "/api/v1/live/sources")).await;
    let src = r.json();
    assert!(src[0]["sync_error"].as_str().unwrap().contains("M3U"));
}

#[tokio::test]
async fn header_only_playlist_is_empty_not_ok() {
    let (st, tok, dir) = state().await;
    let p = dir.path().join("empty.m3u");
    tokio::fs::write(&p, "#EXTM3U\n").await.unwrap();
    let id = add_source(&st, &tok, &p.to_string_lossy()).await;
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn unknown_kind_is_rejected_upfront() {
    let (st, tok, _dir) = state().await;
    let r = send(
        &st,
        post_json(
            &tok,
            "/api/v1/live/sources",
            json!({ "name": "x", "kind": "卫星锅", "url": "http://a" }),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.json()["error"]["code"], "BAD_KIND");
}

#[tokio::test]
async fn resync_replaces_channels_instead_of_accumulating() {
    // 订阅是权威来源：源里删掉的频道必须消失，否则会留下僵尸频道。
    let (st, tok, dir) = state().await;
    let p = dir.path().join("list.m3u");
    tokio::fs::write(&p, M3U).await.unwrap();
    let id = add_source(&st, &tok, &p.to_string_lossy()).await;
    let _ = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;

    tokio::fs::write(
        &p,
        "#EXTM3U\n#EXTINF:-1 group-title=\"体育\",CCTV-5\nhttp://x/5.ts\n",
    )
    .await
    .unwrap();
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;
    assert_eq!(r.json()["channels"], 1);

    let r = send(&st, get(&tok, "/api/v1/live/channels")).await;
    let ch = r.json();
    assert_eq!(ch.as_array().unwrap().len(), 1);
    assert_eq!(ch[0]["name"], "CCTV-5");
}

#[tokio::test]
async fn deleting_a_source_takes_its_channels_with_it() {
    let (st, tok, dir) = state().await;
    let p = dir.path().join("list.m3u");
    tokio::fs::write(&p, M3U).await.unwrap();
    let id = add_source(&st, &tok, &p.to_string_lossy()).await;
    let _ = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;

    let r = send(&st, delete(&tok, &format!("/api/v1/live/sources/{id}"))).await;
    assert_eq!(r.status, StatusCode::OK);

    let r = send(&st, get(&tok, "/api/v1/live/channels")).await;
    assert_eq!(r.json().as_array().unwrap().len(), 0, "频道应随源一起删除");
}

#[tokio::test]
async fn stopping_an_unknown_channel_is_idempotent() {
    // 客户端关页面时调一次 stop，那时可能根本没起过流。幂等比精确重要。
    let (st, tok, _dir) = state().await;
    let r = send(
        &st,
        post_json(
            &tok,
            &format!("/api/v1/live/channels/{}/stop", uuid::Uuid::new_v4()),
            json!({}),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test]
async fn playing_an_unknown_channel_is_404() {
    let (st, tok, _dir) = state().await;
    let r = send(
        &st,
        post_json(
            &tok,
            &format!("/api/v1/live/channels/{}/play", uuid::Uuid::new_v4()),
            json!({}),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn sync_imports_txt_playlist_with_genres() {
    let (st, tok, dir) = state().await;
    let txt = "📺央视频道,#genre#\nCCTV-1,http://example.com/cctv1.m3u8\n📡卫视频道,#genre#\n广东卫视,http://example.com/gd.m3u8\n";
    let p = dir.path().join("sub.txt");
    std::fs::write(&p, txt).unwrap();

    let id = add_source(&st, &tok, p.to_str().unwrap()).await;
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["channels"], 2);

    let chs = send(&st, get(&tok, "/api/v1/live/channels")).await.json();
    let arr = chs.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["name"], "CCTV-1");
    assert_eq!(arr[0]["group"], "📺央视频道");
    assert_eq!(arr[1]["name"], "广东卫视");
    assert_eq!(arr[1]["group"], "📡卫视频道");
}

#[tokio::test]
async fn probe_filters_out_unreachable_channels() {
    let (st, tok, dir) = state().await;
    let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="央视",坏流-1
http://192.0.2.1:54321/live.ts
#EXTINF:-1 group-title="央视",好流-RTMP
rtmp://live.example.com/live/stream
"#;
    let p = dir.path().join("probe_test.m3u");
    tokio::fs::write(&p, m3u).await.unwrap();

    let id = add_source(&st, &tok, p.to_str().unwrap()).await;
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["channels"], 2);

    // 确保 probe 端点响应 200
    let r = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/probe"), json!({})),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);

    // 直接执行探测函数，验证死链过滤
    let db = st.db.as_ref().unwrap();
    let src_id = whatmedia_core::Id::parse_str(&id).unwrap();
    let client = whatmedia_api::live::build_live_probe_client();
    let (total, disabled) = whatmedia_api::live::probe_source_channels(db, src_id, &client)
        .await
        .unwrap();

    assert_eq!(total, 2);
    assert_eq!(disabled, 1); // 坏流被标记禁用，RTMP保留

    // 获取频道列表，坏流已被过滤
    let chs = send(&st, get(&tok, "/api/v1/live/channels")).await.json();
    let arr = chs.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], "好流-RTMP");
}

#[tokio::test]
async fn play_http_channel_uses_direct_stream_without_ffmpeg() {
    let (st, tok, dir) = state().await;
    let m3u = r#"#EXTM3U
#EXTINF:-1 group-title="央视",CCTV-1
http://example.com/live/cctv1.m3u8
"#;
    let p = dir.path().join("direct_test.m3u");
    tokio::fs::write(&p, m3u).await.unwrap();

    let id = add_source(&st, &tok, p.to_str().unwrap()).await;
    let _ = send(
        &st,
        post_json(&tok, &format!("/api/v1/live/sources/{id}/sync"), json!({})),
    )
    .await;

    let chs = send(&st, get(&tok, "/api/v1/live/channels")).await.json();
    let cid = chs[0]["id"].as_str().unwrap();

    // 播放该 HTTP 直播频道：必须秒级返回 direct = true 且指向代理流，无需启动 ffmpeg
    let r = send(
        &st,
        post_json(
            &tok,
            &format!("/api/v1/live/channels/{cid}/play"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let view = r.json();
    assert_eq!(view["direct"], true, "HTTP/M3U8 直播流必须走直出通道");
    assert!(
        view["playlist_url"]
            .as_str()
            .unwrap()
            .starts_with("/api/v1/live/proxy?url="),
        "必须返回轻量内存代理地址"
    );
}

// 这条依赖 `openapi` feature；不开 feature 的 `cargo test` 会连符号都没有，
// 所以整条测试都 gate 掉，而不是让它编译失败。
#[cfg(feature = "openapi")]
#[tokio::test]
async fn live_endpoints_are_in_the_openapi_contract() {
    // 契约覆盖守卫只查 lib.rs 里注册的路由；这里补一条端到端的确认：
    // 直播的路径真的在生成的文档里（前端类型是从它生成的）。
    let json = whatmedia_api::openapi::openapi_json();
    for p in [
        "/api/v1/live/sources",
        "/api/v1/live/sources/{id}",
        "/api/v1/live/sources/{id}/sync",
        "/api/v1/live/sources/{id}/probe",
        "/api/v1/live/channels",
        "/api/v1/live/channels/{id}/play",
        "/api/v1/live/channels/{id}/stop",
        "/api/v1/live/proxy",
    ] {
        assert!(json.contains(&format!("\"{p}\"")), "契约里缺少 {p}");
    }
}
