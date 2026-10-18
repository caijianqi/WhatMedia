//! 角色、能力、内容分级。
//!
//! 这一层是**纯函数**：输入 principal，输出"能 / 不能"。不带数据库、不带时钟。
//! 理由很直接 —— 鉴权判断是全系统里最不适合"I/O 顺便失败"的地方，
//! 它必须能被上百条单测覆盖到每一个格子，而带 I/O 就做不到这一点。
//!
//! ## 三个角色
//!
//! | 角色    | 定位                          |
//! | ----- | --------------------------- |
//! | admin | 服务器所有者。能建用户、扫盘、改设置、批准儿童请求 |
//! | user  | 家庭成员成人。能看片、改自己的资料，不能动服务器      |
//! | child | 儿童。看的片要过分级闸门，新设备要家长批准         |
//!
//! **刻意没有 "管理员比 child 多一层" 的继承链。** flat 的三个角色配一张能力表，
//! 读代码的人在一个屏幕里看得全；继承链省下的行数远不如它制造的疑惑值钱。

use serde::{Deserialize, Serialize};
use whatmedia_core::Id;

#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// 服务器所有者。
    Admin,
    /// 成人家庭成员。
    User,
    /// 儿童账户。
    Child,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
            Role::Child => "child",
        }
    }

    /// 解析失败返回 `None`，不给默认值。
    ///
    /// 数据库里出现认不出的 role 时，正确做法是**拒绝登录并让运维看见**，
    /// 而不是悄悄当普通用户放行 —— 后者等于给升级／迁移留后门。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "admin" | "owner" => Some(Role::Admin),
            "user" | "member" => Some(Role::User),
            "child" | "kid" => Some(Role::Child),
            _ => None,
        }
    }
}

/// 能力点。新增能力先加在这里，再去 `GRANTS` 表里逐个角色表态。
///
/// 用 enum 而不是字符串权限名：字符串权限名在编译期不检查拼写，
/// 一个 typo 就会让某扇门永远打不开，而且没人会发现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ability {
    /// 浏览媒体。
    BrowseLibrary,
    /// 播放。
    Play,
    /// 扫描磁盘、增删媒体库。
    ManageLibraries,
    /// 刮削元数据、改锁定的字段。
    ManageMetadata,
    /// 管理直播源与频道。
    ManageLive,
    /// 建／删用户、改别人角色。
    ManageUsers,
    /// 审批儿童请求（设备登录、超分级播放）。
    ApproveRequests,
    /// 查看与修改服务器设置。
    ManageSettings,
    /// 查看审计日志。
    ViewAuditLog,
}

/// 角色 → 能力。这张表就是全部权限。
fn grants(role: Role, ability: Ability) -> bool {
    match (role, ability) {
        // admin 拥有全部 —— 包括 ViewAuditLog：能给人授权的人，必须能看到授权记录。
        (Role::Admin, _) => true,
        (Role::User, Ability::BrowseLibrary) => true,
        (Role::User, Ability::Play) => true,
        (Role::User, Ability::ManageMetadata) => true,
        (Role::User, Ability::ManageLive) => true,
        // child 只有看，且看的边界在 `allows_rating` 那道闸。
        (Role::Child, Ability::BrowseLibrary) => true,
        (Role::Child, Ability::Play) => true,
        _ => false,
    }
}

/// 一次请求的身份。
///
/// `Principal` 只能是**服务端中间件构造出来的**。handler 拿到的永远是一个
/// 已经解析好、已经过期检查过的结构体，不存在"忘了检查"的可能 ——
/// 这是把鉴权做进类型的好处。
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Debug, Clone, Serialize)]
pub struct Principal {
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub user_id: Id,
    pub username: String,
    pub display_name: Option<String>,
    pub role: Role,
    /// 发起请求的设备。浏览器首次请求时可能还没有，逻辑 device-less 允许。
    #[cfg_attr(feature = "openapi", schema(value_type = Option<uuid::Uuid>))]
    pub device_id: Option<Id>,
    #[cfg_attr(feature = "openapi", schema(value_type = uuid::Uuid))]
    pub session_id: Id,
}

impl Principal {
    pub fn can(&self, ability: Ability) -> bool {
        grants(self.role, ability)
    }

    /// 要求具备某项能力；不具备时返回统一错误。
    pub fn require(&self, ability: Ability) -> Result<(), Denied> {
        if self.can(ability) {
            Ok(())
        } else {
            Err(Denied {
                ability,
                role: self.role,
            })
        }
    }

    pub fn is_child(&self) -> bool {
        self.role == Role::Child
    }
}

/// 被拒绝时的结构化结果。带上 `role` 是为了让错误信息能说清
/// "为什么不行"，而不是一句干巴巴的 403。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    pub ability: Ability,
    pub role: Role,
}

// ───────────────── 内容分级 ─────────────────
//
// 分级闸门只做一件事：把"这个内容可能不适合 X 岁以下"翻译成一个可比较的序号。
// 元数据来自 TMDB，分级字符串是各国的本地叫法，所以这里做的是**归一**。

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 全年龄。
    G = 0,
    /// 建议家长指导。
    Pg = 1,
    /// 13+ / TV-14。
    Pg13 = 2,
    /// 17+。
    R = 3,
    /// 成人。
    Nc17 = 4,
}

