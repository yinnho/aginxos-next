//! agmem — 记忆工具 CLI 的库面（M35）。
//!
//! kv_get / kv_set / kv_list / kv_delete / memory_tree 的语义从
//! carrier-runtime 的 tools/kv.rs 与 tools/memory.rs 整体搬来，knowledge
//! 库 + flows 面（list/read/add/update/remove/import/lint/heal/extract/
//! index、clone_evaluate、flow_create/update/load）从 tools/knowledge.rs
//! 搬来（行为同构：同样的身份三元组隔离、同样的输出文案、同样的截断
//! 边界处理、同样的凭证闸与 frontmatter 闸）。runtime 侧只留
//! `agmem_bridge`：同名 ToolDefinition + spawn `agmem tool <name>`；
//! apply_patch 与 session_summarize 留守 runtime（内核耦合面，不属记忆域）。
//!
//! 两张脸：
//! - 人/流程脚本：`agmem kv get <key>`、`agmem kv set <k> <v>`、
//!   `agmem tree search <q>`…（见 main.rs）
//! - 机读（runtime 桥用）：`agmem tool <name>`，stdin 收工具入参 JSON，
//!   stdout 出 D1 信封（`{"ok":true,"data":"…"}` / `{"ok":false,"error":…}`）。
//!
//! 与 agf 的路径分工不同，本 CLI 直开 memory substrate（rusqlite WAL +
//! busy_timeout=5000，substrate.rs open 内建），默认
//! `$HOME/.aginx/carrier/data/carrier.db` —— 与守护同一库、同一迁移链，
//! 并发安全（M35a spike 2026-09-03）。knowledge/flows 面不吃库，吃
//! workspace 文件树（knowledge/、flows/、MEMORY.md），路径经 `_ctx.workspace_root`
//! 注入（对齐 runtime ToolContext.workspace_root = 化身 manifest 的
//! workspace 路径）；人面用 --workspace 直给。身份经 stdin JSON 保留键
//! `_ctx` 注入：
//!
//! ```json
//! {
//!   "key": "theme",
//!   "_ctx": {
//!     "agent_id": "mo", "owner_id": "default", "user_id": "u1@im",
//!     "home_dir": "/home"
//!   }
//! }
//! ```
//!
//! `_ctx` 缺席 = 人面/裸调用，走 `--agent/--owner/--user` 旗标默认值。
//! kv 域按 (agent, owner, user) 隔离——人面默认身份看不到化身私域是
//! 正确行为，不是 bug。

pub mod knowledge;
pub mod kv;
pub mod tree;

use carrier_memory::MemorySubstrate;
use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;
use std::path::PathBuf;

/// 本 CLI 承载的全部工具名（与 runtime 桥的 definitions 一一对应；
/// kv_delete 是 CLI 补位——substrate 一直有 delete，上游工具面漏了。
/// apply_patch / session_summarize 不在表内：留守 runtime，内核耦合面）。
pub const TOOL_NAMES: &[&str] = &[
    "kv_get",
    "kv_set",
    "kv_list",
    "kv_delete",
    "memory_tree",
    "knowledge_list",
    "knowledge_read",
    "knowledge_add",
    "knowledge_update",
    "knowledge_remove",
    "knowledge_import",
    "knowledge_lint",
    "knowledge_heal",
    "knowledge_extract",
    "knowledge_index",
    "clone_evaluate",
    "flow_create",
    "flow_update",
    "flow_load",
];

