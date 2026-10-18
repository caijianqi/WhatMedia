//! WhatMedia CLI —— 运维与排障入口。
//!
//! 服务端调试基本全靠它：看配置、看迁移、扫一个目录、看看扫出来什么。
//! 它和 HTTP API 走**同一套** repo，不存在"CLI 能查到但 API 查不到"的分叉。

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use whatmedia_config::AppConfig;
use whatmedia_core::MediaKind;
use whatmedia_database::{repo, Db};
use whatmedia_scanner::{ScanConfig, ScanControl, ScanProgress, Scanner};
use whatmedia_storage::LocalStorage;

#[derive(Parser)]
#[command(name = "whatmedia-cli", about = "WhatMedia 运维命令行", version)]
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
        /// 迁移目录，默认取配置里的值
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// 扫描一个目录并入库。**P1 的主交付物就是这条命令。**
    Scan {
        /// 要扫描的目录
        #[arg(long)]
        path: PathBuf,
        /// 媒体库名称，缺省用目录名
        #[arg(long)]
        name: Option<String>,
        /// 调用 ffprobe 读取流信息（需要本机装有 FFmpeg）
        #[arg(long)]
        probe: bool,
        /// 并发入库上限
        #[arg(long, default_value = "4")]
        concurrency: usize,
        /// 强制全量重扫（忽略增量指纹）
        #[arg(long)]
        full: bool,
    },
    /// 列出已入库的媒体条目。
    List {
        #[arg(long, default_value = "50")]
        limit: u32,
        #[arg(long)]
        kind: Option<String>,
    },
    /// 全文检索。
    Search {
        #[arg(long)]
        q: String,
    },
    /// 列出媒体库。
    Libraries,
    /// 刮削元数据。**P2 的主交付物。**
    Scrape {
        /// 媒体库 id；不给则处理全部库
        #[arg(long)]
        library: Option<String>,
        /// 只重跑失败项
        #[arg(long)]
        retry_failed: bool,
        /// 关闭自动采用，全部进人工确认队列
        #[arg(long)]
        review_only: bool,
        /// 不下载图片（压测匹配率时用）
        #[arg(long)]
        no_images: bool,
    },
    /// 查看刮削进度。
    ScrapeStats {
        #[arg(long)]
        library: Option<String>,
    },
    /// 列出待人工确认的条目。
    Review {
        #[arg(long)]
        library: Option<String>,
    },
    /// 给某个条目搜索候选。
    Candidates {
        #[arg(long)]
        item: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        year: Option<i32>,
    },
    /// 人工确认：采用指定 Provider 的指定 id。
    Identify {
        #[arg(long)]
        item: String,
        #[arg(long, default_value = "tmdb")]
        provider: String,
        #[arg(long)]
        provider_id: String,
    },
    /// 锁定 / 解锁字段，使其不被刮削覆盖。
    Lock {
        #[arg(long)]
        item: String,
        /// 字段名，逗号分隔
        #[arg(long)]
        fields: String,
        #[arg(long)]
        unlock: bool,
    },
    /// 图片缓存统计与清理。
    Images {
        #[arg(long)]
        stats: bool,
        /// 清理超出预算的缓存
        #[arg(long)]
        evict: bool,
    },
    /// 硬件能力自检。**P3。**
    Hardware,
    /// 播放决策预览：看看这个片在这台设备上会怎么播。**P3。**
    Play {
        /// 条目 id；不给 file 时取该条目的第一个可用文件
        #[arg(long)]
        item: Option<String>,
        /// 文件 id
        #[arg(long)]
        file: Option<String>,
        /// 客户端能力档：web / tauri-mpv / android / apple / minimal
        #[arg(long, default_value = "web")]
        client: String,
        /// 可用带宽（kbps）。用来模拟弱网
        #[arg(long)]
        network_kbps: Option<u32>,
    },
    /// 列出某个条目的字幕轨（含外挂）。
    Subtitles {
        #[arg(long)]
        item: String,
    },
    /// 继续观看列表。**P3。**
    Resume {
        #[arg(long, default_value = "20")]
        limit: u32,
    },
    /// 清理遗留的流式输出目录（上次崩溃留下的 HLS 分片）。
    ReapStreams {
        /// 空闲多少秒算僵尸，默认取配置值
        #[arg(long)]
        idle_secs: Option<i64>,
    },
    /// 导出 OpenAPI 契约。**P4-2。**需要以 `--features openapi` 构建。
    Openapi {
        /// 输出路径，默认 `docs/openapi/openapi.json`
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = AppConfig::load(None)?;
    init_logging(&cfg);

    match cli.command {
        Command::Config { file } => {
            let c = AppConfig::load(file)?;
            println!("{}", toml::to_string_pretty(&c)?);
        }
        Command::Migrations { dir } => {
            let dir = dir.unwrap_or(cfg.paths.migrations_dir.clone());
            let files = whatmedia_database::migration_files(&dir)?;
            if files.is_empty() {
                println!("(无迁移文件：{})", dir.display());
            }
            for f in files {
                println!("{}", f.display());
            }
        }
        Command::Libraries => {
            let db = open_db(&cfg).await?;
            for l in repo::list_libraries(&db).await? {
                println!("{}\t{}\t{}", l.id, l.name, l.root_path);
            }
        }
        Command::List { limit, kind } => {
            let db = open_db(&cfg).await?;
            let q = repo::ItemQuery {
                kind: kind.as_deref().and_then(parse_kind),
                page_size: limit.min(200),
                ..Default::default()
            };
            let page = repo::list_items(&db, &q).await?;
            for it in &page.data {
                println!("{}\t{:?}\t{}\t{:?}", it.id, it.kind, it.title, it.year);
            }
            println!("共 {} 条", page.page.total);
        }
        Command::Search { q } => {
            let db = open_db(&cfg).await?;
            let page = repo::search_items(&db, &q, &repo::ItemQuery::default()).await?;
            for it in &page.data {
                println!("{}\t{}", it.id, it.title);
            }
            println!("命中 {} 条", page.page.total);
        }
        Command::Scan {
            path,
            name,
            probe,
            concurrency,
            full,
        } => {
            let db = open_db(&cfg).await?;
            scan_cmd(&db, &cfg, path, name, probe, concurrency, full).await?;
        }
        Command::Scrape {
            library,
            retry_failed,
            review_only,
            no_images,
        } => {
            let db = open_db(&cfg).await?;
            scrape_cmd(&db, &cfg, library, retry_failed, review_only, no_images).await?;
        }
        Command::ScrapeStats { library } => {
            let db = open_db(&cfg).await?;
            let lib = resolve_library(&db, library.as_deref()).await?;
            let s = repo::metadata::job_stats(&db, lib.map(|l| l.id)).await?;
            println!(
                "待处理 {}\t运行中 {}\t已完成 {}\t失败 {}\t待确认 {}\t跳过 {}",
                s.pending, s.running, s.done, s.failed, s.review, s.skipped
            );
        }
        Command::Review { library } => {
            let db = open_db(&cfg).await?;
            let lib = resolve_library(&db, library.as_deref()).await?;
            for t in repo::metadata::list_review(&db, lib.map(|l| l.id), 200).await? {
                println!(
                    "{}\t{:?}\t{}\t搜索用标题：{}",
                    t.id, t.kind, t.title, t.match_title
                );
            }
        }
        Command::Candidates { item, title, year } => {
            let db = open_db(&cfg).await?;
            let scraper = build_scraper(&db, &cfg, whatmedia_metadata::ScrapeConfig::default())?;
            let id = parse_id(&item)?;
            for (i, c) in scraper
                .candidates(id, title, year)
                .await?
                .iter()
                .enumerate()
            {
                println!(
                    "{}. [{}/{}] {} ({:?})\t得分 {:.3}\t{}",
                    i + 1,
                    c.provider,
                    c.provider_id,
                    c.title,
                    c.year,
                    c.score,
                    c.confidence
                );
            }
        }
        Command::Identify {
            item,
            provider,
            provider_id,
        } => {
            let db = open_db(&cfg).await?;
            let scraper = build_scraper(&db, &cfg, whatmedia_metadata::ScrapeConfig::default())?;
            scraper
                .identify(parse_id(&item)?, &provider, &provider_id)
                .await?;
            println!("已采用 {provider}:{provider_id}");
        }
        Command::Lock {
            item,
            fields,
            unlock,
        } => {
            let db = open_db(&cfg).await?;
            let id = parse_id(&item)?;
            let current = repo::metadata::locked_fields_of_item(&db, id).await?;
            let mut locked = whatmedia_metadata::parse_locked(&current);
            for f in fields.split(',') {
                let f = f.trim();
                if !f.is_empty() && !whatmedia_metadata::set_locked(&mut locked, f, !unlock) {
                    println!("跳过非法字段名：{f}");
                }
            }
            repo::metadata::update_item_meta(
                &db,
                id,
                &repo::metadata::ItemMetaUpdate {
                    locked_fields: Some(whatmedia_metadata::serialize_locked(&locked)),
                    ..Default::default()
                },
            )
            .await?;
            let mut out: Vec<String> = locked.into_iter().collect();
            out.sort();
            println!("已锁定：{:?}", out);
        }
        Command::Images { stats, evict } => {
            let db = open_db(&cfg).await?;
            let cache = whatmedia_metadata::ImageCache::new(
                db,
                whatmedia_metadata::ImageCacheConfig {
                    dir: cfg.images_dir(),
                    max_bytes: cfg.images.max_bytes,
                    max_entries: cfg.images.max_entries,
                    max_age: chrono::Duration::days(cfg.images.max_age_days),
                },
            )?;
            if stats {
                let s = cache.stats().await?;
                println!(
                    "条目 {}\t占用 {:.1} MB\t累计命中 {}",
                    s.entries,
                    s.bytes as f64 / 1024.0 / 1024.0,
                    s.hits
                );
            }
            if evict {
                let r = cache.evict().await?;
                println!(
                    "清理 {} 项，释放 {:.1} MB（按时间 {}，按体积 {}）",
                    r.removed,
                    r.bytes_freed as f64 / 1024.0 / 1024.0,
                    r.by_age,
                    r.by_size
                );
            }
        }
        Command::Hardware => {
            hardware_cmd(&cfg).await?;
        }
        Command::Play {
            item,
            file,
            client,
            network_kbps,
        } => {
            let db = open_db(&cfg).await?;
            play_cmd(&db, &cfg, item, file, &client, network_kbps).await?;
        }
        Command::Subtitles { item } => {
            let db = open_db(&cfg).await?;
            subtitles_cmd(&db, &cfg, &item).await?;
        }
        Command::Resume { limit } => {
            let db = open_db(&cfg).await?;
            let m = whatmedia_playback::SessionManager::new(db);
            let pts = m.resume(None, limit.min(100)).await?;
            if pts.is_empty() {
                println!("(没有继续观看的记录)");
            }
            for p in pts {
                let pct = match p.duration_ms {
                    Some(d) if d > 0 => format!("{:.0}%", p.position_ms as f64 * 100.0 / d as f64),
                    _ => "-".to_string(),
                };
                println!(
                    "{}\t{}\t进度 {}\t{}",
                    p.item_id,
                    p.title,
                    human_ms(p.position_ms),
                    pct
                );
            }
        }
        Command::ReapStreams { idle_secs } => {
            let db = open_db(&cfg).await?;
            let idle = idle_secs.unwrap_or(cfg.transcode.stream_idle_timeout_secs);
            let orphan =
                whatmedia_api::playback::reap_orphan_streams(&db, &cfg.paths.cache_dir.join("hls"))
                    .await?;
            let stale = repo::list_idle_stream_sessions(&db, idle).await?;
            let mut n = 0;
            for s in stale {
                let dir = std::path::Path::new(&s.output_dir);
                if dir.starts_with(cfg.paths.cache_dir.join("hls")) {
                    let _ = std::fs::remove_dir_all(dir);
                    n += 1;
                }
                repo::delete_stream_session(&db, s.job_id).await?;
            }
            println!("清理遗留目录 {orphan} 个，僵尸流式输出 {n} 个");
        }
        Command::Openapi { out } => {
            // feature 关掉时 API 里根本没有这份文档。与其静默产出空文件，
            // 不如明确告诉使用者该用什么命令重新构建。
            #[cfg(feature = "openapi")]
            {
                let path = out.unwrap_or_else(|| PathBuf::from("docs/openapi/openapi.json"));
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        std::fs::create_dir_all(parent)?;
                    }
                }
                let json = whatmedia_api::openapi::openapi_json();
                std::fs::write(&path, format!("{json}\n"))?;
                println!("已写入 {}", path.display());
            }
            #[cfg(not(feature = "openapi"))]
            {
                let _ = out;
                anyhow::bail!(
                    "本二进制未启用 openapi feature：请用 cargo run --features openapi 重新构建"
                );
            }
        }
    }
    Ok(())
}

