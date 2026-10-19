//! OpenAPI 契约规范生成与导出子命令模块。
//!
//! 供前端代码生成脚本（`gen-api.mjs`）及 API 契约文档自动化校验使用。
//! 仅在以 `--features openapi` 编译时包含完整文档生成逻辑。

use anyhow::Result;
use std::path::PathBuf;

/// 导出 OpenAPI JSON 契约文件。
pub fn openapi_cmd(out: Option<PathBuf>) -> Result<()> {
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
        println!("OpenAPI 契约文件已成功导出至：{}", path.display());
        Ok(())
    }
    #[cfg(not(feature = "openapi"))]
    {
        let _ = out;
        anyhow::bail!(
            "当前二进制构建未开启 openapi 特性！\n\
             请使用以下命令重新编译运行：\n  cargo run --features openapi -p whatmedia -- openapi"
        );
    }
}
