// agent — 前台客户端（宪法 D10/D11）：路由器内置的 aginx-server UDS 面。
// 这是母体门面的对话口——`aginx` 是唯一裸命令，说话也是喊母体的名字。
//
//   aginx agent send <名字> <文本…>   点名（进/切：光标落到该化身）
//   aginx agent send <文本…>          住（给当前光标；开机是母体）
//   aginx agent list                  花名册
//   aginx agent status                前台状态（光标 + 在册）
//   aginx agent create <名字> [SOUL]  进：建化身文件夹
//
// send 的回复按人面打印（这是对话，不是机器输出）；--json 打印原始
// D1 信封给脚本用。文本里说退房词（再见/退下/…）= 退，光标回母体。
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
        "send" => return send(&rest, json_mode),
        "list" => json!({"op": "list"}),
        "status" => json!({"op": "status"}),
        "create" => {
            let name = match rest.first() {
                Some(n) => n.as_str(),
                None => {
                    eprintln!("aginx agent: create needs a name");
                    usage();
                    return 2;
                }
            };
            let soul = rest.get(1).map(|s| s.as_str());
            json!({"op": "create", "avatar": name, "soul": soul})
        }
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
    print_human(&resp, verb.as_str());
    if resp["ok"].as_bool().unwrap_or(false) {
        0
    } else {
        1
    }
}

fn send(rest: &[&String], json_mode: bool) -> i32 {
    let (avatar, text) = if rest.len() >= 2 {
        (Some(rest[0].as_str()), rest[1..].iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "))
    } else if rest.len() == 1 {
        (None, rest[0].clone())
    } else {
        eprintln!("aginx agent: send needs text");
        usage();
        return 2;
    };
    if text.trim().is_empty() {
        eprintln!("aginx agent: send needs non-empty text");
        return 2;
    }
    let op = json!({"op": "send", "avatar": avatar, "text": text});
    let resp = match roundtrip(&op) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("aginx agent: {e}");
            return 1;
        }
    };
    if json_mode {
        println!("{resp}");
    } else if resp["ok"].as_bool().unwrap_or(false) {
        println!("{}", resp["data"]["text"].as_str().unwrap_or(""));
    } else {
        let e = &resp["error"];
        eprintln!("aginx agent: [{}] {}", e["code"].as_str().unwrap_or("?"), e["message"].as_str().unwrap_or(""));
        if let Some(hint) = e["hint"].as_str() {
            eprintln!("try: {hint}");
        }
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

/// list/status/create 的人面打印。
fn print_human(resp: &Value, verb: &str) {
    if !resp["ok"].as_bool().unwrap_or(false) {
        let e = &resp["error"];
        eprintln!("aginx agent: [{}] {}", e["code"].as_str().unwrap_or("?"), e["message"].as_str().unwrap_or(""));
        return;
    }
    let d = &resp["data"];
    match verb {
        "list" => {
            for a in d["avatars"].as_array().unwrap_or(&vec![]) {
                println!("{}", a.as_str().unwrap_or(""));
            }
        }
        "status" => {
            let cursor = d["cursor"].as_str().unwrap_or("?");
            let n = d["avatars"].as_array().map(Vec::len).unwrap_or(0);
            if cursor == "me" {
                println!("前台：母体（me）");
            } else {
                println!("前台：化身 {cursor}");
            }
            println!("在册化身：{n}");
        }
        "create" => {
            println!("已进：化身 {}", d["avatar"].as_str().unwrap_or("?"));
        }
        _ => println!("{d}"),
    }
}

fn usage() {
    eprintln!("usage: aginx agent send [<名字>] <文本…>   点名/住（退房词=回母体）");
    eprintln!("       aginx agent list | status");
    eprintln!("       aginx agent create <名字> [SOUL 描述]");
    eprintln!("env:   AGINX_SOCK (default /run/aginx.sock)");
}