/// kv/tree 域的身份三元组 + substrate 定位 + knowledge 域的 workspace。
///
/// owner/user 是 `Option`：桥注入的 `_ctx` 里显式 `null` = 上游
/// `ctx.owner_id`/`ctx.sender_id` 为 None 的路径——kv 面此时回落 ""、
/// tree 面回落 "default"/不过滤（与被搬的 runtime 模块逐字一致）。
#[derive(Debug, Clone)]
pub struct AgmemCtx {
    pub agent_id: String,
    pub owner_id: Option<String>,
    pub user_id: Option<String>,
    /// substrate 所在 HOME（默认 $HOME；人面可用 --db 直给库路径）。
    pub home_dir: Option<PathBuf>,
    /// 显式库路径覆盖（桥为 `memory.sqlite_path` 非默认时预留）。
    pub db_path: Option<PathBuf>,
    /// knowledge/flows 面的 workspace 根（对齐 runtime
    /// ToolContext.workspace_root = 化身 manifest 的 workspace 路径）。
    pub workspace_root: Option<PathBuf>,
}

/// 人面默认身份。owner="default"/user="local" 是人面自己的域；agent="me"
/// 对齐手机注册的主化身；user="local" 标记"这是本机人手，不是任何渠道
/// 来信"。
pub fn default_identity() -> AgmemCtx {
    AgmemCtx {
        agent_id: "me".to_string(),
        owner_id: Some("default".to_string()),
        user_id: Some("local".to_string()),
        home_dir: None,
        db_path: None,
        workspace_root: None,
    }
}

/// `_ctx` 里的可选字符串字段：显式 null → None（桥传上游 None 路径），
/// 字符串 → Some，缺席 → 回落 fallback。
fn opt_field(c: &serde_json::Map<String, Value>, key: &str, fallback: &Option<String>) -> Option<String> {
    match c.get(key) {
        Some(Value::Null) => None,
        Some(v) => v.as_str().map(String::from).or_else(|| fallback.clone()),
        None => fallback.clone(),
    }
}

