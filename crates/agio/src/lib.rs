// agio — D1 输出契约的信封发射器（宪法 D1，自老仓 aginxos/crates/agio 搬入）。
//
// 一切 aginx-* CLI 的 stdout 都是同一个 JSON 信封，agent 端只需要一种解析器：
//
//   {"ok":true,  "data":…, "meta":{…}?}
//   {"ok":false, "error":{"type":"usage", "code":"<cmd 稳定码>",
//                         "message":"…", "hint":"…"?}}
//
// 约定（与老仓 M24 `ag commands --json` 的既有形状同构，此处为正本）：
// - 信封永远走 stdout 且永远可解析；退出码同时给出：0=ok，1=fail，
//   2=usage 类（ErrorType::Usage）。stderr 留给人类诊断，不进契约。
// - error.type 是封闭小集合（ErrorType）；code 是命令自己的稳定字符串，
//   跨版本不 renamed——agent 脚本靠它分支。
// - meta 可选，装 count/来源等一眼信息；数据本体全在 data。
// - 键序不构成契约（serde_json 默认字母序），解析按键取值。
//
// 本 crate 只依赖 serde_json，无 tokio/clap——给小静态 CLI 共用
// （aginx 路由 / aginx-server 客户端面 / 后续工具包）。

use serde_json::{json, Value};

/// 封闭的错误大类。usage 退出码 2，其余退出码 1。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrorType {
    /// 调用方式错：缺必选参数、坏 flag —— 退出码 2。
    Usage,
    /// 目标不存在：无名单位、无此路由 —— 退出码 1。
    NotFound,
    /// 文件系统/设备 IO 失败 —— 退出码 1。
    Io,
    /// 系统状态不允许：未初始化、正忙、冲突 —— 退出码 1。
    State,
    /// 凭证/权限不足 —— 退出码 1。
    Auth,
    /// 不该发生的内部错误 —— 退出码 1。
    Internal,
}

impl ErrorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorType::Usage => "usage",
            ErrorType::NotFound => "not_found",
            ErrorType::Io => "io",
            ErrorType::State => "state",
            ErrorType::Auth => "auth",
            ErrorType::Internal => "internal",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            ErrorType::Usage => 2,
            _ => 1,
        }
    }
}

/// 成功信封：`{"ok":true,"data":…}`。
pub fn ok(data: Value) -> Value {
    json!({"ok": true, "data": data})
}

/// 成功信封带 meta：`{"ok":true,"data":…,"meta":…}`。
pub fn ok_meta(data: Value, meta: Value) -> Value {
    json!({"ok": true, "data": data, "meta": meta})
}

/// 失败信封（无 hint）。
pub fn fail(etype: ErrorType, code: &str, message: &str) -> Value {
    json!({"ok": false, "error": {"type": etype.as_str(), "code": code, "message": message}})
}

/// 失败信封带 hint（给 agent 的下一步建议）。
pub fn fail_hint(etype: ErrorType, code: &str, message: &str, hint: &str) -> Value {
    json!({"ok": false, "error": {
        "type": etype.as_str(), "code": code, "message": message, "hint": hint
    }})
}

/// 把信封单行紧凑打到 stdout。
pub fn print(env: &Value) {
    println!("{env}");
}

/// 打成功信封并退出 0。
pub fn exit_ok(data: Value) -> ! {
    print(&ok(data));
    std::process::exit(0)
}

/// 打成功信封（带 meta）并退出 0。
pub fn exit_ok_meta(data: Value, meta: Value) -> ! {
    print(&ok_meta(data, meta));
    std::process::exit(0)
}

/// 打失败信封并按错误大类退出（usage=2，其余=1）。
pub fn exit_fail(etype: ErrorType, code: &str, message: &str) -> ! {
    print(&fail(etype, code, message));
    std::process::exit(etype.exit_code())
}

/// 打失败信封（带 hint）并按错误大类退出。
pub fn exit_fail_hint(etype: ErrorType, code: &str, message: &str, hint: &str) -> ! {
    print(&fail_hint(etype, code, message, hint));
    std::process::exit(etype.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_shape() {
        let v = ok_meta(json!([1, 2]), json!({"count": 2}));
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["data"], json!([1, 2]));
        assert_eq!(v["meta"]["count"], json!(2));
        // 键序不构成契约，但 ok/data/error 三键必在。
        let o = v.as_object().unwrap();
        assert!(o.contains_key("ok") && o.contains_key("data") && o.contains_key("meta"));
    }

    #[test]
    fn fail_shape_and_codes() {
        let v = fail_hint(ErrorType::Usage, "missing_arg", "need <name>", "try: aginx agent send me 你好");
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["error"]["type"], json!("usage"));
        assert_eq!(v["error"]["code"], json!("missing_arg"));
        assert_eq!(v["error"]["hint"], json!("try: aginx agent send me 你好"));
        assert_eq!(ErrorType::Usage.exit_code(), 2);
        assert_eq!(ErrorType::NotFound.exit_code(), 1);
        assert_eq!(ErrorType::Internal.exit_code(), 1);
    }

    #[test]
    fn error_type_strings_are_stable() {
        // agent 脚本按 type 分支——这些字符串跨版本不能变。
        assert_eq!(ErrorType::Usage.as_str(), "usage");
        assert_eq!(ErrorType::NotFound.as_str(), "not_found");
        assert_eq!(ErrorType::Io.as_str(), "io");
        assert_eq!(ErrorType::State.as_str(), "state");
        assert_eq!(ErrorType::Auth.as_str(), "auth");
        assert_eq!(ErrorType::Internal.as_str(), "internal");
    }
}