/// 把各国分级字符串归一化到一个可排序的等级。
///
/// 认不出来返回 `None`，**调用方必须决定**这时是放行还是拦截 ——
/// 如果在这里默认把"认不出"当成最严等级，绝大多数自制与非英语内容会被一刀切，
/// 结果就是家长控制这个功能在上线的第一天被人关掉。
pub fn level_of(label: &str) -> Option<Level> {
    let s = label.trim().to_ascii_uppercase().replace('-', "");
    match s.as_str() {
        // 电影（US MPAA）
        "G" | "TVG" | "TVY" | "TVY7" | "ALL" | "U" => Some(Level::G),
        "PG" | "TVPG" | "PG12" | "12A" => Some(Level::Pg),
        "PG13" | "13" | "TV14" | "12" | "M" => Some(Level::Pg13),
        "R" | "NC17" | "NC 17" | "18" | "TVM" | "TVMA" | "MA" | "R18" | "AO" => Some(Level::R),
        _ => None,
    }
}

/// 分级闸门。
///
/// `ceiling` 是账户的上限（`max_rating`），`certification` 是这将被播放内容的分级。
/// 返回 `true` 表示放行。
///
/// 三条规则：
///   1. 没有上限（成人账户）→ 一律放行
///   2. 内容没分级 → **放行**。理由见 [`level_of`] 的注释。
///   3. 上限字符串认不出来 → **拦截**：家长写了 `max_rating = "12岁"`
///      这种我们没有映射的值，宁可先拦下让他来修正，也好过默认敞开。
pub fn allows_rating(ceiling: Option<&str>, certification: Option<&str>) -> bool {
    let Some(ceiling) = ceiling else {
        return true; // 规则 1
    };
    let Some(ceiling) = level_of(ceiling) else {
        return false; // 规则 3
    };
    let Some(cert) = certification.and_then(level_of) else {
        return true; // 规则 2
    };
    cert <= ceiling
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(role: Role) -> Principal {
        Principal {
            user_id: Id::new_v4(),
            username: "u".into(),
            display_name: None,
            role,
            device_id: None,
            session_id: Id::new_v4(),
        }
    }

    #[test]
    fn role_roundtrip() {
        for r in [Role::Admin, Role::User, Role::Child] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("ADMIN"), Some(Role::Admin));
        assert_eq!(Role::parse(" owner "), Some(Role::Admin));
        assert_eq!(Role::parse("nope"), None);
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn admin_grants_everything() {
        let a = p(Role::Admin);
        let all = [
            Ability::BrowseLibrary,
            Ability::Play,
            Ability::ManageLibraries,
            Ability::ManageMetadata,
            Ability::ManageLive,
            Ability::ManageUsers,
            Ability::ApproveRequests,
            Ability::ManageSettings,
            Ability::ViewAuditLog,
        ];
        for ab in all {
            assert!(a.can(ab), "admin 应当拥有 {ab:?}");
        }
    }

    #[test]
    fn member_cannot_touch_server() {
        let u = p(Role::User);
        assert!(u.can(Ability::BrowseLibrary));
        assert!(u.can(Ability::Play));
        assert!(!u.can(Ability::ManageUsers));
        assert!(!u.can(Ability::ManageSettings));
        assert!(!u.can(Ability::ManageLibraries));
        assert!(!u.can(Ability::ApproveRequests));
        assert!(!u.can(Ability::ViewAuditLog));
    }

    #[test]
    fn child_is_read_only() {
        let c = p(Role::Child);
        assert!(c.can(Ability::Play));
        assert!(c.can(Ability::BrowseLibrary));
        assert!(!c.can(Ability::ManageMetadata));
        assert!(!c.can(Ability::ManageLive));
        assert!(c.is_child());
        assert!(!p(Role::User).is_child());
    }

    #[test]
    fn denial_carries_context() {
        let d = p(Role::Child).require(Ability::ManageUsers).unwrap_err();
        assert_eq!(d.ability, Ability::ManageUsers);
        assert_eq!(d.role, Role::Child);
        assert!(p(Role::Admin).require(Ability::ManageUsers).is_ok());
    }

    #[test]
    fn level_ordering() {
        assert!(Level::G < Level::Pg);
        assert!(Level::Pg < Level::Pg13);
        assert!(Level::Pg13 < Level::R);
        assert!(Level::R < Level::Nc17);
    }

    #[test]
    fn rating_normalisation() {
        assert_eq!(level_of("PG-13"), Some(Level::Pg13));
        assert_eq!(
            level_of("PG_13".replace('_', "-").as_str()),
            Some(Level::Pg13)
        );
        assert_eq!(level_of("TV-MA"), Some(Level::R));
        assert_eq!(level_of("tv-ma"), Some(Level::R));
        assert_eq!(level_of("g"), Some(Level::G));
        assert_eq!(level_of("NC-17"), Some(Level::R));
        assert_eq!(level_of("什么鬼"), None);
    }

    #[test]
    fn no_ceiling_means_no_gate() {
        assert!(allows_rating(None, Some("R")));
        assert!(allows_rating(None, Some("NC-17")));
    }

    #[test]
    fn unrated_content_passes() {
        // 自制影片、非英语剧情库很多时候没有 certification，不该把儿童墙做成石头墙。
        assert!(allows_rating(Some("PG"), None));
        assert!(allows_rating(Some("PG"), Some("")));
    }

    #[test]
    fn gate_blocks_over_ceiling() {
        assert!(allows_rating(Some("PG"), Some("G")));
        assert!(allows_rating(Some("PG"), Some("PG")));
        assert!(!allows_rating(Some("PG"), Some("PG-13")));
        assert!(!allows_rating(Some("PG-13"), Some("R")));
        assert!(allows_rating(Some("NC-17"), Some("R")));
    }

    #[test]
    fn unknown_ceiling_fails_closed() {
        // 家长写了我们不认识的等级串，宁可拦下让他来改，也不能默认敞开。
        assert!(!allows_rating(Some("十二岁"), Some("R")));
    }
}