/// 从入参 JSON 提取 `_ctx`（缺席 = 人面/裸调用 → 默认身份）。
pub fn ctx_of(input: &Value, fallback: AgmemCtx) -> AgmemCtx {
    let Some(c) = input.get("_ctx") else {
        return fallback;
    };
    let c = c.as_object();
    let get = |k: &str| c.and_then(|m| m.get(k));
    AgmemCtx {
        agent_id: get("agent_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&fallback.agent_id)
            .to_string(),
        owner_id: match c {
            Some(m) => opt_field(m, "owner_id", &fallback.owner_id),
            None => fallback.owner_id,
        },
        user_id: match c {
            Some(m) => opt_field(m, "user_id", &fallback.user_id),
            None => fallback.user_id,
        },
        home_dir: get("home_dir")
            .and_then(|v| v.as_str())
            .map(PathBuf::from),
        db_path: get("db_path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from),
        workspace_root: get("workspace_root")
            .and_then(|v| v.as_str())
            .map(PathBuf::from),
    }
}

/// substrate 库路径：显式 --db > _ctx.db_path > `_ctx.home_dir` 推导 >
/// `$HOME` 推导。两种推导的**语义不同**（M35d 设备探针实证的坑：桥把
/// carrier home 原样塞进来，`.aginx/carrier` 拼了两遍打不开库）：
/// 桥注入的 home_dir 是 carrier home（runtime ToolContext.home_dir =
/// config.home_dir；agf 的 sender_data_dir 同款语义），布局
/// `{home}/data/carrier.db`；`$HOME` 是用户主目录，布局
/// `{HOME}/.aginx/carrier/data/carrier.db`。默认布局下两者同库——
/// 守护开的就是 config.data_dir（carrier home + data）下的 carrier.db。
pub fn db_path_of(ctx: &AgmemCtx, db_flag: Option<&PathBuf>) -> PathBuf {
    if let Some(p) = db_flag {
        return p.clone();
    }
    if let Some(p) = &ctx.db_path {
        return p.clone();
    }
    if let Some(home) = &ctx.home_dir {
        // 桥注入 = carrier home（data/ 直接挂在它下面）
        return home.join("data").join("carrier.db");
    }
    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home"));
    user_home
        .join(".aginx")
        .join("carrier")
        .join("data")
        .join("carrier.db")
}

/// workspace 根：显式 --workspace > _ctx.workspace_root > 无（knowledge
/// 面的工具此时按上游同款 Internal 报错——人面裸调需要 --workspace）。
pub fn workspace_of(ctx: &AgmemCtx, ws_flag: Option<&PathBuf>) -> Option<PathBuf> {
    ws_flag.cloned().or_else(|| ctx.workspace_root.clone())
}

/// 打开 substrate（WAL + busy_timeout 在 open 内建；迁移同步进行）。
pub fn open_substrate(path: &std::path::Path) -> CarrierResult<MemorySubstrate> {
    MemorySubstrate::open(path)
}

/// 机读面单真源入口：工具名 + 入参 JSON → String 结果。
/// 人面子命令在 main.rs 把参数拼回工具入参后也走这里。
pub async fn execute_tool(
    name: &str,
    input: &Value,
    db_flag: Option<&PathBuf>,
    ws_flag: Option<&PathBuf>,
    fallback: AgmemCtx,
) -> Option<CarrierResult<String>> {
    let ctx = ctx_of(input, fallback);
    let ws = workspace_of(&ctx, ws_flag);
    match name {
        // kv 面：短平快，直接在当前线程查（WAL 读不挡守护）。
        "kv_get" => Some(kv::kv_get(input, &ctx, db_flag)),
        "kv_set" => Some(kv::kv_set(input, &ctx, db_flag)),
        "kv_list" => Some(kv::kv_list(input, &ctx, db_flag)),
        "kv_delete" => Some(kv::kv_delete(input, &ctx, db_flag)),
        // tree 面：substrate 的 tree_* 是同步调用，包 blocking 线程
        // 保持与守护 runtime 同样的不挡调度器纪律。
        "memory_tree" => Some(
            tokio::task::spawn_blocking({
                let input = input.clone();
                let ctx = ctx.clone();
                let db_flag = db_flag.cloned();
                move || tree::memory_tree(&input, &ctx, db_flag.as_ref())
            })
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))
            .and_then(|r| r),
        ),
        // knowledge/flows 面：纯 workspace 文件树操作（不吃库），
        // tokio::fs 与上游 runtime 同款执行形状。
        "knowledge_list" => Some(knowledge::knowledge_list(ws.as_deref()).await),
        "knowledge_read" => Some(knowledge::knowledge_read(input, ws.as_deref()).await),
        "knowledge_add" => Some(knowledge::knowledge_add(input, ws.as_deref()).await),
        "knowledge_update" => Some(knowledge::knowledge_update(input, ws.as_deref()).await),
        "knowledge_remove" => Some(knowledge::knowledge_remove(input, ws.as_deref()).await),
        "knowledge_import" => Some(knowledge::knowledge_import(input, ws.as_deref()).await),
        "knowledge_lint" => Some(knowledge::knowledge_lint(ws.as_deref()).await),
        "knowledge_heal" => Some(knowledge::knowledge_heal(ws.as_deref()).await),
        "knowledge_extract" => Some(knowledge::knowledge_extract(input, ws.as_deref()).await),
        "knowledge_index" => Some(knowledge::knowledge_index(ws.as_deref()).await),
        "clone_evaluate" => Some(knowledge::clone_evaluate(ws.as_deref()).await),
        "flow_create" => Some(knowledge::flow_create(input, ws.as_deref()).await),
        "flow_update" => Some(knowledge::flow_update(input, ws.as_deref()).await),
        "flow_load" => Some(knowledge::flow_load(input, ws.as_deref()).await),
        _ => None,
    }
}

