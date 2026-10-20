//! 配置加载：TOML 文件 + 环境变量覆盖。
//!
//! 默认目录布局（容器内）：
//!   /config  配置与数据库
//!   /data    运行时数据
//!   /cache   元数据 / 图片 / 转码缓存
//!   /logs    日志

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub paths: PathsConfig,
    pub database: DatabaseConfig,
    pub logging: LoggingConfig,
    pub metadata: MetadataConfig,
    pub images: ImagesConfig,
    pub transcode: TranscodeConfig,
    pub security: SecurityConfig,
    pub ai: AiConfig,
}

impl AppConfig {
    /// 图片缓存目录。未显式配置时落在 `<cache_dir>/images`。
    ///
    /// 做成方法而不是字段，是因为它依赖另一个字段的值 ——
    /// 若存成字段，改了 `cache_dir` 之后这里就过期了。
    pub fn images_dir(&self) -> PathBuf {
        self.images
            .dir
            .clone()
            .unwrap_or_else(|| self.paths.cache_dir.join("images"))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsConfig {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub log_dir: PathBuf,
    /// SQL 迁移文件目录。
    ///
    /// 容器里是 `/usr/local/share/whatmedia/migrations`，进程 CWD 却是 `/config`，
    /// 所以必须可配置 —— 这也是 `Db::migrate` 用运行时路径而非编译期常量的原因。
    pub migrations_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// "sqlite" | "postgres"。v1.0 之前只用 sqlite，postgres 为预留分支。
    pub kind: String,
    pub url: String,
    pub max_connections: u32,
}

/// 元数据刮削（P2）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataConfig {
    /// TMDB v3 API Key。
    pub tmdb_api_key: Option<String>,
    /// TMDB v4 Bearer Token。两个都配时用它。
    pub tmdb_bearer: Option<String>,
    /// TMDB API 自定义代理/镜像 Base URL（留空使用官方默认 https://api.themoviedb.org/3）。
    #[serde(default)]
    pub tmdb_base_url: Option<String>,
    /// 启用 Bangumi（动漫）。中文日番在 Bangumi 上命中率更高。
    pub bangumi_enabled: bool,
    pub language: String,
    /// 每秒请求上限。默认 4 —— 保守一点换不被封。
    pub requests_per_second: f64,
    /// 关掉后所有结果都进人工确认队列。新库第一次刮建议关掉。
    pub auto_accept: bool,
    pub concurrency: usize,
    pub fetch_images: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagesConfig {
    /// 缓存根目录。默认 `<cache_dir>/images`。
    pub dir: Option<PathBuf>,
    /// 字节上限。超出后按 LRU 清理。
    pub max_bytes: u64,
    pub max_entries: u64,
    /// 多少天没访问就清掉。
    pub max_age_days: i64,
}

/// 转码（P3）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodeConfig {
    /// ffmpeg 可执行文件路径。None 时用 PATH 里的 `ffmpeg`，
    /// 环境变量 `WHATMEDIA_FFMPEG_PATH` 可以覆盖（ffmpeg-rockchip 就靠它切换）。
    pub ffmpeg_path: Option<String>,
    /// HLS 分片时长（秒）。太短会让分片数量爆炸，太长会让 Seek 变慢。
    pub hls_segment_seconds: u32,
    /// 并发上限。0 表示由硬件能力自动决定。
    pub max_concurrent: u32,
    /// 流式输出多久没被访问就算僵尸（秒）。
    pub stream_idle_timeout_secs: i64,
}

/// 鉴权与会话（多用户）。
///
/// **没有 `enabled` 字段是有意为之。** 只要 HTTP 端口在监听，鉴权就必须开着；
/// 一旦提供开关，现实里它一定会被人在某个深夜关掉然后忘掉 ——
/// 而媒体库里存的是整个家盘的路径、封面、观看记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// 签名 URL 的根密钥（base64url，32 字节）。
    ///
    /// 留空时每次启动随机生成 —— 好处是重启即失效，代价是**多实例部署时
    /// 另一个实例签的 URL 会被判无效**，那种部署必须显式配置同一个值。
    pub signing_key: Option<String>,
    /// 浏览器会话有效期（小时）。短一点是为了让"借用一台公用电脑"有上限。
    pub web_session_hours: u64,
    /// 客户端（桌面 / 移动 / 电视）会话有效期（天）。长一点是因为
    /// 没人愿意每周在电视上重新输一次密码。
    pub device_session_days: u64,
    /// 签名 URL 有效期（小时）。它会出现在日志和 Referer 里，不能太长。
    pub signed_url_hours: u64,
    /// 连续登录失败达到这个次数后锁定账户。
    pub lockout_threshold: i32,
    /// 锁定时长（分钟）。
    pub lockout_minutes: i64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            signing_key: None,
            web_session_hours: 12,
            device_session_days: 90,
            signed_url_hours: 4,
            lockout_threshold: 5,
            lockout_minutes: 15,
        }
    }
}

