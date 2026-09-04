// tools — 工具面（D12：外件一律 CLI）。
//
// carrier 的 ToolModule 链（media/shell/sqlite/document 内留模块）不搬；
// 工具宇宙就是 aginx-* 命令注册表本身。发现不靠链接 router 代码，靠
// 路由器自己的 CLI 面：`aginx commands --json` 的 D1 信封就是注册表
// 快照——运行时与注册表之间隔着一条命令行，注册表怎么变都不用重编。
//
// v0 的参数面是统一的 argv 模式：每个工具的 schema 都是
// {args: string[]}（argv 尾巴原样传给命令）。够覆盖全部现有 CLI 形状
// （位置参数与 --flag 都在 argv 里）；更细的 per-tool schema 等命令头
// 长出 params= 键再接，协议侧不用动。

use serde_json::{json, Value};
use std::io;
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl ToolDef {
    /// parameters 为 None 时给默认 argv schema。
    pub fn new(name: &str, description: &str, parameters: Option<Value>) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: description.to_string(),
            parameters: parameters.unwrap_or_else(default_schema),
        }
    }
}

fn default_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "args": {
                "type": "array",
                "items": {"type": "string"},
                "description": "argv tail — passed to the command verbatim",
            }
        },
        "additionalProperties": false,
    })
}

/// 问路由器要注册表快照。失败不给工具（聊 天不受影响），调用方自行
/// 决定要不要吼一嗓子。
pub fn discover_tools(aginx_bin: &str) -> io::Result<Vec<ToolDef>> {
    let out = Command::new(aginx_bin).args(["commands", "--json"]).output()?;
    if !out.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{aginx_bin} commands --json exited {}", out.status),
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("bad envelope: {e}")))?;
    if v["ok"] != json!(true) {
        return Err(io::Error::new(io::ErrorKind::Other, "envelope ok=false"));
    }
    let recs = v["data"]
        .as_array()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "envelope data is not an array"))?;
    // 隐藏命令 commands --json 本来就不吐；这里再挡一道 requires-sudo 的
    // ——daemon 里跑也提不了权，挂上去只会白失败。
    Ok(recs
        .iter()
        .filter(|r| !r["requires_sudo"].as_bool().unwrap_or(false))
        .map(|r| {
            ToolDef::new(
                r["name"].as_str().unwrap_or_default(),
                r["summary"].as_str().unwrap_or_default(),
                None,
            )
        })
        .filter(|t| !t.name.is_empty())
        .collect())
}

/// 路由器二进制：AGINX_BIN 覆写（server spawn runtime 时会带上），
/// 否则 PATH 里的 aginx。
pub fn aginx_bin_from_env() -> String {
    std::env::var("AGINX_BIN").unwrap_or_else(|_| "aginx".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_schema_is_argv_style() {
        let t = ToolDef::new("dev-hello", "smoke", None);
        assert_eq!(t.parameters["properties"]["args"]["type"], json!("array"));
        assert_eq!(t.parameters["additionalProperties"], json!(false));
        // 显式 schema 原样透传
        let custom = ToolDef::new("x", "y", Some(json!({"type": "object"})));
        assert_eq!(custom.parameters, json!({"type": "object"}));
    }

    #[test]
    fn discover_via_cli_contract() {
        // 假路由器：吐一份 D1 信封。证明的是子进程契约（发现走 CLI 面，
        // 不走链接），不是路由器本身。
        let dir = std::env::temp_dir().join("aginx-runtime-test-discover");
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("fake-aginx");
        let script = concat!(
            "#!/bin/sh\necho '{\"ok\":true,\"data\":[",
            "{\"route\":\"dev-hello\",\"name\":\"dev-hello\",\"summary\":\"smoke face\",\"requires_sudo\":false},",
            "{\"route\":\"sys-power\",\"name\":\"sys-power\",\"summary\":\"needs root\",\"requires_sudo\":true}",
            "],\"meta\":{\"count\":2}}'\n",
        );
        std::fs::write(&bin, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tools = discover_tools(bin.to_str().unwrap()).unwrap();
        assert_eq!(tools.len(), 1, "sudo 工具应被滤掉");
        assert_eq!(tools[0].name, "dev-hello");
        assert_eq!(tools[0].description, "smoke face");
        assert_eq!(tools[0].parameters["properties"]["args"]["type"], json!("array"));
    }

    #[test]
    fn discover_bad_envelope_is_error() {
        let dir = std::env::temp_dir().join("aginx-runtime-test-discover-bad");
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("fake-aginx-bad");
        std::fs::write(&bin, "#!/bin/sh\necho 'not json'\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(discover_tools(bin.to_str().unwrap()).is_err());
    }
}
