//! `whatmedia-server` 向后兼容可执行垫片。
//!
//! 兼容旧版独立构建与启动脚本（如旧版 Dockerfile 或 Makefile 中的 `whatmedia-server`）。
//! 内部直接转调统一入口的服务端启动逻辑。

use anyhow::Result;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    // 兼容位置参数传入 config.toml 或 --config
    let config_path = std::env::args()
        .nth(1)
        .filter(|a| a.ends_with(".toml"))
        .map(PathBuf::from);

    whatmedia::server::run_server(config_path).await
}