/// AI 媒体管家（P7）。
///
/// 两个默认值是刻意保守的：
///   · `enabled = false` —— 没配模型时 AI 端点返回 503，而不是"能聊但什么都干不了"
///   · `allow_dangerous = false` —— 危险工具默认连预览都不给，必须先显式打开
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub enabled: bool,
    /// `openai` / `deepseek` / `qwen` / `glm` / `ollama` / `vllm` / `llama_cpp` / `custom`。
    /// 前七种自带默认 base_url，`custom` 必须自己给 `base_url`。
    pub provider: String,
    /// 留空则用 provider 的默认值。Ollama 通常是 `http://localhost:11434`。
    pub base_url: Option<String>,
    pub model: String,
    /// **存环境变量名，不存密钥本身** —— TOML 常被误提交，环境变量不会。
    pub api_key_env: String,
    /// 直接配置的 API Key（支持在 Web 界面直接配置，优先级高于 api_key_env）。
    #[serde(default)]
    pub api_key: Option<String>,
    /// 单次模型请求超时（秒）。家庭网络里模型可能跑在本地小主机上，别设太短。
    pub timeout_secs: u64,
    /// 一次对话里最多允许几轮工具调用。这是防失控的硬闸：
    /// 模型陷入"调工具→不满意→再调"的循环时必须有上限。
    pub max_tool_rounds: usize,
    /// 是否允许危险工具（扫描 / 重新识别 / 改名 / 移动 / 删除）。
    /// 关掉时它们连 `tools/list` 都不出现 —— 不是调用时才拒绝。
    pub allow_dangerous: bool,
    /// 模型不可用（未配置 / 断网 / 超时）时，是否降级到内置规则意图解析。
    /// 关掉则直接返回 `AI_UNAVAILABLE`。
    pub fallback_to_rules: bool,
    /// 对外暴露 MCP Server。只有只读工具会出去（见 ADR-0006）。
    pub mcp_enabled: bool,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: "openai".into(),
            base_url: None,
            model: "gpt-4o-mini".into(),
            api_key_env: "WHATMEDIA_AI_API_KEY".into(),
            api_key: None,
            timeout_secs: 30,
            max_tool_rounds: 6,
            allow_dangerous: false,
            fallback_to_rules: true,
            mcp_enabled: false,
        }
    }
}

impl AiConfig {
    /// 读 API Key。优先读直接配置的 api_key，其次读环境变量。
    /// 环境变量缺失视为"未配置"，不报错 —— Ollama / llama.cpp 本来就不需要 Key。
    pub fn api_key(&self) -> Option<String> {
        if let Some(key) = &self.api_key {
            let k = key.trim();
            if !k.is_empty() {
                return Some(k.to_string());
            }
        }
        std::env::var(&self.api_key_env)
            .ok()
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    /// "pretty" 本地开发，"json" 容器与生产。
    pub format: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: "0.0.0.0".into(),
                port: 8080,
            },
            paths: PathsConfig {
                config_dir: PathBuf::from("/config"),
                data_dir: PathBuf::from("/data"),
                cache_dir: PathBuf::from("/cache"),
                log_dir: PathBuf::from("/logs"),
                // 开发默认指向仓库里的 migrations；容器里由 WHATMEDIA_MIGRATIONS_DIR 覆盖。
                migrations_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join("migrations"),
            },
            database: DatabaseConfig {
                kind: "sqlite".into(),
                url: "sqlite:/config/whatmedia.db?mode=rwc".into(),
                max_connections: 8,
            },
            logging: LoggingConfig {
                level: "info".into(),
                format: "pretty".into(),
            },
            metadata: MetadataConfig {
                tmdb_api_key: None,
                tmdb_bearer: None,
                tmdb_base_url: None,
                bangumi_enabled: false,
                language: "zh-CN".into(),
                requests_per_second: 4.0,
                auto_accept: true,
                concurrency: 4,
                fetch_images: true,
            },
            images: ImagesConfig {
                dir: None,
                max_bytes: 1024 * 1024 * 1024,
                max_entries: 50_000,
                max_age_days: 90,
            },
            transcode: TranscodeConfig {
                ffmpeg_path: None,
                hls_segment_seconds: 6,
                max_concurrent: 0,
                stream_idle_timeout_secs: 300,
            },
            security: SecurityConfig::default(),
            ai: AiConfig::default(),
        }
    }
}

