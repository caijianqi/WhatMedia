// Tauri 2 客户端外壳。P4 才会真正启用（桌面端）；S1 Spike 会先在这里做原型。
//
// 重要：业务逻辑全部在 src/（Vue 3 + TypeScript），
// 平台原生能力全部在 plugins/（Tauri Plugin）里用 Rust/Kotlin/Swift 实现。
// 这个文件只做最小装配。

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![ping])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 应用失败");
}
