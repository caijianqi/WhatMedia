//! 前端与服务端的能力声明必须一致。
//!
//! **为什么需要这个测试**：同一份"客户端能放什么"的知识，有两个消费者。
//!    · 服务端 `ClientCapability` 用它决定 Direct Play 还是转码；
//!    · 前端 UI 用它决定显示哪些按钮（画质菜单、音轨菜单、字幕菜单）。
//! 两边各写一份，迟早会漂移。而这种漂移**不会报错** ——
//! 它只会表现为"服务端觉得你能放 PGS，前端却给了一个烧录开关"，
//! 或者"前端显示了音轨菜单，点了却没反应"。
//!
//! 所以单一真源放在 `client/src/player/capabilities.json`，
//! 前端 `import` 它，这个测试拿它跟 Rust 的预置档逐字段比对。

use whatmedia_core::{AudioCodec, Container, SubtitleCodec, VideoCodec};
use whatmedia_playback::ClientCapability;
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;

/// 前端的能力清单。
fn manifest() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../client/src/player/capabilities.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("读不到前端能力清单 {path:?}：{e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("能力清单不是合法 JSON：{e}"))
}

/// 取出清单里的每个客户端声明。
///
/// **用 `id` 字段而不是对象的 key**。前端 `CLIENT_CAPABILITIES` 是以 `id` 字段为键
/// 建索引的，如果这里图省事用 key，那么 key 与 id 不一致时测试会全部通过，
/// 而前端实际发出的却是另一个 id —— 那正好是这条测试要防的事。
/// 所以顺带断言两者必须相等。
fn clients() -> Vec<(String, Value)> {
    let m = manifest();
    let map = m["clients"].as_object().expect("clients 应当是对象");
    map.iter()
        .map(|(key, v)| {
            let id = v["id"]
                .as_str()
                .unwrap_or_else(|| panic!("客户端 `{key}` 缺少 id 字段"))
                .to_string();
            assert_eq!(
                key.as_str(),
                id.as_str(),
                "清单里客户端的 key `{key}` 与 id 字段 `{id}` 不一致 —— \
                 前端按 id 索引、测试按 key 校验，不一致就会漏掉漂移"
            );
            (id, v.clone())
        })
        .collect()
}

/// 把 Rust 侧的枚举列表序列化成 JSON 值数组，跟清单里的字符串数组比。
///
/// 用 serde 的输出而不是手写映射表 —— 手写映射表本身也会漂移，
/// 那就成了"用一份可能错的知识去校验另一份"。
fn assert_same_list<T: Serialize>(rust: &[T], json: &Value, what: &str) {
    let a: Vec<Value> = rust
        .iter()
        .map(|x| serde_json::to_value(x).expect("枚举序列化不会失败"))
        .collect();
    let b = json
        .as_array()
        .unwrap_or_else(|| panic!("{what} 应当是数组"));
    assert_eq!(
        serde_json::to_value(&a).unwrap(),
        serde_json::to_value(b).unwrap(),
        "{what} 与前端声明不一致"
    );
}

/// 解析清单里的 codec 字符串，**并确认它没有落进 `Other`**。
///
/// 这几个枚举都带 `#[serde(untagged)] Other(String)` 兜底，写错的 codec 名
/// 不会报错，只会安静地变成一个"未知编码"。那样这个测试就形同虚设了 ——
/// 所以显式断言没有落进 Other。
fn parse_codec<T>(raw: &str) -> T
where
    T: serde::de::DeserializeOwned,
{
    let v: Value = serde_json::from_str(&format!("\"{raw}\"")).expect("codec 名应当是合法字符串");
    serde_json::from_value(v).unwrap_or_else(|e| panic!("codec `{raw}` 无法解析：{e}"))
}

#[test]
fn every_manifest_client_is_recognized_by_server() {
    for (id, _) in clients() {
        let cap = ClientCapability::by_id(&id);
        assert_eq!(
            cap.client_id, id,
            "前端声明的客户端 id `{id}` 服务端不认得，决策时会回落到 web 档 —— \
             这会让 Direct Play 覆盖率的数字失去意义"
        );
    }
}

