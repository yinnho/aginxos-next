//! aginx-done CLI — provision 的 done 标记（M27；lib.rs 有语义与测试）。
//!
//! 退出码三分：0=成功/已标记，3=check 的"未标记"（查询的合法答案，
//! 不是失败——provision 用 `aginx-done check x || step && aginx-done mark x`），
//! 1=io 故障，2=usage。`--json` 出 D1 信封：查询本身成功仍是 ok:true
//! （data.marked=false），但 rc 保持 3 —— 脚本按 rc 分支，JSON 消费者按
//! data 分支，两条路都通。

use serde_json::json;
use std::process::exit;

use aginx_done::{is_marked, list, mark, marked_at, reset, reset_all, validate_name};

const NOT_MARKED: i32 = 3;

fn usage() -> &'static str {
    "usage: aginx-done [--json] <command> [args]
  check <name>     0 = marked, 3 = not marked (bad marker counts as not)
  mark <name>      stamp <name> done now (refreshes if already there)
  ensure <name>    mark if absent; for naturally idempotent stamps only —
                   NOT for steps: a step marked before it runs defeats the
                   discipline. The step pattern is check || step && mark.
  reset <name>     remove one marker (missing = already gone, rc 0)
  reset --all      remove every marker
  list             names + recorded epochs

markers live in /var/lib/aginx/done/<name>; content is the epoch seconds of
the mark (advisory). AGINX_DONE_DIR overrides the directory (tests, dev)."
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&str> = args.iter().filter(|a| *a != "--json").map(String::as_str).collect();

    match rest.as_slice() {
        [] | ["--help"] | ["-h"] => {
            println!("{}", usage());
            exit(if rest.is_empty() { 2 } else { 0 });
        }
        ["check", name] => {
            want_name(name, json);
            let marked = is_marked(name);
            if json {
                let env = agio::ok_meta(
                    json!({"name": name, "marked": marked}),
                    json!({"at": marked_at(name), "dir": aginx_done::dir().display().to_string()}),
                );
                agio::print(&env);
            } else if marked {
                println!("aginx-done: {name} marked at {}", marked_at(name).map(|t| t.to_string()).unwrap_or_else(|| "?".into()));
            } else {
                println!("aginx-done: {name} not marked");
            }
            exit(if marked { 0 } else { NOT_MARKED });
        }
        ["mark", name] => {
            want_name(name, json);
            match mark(name) {
                Ok(at) => {
                    if json {
                        agio::print(&agio::ok(json!({"name": name, "marked": true, "at": at})));
                    } else {
                        println!("aginx-done: marked {name} at {at}");
                    }
                    exit(0);
                }
                Err(e) => io_fail(name, &e, json),
            }
        }
        ["ensure", name] => {
            want_name(name, json);
            if is_marked(name) {
                if json {
                    agio::print(&agio::ok(json!({"name": name, "marked": true, "at": marked_at(name), "fresh": false})));
                } else {
                    println!("aginx-done: {name} already marked at {}", marked_at(name).map(|t| t.to_string()).unwrap_or_else(|| "?".into()));
                }
                exit(0);
            }
            match mark(name) {
                Ok(at) => {
                    if json {
                        agio::print(&agio::ok(json!({"name": name, "marked": true, "at": at, "fresh": true})));
                    } else {
                        println!("aginx-done: marked {name} at {at}");
                    }
                    exit(0);
                }
                Err(e) => io_fail(name, &e, json),
            }
        }
        ["reset", "--all"] => {
            let n = reset_all();
            if json {
                agio::print(&agio::ok_meta(json!({"removed": n}), json!({"count": n})));
            } else {
                println!("aginx-done: removed {n} marker{}", if n == 1 { "" } else { "s" });
            }
            exit(0);
        }
        ["reset", name] => {
            want_name(name, json);
            let removed = reset(name);
            if json {
                agio::print(&agio::ok(json!({"name": name, "removed": removed})));
            } else {
                println!("aginx-done: {}", if removed { format!("removed {name}") } else { format!("{name} had no marker") });
            }
            exit(0);
        }
        ["list"] => {
            let all = list();
            if json {
                let arr: Vec<_> = all
                    .iter()
                    .map(|(n, at)| json!({"name": n, "at": at}))
                    .collect();
                agio::print(&agio::ok_meta(json!(arr), json!({"count": arr.len()})));
            } else {
                for (n, at) in &all {
                    println!("aginx-done: {n}\t{}", at.map(|t| t.to_string()).unwrap_or_else(|| "?".into()));
                }
            }
            exit(0);
        }
        _ => {
            if json {
                agio::print(&agio::fail(agio::ErrorType::Usage, "usage", &format!("unknown command {rest:?}")));
            } else {
                eprintln!("{}", usage());
            }
            exit(2);
        }
    }
}

/// Validate the marker name; usage envelope / usage text on a bad one.
fn want_name(name: &str, json: bool) {
    if let Err(e) = validate_name(name) {
        if json {
            agio::print(&agio::fail_hint(agio::ErrorType::Usage, "bad_name", &e, "allowed: A-Za-z0-9._-"));
        } else {
            eprintln!("aginx-done: {e}");
        }
        exit(2);
    }
}

fn io_fail(name: &str, e: &std::io::Error, json: bool) -> ! {
    let msg = format!("marker {name}: {e}");
    if json {
        agio::print(&agio::fail(agio::ErrorType::Io, "marker_io", &msg));
    } else {
        eprintln!("aginx-done: {msg}");
    }
    exit(1);
}
