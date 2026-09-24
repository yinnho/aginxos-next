//! D1 输出契约的 carrier 侧镜射（AginxOS `crates/agio` 同形状，M25）。
//!
//! 契约是 JSON 形状，不是代码依赖——aginxos 与 carrier 是两个 workspace，
//! 各持一份实现，形状由两边的测试锁住。机器可读面（`agent list --json`、
//! `cron list`）stdout 一律此信封；非零退出码同给（0=ok，1=fail，2=usage），
//! stderr 留给人类提示，不进契约。键序不构成契约，解析按键取值。

use serde_json::{json, Value};

/// 成功信封：`{"ok":true,"data":…}`。
pub fn ok(data: Value) -> Value {
    json!({"ok": true, "data": data})
}

/// 成功信封带 meta：`{"ok":true,"data":…,"meta":…}`。
pub fn ok_meta(data: Value, meta: Value) -> Value {
    json!({"ok": true, "data": data, "meta": meta})
}

/// 失败信封：`{"ok":false,"error":{"type","code","message","hint"?}}`。
/// type 取封闭集合：usage | not_found | io | state | auth | internal。
pub fn fail(etype: &str, code: &str, message: &str, hint: Option<&str>) -> Value {
    let mut error = json!({"type": etype, "code": code, "message": message});
    if let Some(h) = hint {
        error["hint"] = json!(h);
    }
    json!({"ok": false, "error": error})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_locks_with_agio() {
        // 与 aginxos crates/agio 的测试同一断言集——形状漂移两边同时红。
        let v = ok_meta(json!([{"id": "a"}]), json!({"count": 1}));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"][0]["id"], json!("a"));
        assert_eq!(v["meta"]["count"], json!(1));
        let f = fail("not_found", "no_agent", "没有该化身", Some("try: agent list"));
        assert_eq!(f["ok"], json!(false));
        assert_eq!(f["error"]["type"], json!("not_found"));
        assert_eq!(f["error"]["hint"], json!("try: agent list"));
    }
}