impl AppConfig {
    /// 从 TOML 加载，随后用 `WHATMEDIA_*` 环境变量覆盖。
    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let mut cfg = match path {
            Some(p) if p.exists() => {
                let raw = std::fs::read_to_string(&p)?;
                toml::from_str(&raw)?
            }
            _ => Self::default(),
        };
        cfg.apply_env();
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("WHATMEDIA_HOST") {
            self.server.host = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_PORT") {
            if let Ok(p) = v.parse() {
                self.server.port = p;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_CONFIG_DIR") {
            self.paths.config_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_DATA_DIR") {
            self.paths.data_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_CACHE_DIR") {
            self.paths.cache_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_LOG_DIR") {
            self.paths.log_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_MIGRATIONS_DIR") {
            self.paths.migrations_dir = PathBuf::from(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_DATABASE_URL") {
            if v.starts_with("postgres") {
                self.database.kind = "postgres".into();
            }
            self.database.url = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_LOG_LEVEL") {
            self.logging.level = v;
        }
        // ── 元数据 ────────────────────────────────────────────
        // API Key 走环境变量而不是写进 TOML：TOML 常被误提交到仓库。
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_TMDB_API_KEY") {
            self.metadata.tmdb_api_key = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_TMDB_BEARER") {
            self.metadata.tmdb_bearer = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_TMDB_BASE_URL") {
            self.metadata.tmdb_base_url = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_BANGUMI") {
            self.metadata.bangumi_enabled =
                matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_LANGUAGE") {
            self.metadata.language = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_RPS") {
            if let Ok(n) = v.parse() {
                self.metadata.requests_per_second = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_METADATA_AUTO_ACCEPT") {
            self.metadata.auto_accept =
                matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(v) = std::env::var("WHATMEDIA_IMAGES_DIR") {
            self.images.dir = Some(PathBuf::from(v));
        }
        if let Ok(v) = std::env::var("WHATMEDIA_IMAGES_MAX_MB") {
            if let Ok(n) = v.parse::<u64>() {
                self.images.max_bytes = n * 1024 * 1024;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_LOG_FORMAT") {
            self.logging.format = v;
        }
        // ── 转码 ────────────────────────────────────────────
        if let Ok(v) = std::env::var("WHATMEDIA_FFMPEG_PATH") {
            self.transcode.ffmpeg_path = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_HLS_SEGMENT_SECONDS") {
            if let Ok(n) = v.parse() {
                self.transcode.hls_segment_seconds = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_TRANSCODE_MAX_CONCURRENT") {
            if let Ok(n) = v.parse() {
                self.transcode.max_concurrent = n;
            }
        }
        // ── 鉴权（security）────────────────────────────────────
        if let Ok(v) = std::env::var("WHATMEDIA_SIGNING_KEY") {
            self.security.signing_key = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_WEB_SESSION_HOURS") {
            if let Ok(n) = v.parse() {
                self.security.web_session_hours = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_DEVICE_SESSION_DAYS") {
            if let Ok(n) = v.parse() {
                self.security.device_session_days = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_SIGNED_URL_HOURS") {
            if let Ok(n) = v.parse() {
                self.security.signed_url_hours = n;
            }
        }
        // ── AI ────────────────────────────────────────────
        // AI 的开关默认关，所以这里的环境变量是**唯一**的打开方式之一；
        // 允许 `1/true/yes/on`。
        if let Ok(v) = std::env::var("WHATMEDIA_AI_ENABLED") {
            self.ai.enabled = matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_PROVIDER") {
            self.ai.provider = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_BASE_URL") {
            self.ai.base_url = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_MODEL") {
            self.ai.model = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_API_KEY_ENV") {
            self.ai.api_key_env = v;
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_API_KEY") {
            self.ai.api_key = Some(v);
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_TIMEOUT_SECS") {
            if let Ok(n) = v.parse() {
                self.ai.timeout_secs = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_MAX_TOOL_ROUNDS") {
            if let Ok(n) = v.parse() {
                self.ai.max_tool_rounds = n;
            }
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_ALLOW_DANGEROUS") {
            self.ai.allow_dangerous =
                matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
        if let Ok(v) = std::env::var("WHATMEDIA_AI_MCP") {
            self.ai.mcp_enabled = matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on");
        }
    }

    /// 创建所有必需目录。启动阶段调用，失败即退出。
    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        for dir in [
            &self.paths.config_dir,
            &self.paths.data_dir,
            &self.paths.cache_dir,
            &self.paths.log_dir,
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.server.host, self.server.port)
    }
}

// ─────────────────────────────────────────────────────────────
// 可写设置
// ─────────────────────────────────────────────────────────────
//
// 配置原本只读：改它要编辑 TOML 或改环境变量，两者都要重启容器。
// 界面上摆一排开关却改不动，比不给开关更糟 —— 所以得开出一条**受控的**写路径。
//
// 受控的意思是**白名单**：HTTP 层只能改下面这张表里列出来的键，
// 每个键都自带类型与取值范围。理由很直接：谁能 PATCH `paths.data_dir`，
// 谁就能把服务指向一个不存在的目录，而那时界面已经打不开了、无从改回。
// 路径、端口、数据库 URL、密钥一类**一个都不在表里**，这是刻意的。

/// 设置的作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingScope {
    /// 全服生效，只有管理员能改。
    Server,
    /// 每人一份（播放偏好），本人就能改。
    User,
}

/// 设置值的类型。决定了界面渲染成开关、数字框还是下拉框。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingKind {
    Bool,
    Int,
    Float,
    Text,
    Enum,
}

/// 一个可写设置的元数据。静态表，编译期定死。
pub struct SettingSpec {
    pub key: &'static str,
    pub label: &'static str,
    /// 给界面的一句人话说明。写给"第一次看到这个开关的人"。
    pub hint: &'static str,
    pub scope: SettingScope,
    pub kind: SettingKind,
    /// `Enum` 的可选值；其余类型为空。
    pub choices: &'static [&'static str],
    /// 数值下限 / 上限。`min` 对 Bool/Text 无意义。
    pub min: Option<f64>,
    pub max: Option<f64>,
}

/// 全服设置固定写在这个 scope_id 下。
pub const SERVER_SCOPE_ID: &str = "default";

/// 白名单。**加一行就等于开放一个可以被 HTTP 改动的字段**，请逐条想清楚。
pub const SETTING_SPECS: &[SettingSpec] = &[
    // ── 元数据刮削 ──────────────────────────────────────────
    SettingSpec {
        key: "metadata.language",
        label: "刮削语言",
        hint: "TMDB 返回结果的语种，如 zh-CN、en-US。改完要重新刮削才会体现在已有条目上。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(32.0),
    },
    SettingSpec {
        key: "metadata.auto_accept",
        label: "自动接受高置信度匹配",
        hint: "关掉之后所有候选都进人工确认队列。新库第一次刮建议关掉，看一遍命中质量再开。",
        scope: SettingScope::Server,
        kind: SettingKind::Bool,
        choices: &[],
        min: None,
        max: None,
    },
    SettingSpec {
        key: "metadata.bangumi_enabled",
        label: "启用 Bangumi",
        hint: "中文日番在 Bangumi 上的命中率明显高于 TMDB。",
        scope: SettingScope::Server,
        kind: SettingKind::Bool,
        choices: &[],
        min: None,
        max: None,
    },
    SettingSpec {
        key: "metadata.fetch_images",
        label: "抓取海报与背景图",
        hint: "关掉之后条目没有海报，界面上全是占位块。",
        scope: SettingScope::Server,
        kind: SettingKind::Bool,
        choices: &[],
        min: None,
        max: None,
    },
    SettingSpec {
        key: "metadata.requests_per_second",
        label: "Provider 请求速率",
        hint: "每秒请求上限。调高会更快，也会被封。",
        scope: SettingScope::Server,
        kind: SettingKind::Float,
        choices: &[],
        min: Some(0.1),
        max: Some(20.0),
    },
    SettingSpec {
        key: "metadata.tmdb_api_key",
        label: "TMDB API 密钥 (v3 Key)",
        hint: "The Movie Database (TMDB) v3 API Key。配置后解锁电影与剧集元数据刮削。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(128.0),
    },
    SettingSpec {
        key: "metadata.tmdb_bearer",
        label: "TMDB 访问令牌 (v4 Bearer)",
        hint: "TMDB v4 Bearer 读访问令牌。若同时配置 v3 Key 与 v4 Token，服务端优先采用 Bearer 认证。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(512.0),
    },
    SettingSpec {
        key: "metadata.tmdb_base_url",
        label: "TMDB 代理地址 (Base URL)",
        hint: "留空使用官方默认地址（https://api.themoviedb.org/3）。国内网络受限环境可配置反代镜像加速刮削。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(256.0),
    },
    // ── 图片缓存 ────────────────────────────────────────────
    SettingSpec {
        key: "images.max_bytes_mb",
        label: "图片缓存上限（MB）",
        hint: "超出后按最久未访问清理。",
        scope: SettingScope::Server,
        kind: SettingKind::Int,
        choices: &[],
        min: Some(64.0),
        max: Some(102_400.0),
    },
    SettingSpec {
        key: "images.max_age_days",
        label: "图片缓存保留天数",
        hint: "多少天没访问就清掉。",
        scope: SettingScope::Server,
        kind: SettingKind::Int,
        choices: &[],
        min: Some(1.0),
        max: Some(3650.0),
    },
    // ── 转码 ────────────────────────────────────────────────
    SettingSpec {
        key: "transcode.max_concurrent",
        label: "最大并发转码",
        hint: "0 表示交给硬件能力自动决定。小主机上填 1~2 会比自动更稳。",
        scope: SettingScope::Server,
        kind: SettingKind::Int,
        choices: &[],
        min: Some(0.0),
        max: Some(64.0),
    },
    SettingSpec {
        key: "transcode.hls_segment_seconds",
        label: "HLS 分片时长（秒）",
        hint: "太短会让分片数量爆炸，太长会让拖动变慢。",
        scope: SettingScope::Server,
        kind: SettingKind::Int,
        choices: &[],
        min: Some(2.0),
        max: Some(30.0),
    },
    SettingSpec {
        key: "transcode.ffmpeg_path",
        label: "ffmpeg 路径",
        hint: "留空则用 PATH 里的 ffmpeg。Rockchip 等硬件加速版本靠它切换。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(512.0),
    },
    // ── AI ──────────────────────────────────────────────────
    SettingSpec {
        key: "ai.enabled",
        label: "启用 AI 助手",
        hint: "开启后可在全站唤起智能媒体助手，支持自然语言检索影视、对话问答与库管理。",
        scope: SettingScope::Server,
        kind: SettingKind::Bool,
        choices: &[],
        min: None,
        max: None,
    },
    SettingSpec {
        key: "ai.provider",
        label: "AI 服务商 (Provider)",
        hint: "选择大模型提供商。内置 DeepSeek、OpenAI、阿里通义千问、智谱 GLM、本地 Ollama / vLLM / llama.cpp 或自定义网关。",
        scope: SettingScope::Server,
        kind: SettingKind::Enum,
        choices: &[
            "deepseek",
            "openai",
            "qwen",
            "glm",
            "minimax",
            "ollama",
            "vllm",
            "llama_cpp",
            "custom",
        ],
        min: None,
        max: None,
    },
    SettingSpec {
        key: "ai.base_url",
        label: "AI 接口接入点 (Base URL)",
        hint: "留空使用所选服务商官方地址（如 DeepSeek 为 https://api.deepseek.com/v1，Ollama 为 http://localhost:11434）。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(256.0),
    },
    SettingSpec {
        key: "ai.api_key",
        label: "AI 接口密钥 (API Key)",
        hint: "大模型调用 API 密钥。使用本地部署的 Ollama、vLLM 或 llama.cpp 时可留空。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(512.0),
    },
    SettingSpec {
        key: "ai.model",
        label: "AI 模型标识 (Model)",
        hint: "具体调用的模型名，如 deepseek-chat、deepseek-reasoner、gpt-4o-mini、qwen-plus 等。",
        scope: SettingScope::Server,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(128.0),
    },
    SettingSpec {
        key: "ai.fallback_to_rules",
        label: "模型不可用时降级为规则解析",
        hint: "关掉后若断网或模型不可用时 AI 助手直接报错，而不是通过本地规则猜测用户意图。",
        scope: SettingScope::Server,
        kind: SettingKind::Bool,
        choices: &[],
        min: None,
        max: None,
    },
    // ── 播放偏好（每人一份）──────────────────────────────────
    SettingSpec {
        key: "player.autoplay_next",
        label: "自动播放下一集",
        hint: "一集播完自动跳下一集。只看剧集生效。",
        scope: SettingScope::User,
        kind: SettingKind::Bool,
        choices: &[],
        min: None,
        max: None,
    },
    SettingSpec {
        key: "player.subtitle_lang",
        label: "偏好字幕语言",
        hint: "ISO 639-2 三字母码，如 zho / eng / jpn。留空表示跟随音轨语言。",
        scope: SettingScope::User,
        kind: SettingKind::Text,
        choices: &[],
        min: None,
        max: Some(16.0),
    },
    SettingSpec {
        key: "player.preferred_quality",
        label: "默认画质",
        hint: "direct 尽量直出（最省 CPU）；其余档位在需要时才转码。",
        scope: SettingScope::User,
        kind: SettingKind::Enum,
        choices: &["direct", "1080p", "720p", "480p"],
        min: None,
        max: None,
    },
    SettingSpec {
        key: "ui.library_view",
        label: "媒体库默认视图",
        hint: "海报墙 / 列表。",
        scope: SettingScope::User,
        kind: SettingKind::Enum,
        choices: &["grid", "list"],
        min: None,
        max: None,
    },
];

/// 按键查表。查不到就是不在白名单里，一律拒绝。
pub fn setting_spec(key: &str) -> Option<&'static SettingSpec> {
    SETTING_SPECS.iter().find(|s| s.key == key)
}

/// 校验一个待写入的值。返回 `Err` 时里面的话是直接给人看的。
pub fn validate_setting(spec: &SettingSpec, raw: &str) -> Result<(), String> {
    match spec.kind {
        SettingKind::Bool => {
            if parse_bool(raw).is_none() {
                return Err("需要 true 或 false".into());
            }
        }
        SettingKind::Int => {
            let v: i64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("需要整数，收到「{raw}」"))?;
            check_range(v as f64, spec)?;
        }
        SettingKind::Float => {
            let v: f64 = raw
                .trim()
                .parse()
                .map_err(|_| format!("需要数字，收到「{raw}」"))?;
            if !v.is_finite() {
                return Err("不是有限数字".into());
            }
            check_range(v, spec)?;
        }
        SettingKind::Text => {
            if let Some(max) = spec.max {
                if raw.chars().count() as f64 > max {
                    return Err(format!("最多 {} 个字符", max as i64));
                }
            }
            if raw.contains('\n') || raw.contains('\r') {
                return Err("不能包含换行".into());
            }
        }
        SettingKind::Enum => {
            // 枚举必须严格命中。`direct` 写成 `Direct` 就该报错而不是静默降级，
            // 否则用户以为自己选了，实际落到了默认值上。
            if !spec.choices.contains(&raw) {
                return Err(format!("只能是 {} 之一", spec.choices.join(" / ")));
            }
        }
    }
    Ok(())
}

fn check_range(v: f64, spec: &SettingSpec) -> Result<(), String> {
    if let Some(min) = spec.min {
        if v < min {
            return Err(format!("不能小于 {}", min));
        }
    }
    if let Some(max) = spec.max {
        if v > max {
            return Err(format!("不能大于 {}", max));
        }
    }
    Ok(())
}

/// 宽松解析布尔：配置里常见的 `1/0/yes/no/on/off` 都认。
///
/// 只在"读"的方向宽松 —— 存进库之前会被归一成 `true`/`false`，
/// 这样列表接口返回的值永远是同构的，前端不用写第二套解析。
pub fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl AppConfig {
    /// 读出一个设置的当前值（字符串形式）。不在白名单里返回 `None`。
    pub fn get_setting(&self, key: &str) -> Option<String> {
        match key {
            "metadata.language" => Some(self.metadata.language.clone()),
            "metadata.auto_accept" => Some(self.metadata.auto_accept.to_string()),
            "metadata.bangumi_enabled" => Some(self.metadata.bangumi_enabled.to_string()),
            "metadata.fetch_images" => Some(self.metadata.fetch_images.to_string()),
            "metadata.requests_per_second" => Some(self.metadata.requests_per_second.to_string()),
            "metadata.tmdb_api_key" => Some(self.metadata.tmdb_api_key.clone().unwrap_or_default()),
            "metadata.tmdb_bearer" => Some(self.metadata.tmdb_bearer.clone().unwrap_or_default()),
            "metadata.tmdb_base_url" => Some(self.metadata.tmdb_base_url.clone().unwrap_or_default()),
            "images.max_bytes_mb" => Some((self.images.max_bytes / (1024 * 1024)).to_string()),
            "images.max_age_days" => Some(self.images.max_age_days.to_string()),
            "transcode.max_concurrent" => Some(self.transcode.max_concurrent.to_string()),
            "transcode.hls_segment_seconds" => Some(self.transcode.hls_segment_seconds.to_string()),
            "transcode.ffmpeg_path" => Some(self.transcode.ffmpeg_path.clone().unwrap_or_default()),
            "ai.enabled" => Some(self.ai.enabled.to_string()),
            "ai.provider" => Some(self.ai.provider.clone()),
            "ai.base_url" => Some(self.ai.base_url.clone().unwrap_or_default()),
            "ai.api_key" => Some(self.ai.api_key.clone().unwrap_or_default()),
            "ai.model" => Some(self.ai.model.clone()),
            "ai.fallback_to_rules" => Some(self.ai.fallback_to_rules.to_string()),
            _ => None,
        }
    }

    /// 把一个白名单内的设置写进配置。调用方必须先 `validate_setting`。
    pub fn set_setting(&mut self, key: &str, raw: &str) {
        match key {
            "metadata.language" => self.metadata.language = raw.to_string(),
            "metadata.auto_accept" => self.metadata.auto_accept = parse_bool(raw).unwrap_or(true),
            "metadata.bangumi_enabled" => {
                self.metadata.bangumi_enabled = parse_bool(raw).unwrap_or(false)
            }
            "metadata.fetch_images" => self.metadata.fetch_images = parse_bool(raw).unwrap_or(true),
            "metadata.requests_per_second" => {
                if let Ok(v) = raw.trim().parse::<f64>() {
                    self.metadata.requests_per_second = v;
                }
            }
            "metadata.tmdb_api_key" => {
                self.metadata.tmdb_api_key = Some(raw.trim().to_string()).filter(|s| !s.is_empty());
            }
            "metadata.tmdb_bearer" => {
                self.metadata.tmdb_bearer = Some(raw.trim().to_string()).filter(|s| !s.is_empty());
            }
            "metadata.tmdb_base_url" => {
                self.metadata.tmdb_base_url = Some(raw.trim().to_string()).filter(|s| !s.is_empty());
            }
            "images.max_bytes_mb" => {
                if let Ok(v) = raw.trim().parse::<u64>() {
                    self.images.max_bytes = v * 1024 * 1024;
                }
            }
            "images.max_age_days" => {
                if let Ok(v) = raw.trim().parse::<i64>() {
                    self.images.max_age_days = v;
                }
            }
            "transcode.max_concurrent" => {
                if let Ok(v) = raw.trim().parse::<u32>() {
                    self.transcode.max_concurrent = v;
                }
            }
            "transcode.hls_segment_seconds" => {
                if let Ok(v) = raw.trim().parse::<u32>() {
                    self.transcode.hls_segment_seconds = v;
                }
            }
            "transcode.ffmpeg_path" => {
                self.transcode.ffmpeg_path = Some(raw.trim().to_string()).filter(|s| !s.is_empty())
            }
            "ai.enabled" => self.ai.enabled = parse_bool(raw).unwrap_or(false),
            "ai.provider" => self.ai.provider = raw.trim().to_string(),
            "ai.base_url" => {
                self.ai.base_url = Some(raw.trim().to_string()).filter(|s| !s.is_empty());
            }
            "ai.api_key" => {
                self.ai.api_key = Some(raw.trim().to_string()).filter(|s| !s.is_empty());
            }
            "ai.model" => self.ai.model = raw.trim().to_string(),
            "ai.fallback_to_rules" => self.ai.fallback_to_rules = parse_bool(raw).unwrap_or(true),
            // 其余（user 域 / 未知键）不落在 AppConfig 上：
            // 前者由前端自己消费，后者在写入前就被白名单挡掉了。
            _ => {}
        }
    }

    /// 把数据库里存的覆盖值批量套用到配置上。
    ///
    /// 只覆盖 `scope = server` 且**值合法**的项：一个写坏的值不该让服务起不来，
    /// 顶多是那一项回落到 TOML / 环境变量的值。
    pub fn apply_setting_overrides(&mut self, overrides: &[(String, String)]) -> Vec<String> {
        let mut rejected = Vec::new();
        for (k, v) in overrides {
            match setting_spec(k) {
                Some(spec) if spec.scope == SettingScope::Server => {
                    match validate_setting(spec, v) {
                        Ok(()) => self.set_setting(k, v),
                        Err(e) => rejected.push(format!("{k}: {e}")),
                    }
                }
                _ => rejected.push(format!("{k}: 不在可写设置白名单内")),
            }
        }
        rejected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_binds_to_8080() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.bind_addr(), "0.0.0.0:8080");
        assert_eq!(cfg.database.kind, "sqlite");
        // 并发上限默认 0 = 交给硬件能力决定，不要写死一个数字。
        assert_eq!(cfg.transcode.max_concurrent, 0);
        assert_eq!(cfg.transcode.hls_segment_seconds, 6);
    }

    #[test]
    fn ai_is_off_by_default_and_never_stores_a_key() {
        let cfg = AppConfig::default();
        // 没配模型时 AI 端点必须是 503，而不是"能聊但什么都干不了"。
        assert!(!cfg.ai.enabled);
        assert!(!cfg.ai.allow_dangerous);
        // 配置里存的是环境变量名，不是密钥本身。
        assert!(cfg.ai.api_key_env.starts_with("WHATMEDIA_"));
        assert!(!cfg.ai.api_key_env.contains("sk-"));
        // 防失控的硬闸必须大于 0，否则模型一次工具都调不了。
        assert!(cfg.ai.max_tool_rounds > 0);
    }

    #[test]
    fn byte_range_len_is_inclusive() {
        use whatmedia_core::ByteRange;
        assert_eq!(ByteRange::new(0, Some(9)).len(100), 10);
        assert_eq!(ByteRange::new(50, None).len(100), 50);
    }

    #[test]
    fn whitelist_never_exposes_paths_port_or_secrets() {
        // 这几项一旦可从 HTTP 改动，改坏之后界面就再也打不开、无从改回。
        let forbidden = [
            "paths.data_dir",
            "paths.config_dir",
            "paths.cache_dir",
            "server.port",
            "server.host",
            "database.url",
            "security.signing_key",
            "ai.api_key_env",
            "ai.allow_dangerous",
        ];
        for k in forbidden {
            assert!(
                setting_spec(k).is_none(),
                "「{k}」不该出现在可写设置白名单里"
            );
        }
    }

    #[test]
    fn every_spec_has_a_readable_value() {
        let cfg = AppConfig::default();
        for spec in SETTING_SPECS {
            if spec.scope != SettingScope::Server {
                continue;
            }
            let v = cfg
                .get_setting(spec.key)
                .unwrap_or_else(|| panic!("server 级设置「{}」读不出值", spec.key));
            // 读出来的默认值必须能通过自己的校验，否则界面一打开就是一片红。
            validate_setting(spec, &v).unwrap_or_else(|e| {
                panic!("「{}」的默认值 {v:?} 通不过自己的校验：{e}", spec.key)
            });
        }
    }

    #[test]
    fn validation_rejects_out_of_range_and_bad_enums() {
        let rps = setting_spec("metadata.requests_per_second").unwrap();
        assert!(validate_setting(rps, "999").is_err());
        assert!(validate_setting(rps, "4").is_ok());

        let q = setting_spec("player.preferred_quality").unwrap();
        assert!(validate_setting(q, "1080p").is_ok());
        // 大小写不一致必须报错，而不是静默回落到默认值。
        assert!(validate_setting(q, "1080P").is_err());

        let flag = setting_spec("ai.enabled").unwrap();
        assert!(validate_setting(flag, "yes").is_ok());
        assert!(validate_setting(flag, "maybe").is_err());
    }

    #[test]
    fn overrides_apply_and_bad_ones_are_reported_not_fatal() {
        let mut cfg = AppConfig::default();
        let rejected = cfg.apply_setting_overrides(&[
            ("metadata.language".into(), "en-US".into()),
            ("metadata.requests_per_second".into(), "999".into()),
            ("server.port".into(), "1".into()),
        ]);
        // 好的那项生效了。
        assert_eq!(cfg.metadata.language, "en-US");
        // 坏的两项被列出来而不是 panic —— 一个写坏的值不能让服务起不来。
        assert_eq!(rejected.len(), 2);
        assert_eq!(cfg.server.port, 8080);
    }
}