fn parse_id(s: &str) -> Result<whatmedia_core::Id> {
    uuid::Uuid::parse_str(s).with_context(|| format!("不是合法的 UUID：{s}"))
}

async fn resolve_library(
    db: &Db,
    name_or_id: Option<&str>,
) -> Result<Option<whatmedia_core::Library>> {
    let Some(raw) = name_or_id else {
        return Ok(None);
    };
    if let Ok(id) = uuid::Uuid::parse_str(raw) {
        if let Some(l) = repo::get_library(db, id).await? {
            return Ok(Some(l));
        }
    }
    for l in repo::list_libraries(db).await? {
        if l.name == raw || l.root_path == raw {
            return Ok(Some(l));
        }
    }
    anyhow::bail!("找不到媒体库：{raw}")
}

fn build_scraper(
    db: &Db,
    cfg: &AppConfig,
    scrape: whatmedia_metadata::ScrapeConfig,
) -> Result<whatmedia_metadata::Scraper> {
    let chain = whatmedia_metadata::ProviderChainConfig {
        tmdb_api_key: cfg.metadata.tmdb_api_key.clone(),
        tmdb_bearer: cfg.metadata.tmdb_bearer.clone(),
        bangumi_enabled: cfg.metadata.bangumi_enabled,
        language: cfg.metadata.language.clone(),
        requests_per_second: cfg.metadata.requests_per_second,
    };
    let providers = whatmedia_metadata::build_providers(&chain);
    if providers.is_empty() {
        anyhow::bail!(
            "未配置任何元数据 Provider。设置 WHATMEDIA_METADATA_TMDB_API_KEY \
             或 WHATMEDIA_METADATA_BANGUMI=1"
        );
    }
    Ok(whatmedia_metadata::Scraper::new(
        db.clone(),
        providers,
        scrape,
    ))
}

