//! WhatMedia 统一二进制入口。
//!
//! 支持以下两种运行模式：
//! 1. **默认 / 服务模式（Serve）**：无参数运行或使用 `whatmedia serve` 时，启动 HTTP Web 媒体服务；
//! 2. **运维工具模式（CLI）**：通过各类子命令（如 `scan`、`hardware`、`scrape`、`openapi` 等）执行本地管理与排障。

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use whatmedia::cli::{run_cli, CliCommand};
use whatmedia::server::run_server;

#[derive(Parser, Debug)]
#[command(
    name = "whatmedia",
    about = "WhatMedia —— 极简、快速、全能的私有流媒体服务与运维工具",
    version
)]
struct RootArgs {
    #[command(subcommand)]
    command: Option<RootCommand>,
}

#[derive(Subcommand, Debug)]
enum RootCommand {
    /// 启动 WhatMedia HTTP Web 媒体服务（缺省时的默认动作）
    Serve {
        /// 指定外部 TOML 配置文件路径
        #[arg(long, short)]
        config: Option<PathBuf>,
    },

    /// 展开继承全部运维与诊断子命令
    #[command(flatten)]
    Cli(CliCommand),
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = RootArgs::parse();

    match args.command {
        // 无参数时默认直接启动 HTTP 服务
        None => run_server(None).await,
        Some(RootCommand::Serve { config }) => run_server(config).await,
        Some(RootCommand::Cli(cli_cmd)) => run_cli(cli_cmd).await,
    }
}
