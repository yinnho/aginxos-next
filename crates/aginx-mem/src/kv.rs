//! kv 面 — 结构化键值存取（M35 从 runtime tools/kv.rs 整体搬来）。
//!
//! 行为同构上游：同样的 (agent, owner, user) 三元组隔离、同样的输出
//! 文案（`No value found for key '{key}'.` / `Stored value for key '{key}'.`
//! / `- {}: {}` 列表 / `No keys found.`）。substrate 的 system_kv 直连。

use crate::{db_path_of, open_substrate, AgmemCtx};
use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;
use std::path::PathBuf;

/// kv 域三元组：owner/user 缺席回落 ""——与被搬的 runtime kv.rs
/// `unwrap_or("")` 逐字一致（tree 面的回落不同，见 tree.rs）。
fn identity(ctx: &AgmemCtx) -> (&str, &str, &str) {
    (
        ctx.agent_id.as_str(),
        ctx.owner_id.as_deref().unwrap_or(""),
        ctx.user_id.as_deref().unwrap_or(""),
    )
}

pub fn kv_get(input: &Value, ctx: &AgmemCtx, db_flag: Option<&PathBuf>) -> CarrierResult<String> {
    let key = input["key"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'key' parameter".to_string(),
    ))?;
    let (a, o, u) = identity(ctx);
    let sub = open_substrate(&db_path_of(ctx, db_flag))?;
    match sub.system_kv_get(a, o, u, key)? {
        Some(val) => Ok(serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string())),
        None => Ok(format!("No value found for key '{key}'.")),
    }
}

pub fn kv_set(input: &Value, ctx: &AgmemCtx, db_flag: Option<&PathBuf>) -> CarrierResult<String> {
    let key = input["key"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'key' parameter".to_string(),
    ))?;
    let value = input
        .get("value")
        .cloned()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'value' parameter".to_string(),
        ))?;
    let (a, o, u) = identity(ctx);
    let sub = open_substrate(&db_path_of(ctx, db_flag))?;
    sub.system_kv_set(a, o, u, key, value)?;
    Ok(format!("Stored value for key '{key}'."))
}

pub fn kv_list(input: &Value, ctx: &AgmemCtx, db_flag: Option<&PathBuf>) -> CarrierResult<String> {
    let prefix = input["prefix"].as_str();
    let (a, o, u) = identity(ctx);
    let sub = open_substrate(&db_path_of(ctx, db_flag))?;
    let pairs = sub.list_kv(a, o, u)?;
    let filtered: Vec<_> = if let Some(p) = prefix {
        pairs.into_iter().filter(|(k, _)| k.starts_with(p)).collect()
    } else {
        pairs
    };

    if filtered.is_empty() {
        Ok("No keys found.".to_string())
    } else {
        let lines: Vec<String> = filtered
            .iter()
            .map(|(k, v)| format!("- {}: {}", k, v))
            .collect();
        Ok(lines.join("\n"))
    }
}

/// CLI 补位：substrate 一直有 delete，上游工具面漏了这个动词。
pub fn kv_delete(
    input: &Value,
    ctx: &AgmemCtx,
    db_flag: Option<&PathBuf>,
) -> CarrierResult<String> {
    let key = input["key"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'key' parameter".to_string(),
    ))?;
    let (a, o, u) = identity(ctx);
    let sub = open_substrate(&db_path_of(ctx, db_flag))?;
    sub.system_kv_delete(a, o, u, key)?;
    Ok(format!("Deleted key '{key}'."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::default_identity;
    use carrier_memory::MemorySubstrate;
    use serde_json::json;

    /// substrate 换成内存库跑同一套 kv 语义（--db 指向的文件即库，
    /// 这里用 tempfile 落地——与守护同款 open 路径，WAL 生效）。
    fn mem_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("carrier.db");
        MemorySubstrate::open(&p).unwrap();
        (dir, p)
    }

    fn ctx_default() -> AgmemCtx {
        default_identity()
    }

    #[test]
    fn kv_roundtrip_and_listing_match_upstream_copy() {
        let (_d, db) = mem_db();
        let ctx = ctx_default();
        let db = Some(db);

        // set → get 往返，pretty-print 同上游
        kv_set(&json!({"key":"theme","value":"dark"}), &ctx, db.as_ref()).unwrap();
        let out = kv_get(&json!({"key":"theme"}), &ctx, db.as_ref()).unwrap();
        assert_eq!(out, "\"dark\"");

        // 缺键文案
        let out = kv_get(&json!({"key":"nope"}), &ctx, db.as_ref()).unwrap();
        assert_eq!(out, "No value found for key 'nope'.");

        // 前缀过滤 + 列表文案
        kv_set(&json!({"key":"entity.alice","value":1}), &ctx, db.as_ref()).unwrap();
        let out = kv_list(&json!({"prefix":"entity."}), &ctx, db.as_ref()).unwrap();
        assert_eq!(out, "- entity.alice: 1");
        let out = kv_list(&json!({"prefix":"zzz."}), &ctx, db.as_ref()).unwrap();
        assert_eq!(out, "No keys found.");

        // del 补位动词
        kv_delete(&json!({"key":"theme"}), &ctx, db.as_ref()).unwrap();
        let out = kv_get(&json!({"key":"theme"}), &ctx, db.as_ref()).unwrap();
        assert_eq!(out, "No value found for key 'theme'.");
    }

    #[test]
    fn kv_is_scoped_by_identity_triple() {
        let (_d, db) = mem_db();
        let db = Some(db);
        let mo = AgmemCtx {
            agent_id: "mo".into(),
            owner_id: Some("o1".into()),
            user_id: Some("u@im".into()),
            home_dir: None,
            db_path: None,
            workspace_root: None,
        };
        let other = AgmemCtx {
            agent_id: "clone".into(),
            ..mo.clone()
        };
        kv_set(&json!({"key":"secret","value":42}), &mo, db.as_ref()).unwrap();
        // 换 agent 即换域：看不见
        assert_eq!(
            kv_get(&json!({"key":"secret"}), &other, db.as_ref()).unwrap(),
            "No value found for key 'secret'."
        );
        // 人面默认身份同样看不见化身私域
        let def = default_identity();
        assert_eq!(
            kv_get(&json!({"key":"secret"}), &def, db.as_ref()).unwrap(),
            "No value found for key 'secret'."
        );
    }

    #[test]
    fn missing_params_are_input_errors() {
        let (_d, db) = mem_db();
        let ctx = default_identity();
        let r = kv_get(&json!({}), &ctx, Some(&db));
        assert!(matches!(r, Err(CarrierError::InvalidInput(_))));
        let r = kv_set(&json!({"key":"k"}), &ctx, Some(&db));
        assert!(matches!(r, Err(CarrierError::InvalidInput(_))));
    }
}
