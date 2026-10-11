//! 契约覆盖率守卫（P4-2）。
//!
//! OpenAPI 文档是**手挂在处理器上的**：新加一个路由，如果忘了加
//! `#[utoipa::path]`，编译照样过、文档照样生成，只是少了那个端点 ——
//! 前端拿到的是一份看起来完整、实际缺项的契约。这个测试就是为了堵这个洞。
//!
//! 判据是从 `build_router()` 的源码里抠出所有 `.route("...")` 的路径字面量，
//! 与生成文档里的 paths 求差集。抠源码而不是反射 axum 的 Router：
//! axum 没有公开的路由枚举 API，反射不出来。

#![cfg(feature = "openapi")]

use std::collections::BTreeSet;
use std::fs;

/// 这些路由故意不进契约，且**每一条都必须写明理由**。
fn allowlist() -> BTreeSet<&'static str> {
    BTreeSet::from([
        // 容器 HEALTHCHECK 用的别名，与 /api/v1/health 同一个处理器；
        // utoipa 不允许一个处理器挂两个 path，所以只登记规范路径。
        "/healthz",
    ])
}

/// 从 `src/lib.rs` 里抠出所有 `.route(...)` 的路径。
///
/// 路由注册有写成一行的，也有 rustfmt 折成多行的（路径落在下一行），
/// 所以不能按行匹配 —— 从 `.route(` 起往后找第一个字符串字面量。
fn routes_in_source() -> BTreeSet<String> {
    let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
        .expect("读不到 crates/api/src/lib.rs");

    let mut out = BTreeSet::new();
    let mut at = 0;
    while let Some(rel) = src[at..].find(".route(") {
        let start = at + rel + ".route(".len();
        let Some(open) = src[start..].find('"') else {
            panic!("`.route(` 后面没有路径字面量");
        };
        let open = start + open;
        let Some(close) = src[open + 1..].find('"') else {
            panic!("`.route(` 的路径字面量没有闭合：{}", &src[start..]);
        };
        let path = &src[open + 1..open + 1 + close];
        assert!(path.starts_with('/'), "路由路径不是绝对路径：{path}");
        out.insert(path.to_string());
        at = open + 1 + close;
    }
    assert!(
        out.len() >= 35,
        "只抠出 {} 条路由，lib.rs 的写法可能变了（换成了 nest 或宏生成）",
        out.len()
    );
    out
}

fn documented_paths() -> BTreeSet<String> {
    let json = whatmedia_api::openapi::openapi_json();
    let doc: serde_json::Value = serde_json::from_str(&json).expect("契约文档不是合法 JSON");
    doc["paths"]
        .as_object()
        .expect("契约文档里没有 paths")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn every_route_is_documented() {
    let routes = routes_in_source();
    let documented = documented_paths();
    let allow = allowlist();

    let missing: Vec<&String> = routes
        .iter()
        .filter(|r| !documented.contains(*r) && !allow.contains(r.as_str()))
        .collect();

    assert!(
        missing.is_empty(),
        "以下路由没有进 OpenAPI 契约（给处理器加上 `#[cfg_attr(feature = \"openapi\", utoipa::path(...))]`，\
         或到 allowlist 里写明理由）：\n{}",
        missing
            .iter()
            .map(|r| format!("  - {r}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn allowlist_has_no_stale_entries() {
    // 反过来也守：白名单里写了一条路由，后来路由删了，这条就该一起删，
    // 否则白名单会越积越多，最后变成"反正都在白名单里"的摆设。
    let routes = routes_in_source();
    let allow = allowlist();
    let stale: Vec<&&str> = allow.iter().filter(|r| !routes.contains(**r)).collect();
    assert!(
        stale.is_empty(),
        "allowlist 里有已经不存在的路由，请删掉：{stale:?}"
    );
}

#[test]
fn spec_has_all_status_codes_declared() {
    // 每个端点至少要声明一个成功响应。只声明 4xx 的端点说明标注写漏了，
    // 而漏掉的那个成功响应，前端生成的类型里会是 `never`。
    let json = whatmedia_api::openapi::openapi_json();
    let doc: serde_json::Value = serde_json::from_str(&json).unwrap();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            let responses = op["responses"].as_object().unwrap_or_else(|| {
                panic!("{path} {method} 没有 responses");
            });
            let ok = responses.keys().any(|c| c.starts_with('2'));
            assert!(
                ok,
                "{path} {method} 没有声明任何 2xx 响应：{:?}",
                responses.keys()
            );
        }
    }
}
