//! agf — 文件工具 CLI 的库面（M32 D3 批2）。
//!
//! file_read / file_write / file_list / file_convert / image_analyze 五个
//! 工具的语义从 carrier-runtime 的 tools/filesystem.rs 与 media.rs 整体搬来
//! （行为同构：同样的二进制/文档识别、同样的纠偏提示、同样的
//! markitdown/pandoc 编排、同样的 view_url 拼法）。runtime 侧只留
//! `agf_bridge`：同名 ToolDefinition + spawn `agf tool <name>`。
//!
//! 两张脸：
//! - 人/流程脚本：`agf read <path>`、`agf ls <path>`、`agf write <path>`、
//!   `agf convert <in> <fmt>`、`agf inspect <img>` — 路径按 CWD 解析。
//! - 机读（runtime 桥用）：`agf tool <name>`，stdin 收工具入参 JSON，
//!   stdout 出 D1 信封（`{"ok":true,"data":"…"}` / `{"ok":false,"error":…}`）。
//!
//! 路径解析的分工：沙箱与用户数据目录路由留在 runtime 桥（单真源在
//! kernel 侧，§9 计划随末模块退役）；桥把解析结果经 stdin JSON 的保留键
//! `_ctx` 注入：
//!
//! ```json
//! {
//!   "path": "output/x.md",
//!   "_ctx": {
//!     "home_dir": "/home", "sender_id": "u1@im", "owner_id": null,
//!     "agent_name": "mo", "external_url": "https://…",
//!     "resolved": { "path": "/abs/resolved/by/bridge" }
//!   }
//! }
//! ```
//!
//! 工具实现只认 `_ctx.resolved.<param>`（预解析绝对路径）或裸 CWD 相对
//! 路径——本 CLI 不是沙箱边界，越界拦截是 kernel 的事。

pub mod ops;
pub mod view_url;

use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;
use std::path::PathBuf;

/// 本 CLI 承载的全部工具名（与 runtime 桥的 definitions 一一对应）。
pub const TOOL_NAMES: &[&str] = &[
    "file_read",
    "file_write",
    "file_list",
    "file_convert",
    "image_analyze",
];

/// 桥注入的执行身份（view_url / 自动输出目录需要；人面为 None）。
#[derive(Debug, Clone)]
pub struct AgfCtx {
    pub home_dir: Option<PathBuf>,
    pub sender_id: Option<String>,
    pub owner_id: Option<String>,
    pub agent_name: Option<String>,
    pub external_url: Option<String>,
}

/// 从入参 JSON 提取 `_ctx`（缺席 = 人面/裸调用）。
pub fn ctx_of(input: &Value) -> Option<AgfCtx> {
    let c = input.get("_ctx")?;
    Some(AgfCtx {
        home_dir: c.get("home_dir").and_then(|v| v.as_str()).map(PathBuf::from),
        sender_id: c.get("sender_id").and_then(|v| v.as_str()).map(String::from),
        owner_id: c.get("owner_id").and_then(|v| v.as_str()).map(String::from),
        agent_name: c.get("agent_name").and_then(|v| v.as_str()).map(String::from),
        external_url: c
            .get("external_url")
            .and_then(|v| v.as_str())
            .map(String::from),
    })
}

/// 解析一个路径参数：`_ctx.resolved.<param>` 预解析值优先，否则按调用方
/// 给的原样（机读面此时已是桥认可的用户数据/工作区语义，人面即 CWD 相对）。
pub fn resolve_param(input: &Value, param: &str, raw: &str) -> CarrierResult<PathBuf> {
    if let Some(p) = input["_ctx"]["resolved"][param].as_str() {
        return Ok(PathBuf::from(p));
    }
    Ok(PathBuf::from(raw))
}

/// 工具派发 — `agf tool <name>` 的库面。`None` = 不是本 CLI 的工具。
pub async fn execute_tool(name: &str, input: &Value) -> Option<CarrierResult<String>> {
    match name {
        "file_read" => Some(ops::file_read(input).await),
        "file_write" => Some(ops::file_write(input).await),
        "file_list" => Some(ops::file_list(input).await),
        "file_convert" => Some(ops::file_convert(input).await),
        "image_analyze" => Some(ops::image_analyze(input).await),
        _ => None,
    }
}

/// 人读/机读两脸共用的错误出口：Err → stderr 一行 + rc 1。
pub fn bail_human(e: &CarrierError) -> ! {
    eprintln!("agf: {e}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_tool_is_none() {
        assert!(execute_tool("nope", &serde_json::json!({})).await.is_none());
    }

    #[test]
    fn tool_names_are_unique() {
        let mut v = TOOL_NAMES.to_vec();
        v.sort_unstable();
        v.dedup();
        assert_eq!(v.len(), TOOL_NAMES.len());
    }

    #[test]
    fn ctx_extraction_reads_identity_fields() {
        let input = serde_json::json!({
            "path": "output/a.md",
            "_ctx": {
                "home_dir": "/home",
                "sender_id": "u1@im.wechat",
                "owner_id": null,
                "agent_name": "mo-catering-ops",
                "external_url": "https://x.example",
                "resolved": { "path": "/home/workspaces/mo/senders/u1@im.wechat/output/a.md" }
            }
        });
        let ctx = ctx_of(&input).expect("ctx present");
        assert_eq!(ctx.home_dir.as_deref(), Some("/home".as_ref()));
        assert_eq!(ctx.sender_id.as_deref(), Some("u1@im.wechat"));
        assert!(ctx.owner_id.is_none());
        assert_eq!(ctx.agent_name.as_deref(), Some("mo-catering-ops"));
        assert_eq!(ctx.external_url.as_deref(), Some("https://x.example"));

        let p = resolve_param(&input, "path", "output/a.md").unwrap();
        assert_eq!(
            p,
            PathBuf::from("/home/workspaces/mo/senders/u1@im.wechat/output/a.md")
        );
    }

    #[test]
    fn resolve_param_falls_back_to_raw() {
        let p = resolve_param(&serde_json::json!({}), "path", "output/a.md").unwrap();
        assert_eq!(p, PathBuf::from("output/a.md"));
    }
}
