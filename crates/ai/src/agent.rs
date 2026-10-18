//! 五个 Agent（需求稿 §60）。
//!
//! Agent 在这里**不是**独立的执行体，而是一份"人格 + 工具白名单"：
//! 换 Agent = 换系统提示词 + 换可见工具集。执行仍然走同一条 Tool 通路。
//!
//! 这样做的理由：如果每个 Agent 有自己的执行路径，那么"危险操作必须确认"
//! 就得在五处各实现一遍，漏一处就是绕过。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentId {
    /// 找片、问答、解释播放决策。
    Media,
    /// 扫描、识别、海报、重复、元数据。
    Library,
    /// 字幕缺失、字幕轨、外挂字幕。
    Subtitle,
    /// 体检、清理、空间占用、任务。
    Maintenance,
    /// 推荐片单。
    Recommendation,
}

impl AgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentId::Media => "media",
            AgentId::Library => "library",
            AgentId::Subtitle => "subtitle",
            AgentId::Maintenance => "maintenance",
            AgentId::Recommendation => "recommendation",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AgentId::Media => "媒体助手",
            AgentId::Library => "媒体库管家",
            AgentId::Subtitle => "字幕助手",
            AgentId::Maintenance => "运维助手",
            AgentId::Recommendation => "推荐助手",
        }
    }

    /// 允许使用的工具。**写操作只对 Library 与 Maintenance 开放。**
    pub fn tools(self) -> &'static [&'static str] {
        match self {
            AgentId::Media => &[
                "search_media",
                "get_media",
                "get_playback_capability",
                "recommend_media",
            ],
            AgentId::Library => &[
                "search_media",
                "get_media",
                "find_missing_poster",
                "find_duplicate",
                "scan_library",
                "identify_media",
            ],
            // 首批 8 个工具里没有专门的字幕工具；
            // 缺字幕这类问题用 search_media 的 missing=subtitle 解决。
            AgentId::Subtitle => &["search_media", "get_media", "get_playback_capability"],
            AgentId::Maintenance => &[
                "search_media",
                "find_duplicate",
                "find_missing_poster",
                "scan_library",
            ],
            AgentId::Recommendation => &["search_media", "get_media", "recommend_media"],
        }
    }

    pub fn system_prompt(self) -> &'static str {
        match self {
            AgentId::Media => {
                "你是 WhatMedia 的媒体助手。用工具查真实数据，不要凭印象回答。\
                               查不到就直说查不到。涉及播放能力时说清原因。"
            }
            AgentId::Library => {
                "你是 WhatMedia 的媒体库管家。负责扫描、识别、海报与重复检查。\
                                 写操作（扫描、重新识别）必须先给出预览并等待用户确认，\
                                 绝不能自己执行。"
            }
            AgentId::Subtitle => {
                "你是 WhatMedia 的字幕助手。判断字幕缺失或字幕轨问题时先查真实数据。\
                                 位图字幕（PGS）客户端普遍不支持，需要服务端烧录，\
                                 不要建议用户在客户端打开它。"
            }
            AgentId::Maintenance => {
                "你是 WhatMedia 的运维助手。关注空间占用、重复文件、扫描任务。\
                                    任何清理或删除建议都要先列出具体条目并等待确认。"
            }
            AgentId::Recommendation => {
                "你是 WhatMedia 的推荐助手。基于库里真实存在的条目推荐，\
                                       不要推荐库里没有的片子。给出口味相近的理由。"
            }
        }
    }
}

/// 按问题内容挑一个 Agent。
///
/// 只做粗路由：选错了不会出错（工具白名单仍是硬约束），
/// 只是提示词没那么贴。
pub fn route(text: &str) -> AgentId {
    let t = text.to_lowercase();
    if t.contains("字幕") || t.contains("subtitle") || t.contains("srt") || t.contains("ass") {
        return AgentId::Subtitle;
    }
    if t.contains("推荐") || t.contains("看点什么") || t.contains("好看") || t.contains("片单")
    {
        return AgentId::Recommendation;
    }
    if t.contains("清理")
        || t.contains("空间")
        || t.contains("占用")
        || t.contains("体检")
        || t.contains("修复")
        || t.contains("重建")
    {
        return AgentId::Maintenance;
    }
    if t.contains("扫描")
        || t.contains("识别")
        || t.contains("海报")
        || t.contains("重复")
        || t.contains("元数据")
        || t.contains("刮削")
    {
        return AgentId::Library;
    }
    AgentId::Media
}

/// 系统提示词：Agent 人设 + 全局安全约束。
///
/// 安全约束写在提示词里**只是提醒，不是边界**。真正的边界是
/// `AiRuntime` 里的令牌校验与工具白名单（见 ADR-0006 §3）。
pub fn compose_system_prompt(id: AgentId) -> String {
    format!(
        "{}\n\n约束：\n\
         1. 只使用允许的工具，不要假设自己能直接访问数据库或文件系统。\n\
         2. 找不到就说找不到，不要编造片名、id 或数据。\n\
         3. 写操作（扫描、重新识别）会返回一个待确认动作，交给用户决定，不要重复发起。\n\
         4. 用中文回答，简短。",
        id.system_prompt()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_agents_are_declared() {
        let all = [
            AgentId::Media,
            AgentId::Library,
            AgentId::Subtitle,
            AgentId::Maintenance,
            AgentId::Recommendation,
        ];
        assert_eq!(all.len(), 5);
        for a in all {
            assert!(!a.tools().is_empty(), "{:?} 没有工具", a);
            assert!(!a.system_prompt().is_empty());
        }
    }

    #[test]
    fn write_tools_are_only_on_library_and_maintenance() {
        for a in [AgentId::Media, AgentId::Subtitle, AgentId::Recommendation] {
            assert!(!a.tools().contains(&"scan_library"), "{:?}", a);
            assert!(!a.tools().contains(&"identify_media"), "{:?}", a);
        }
        assert!(AgentId::Library.tools().contains(&"scan_library"));
        assert!(AgentId::Maintenance.tools().contains(&"scan_library"));
    }

    #[test]
    fn routing() {
        assert_eq!(route("哪些电视剧没有字幕？"), AgentId::Subtitle);
        assert_eq!(route("推荐两小时以内的电影"), AgentId::Recommendation);
        assert_eq!(route("哪些视频最占空间？"), AgentId::Maintenance);
        assert_eq!(route("扫描一下媒体库"), AgentId::Library);
        assert_eq!(route("找一下星际穿越"), AgentId::Media);
    }

    #[test]
    fn system_prompt_carries_the_safety_reminder() {
        let p = compose_system_prompt(AgentId::Library);
        assert!(p.contains("待确认"));
        assert!(p.contains("不要编造"));
    }
}
