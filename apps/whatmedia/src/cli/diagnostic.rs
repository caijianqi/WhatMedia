//! 硬件诊断、配置排障与播放决策预览子命令模块。
//!
//! 提供转码加速芯片自检、多端播放策略离线预演、字幕轨详情透视及数据库迁移脚本查看。

use anyhow::{Context, Result};
use std::path::PathBuf;
use whatmedia_config::AppConfig;
use whatmedia_database::{repo, Db};
use crate::common::{human_ms, parse_id};

/// 打印最终生效的配置（合并环境变量与 TOML 结果），便于黑屏运维排障。
pub fn config_cmd(file: Option<PathBuf>) -> Result<()> {
    let c = AppConfig::load(file)?;
    println!("{}", toml::to_string_pretty(&c)?);
    Ok(())
}

/// 列出迁移文件及其执行顺序。
pub fn migrations_cmd(cfg: &AppConfig, dir: Option<PathBuf>) -> Result<()> {
    let dir = dir.unwrap_or_else(|| cfg.paths.migrations_dir.clone());
    let files = whatmedia_database::migration_files(&dir)?;
    if files.is_empty() {
        println!("(指定目录下无迁移文件：{})", dir.display());
    } else {
        println!("数据库迁移脚本列表（按执行顺序）：");
        for f in files {
            println!("  {}", f.display());
        }
    }
    Ok(())
}

/// 硬件转码能力与驱动自检。
///
/// 输出转码决策依据：包括 DRM / NVIDIA / Rockchip 节点状态、FFmpeg 可用性及各档位加速特性。
pub async fn hardware_cmd(cfg: &AppConfig) -> Result<()> {
    let env = whatmedia_hardware::RealEnv::new();
    let machine = whatmedia_hardware::detect_machine(&env);
    println!("系统架构  {} / {}", machine.os, machine.arch);
    println!(
        "CPU 平台  {}（{} 逻辑核心）",
        machine.cpu.model.as_deref().unwrap_or("未知"),
        machine.cpu.cores
    );
    println!(
        "物理内存  {:.1} GB",
        machine.memory.total_bytes as f64 / 1024.0 / 1024.0 / 1024.0
    );
    println!(
        "设备节点  DRM节点数={} NVIDIA卡数={} Rockchip MPP服务={} RGA加速={}",
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
        "\nFFmpeg    {}（路径：{}）",
        if ffmpeg_ok { "就绪可用" } else { "不可用" },
        ffmpeg_bin
    );
    if !ffmpeg_ok {
        println!(
            "          → Direct Play 原画播放之外的路径均不可用；\
             请安装 FFmpeg 或设置 WHATMEDIA_FFMPEG_PATH 环境变量"
        );
    }

    println!("\n硬件加速后端（按优先级排列）：");
    let report = whatmedia_hardware::report(machine);
    for (id, p) in &report.profiles {
        println!(
            "  {:<14} 支持编码 {:?}  零拷贝={}  色调映射(Tone Mapping)={}  最大并发路数={}",
            id,
            p.encoders,
            p.supports_zero_copy,
            p.supports_tone_mapping,
            p.max_concurrent_sessions
        );
    }
    Ok(())
}

/// 播放决策模拟：离线预演特定客户端在播放指定媒体时的服务端决策。
pub async fn play_cmd(
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
                parse_id(&item.context("需要指定 --item 或 --file 参数")?)?;
            let files = repo::files_for_item(db, item_id).await?;
            let (f, _) = files
                .into_iter()
                .find(|(f, _)| f.is_available)
                .context("该媒体条目下未找到可用媒体文件")?;
            f.id
        }
    };

    let target = repo::file_target(db, file_id)
        .await?
        .context("指定文件不存在")?;
    let files = repo::files_for_item(db, target.item_id).await?;
    let streams = files
        .into_iter()
        .find(|(f, _)| f.id == file_id)
        .map(|(_, s)| s)
        .unwrap_or_default();

    let media = whatmedia_playback::MediaCapability::from_parts(
        target.file.container.clone(),
        target.file.duration_ms,
        target.file.bitrate.map(|b| (b / 1000) as u32),
        &streams,
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

    println!("媒体文件  {}", target.file.path);
    println!(
        "片源规格  {} / {} / {}bit / {:?} / {}kbps",
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
    println!("模拟终端  {}", client.client_id);
    println!("\n转码策略  {:?}", d.strategy);
    if d.reasons.is_empty() {
        println!("策略依据  客户端完全支持，执行原画直出 (Direct Play)");
    } else {
        for r in &d.reasons {
            println!("策略依据  [{}] {}", r.code(), r.message());
        }
    }

    let mut preview = d;
    if let Some(req) = preview.stream.as_mut() {
        req.input = target.file.path.clone();
    }

    if !preview.accel_chain.is_empty() {
        println!(
            "\n硬件降级链  {}",
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
        println!("\n生成 FFmpeg 转码命令（共 {} 个参数）：", args.len());
        println!("  ffmpeg {}", args.join(" "));
    }
    Ok(())
}

/// 查看当前用户的继续观看（Resume）播放进度列表。
pub async fn resume_cmd(db: &Db, limit: u32) -> Result<()> {
    let m = whatmedia_playback::SessionManager::new(db.clone());
    let pts = m.resume(None, limit.min(100)).await?;
    if pts.is_empty() {
        println!("(暂无未播完的记录)");
        return Ok(());
    }
    for p in pts {
        let pct = match p.duration_ms {
            Some(d) if d > 0 => format!("{:.0}%", p.position_ms as f64 * 100.0 / d as f64),
            _ => "-".to_string(),
        };
        println!(
            "{}\t{}\t播放进度 {}\t{}",
            p.item_id,
            p.title,
            human_ms(p.position_ms),
            pct
        );
    }
    Ok(())
}