/// 人面错误出口：agf 同款——非零退出 + stderr 一行。
pub fn bail_human(e: &CarrierError) -> ! {
    eprintln!("agmem: {e}");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ctx_of_takes_identity_and_home() {
        let input = json!({
            "key": "k",
            "_ctx": {
                "agent_id": "mo", "owner_id": "o1", "user_id": "u@im",
                "home_dir": "/tmp/h", "workspace_root": "/var/lib/ws/mo"
            }
        });
        let c = ctx_of(&input, default_identity());
        assert_eq!(c.agent_id, "mo");
        assert_eq!(c.owner_id, Some("o1".to_string()));
        assert_eq!(c.user_id, Some("u@im".to_string()));
        assert_eq!(c.home_dir, Some(PathBuf::from("/tmp/h")));
        // workspace 随 _ctx 进来（桥注入化身 manifest 的 workspace 路径）
        assert_eq!(
            c.workspace_root,
            Some(PathBuf::from("/var/lib/ws/mo"))
        );
        // db 路径按 carrier home 布局推导（桥注入语义；M35d 设备实证）
        assert_eq!(
            db_path_of(&c, None),
            PathBuf::from("/tmp/h/data/carrier.db")
        );
    }

    #[test]
    fn ctx_null_owner_user_maps_to_none_not_fallback() {
        // 桥把上游 ctx.owner_id/sender_id 的 None 以显式 null 传入——
        // 必须还原成 None（kv 面 → ""，tree 面 → "default"/不过滤），
        // 不能错拿人面默认 default/local。
        let input = json!({"_ctx": {"agent_id": "mo", "owner_id": null, "user_id": null}});
        let c = ctx_of(&input, default_identity());
        assert_eq!(c.owner_id, None);
        assert_eq!(c.user_id, None);
        // 缺席键才是回落人面默认
        let c = ctx_of(&json!({"_ctx": {"agent_id": "mo"}}), default_identity());
        assert_eq!(c.owner_id, Some("default".to_string()));
        assert_eq!(c.user_id, Some("local".to_string()));
    }

    #[test]
    fn ctx_db_path_overrides_home_derivation() {
        let c = ctx_of(
            &json!({"_ctx": {"home_dir": "/tmp/h", "db_path": "/tmp/x/custom.db"}}),
            default_identity(),
        );
        assert_eq!(db_path_of(&c, None), PathBuf::from("/tmp/x/custom.db"));
    }

    #[test]
    fn workspace_flag_wins_over_ctx() {
        let c = AgmemCtx {
            workspace_root: Some(PathBuf::from("/tmp/ws-from-ctx")),
            ..default_identity()
        };
        assert_eq!(
            workspace_of(&c, Some(&PathBuf::from("/tmp/ws-flag"))),
            Some(PathBuf::from("/tmp/ws-flag"))
        );
        assert_eq!(
            workspace_of(&c, None),
            Some(PathBuf::from("/tmp/ws-from-ctx"))
        );
        assert_eq!(workspace_of(&default_identity(), None), None);
    }

    #[test]
    fn ctx_of_falls_back_to_default_identity() {
        let c = ctx_of(&json!({"key": "k"}), default_identity());
        assert_eq!(c.agent_id, "me");
        assert_eq!(c.owner_id, Some("default".to_string()));
        assert_eq!(c.user_id, Some("local".to_string()));
    }

    #[test]
    fn db_flag_wins_over_ctx_home() {
        let c = AgmemCtx {
            home_dir: Some(PathBuf::from("/tmp/h")),
            ..default_identity()
        };
        assert_eq!(
            db_path_of(&c, Some(&PathBuf::from("/tmp/x.db"))),
            PathBuf::from("/tmp/x.db")
        );
    }

    #[test]
    fn tool_names_are_the_contract() {
        // runtime 桥按这个名字表对齐 definitions；改名 = 破金样本。
        // apply_patch / session_summarize 刻意缺席：留守 runtime。
        assert_eq!(TOOL_NAMES.len(), 19);
        assert!(TOOL_NAMES.contains(&"kv_get"));
        assert!(TOOL_NAMES.contains(&"memory_tree"));
        assert!(TOOL_NAMES.contains(&"knowledge_list"));
        assert!(TOOL_NAMES.contains(&"flow_load"));
        assert!(!TOOL_NAMES.contains(&"apply_patch"));
        assert!(!TOOL_NAMES.contains(&"session_summarize"));
    }
}
