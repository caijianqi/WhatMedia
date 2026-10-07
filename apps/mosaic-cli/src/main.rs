//! Mosaic CLI —— 运维与排障入口。调试服务端基本全靠它。

use anyhow::Result;
use clap::{Parser, Subcommand};
use mosaic_config::AppConfig;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mosaic-cli", about = "Mosaic 运维命令行", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 打印解析后的最终配置（含环境变量覆盖），用于排障。
    Config {
        /// 指定 TOML 配置文件
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// 列出迁移文件及其执行顺序。
    Migrations {
        /// workspace 根目录，默认当前目录
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    match Cli::parse().command {
        Command::Config { file } => {
            let cfg = AppConfig::load(file)?;
            println!("{}", toml::to_string_pretty(&cfg)?);
        }
        Command::Migrations { root } => {
            let files = mosaic_database::migration_files(&root)?;
            if files.is_empty() {
                println!("(无迁移文件)");
            }
            for f in files {
                println!("{}", f.display());
            }
        }
    }
    Ok(())
}
