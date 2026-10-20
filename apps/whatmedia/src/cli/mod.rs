//! WhatMedia 运维与诊断命令行工具集，包含：
//! - 媒体库增删查与入库扫描（`scan`, `list`, `search`, `libraries`）
//! - 元数据刮削与审核确认（`scrape`, `scrape-stats`, `review`, `candidates`, `identify`, `lock`）
//! - 硬件诊断与播放决策模拟（`hardware`, `play`, `subtitles`, `resume`, `config`, `migrations`）
//! - 缓存与流清理维护（`images`, `reap-streams`）
//! - OpenAPI 契约导出（`openapi`）

pub mod diagnostic;
pub mod maintenance;
pub mod openapi;
pub mod scan;
pub mod scrape;

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;
use whatmedia_config::AppConfig;

use crate::common::{init_logging, open_db};

/// 运维 CLI 子命令枚举定义。
#[derive(Subcommand, Debug, Clone)]
pub enum CliCommand {
    /// 打印解析后的最终配置（含环境变量覆盖），用于黑屏排障
    Config {
        /// 指定外部 TOML 配置文件路径
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// 列出数据库迁移脚本及其执行顺序
    Migrations {
        /// 迁移目录，缺省使用配置文件中的路径
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// 扫描指定目录并将媒体文件批量增量入库
    Scan {
        /// 要扫描的媒体文件夹路径
        #[arg(long)]
        path: PathBuf,
        /// 媒体库名称，缺省使用目录本身名称
        #[arg(long)]
        name: Option<String>,
        /// 调用 ffprobe 读取编码与流信息（需安装 FFmpeg/FFprobe）
        #[arg(long)]
        probe: bool,
        /// 并发入库处理线程上限
        #[arg(long, default_value = "4")]
        concurrency: usize,
        /// 强制全量重扫（忽略文件修改时间指纹）
        #[arg(long)]
        full: bool,
    },
    /// 列出数据库中已登记的媒体条目
    List {
        /// 最大展示条数
        #[arg(long, default_value = "50")]
        limit: u32,
        /// 按媒体类型过滤（movie / series）
        #[arg(long)]
        kind: Option<String>,
    },
    /// 全文检索媒体条目
    Search {
        /// 搜索关键字
        #[arg(long)]
        q: String,
    },
    /// 列出全部已创建的媒体库及其根路径
    Libraries,
    /// 批量刮削元数据（剧情、海报、演职人员）
    Scrape {
        /// 媒体库 ID 或名称；不传则处理全部媒体库
        #[arg(long)]
        library: Option<String>,
        /// 仅重试历史失败项
        #[arg(long)]
        retry_failed: bool,
        /// 关闭自动采纳，全部放入人工审核队列
        #[arg(long)]
        review_only: bool,
        /// 不下载图片（用于网络压测与纯元数据匹配）
        #[arg(long)]
        no_images: bool,
    },
    /// 查看刮削任务队列状态与统计
    ScrapeStats {
        /// 过滤指定媒体库 ID 或名称
        #[arg(long)]
        library: Option<String>,
    },
    /// 列出待人工确认匹配的媒体条目
    Review {
        /// 过滤指定媒体库 ID 或名称
        #[arg(long)]
        library: Option<String>,
    },
    /// 为指定条目在线搜索元数据匹配候选
    Candidates {
        /// 媒体条目 ID
        #[arg(long)]
        item: String,
        /// 覆盖默认搜索用标题
        #[arg(long)]
        title: Option<String>,
        /// 覆盖默认搜索用上映年份
        #[arg(long)]
        year: Option<i32>,
    },
    /// 人工确认：为指定条目指定 Provider 的 ID 进行采纳
    Identify {
        /// 媒体条目 ID
        #[arg(long)]
        item: String,
        /// 元数据提供源（tmdb / bangumi）
        #[arg(long, default_value = "tmdb")]
        provider: String,
        /// 对应源在第三方系统的唯一 ID
        #[arg(long)]
        provider_id: String,
    },
    /// 锁定或解锁元数据字段，避免后续自动刮削覆盖
    Lock {
        /// 媒体条目 ID
        #[arg(long)]
        item: String,
        /// 待锁定字段名，逗号分隔（如 title,overview,year）
        #[arg(long)]
        fields: String,
        /// 是否执行解锁（默认为锁定）
        #[arg(long)]
        unlock: bool,
    },
    /// 图片缓存使用统计与容量清理
    Images {
        /// 查看缓存条数与体积统计
        #[arg(long)]
        stats: bool,
        /// 执行缓存过期与超容主动驱逐
        #[arg(long)]
        evict: bool,
    },
    /// 硬件转码加速能力自检与驱动检测
    Hardware,
    /// 播放决策模拟预览：离线模拟特定客户端对某视频的转码决策
    Play {
        /// 媒体条目 ID（与 --file 二选一）
        #[arg(long)]
        item: Option<String>,
        /// 媒体文件 ID（与 --item 二选一）
        #[arg(long)]
        file: Option<String>,
        /// 模拟客户端档案标识（web / tauri-mpv / android / apple / minimal）
        #[arg(long, default_value = "web")]
        client: String,
        /// 可用网络带宽（kbps），用于测试弱网自动降码率
        #[arg(long)]
        network_kbps: Option<u32>,
    },
    /// 列出指定媒体条目的内嵌与外挂字幕轨
    Subtitles {
        /// 媒体条目 ID
        #[arg(long)]
        item: String,
    },
    /// 查看当前播放进度列表（继续观看）
    Resume {
        /// 最大展示条数
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// 清理因崩溃遗留的临时 HLS 流式转码分片目录
    ReapStreams {
        /// 空闲超时时间（秒），缺省取配置值
        #[arg(long)]
        idle_secs: Option<i64>,
    },
    /// 导出当前系统 OpenAPI 3.0 契约 JSON 文件
    Openapi {
        /// 导出文件目标路径（默认 docs/openapi/openapi.json）
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

/// 执行 CLI 子命令分发。
pub async fn run_cli(command: CliCommand) -> Result<()> {
    let cfg = AppConfig::load(None)?;
    init_logging(&cfg.logging);

    match command {
        CliCommand::Config { file } => diagnostic::config_cmd(file)?,
        CliCommand::Migrations { dir } => diagnostic::migrations_cmd(&cfg, dir)?,
        CliCommand::Libraries => {
            let db = open_db(&cfg).await?;
            scan::libraries_cmd(&db).await?;
        }
        CliCommand::List { limit, kind } => {
            let db = open_db(&cfg).await?;
            scan::list_cmd(&db, limit, kind).await?;
        }
        CliCommand::Search { q } => {
            let db = open_db(&cfg).await?;
            scan::search_cmd(&db, &q).await?;
        }
        CliCommand::Scan {
            path,
            name,
            probe,
            concurrency,
            full,
        } => {
            let db = open_db(&cfg).await?;
            scan::scan_cmd(&db, &cfg, path, name, probe, concurrency, full).await?;
        }
        CliCommand::Scrape {
            library,
            retry_failed,
            review_only,
            no_images,
        } => {
            let db = open_db(&cfg).await?;
            scrape::scrape_cmd(&db, &cfg, library, retry_failed, review_only, no_images).await?;
        }
        CliCommand::ScrapeStats { library } => {
            let db = open_db(&cfg).await?;
            scrape::scrape_stats_cmd(&db, library).await?;
        }
        CliCommand::Review { library } => {
            let db = open_db(&cfg).await?;
            scrape::review_cmd(&db, library).await?;
        }
        CliCommand::Candidates { item, title, year } => {
            let db = open_db(&cfg).await?;
            scrape::candidates_cmd(&db, &cfg, &item, title, year).await?;
        }
        CliCommand::Identify {
            item,
            provider,
            provider_id,
        } => {
            let db = open_db(&cfg).await?;
            scrape::identify_cmd(&db, &cfg, &item, &provider, &provider_id).await?;
        }
        CliCommand::Lock {
            item,
            fields,
            unlock,
        } => {
            let db = open_db(&cfg).await?;
            scrape::lock_cmd(&db, &item, &fields, unlock).await?;
        }
        CliCommand::Images { stats, evict } => {
            let db = open_db(&cfg).await?;
            maintenance::images_cmd(db, &cfg, stats, evict).await?;
        }
        CliCommand::Hardware => {
            diagnostic::hardware_cmd(&cfg).await?;
        }
        CliCommand::Play {
            item,
            file,
            client,
            network_kbps,
        } => {
            let db = open_db(&cfg).await?;
            diagnostic::play_cmd(&db, &cfg, item, file, &client, network_kbps).await?;
        }
        CliCommand::Subtitles { item } => {
            let db = open_db(&cfg).await?;
            diagnostic::subtitles_cmd(&db, &item).await?;
        }
        CliCommand::Resume { limit } => {
            let db = open_db(&cfg).await?;
            diagnostic::resume_cmd(&db, limit).await?;
        }
        CliCommand::ReapStreams { idle_secs } => {
            let db = open_db(&cfg).await?;
            maintenance::reap_streams_cmd(&db, &cfg, idle_secs).await?;
        }
        CliCommand::Openapi { out } => {
            openapi::openapi_cmd(out)?;
        }
    }
    Ok(())
}