#[test]
fn codec_lists_match_between_frontend_and_server() {
    for (id, decl) in clients() {
        let cap = ClientCapability::by_id(&id);
        assert_same_list(
            &cap.containers,
            &decl["containers"],
            &format!("{id}.containers"),
        );
        assert_same_list(
            &cap.video_codecs,
            &decl["videoCodecs"],
            &format!("{id}.videoCodecs"),
        );
        assert_same_list(
            &cap.audio_codecs,
            &decl["audioCodecs"],
            &format!("{id}.audioCodecs"),
        );
        assert_same_list(
            &cap.subtitle_codecs,
            &decl["subtitleCodecs"],
            &format!("{id}.subtitleCodecs"),
        );
        assert_eq!(
            cap.supports_bitmap_subtitle,
            decl["bitmapSubtitles"].as_bool().unwrap_or(false),
            "{id}.bitmapSubtitles 与服务端不一致"
        );
    }
}

#[test]
fn no_codec_string_silently_falls_into_other() {
    for (id, decl) in clients() {
        for raw in decl["containers"].as_array().unwrap() {
            let s = raw.as_str().unwrap();
            assert!(
                !matches!(parse_codec::<Container>(s), Container::Other(_)),
                "{id} 的容器 `{s}` 落进了 Container::Other —— 名字写错了"
            );
        }
        for raw in decl["videoCodecs"].as_array().unwrap() {
            let s = raw.as_str().unwrap();
            assert!(
                !matches!(parse_codec::<VideoCodec>(s), VideoCodec::Other(_)),
                "{id} 的视频编码 `{s}` 落进了 VideoCodec::Other —— 名字写错了"
            );
        }
        for raw in decl["audioCodecs"].as_array().unwrap() {
            let s = raw.as_str().unwrap();
            assert!(
                !matches!(parse_codec::<AudioCodec>(s), AudioCodec::Other(_)),
                "{id} 的音频编码 `{s}` 落进了 AudioCodec::Other —— 名字写错了"
            );
        }
        for raw in decl["subtitleCodecs"].as_array().unwrap() {
            let s = raw.as_str().unwrap();
            assert!(
                !matches!(parse_codec::<SubtitleCodec>(s), SubtitleCodec::Other(_)),
                "{id} 的字幕格式 `{s}` 落进了 SubtitleCodec::Other —— 名字写错了"
            );
        }
    }
}

#[test]
fn web_client_declares_what_browsers_actually_cannot_do() {
    // 这几条不是"顺带断言"，它们是前端 UI 的依据：
    // 前端靠 `canSwitchAudioTrack` 决定要不要画音轨菜单。
    // 如果有人把 JSON 里的 web.canSwitchAudioTrack 改成 true，
    // 用户会得到一个点了没反应的菜单。
    let decl = &manifest()["clients"]["web"];
    assert_eq!(
        decl["bitmapSubtitles"], false,
        "浏览器渲染不了 PGS 位图字幕"
    );
    assert_eq!(
        decl["canSwitchAudioTrack"], false,
        "浏览器 <video> 切不了音轨"
    );
    assert_eq!(decl["localFile"], false, "浏览器读不了本地文件路径");

    // 反向：服务端也得同意。
    let cap = ClientCapability::preset_web();
    assert!(!cap.supports_bitmap_subtitle);
}

#[test]
fn mpv_client_is_the_one_that_can_render_bitmaps() {
    let decl = &manifest()["clients"]["tauri-mpv"];
    assert_eq!(decl["bitmapSubtitles"], true);
    assert_eq!(decl["localFile"], true);
    assert_eq!(decl["canSwitchAudioTrack"], true);
    assert!(ClientCapability::preset_mpv().supports_bitmap_subtitle);
}

#[test]
fn manifest_is_not_empty() {
    // 守住"文件被误清空"的情况 —— 空对象会让上面所有测试因为循环零次而全部通过。
    assert!(
        !clients().is_empty(),
        "能力清单里一个客户端都没有，前面的比对会因为循环零次而假通过"
    );
    assert!(clients().len() >= 4);
}
