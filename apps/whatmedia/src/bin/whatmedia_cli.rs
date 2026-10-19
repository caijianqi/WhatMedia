//! `whatmedia-cli` 向后兼容可执行垫片。
//!
//! 兼容旧版命令行调用习惯（如旧版 Makefile、CI 脚本或本地运行的 `cargo run --bin whatmedia-cli`）。
//! 内部直接转调统一入口的 CLI 分发逻辑。

use anyhow::Result;
use clap::Parser;
use whatmedia::cli::{run_cli, CliCommand};

#[derive(Parser, Debug)]
#[command(
    name = "whatmedia-cli",
    about = "WhatMedia 运维命令行（兼容垫片，推荐使用 whatmedia <command>）",
    version
)]
struct CliShimArgs {
    #[command(subcommand)]
    command: CliCommand,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = CliShimArgs::parse();
    run_cli(args.command).await
}
