//! 字幕投递策略 —— **降级链必须显式写下来**（P3 DoD 要求）。
//!
//! 一个字幕轨最终只有四种归宿：
//!
//! ```text
//! Direct   原样交给客户端（内嵌轨随容器走 / 外挂文件直出）
//!   ↓ 客户端不认这个格式
//! Convert  转成客户端认的文本格式（SRT ↔ VTT），服务端转码，CPU 很便宜
//!   ↓ 客户端连 SRT/VTT 都不认
//! BurnIn   服务端把字幕烧进画面（必须重编码，最贵）
//!   ↓ 服务端不能烧（没有 ffmpeg / 并发打满）
//! Drop     不给字幕，播放继续
//! ```
//!
//! **两条硬性规则**：
//! 1. 位图字幕（PGS / DVD SUB / DVB SUB）**不做 OCR**。文档 §51 没承诺 OCR，
//!    做 OCR 会引入一整个重度依赖，收益却只在极少数片源上。
//!    位图字幕只有 BurnIn 或 Drop 两条路。
//! 2. 字幕永远不该让"能不能播"变成不能播。任何降级失败都落到 Drop，
//!    绝不因为字幕问题把整条播放路径判成 Unsupported。

use whatmedia_core::SubtitleCodec;

use crate::lang::{is_bitmap, is_text};

/// 投递方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleMode {
    /// 原样投递。
    Direct,
    /// 服务端转换格式（SRT / VTT 互转）。
    Convert,
    /// 服务端烧录（必须重编码视频）。
    BurnIn,
    /// 放弃这条字幕。播放不受影响。
    Drop,
}

impl SubtitleMode {
    /// 是否需要重编码视频。只有烧录需要 —— 它直接决定决策引擎走 Remux 还是 Transcode。
    pub fn requires_transcode(&self) -> bool {
        matches!(self, SubtitleMode::BurnIn)
    }
}

/// 客户端的字幕能力。从 `ClientCapability` 里抽出来的最小集合。
#[derive(Debug, Clone, Default)]
pub struct SubtitleTarget {
    /// 客户端能直接渲染的字幕格式。
    pub supported: Vec<SubtitleCodec>,
    /// 客户端是否能自己渲染位图字幕（mpv / VLC 可以，浏览器不行）。
    pub supports_bitmap: bool,
}

impl SubtitleTarget {
    pub fn supports(&self, codec: &SubtitleCodec) -> bool {
        if is_bitmap(codec) {
            return self.supports_bitmap;
        }
        self.supported.contains(codec)
    }

    /// 挑一个客户端认的文本格式作为转换目标。优先 VTT（HLS 友好），其次 SRT。
    pub fn convert_target(&self) -> Option<SubtitleCodec> {
        [SubtitleCodec::Vtt, SubtitleCodec::Srt, SubtitleCodec::Ass]
            .into_iter()
            .find(|c| self.supported.contains(c))
    }
}

/// 服务端能力。烧录要重编码，所以受并发上限约束。
#[derive(Debug, Clone, Default)]
pub struct BurnCapability {
    /// 有没有可用的 ffmpeg。
    pub ffmpeg_available: bool,
    /// 当前还能不能再开一路烧录。
    pub has_budget: bool,
}

impl BurnCapability {
    pub fn can_burn(&self) -> bool {
        self.ffmpeg_available && self.has_budget
    }
}

/// 单条字幕轨的投递计划。
#[derive(Debug, Clone)]
pub struct SubtitlePlan {
    /// `internal:{index}` 或 `external:{path}`，供 API 定位。
    pub track_id: String,
    pub codec: SubtitleCodec,
    pub language: Option<String>,
    pub mode: SubtitleMode,
    /// 转换目标格式。`Convert` 时一定有值。
    pub convert_to: Option<SubtitleCodec>,
    /// 为什么这么选。用户问"为什么中文字幕没了"时要答得出来。
    pub reason: &'static str,
}

/// 给一条字幕轨定投递方式。
pub fn plan_track(
    track_id: impl Into<String>,
    codec: SubtitleCodec,
    language: Option<String>,
    client: &SubtitleTarget,
    server: &BurnCapability,
) -> SubtitlePlan {
    let track_id = track_id.into();

    if is_bitmap(&codec) {
        // 规则 1：位图字幕不做 OCR，只有"客户端自己渲染 / 服务端烧录 / 丢弃"三条路。
        //
        // 先问客户端：**mpv 与 VLC 是能自己渲染 PGS 的**。
        // 早期版本这里直接烧录，结果桌面端放蓝光原盘也在转码 ——
        // 兼容性矩阵测出来才发现的。
        return if client.supports(&codec) {
            SubtitlePlan {
                track_id,
                codec,
                language,
                mode: SubtitleMode::Direct,
                convert_to: None,
                reason: "客户端支持位图字幕",
            }
        } else if server.can_burn() {
            SubtitlePlan {
                track_id,
                codec,
                language,
                mode: SubtitleMode::BurnIn,
                convert_to: None,
                reason: "位图字幕客户端无法渲染，服务端烧录",
            }
        } else {
            SubtitlePlan {
                track_id,
                codec,
                language,
                mode: SubtitleMode::Drop,
                convert_to: None,
                reason: "位图字幕且无法烧录（不做 OCR）",
            }
        };
    }

    if !is_text(&codec) {
        // 没见过的字幕格式：按文本处理太冒险，按位图处理又没依据 —— 直接投原样，
        // 让客户端自己决定能不能解。这是最保守也最不容易出错的一条路。
        return SubtitlePlan {
            track_id,
            codec,
            language,
            mode: SubtitleMode::Direct,
            convert_to: None,
            reason: "未知字幕格式，原样投递",
        };
    }

    if client.supports(&codec) {
        return SubtitlePlan {
            track_id,
            codec,
            language,
            mode: SubtitleMode::Direct,
            convert_to: None,
            reason: "客户端支持该字幕格式",
        };
    }

    if let Some(target) = client.convert_target() {
        return SubtitlePlan {
            track_id,
            codec,
            language,
            mode: SubtitleMode::Convert,
            convert_to: Some(target),
            reason: "客户端不支持该格式，服务端转换为文本字幕",
        };
    }

    if server.can_burn() {
        return SubtitlePlan {
            track_id,
            codec,
            language,
            mode: SubtitleMode::BurnIn,
            convert_to: None,
            reason: "客户端不支持任何文本字幕，服务端烧录",
        };
    }

    SubtitlePlan {
        track_id,
        codec,
        language,
        mode: SubtitleMode::Drop,
        convert_to: None,
        reason: "客户端不支持且服务端无法烧录",
    }
}