async fn scrape_cmd(
    db: &Db,
    cfg: &AppConfig,
    library: Option<String>,
    retry_failed: bool,
    review_only: bool,
    no_images: bool,
) -> Result<()> {
    let lib = resolve_library(db, library.as_deref()).await?;
    let scraper = build_scraper(
        db,
        cfg,
        whatmedia_metadata::ScrapeConfig {
            auto_accept: !review_only,
            fetch_images: !no_images,
            ..Default::default()
        },
    )?;

    let queued = match (&lib, retry_failed) {
        (Some(l), true) => scraper.requeue_failed(l.id).await?,
        (Some(l), false) => scraper.enqueue(l.id).await?,
        (None, _) => 0,
    };
    // 崩溃遗留的 running 任务先捞回来
    let reaped = scraper.reap_running().await?;
    if reaped > 0 {
        println!("已回收 {reaped} 个中断的任务");
    }
    println!("入队 {queued} 项，开始刮削…");

    let stats = scraper.run(lib.map(|l| l.id), Default::default()).await?;
    println!(
        "完成：匹配 {}，待确认 {}，失败 {}，跳过 {} —— 命中率 {:.1}%",
        stats.matched,
        stats.review,
        stats.failed,
        stats.skipped,
        stats.hit_rate() * 100.0
    );
    Ok(())
}

