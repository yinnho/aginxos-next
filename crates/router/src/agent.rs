// agent — 前台客户端：路由器内置的 aginx-server UDS 面。
//
// 刀④-4（2026-10-02）：send/create 随 in-process 引擎退役——对话真源=
// 网关 agent://<机>.relay.aginx.net/<名>（system 与助理都是名册里的
// codex 条目）。本面剩装卸与名册：
//
//   aginx agent list                  花名册（workflows/ 能力面）
//   aginx agent status                在册状态
//   aginx agent install <名> <包.tar> 装：克隆格式包落 workflows/（④-3）
//   aginx agent remove <名>           卸：三删（工位/网关条目/名册行）
//
// --json 打印原始 D1 信封给脚本用。
//
// env：AGINX_SOCK（默认 /run/aginx.sock；host 试跑两边都得设）

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub fn run(args: &[String]) -> i32 {
    let mut json_mode = false;
    let mut rest: Vec<&String> = args
        .iter()
        .filter(|a| {
            if a.as_str() == "--json" {
                json_mode = true;
                false
            } else {
                true
            }
        })
        .collect();
    if rest.is_empty() {
        usage();
        return 2;
    }
    let verb = rest.remove(0);
    let op = match verb.as_str() {
        "list" => json!({"op": "list"}),
        "status" => json!({"op": "status"}),
        "install" => match (rest.first(), rest.get(1)) {
            (Some(n), Some(p)) => json!({"op": "install", "avatar": n.as_str(), "path": p.as_str()}),
            _ => {
                eprintln!("aginx agent: install needs <名> <包.tar>");
                usage();
                return 2;
            }
        },
        "remove" => match rest.first() {
            Some(n) => json!({"op": "remove", "avatar": n.as_str()}),
            None => {
                eprintln!("aginx agent: remove needs a name");
                usage();
                return 2;
            }
        },
        "--help" | "-h" | "help" => {
            usage();
            return 0;
        }
        other => {
            eprintln!("aginx agent: unknown verb '{other}'");
            usage();
            return 2;
        }
    };
    let resp = match roundtrip(&op) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("aginx agent: {e}");
            return 1;
        }
    };
    if json_mode {
        println!("{resp}");
    } else {
        print_human(&resp, verb.as_str());
    }
    if resp["ok"].as_bool().unwrap_or(false) {
        0
    } else {
        1
    }
}

/// 一问一答：连 UDS、写一行 op、读一行信封。
fn roundtrip(op: &Value) -> Result<Value, String> {
    let sock: PathBuf = std::env::var("AGINX_SOCK")
        .unwrap_or_else(|_| "/run/aginx.sock".into())
        .into();
    let mut stream = UnixStream::connect(&sock)
        .map_err(|e| format!("server not reachable at {} ({e}) — is aginx-server running?", sock.display()))?;
    writeln!(&mut stream, "{op}").map_err(|e| e.to_string())?;
    let _ = stream.flush();
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    serde_json::from_str(line.trim()).map_err(|e| format!("bad response: {e}"))
}

/// list/status/install/remove 的人面打印。
fn print_human(resp: &Value, verb: &str) {
    if !resp["ok"].as_bool().unwrap_or(false) {
        let e = &resp["error"];
        eprintln!("aginx agent: [{}] {}", e["code"].as_str().unwrap_or("?"), e["message"].as_str().unwrap_or(""));
        if let Some(hint) = e["hint"].as_str() {
            eprintln!("try: {hint}");
        }
        return;
    }
    let d = &resp["data"];
    match verb {
        "list" => {
            for a in d["agents"].as_array().unwrap_or(&vec![]) {
                println!("{}", a.as_str().unwrap_or(""));
            }
        }
        "status" => {
            let agents = d["agents"].as_array().map(Vec::len).unwrap_or(0);
            println!("在册能力面：{agents}");
            for a in d["agents"].as_array().unwrap_or(&vec![]) {
                println!("  {}", a.as_str().unwrap_or(""));
            }
        }
        "install" | "remove" => {
            println!("{}", d["text"].as_str().unwrap_or("完成"));
        }
        _ => println!("{d}"),
    }
}

fn usage() {
    eprintln!("usage: aginx agent list | status");
    eprintln!("       aginx agent install <名> <包.tar>");
    eprintln!("       aginx agent remove <名>");
    eprintln!("       对话走 agent://<机>.relay.aginx.net/<名>（send 已随引擎退役）");
    eprintln!("env:   AGINX_SOCK (default /run/aginx.sock)");
}