/// 一批字幕轨里是否需要烧录。决策引擎用它把 Remux 升级成 Transcode。
pub fn requires_burn(plans: &[SubtitlePlan]) -> bool {
    plans.iter().any(|p| p.mode.requires_transcode())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn web_client() -> SubtitleTarget {
        // 浏览器：只能渲染 VTT（通过 <track>），不认 ASS，不认位图。
        SubtitleTarget {
            supported: vec![SubtitleCodec::Vtt],
            supports_bitmap: false,
        }
    }

    fn mpv_client() -> SubtitleTarget {
        SubtitleTarget {
            supported: vec![SubtitleCodec::Srt, SubtitleCodec::Ass, SubtitleCodec::Vtt],
            supports_bitmap: true,
        }
    }

    fn server_ok() -> BurnCapability {
        BurnCapability {
            ffmpeg_available: true,
            has_budget: true,
        }
    }

    fn server_busy() -> BurnCapability {
        BurnCapability {
            ffmpeg_available: true,
            has_budget: false,
        }
    }

    #[test]
    fn web_client_gets_ass_converted_to_vtt() {
        let p = plan_track(
            "internal:2",
            SubtitleCodec::Ass,
            Some("zh".into()),
            &web_client(),
            &server_ok(),
        );
        assert_eq!(p.mode, SubtitleMode::Convert);
        assert_eq!(p.convert_to, Some(SubtitleCodec::Vtt));
        assert!(!p.mode.requires_transcode());
    }

    #[test]
    fn mpv_client_plays_ass_directly() {
        let p = plan_track(
            "internal:2",
            SubtitleCodec::Ass,
            None,
            &mpv_client(),
            &server_ok(),
        );
        assert_eq!(p.mode, SubtitleMode::Direct);
    }

    #[test]
    fn pgs_is_burned_not_ocrd() {
        // 这是文档 §51 的硬约束：位图字幕只烧不 OCR。
        let p = plan_track(
            "internal:5",
            SubtitleCodec::Pgs,
            None,
            &web_client(),
            &server_ok(),
        );
        assert_eq!(p.mode, SubtitleMode::BurnIn);
        assert!(p.mode.requires_transcode());
    }

    #[test]
    fn pgs_is_direct_on_clients_that_render_bitmaps() {
        // 桌面播放器自己能渲染 PGS，不该被逼着重编码。
        let p = plan_track(
            "internal:5",
            SubtitleCodec::Pgs,
            None,
            &mpv_client(),
            &server_ok(),
        );
        assert_eq!(p.mode, SubtitleMode::Direct);
        assert!(!p.mode.requires_transcode());
    }

    #[test]
    fn pgs_without_burn_budget_is_dropped() {
        let p = plan_track(
            "internal:5",
            SubtitleCodec::Pgs,
            None,
            &web_client(),
            &server_busy(),
        );
        assert_eq!(p.mode, SubtitleMode::Drop);
        // 关键：字幕降级失败不能把播放判成不支持。
        assert!(!p.mode.requires_transcode());
    }

    #[test]
    fn client_with_no_text_support_falls_back_to_burn() {
        let dumb = SubtitleTarget::default();
        let p = plan_track(
            "external:/a.srt",
            SubtitleCodec::Srt,
            None,
            &dumb,
            &server_ok(),
        );
        assert_eq!(p.mode, SubtitleMode::BurnIn);
    }

    #[test]
    fn convert_target_prefers_vtt_then_srt() {
        assert_eq!(web_client().convert_target(), Some(SubtitleCodec::Vtt));
        let srt_only = SubtitleTarget {
            supported: vec![SubtitleCodec::Srt],
            supports_bitmap: false,
        };
        assert_eq!(srt_only.convert_target(), Some(SubtitleCodec::Srt));
        assert_eq!(SubtitleTarget::default().convert_target(), None);
    }

    #[test]
    fn bitmap_support_is_separate_flag() {
        let mut c = web_client();
        assert!(!c.supports(&SubtitleCodec::Pgs));
        c.supports_bitmap = true;
        assert!(c.supports(&SubtitleCodec::Pgs));
    }

    #[test]
    fn requires_burn_scans_all_plans() {
        let plans = vec![
            plan_track("a", SubtitleCodec::Srt, None, &web_client(), &server_ok()),
            plan_track("b", SubtitleCodec::Pgs, None, &web_client(), &server_ok()),
        ];
        assert!(requires_burn(&plans));
        let plans = vec![plan_track(
            "a",
            SubtitleCodec::Ass,
            None,
            &web_client(),
            &server_ok(),
        )];
        assert!(!requires_burn(&plans));
    }
}