async fn open_db(cfg: &AppConfig) -> Result<Db> {
    let db = Db::open(&cfg.database.url).await?;
    db.migrate(&cfg.paths.migrations_dir).await?;
    Ok(db)
}

async fn scan_cmd(
    db: &Db,
    cfg: &AppConfig,
    path: PathBuf,
    name: Option<String>,
    probe: bool,
    concurrency: usize,
    full: bool,
) -> Result<()> {
    let root = whatmedia_storage::clean_path(
        &path
            .canonicalize()
            .with_context(|| format!("目录不可访问：{}", path.display()))?,
    );

    let lib_name = name.unwrap_or_else(|| {
        root.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "媒体库".into())
    });

    // 按 (存储, 根路径) 幂等：重复扫同一个目录不会产生第二个库。
    let library = match repo::find_library_by_root(db, "local", &root.to_string_lossy()).await? {
        Some(l) => {
            println!("复用已有媒体库：{}（{}）", l.name, l.root_path);
            l
        }
        None => {
            repo::insert_library(
                db,
                &lib_name,
                MediaKind::Mixed,
                "local",
                &root.to_string_lossy(),
                &[],
            )
            .await?
        }
    };

    let storage = Arc::new(LocalStorage::new("local", &root)?);
    let ffprobe = if probe {
        Some(Arc::new(whatmedia_media::FfprobeRunner::default()))
    } else {
        None
    };

    let scanner = Scanner::new(
        storage,
        db.clone(),
        ScanConfig {
            concurrency: concurrency.max(1),
            incremental: !full,
            probe,
            ffprobe,
            ..Default::default()
        },
    );

    let (tx, mut rx) = tokio::sync::broadcast::channel::<ScanProgress>(64);
    let printer = tokio::spawn(async move {
        use std::io::Write;
        while let Ok(p) = rx.recv().await {
            print!(
                "\r扫描中 {}/{}  新增 {} 更新 {} 跳过 {} 失败 {}   ",
                p.done, p.total, p.added, p.updated, p.skipped, p.failed
            );
            let _ = std::io::stdout().flush();
            if p.finished {
                println!();
                break;
            }
        }
    });

    let stats = scanner.scan(&library, ScanControl::new(), Some(tx)).await?;
    let _ = printer.await;

    println!(
        "完成：发现 {}，新增 {}，更新 {}，跳过 {}，失败 {}，已失效 {}",
        stats.found, stats.added, stats.updated, stats.skipped, stats.failed, stats.missing
    );
    println!("数据库：{}", cfg.database.url);
    Ok(())
}

