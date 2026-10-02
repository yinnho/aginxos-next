// ops — UDS 面的操作层：一行 JSON 请求 → 一行 D1 信封响应。
//
// 客户端（`aginx agent …`，路由器内置）与 server 之间的一问一答协议：
//   → {"op":"status"} | {"op":"list"}
//   → {"op":"install","avatar":"…","path":"包.tar"} | {"op":"remove","avatar":"…"}
//   ← agio 信封（ok/data/error），单行，连接即关。
// 刀④-4：send/create 随 in-process 引擎退役——对话走网关
// agent://<机>.relay.aginx.net/<名>（system 与助理同名册直通）。

use carrier_types::config::KernelConfig;
use serde_json::{json, Value};

pub fn handle_line(config: &KernelConfig, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => return agio::fail(agio::ErrorType::Usage, "bad_request", &format!("not json: {e}")),
    };
    let op = req["op"].as_str().unwrap_or("");
    match op {
        "status" => agio::ok(json!({"agents": roster(config)})),
        "list" => agio::ok(json!({"agents": roster(config)})),
        "install" => op_install(config, &req),
        "remove" => op_remove(config, &req),
        other => agio::fail(
            agio::ErrorType::Usage,
            "bad_op",
            &format!("unknown op '{other}' (status/list/install/remove)"),
        ),
    }
}

/// workflows/ 下的目录名（跳过文件与隐藏目录）= 在册能力面。
fn roster(config: &KernelConfig) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(config.effective_workflows_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// 刀④-3 装卸链搬家：install/remove 从 carrier CLI 面搬进 server UDS 面。
/// 装不走 kernel 注册表（不起 in-kernel 腿）——新助理 gateway 直达
/// `agent://<机>.relay.aginx.net/<名>`，对话不经这里；本面只管格式层
/// 落盘 + 对外两写（见 install.rs）。
fn op_install(config: &KernelConfig, req: &Value) -> Value {
    let Some(name) = req["avatar"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return agio::fail(agio::ErrorType::Usage, "bad_request", "install needs avatar name and tar path");
    };
    let Some(path) = req["path"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return agio::fail(agio::ErrorType::Usage, "bad_request", "install needs tar path");
    };
    match crate::install::install_tar(config, name, std::path::Path::new(path)) {
        Ok(r) => agio::ok(json!({
            "avatar": r.name,
            "display": r.display,
            "description": r.description,
            "files": r.file_count,
            "warnings": r.warnings,
            "text": format!(
                "已装 {}（{}，{} 件）；对话走 agent://<本机>/<{}>，aginx-svc restart aginx-gateway 后名册生效",
                r.name, r.display, r.file_count, r.name
            ),
        })),
        Err(e) => agio::fail(agio::ErrorType::State, "install", &e),
    }
}

fn op_remove(config: &KernelConfig, req: &Value) -> Value {
    let Some(name) = req["avatar"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return agio::fail(agio::ErrorType::Usage, "bad_request", "remove needs avatar name");
    };
    match crate::install::remove(config, name) {
        Ok(msg) => agio::ok(json!({"avatar": name, "text": msg})),
        Err(e) => agio::fail(agio::ErrorType::State, "remove", &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(tag: &str) -> KernelConfig {
        let home = std::env::temp_dir().join(format!("aginx-ops-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("data")).unwrap();
        std::os::unix::fs::symlink(home.join("data"), home.join("gateway-data")).unwrap();
        KernelConfig {
            home_dir: home,
            data_dir: std::path::PathBuf::new(),
            workflows_dir: None,
            ..KernelConfig::default()
        }
    }

    #[test]
    fn status_lists_workflows_dirs_only() {
        let cfg = config("roster");
        std::fs::create_dir_all(cfg.home_dir.join("workflows/b-agent")).unwrap();
        std::fs::create_dir_all(cfg.home_dir.join("workflows/a-agent")).unwrap();
        std::fs::write(cfg.home_dir.join("workflows/stray.txt"), b"x").unwrap();
        let r = handle_line(&cfg, r#"{"op":"status"}"#);
        assert_eq!(r["data"]["agents"], json!(["a-agent", "b-agent"]));
        let r = handle_line(&cfg, r#"{"op":"zzz"}"#);
        assert_eq!(r["error"]["code"], json!("bad_op"));
        let r = handle_line(&cfg, r#"{"op":"remove"}"#);
        assert_eq!(r["error"]["code"], json!("bad_request"));
        let _ = std::fs::remove_dir_all(&cfg.home_dir);
    }
}
