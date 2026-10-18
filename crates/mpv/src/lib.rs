//! mpv 后端：**外部 mpv 进程 + JSON IPC**，而不是把 libmpv 链进二进制。
//!
//! ## 为什么不做 libmpv 静态链接
//!
//! 1. **双架构打包成本**。链 libmpv 意味着 x86_64 与 ARM64 各要编一份 mpv
//!    及其全部依赖（ffmpeg、placebo、lua…），且要和 Tauri 的链接器设置对齐。
//! 2. **失败模式**。静态链接时 mpv 出问题就是整个桌面端起不来；
//!    外部进程模式下，mpv 缺失只是**降级**到内嵌 Web 播放器。
//! 3. **可测性**。IPC 是换行分隔的 JSON，编解码是纯函数 —— 取消、事件乱序、
//!    半包这些真正会出错的地方都能离线测；而 libmpv 的这些行为只能上真机看。
//!
//! 代价是控制条要自己实现、进程要自己看住。这是明确接受的取舍。
//!
//! ## 与 需求稿 §53 的关系
//!
//! §53 定义的是 `MediaPlayer` 的 15 个方法，本 crate 不实现那个接口
//! （它是在前端 TypeScript 里实现的）。这里提供的是它底下那一层：
//! 怎么把 mpv 拉起来、怎么跟它说话。

pub mod args;
pub mod detect;
pub mod ipc;

pub use args::{
    build_args, seek_cmd, set_property_cmd, url_allowed, LaunchRequest, SubtitleAttach,
};
pub use detect::{detect, detect_with_override, MpvBinary, MpvEnv, Os, RealMpvEnv};
pub use ipc::{
    encode_async, encode_command, ipc_path, observe_properties, EventReader, IpcPath, MpvEvent,
};

/// 这个 crate 覆盖的 mpv 最低版本。
///
/// `--input-ipc-server` 很早就有，但 `sub-add` 的 `select` 标志和
/// `seek` 的 `absolute+exact` 是后来的。低于这个版本就当"没有 mpv"处理，
/// 让上层降级，而不是带着半套能力去播。
pub const MIN_MPV_VERSION: (u32, u32) = (0, 34);

/// 判断版本号是否满足最低要求。解析不出来一律返回 false ——
/// 认不出来就不敢用，报保守比报激进安全。
pub fn version_ok(version: &str) -> bool {
    let Some((maj, min)) = parse_version(version) else {
        return false;
    };
    (maj, min) >= MIN_MPV_VERSION
}

/// 从 `mpv --version` 的输出里抠版本号。
///
/// 真实输出形如 `mpv 0.37.0 Copyright © 2000-2023 mpv/MPlayer/mplayer2 projects`。
/// 也有发行版打补丁后变成 `mpv 0.36.0+git.20230101.abcdef`。
fn parse_version(raw: &str) -> Option<(u32, u32)> {
    // 只取第一个看起来像 `x.y` 的 token，后面的补丁号与 git 尾巴一律忽略。
    let token = raw.split_whitespace().find(|t| t.contains('.'))?;
    let mut parts = token.split('.');
    let maj = parts.next()?.parse::<u32>().ok()?;
    let min = parts.next()?.parse::<u32>().ok()?;
    Some((maj, min))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_parsed_from_real_mpv_output() {
        assert_eq!(
            parse_version("mpv 0.37.0 Copyright © 2000-2023 mpv/MPlayer/mplayer2 projects"),
            Some((0, 37))
        );
    }

    #[test]
    fn version_tolerates_distro_suffixes() {
        assert_eq!(
            parse_version("mpv 0.36.0+git.20230101.abcdef"),
            Some((0, 36))
        );
    }

    #[test]
    fn unparsable_version_is_rejected_not_guessed() {
        assert_eq!(parse_version("mpv (unknown)"), None);
        assert!(!version_ok("mpv (unknown)"));
    }

    #[test]
    fn below_minimum_is_rejected() {
        assert!(!version_ok("mpv 0.33.1"));
        assert!(version_ok("mpv 0.34.0"));
        assert!(version_ok("mpv 0.37.0"));
    }
}