/// 硬件能力自检。
///
/// 输出的是**决策依据**，不是炫技信息：用户问"为什么我的机器不硬件转码"时，
/// 这条命令的输出就是答案。
async fn hardware_cmd(cfg: &AppConfig) -> Result<()> {
    let env = whatmedia_hardware::RealEnv::new();
    let machine = whatmedia_hardware::detect_machine(&env);
    println!("系统      {} / {}", machine.os, machine.arch);
    println!(
        "CPU       {}（{} 核）",
        machine.cpu.model.as_deref().unwrap_or("未知"),
        machine.cpu.cores
    );
    println!(
        "内存      {:.1} GB",
        machine.memory.total_bytes as f64 / 1024.0 / 1024.0 / 1024.0
    );
    println!(
        "设备节点  DRM={} NVIDIA={} MPP={} RGA={}",
        machine.devices.drm_render_nodes.len() + machine.devices.drm_cards.len(),
        machine.devices.nvidia.len(),
        machine.devices.mpp_service,
        machine.devices.rga
    );

    let ffmpeg_bin = cfg
        .transcode
        .ffmpeg_path
        .clone()
        .unwrap_or_else(|| "ffmpeg".to_string());
    let runner = whatmedia_transcoder::FfmpegRunner::new(&ffmpeg_bin);
    let ffmpeg_ok = runner.is_available().await;
    println!(
        "\nffmpeg    {}（{}）",
        if ffmpeg_ok { "可用" } else { "不可用" },
        ffmpeg_bin
    );
    if !ffmpeg_ok {
        println!(
            "          → Direct Play 之外的路径都不可用；\
             请安装 FFmpeg 或用 WHATMEDIA_FFMPEG_PATH 指定路径"
        );
    }

    println!("\n转码后端（按优先级）：");
    let report = whatmedia_hardware::report(machine);
    for (id, p) in &report.profiles {
        println!(
            "  {:<12} 编码 {:?}  零拷贝 {}  tone mapping {}  并发 {}",
            id,
            p.encoders,
            p.supports_zero_copy,
            p.supports_tone_mapping,
            p.max_concurrent_sessions
        );
    }
    Ok(())
}

/// 播放决策预览。
async fn play_cmd(
    db: &Db,
    cfg: &AppConfig,
    item: Option<String>,
    file: Option<String>,
    client_id: &str,
    network_kbps: Option<u32>,
) -> Result<()> {
    let file_id: whatmedia_core::Id = match file {
        Some(f) => parse_id(&f)?,
        None => {
            let item_id: whatmedia_core::Id =
                parse_id(&item.context("需要 --item 或 --file 之一")?)?;
            let files = repo::files_for_item(db, item_id).await?;
            let (f, _) = files
                .into_iter()
                .find(|(f, _)| f.is_available)
                .context("该条目下没有可用文件")?;
            f.id
        }
    };

    let target = repo::file_target(db, file_id)
        .await?
        .context("文件不存在")?;
    let files = repo::files_for_item(db, target.item_id).await?;
    let streams = files
        .into_iter()
        .find(|(f, _)| f.id == file_id)
        .map(|(_, s)| s)
        .unwrap_or_default();

    let storage = LocalStorage::new("local", std::path::PathBuf::from(&target.library_root))?;
    let external = whatmedia_subtitle::discover_sidecars(&storage, &target.file.path)
        .await
        .unwrap_or_default();

    let media = whatmedia_playback::MediaCapability::from_parts(
        target.file.container.clone(),
        target.file.duration_ms,
        target.file.bitrate.map(|b| (b / 1000) as u32),
        &streams,
        external.clone(),
    );
    let client = whatmedia_playback::ClientCapability::by_id(client_id);

    let ffmpeg_bin = cfg
        .transcode
        .ffmpeg_path
        .clone()
        .unwrap_or_else(|| "ffmpeg".to_string());
    let runner = whatmedia_transcoder::FfmpegRunner::new(&ffmpeg_bin);
    let machine = whatmedia_hardware::detect_machine(&whatmedia_hardware::RealEnv::new());
    let server = whatmedia_playback::ServerCapability {
        ffmpeg_available: runner.is_available().await,
        encoders: whatmedia_hardware::build_accels(&machine)
            .iter()
            .filter_map(|a| a.probe().ok())
            .collect(),
        active_transcodes: 0,
        accel_ctx: machine.accel_context(),
    };

    let d = whatmedia_playback::decide(whatmedia_playback::DecisionInput {
        media: &media,
        client: &client,
        server: &server,
        network_kbps,
        start_ms: 0,
        hls_dir: None,
        seg_seconds: cfg.transcode.hls_segment_seconds,
    });

    println!("文件      {}", target.file.path);
    println!(
        "片源      {} / {} / {}bit / {:?} / {}kbps",
        target
            .file
            .container
            .as_ref()
            .map(|c| format!("{c:?}"))
            .unwrap_or_else(|| "未知容器".into()),
        media
            .video
            .as_ref()
            .map(|v| format!("{:?}", v.codec))
            .unwrap_or_else(|| "无视频轨".into()),
        media.video.as_ref().map(|v| v.bit_depth).unwrap_or(0),
        media.video.as_ref().map(|v| v.hdr).unwrap_or_default(),
        media.bitrate_kbps.unwrap_or(0)
    );
    println!("客户端    {}", client.client_id);
    println!("\n决策      {:?}", d.strategy);
    if d.reasons.is_empty() {
        println!("原因      直接播放，无需处理");
    } else {
        for r in &d.reasons {
            println!("原因      [{}] {}", r.code(), r.message());
        }
    }
    // 决策层不知道真实路径（`StreamRequest.input` 留空由调用方填），
    // 预览里不填就会打出一条 `-i ` 后面没有参数的命令 —— 看着像 bug 其实是漏填。
    let mut preview = d;
    if let Some(req) = preview.stream.as_mut() {
        req.input = target.file.path.clone();
    }

    if !preview.accel_chain.is_empty() {
        // 候选链是"硬件失败会往哪退"的答案，必须打印出来 ——
        // 否则用户看到软转很慢，却无从判断是硬件没探到还是降级了。
        println!(
            "\n转码后端链  {}",
            preview
                .accel_chain
                .iter()
                .map(|a| a.id.clone())
                .collect::<Vec<_>>()
                .join(" → ")
        );
    }
    if let Some(req) = &preview.stream {
        let args = whatmedia_transcoder::build_args(req);
        println!("\nffmpeg 参数（{} 个）：", args.len());
        println!("  ffmpeg {}", args.join(" "));
    }
    if !external.is_empty() {
        println!("\n外挂字幕：");
        for e in &external {
            println!(
                "  {}\t{:?}\t{:?}\tforced={}",
                e.name, e.codec, e.language, e.forced
            );
        }
    }
    if !preview.subtitle_plans.is_empty() {
        println!("\n字幕投递：");
        for p in &preview.subtitle_plans {
            println!("  {}\t{:?}\t{}", p.track_id, p.mode, p.reason);
        }
    }
    Ok(())
}

/// 列出条目的字幕轨。
async fn subtitles_cmd(db: &Db, cfg: &AppConfig, item: &str) -> Result<()> {
    let item_id: whatmedia_core::Id = parse_id(item)?;
    let files = repo::files_for_item(db, item_id).await?;
    let Some((file, streams)) = files.into_iter().next() else {
        println!("(条目下没有文件)");
        return Ok(());
    };
    println!("文件：{}", file.path);

    println!("\n内嵌字幕：");
    let mut any = false;
    for s in streams
        .iter()
        .filter(|s| matches!(s.stream_type, whatmedia_core::StreamType::Subtitle))
    {
        any = true;
        println!(
            "  #{}  {}\t{:?}\t{:?}\tforced={}",
            s.index,
            s.codec.as_deref().unwrap_or("-"),
            whatmedia_subtitle::codec_from_probe_name(s.codec.as_deref().unwrap_or("")),
            s.language,
            s.is_forced
        );
    }
    if !any {
        println!("  (无)");
    }

    let storage = match storage_for(db, &file.path).await? {
        Some(s) => s,
        None => {
            println!("\n外挂字幕：(文件不在任何媒体库内，跳过扫描)");
            return Ok(());
        }
    };
    let external = whatmedia_subtitle::discover_sidecars(storage.as_ref(), &file.path)
        .await
        .unwrap_or_default();
    println!("\n外挂字幕：");
    if external.is_empty() {
        println!("  (无)");
    }
    for e in &external {
        println!(
            "  {}\t{:?}\t语言 {:?}\tforced={} default={}",
            e.name, e.codec, e.language, e.forced, e.default
        );
    }
    let _ = cfg;
    Ok(())
}

/// 找出路径所属的媒体库存储实例。
async fn storage_for(db: &Db, path: &str) -> Result<Option<Arc<LocalStorage>>> {
    let p = std::path::Path::new(path);
    for lib in repo::list_libraries(db).await? {
        if p.starts_with(&lib.root_path) {
            return Ok(Some(Arc::new(LocalStorage::new(
                "local",
                std::path::PathBuf::from(&lib.root_path),
            )?)));
        }
    }
    Ok(None)
}

fn human_ms(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn parse_kind(s: &str) -> Option<MediaKind> {
    serde_json::from_str::<MediaKind>(&format!("\"{s}\"")).ok()
}

fn init_logging(cfg: &AppConfig) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cfg.logging.level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
