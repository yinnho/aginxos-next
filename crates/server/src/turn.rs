// turn — fast-agi 的 server 端：spawn aginx-runtime，转发帧，执行工具。
//
// 一轮的完整记账（D8：server 记它经手的所有线上帧）：
//   request（先记账再 spawn）→ [tool_call → tool_result]* → done
// 工具执行在 server 侧（D12：外件一律 CLI，母体唯一外部接口 = spawn），
// 派发走路由器 `aginx <工具名> <argv…>`——server 不需要知道任何
// aginx-* 二进制的存在。
//
// v0 无工具级超时：设备上的 aginx-* 是可信 CLI，轮级活性由 runtime 的
// 卡死检测兜底；真挂死的工具（交互式等待）等 N2 收据说话。

use crate::ServerCfg;
use agi::{Done, Frame, FrameReader, Request, ToolCall, ToolResult};
use serde_json::Value;
use std::io::BufReader;
use std::process::{Child, Command, Stdio};

/// 工具输出进账前的字符上限。CLI 倾倒可以肥，账本和帧线不能无边；
/// runtime 侧另有 12k 的 brain 截断，这里的 20 万字符是审计留量。
const TOOL_OUT_MAX_CHARS: usize = 200_000;

/// 跑一轮化身对话。所有失败都折进返回的 Done（协议同构：done 是唯一
/// 终帧），调用方只管把它回给前台。
pub fn run_avatar_turn(cfg: &ServerCfg, avatar: &str, text: &str) -> Done {
    let ws = cfg.workspaces_root.join(avatar);
    let log = crate::ledger::session_log(&cfg.workspaces_root, avatar, crate::front::SESSION_MAIN);
    let request = Request {
        avatar: avatar.to_string(),
        session: crate::front::SESSION_MAIN.to_string(),
        text: text.to_string(),
    };

    // 铁律：先记账、再 spawn——模型可见即已记录
    if let Err(e) = crate::ledger::append(&log, &Frame::Request(request.clone())) {
        return Done::err("ledger", format!("cannot append session log: {e}"));
    }

    let mut child = match Command::new(&cfg.runtime_bin)
        .arg("--workspace")
        .arg(&ws)
        .arg("--session")
        .arg(crate::front::SESSION_MAIN)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Done::err("spawn", format!("cannot spawn {}: {e}", cfg.runtime_bin)),
    };
    let mut stdin = child.stdin.take().expect("runtime stdin");
    let stdout = child.stdout.take().expect("runtime stdout");

    // 管里再发一遍 request 当 go 信号（文本已在账上，runtime 不叠份）
    if agi::write(&mut stdin, &Frame::Request(request)).is_err() {
        let d = Done::err("protocol", "runtime closed stdin before accepting the request");
        let _ = crate::ledger::append(&log, &Frame::Done(d.clone()));
        let _ = child.kill();
        return d;
    }

    let done = relay(cfg, &mut child, stdout, &mut stdin, &log);
    let _ = child.wait();
    done
}

/// 帧转发主循环：runtime → server 的每帧记账，tool_call 就地执行回账。
fn relay(
    cfg: &ServerCfg,
    child: &mut Child,
    stdout: std::process::ChildStdout,
    stdin: &mut std::process::ChildStdin,
    log: &std::path::Path,
) -> Done {
    let mut rd = FrameReader::new(BufReader::new(stdout));
    loop {
        match rd.next() {
            Ok(Some(Frame::ToolCall(tc))) => {
                let _ = crate::ledger::append(log, &Frame::ToolCall(tc.clone()));
                let result = execute_tool(&cfg.aginx_bin, tc);
                let _ = crate::ledger::append(log, &Frame::ToolResult(result.clone()));
                if agi::write(stdin, &Frame::ToolResult(result)).is_err() {
                    return close_broken(log, child, "runtime closed stdin mid-turn");
                }
            }
            Ok(Some(Frame::Artifact(a))) => {
                // v0 没有投影面：账本就是投影记录（D6 渲染器后续接）
                let _ = crate::ledger::append(log, &Frame::Artifact(a));
            }
            Ok(Some(Frame::Done(d))) => {
                let _ = crate::ledger::append(log, &Frame::Done(d.clone()));
                return d;
            }
            Ok(Some(other)) => {
                // request/steer/tool_result 是 server→runtime 方向的帧，
                // 从 runtime 吐出来就是协议破裂
                let tag = crate::frame_tag(&other);
                let d = Done::err("protocol", format!("runtime sent a server-bound {tag} frame"));
                let _ = crate::ledger::append(log, &Frame::Done(d.clone()));
                let _ = child.kill();
                return d;
            }
            Ok(None) => {
                return close_broken(log, child, "runtime exited without a done frame");
            }
            Err(e) => {
                return close_broken(log, child, format!("bad frame from runtime: {e}"));
            }
        }
    }
}

/// runtime 半路断流：补一帧 synthetic done(err) 让账本轮次收口，重放
/// 永远落在合法形状上。
fn close_broken(log: &std::path::Path, child: &mut Child, why: impl Into<String>) -> Done {
    let d = Done::err("protocol", why);
    let _ = crate::ledger::append(log, &Frame::Done(d.clone()));
    let _ = child.kill();
    d
}

/// 执行一次工具调用：spawn `aginx <tool> <argv…>`，stdout 进 out、
/// stderr 进 err、退出码定成败。
fn execute_tool(bin: &str, tc: ToolCall) -> ToolResult {
    let argv = build_argv(&tc.args);
    let result = Command::new(bin).arg(&tc.tool).args(&argv).output();
    match result {
        Ok(out) => {
            #[cfg(unix)]
            let code = {
                use std::os::unix::process::ExitStatusExt;
                out.status
                    .code()
                    .or_else(|| out.status.signal().map(|s| 128 + s))
                    .unwrap_or(1)
            };
            #[cfg(not(unix))]
            let code = out.status.code().unwrap_or(1);
            ToolResult {
                id: tc.id,
                ok: out.status.success(),
                code,
                out: cap(String::from_utf8_lossy(&out.stdout).into_owned()),
                err: cap(String::from_utf8_lossy(&out.stderr).into_owned()),
            }
        }
        Err(e) => ToolResult {
            id: tc.id,
            ok: false,
            code: 127,
            out: String::new(),
            err: format!("spawn failed: {e}"),
        },
    }
}

/// 帧上的 args → 命令行尾巴（协议约定）：
/// JSON 数组 = argv 原样（字符串裸传，其余 JSON 文本化）；
/// JSON 对象 = --key value 旗标对（键按字典序，序列化稳定）。
fn build_argv(args: &Value) -> Vec<String> {
    let scalar = |v: &Value| -> String {
        match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }
    };
    match args {
        Value::Array(a) => a.iter().map(scalar).collect(),
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = Vec::new();
            for k in keys {
                out.push(format!("--{k}"));
                out.push(scalar(&m[k]));
            }
            out
        }
        Value::Null => Vec::new(),
        other => vec![scalar(other)],
    }
}

fn cap(s: String) -> String {
    if s.chars().count() <= TOOL_OUT_MAX_CHARS {
        s
    } else {
        let kept: String = s.chars().take(TOOL_OUT_MAX_CHARS).collect();
        format!("{kept}\n…[输出超限已截断]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_runtime(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("fake-runtime.sh");
        std::fs::write(
            &p,
            concat!(
                "#!/bin/sh\n",
                "read -r req\n",
                "printf '%s\\n' '{\"t\":\"tool_call\",\"id\":\"c1\",\"tool\":\"dev-hello\",\"args\":[\"世界\"]}'\n",
                "read -r res\n",
                "printf '%s\\n' '{\"t\":\"done\",\"ok\":true,\"text\":\"你好，世界\",\"error\":null}'\n",
            ),
        )
        .unwrap();
        make_exec(&p);
        p
    }

    /// 假路由器：把 argv 记到脚本旁的 argv.txt，stdout 给一句回执
    fn fake_aginx(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("fake-aginx.sh");
        std::fs::write(
            &p,
            concat!(
                "#!/bin/sh\n",
                "d=$(dirname \"$0\")\n",
                "printf '%s\\n' \"$@\" > \"$d/argv.txt\"\n",
                "echo \"hello from tool\"\n",
            ),
        )
        .unwrap();
        make_exec(&p);
        p
    }

    fn make_exec(p: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn cfg(dir: &std::path::Path) -> ServerCfg {
        let root = dir.join("workspaces");
        std::fs::create_dir_all(root.join("小满/sessions")).unwrap();
        ServerCfg::for_test(root, fake_aginx(dir).to_string_lossy().to_string(), fake_runtime(dir).to_string_lossy().to_string())
    }

    #[test]
    fn full_turn_ledger_and_tool_dispatch() {
        let dir = std::env::temp_dir().join("aginx-server-test-turn-full");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = cfg(&dir);

        let done = run_avatar_turn(&cfg, "小满", "打个招呼");
        assert!(done.ok);
        assert_eq!(done.text, "你好，世界");

        // 工具确实经路由器跑了，argv = 工具名 + 数组原样
        let argv = std::fs::read_to_string(dir.join("argv.txt")).unwrap();
        assert_eq!(argv, "dev-hello\n世界\n");

        // 账本轮次收口：request → tool_call → tool_result → done
        let log = crate::ledger::session_log(&cfg.workspaces_root, "小满", "main");
        let lines: Vec<Frame> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        assert!(matches!(&lines[0], Frame::Request(r) if r.text == "打个招呼"));
        assert!(matches!(&lines[1], Frame::ToolCall(c) if c.tool == "dev-hello"));
        assert!(matches!(&lines[2], Frame::ToolResult(r) if r.ok && r.out.contains("hello from tool")));
        assert!(matches!(&lines[3], Frame::Done(d) if d.ok));
    }

    #[test]
    fn runtime_death_without_done_gets_synthetic_close() {
        let dir = std::env::temp_dir().join("aginx-server-test-turn-die");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // 假 runtime 收完 request 直接退（EOF 无 done）
        let p = dir.join("fake-runtime-die.sh");
        std::fs::write(&p, "#!/bin/sh\nread -r req\n").unwrap();
        make_exec(&p);
        let root = dir.join("workspaces");
        std::fs::create_dir_all(root.join("小满/sessions")).unwrap();
        let cfg = ServerCfg::for_test(root, "aginx".into(), p.to_string_lossy().to_string());

        let done = run_avatar_turn(&cfg, "小满", "x");
        assert!(!done.ok);
        assert_eq!(done.error.as_ref().unwrap().code, "protocol");
        // 账上有 synthetic done：重放落合法形状
        let log = crate::ledger::session_log(&cfg.workspaces_root, "小满", "main");
        let lines: Vec<Frame> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2); // request + synthetic done
        assert!(matches!(&lines[1], Frame::Done(d) if !d.ok));
    }

    #[test]
    fn argv_shapes() {
        assert_eq!(build_argv(&serde_json::json!(["a", "b"])), vec!["a", "b"]);
        assert_eq!(
            build_argv(&serde_json::json!({"b": 1, "a": "x"})),
            vec!["--a", "x", "--b", "1"]
        );
        assert!(build_argv(&serde_json::Value::Null).is_empty());
        assert_eq!(build_argv(&serde_json::json!(42)), vec!["42"]);
    }

    #[test]
    fn tool_spawn_failure_is_a_tool_result_not_a_crash() {
        let tc = ToolCall { id: "c1".into(), tool: "nope".into(), args: serde_json::json!([]) };
        // 环境里没有叫这个的假路由器 → spawn failed 折进回账
        let dir = std::env::temp_dir().join("aginx-server-test-turn-nosuchbin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = execute_tool(
            dir.join("no-such-aginx-bin").to_string_lossy().to_string().as_str(),
            tc,
        );
        assert!(!r.ok);
        assert_eq!(r.code, 127);
        assert!(r.err.contains("spawn failed"));
    }

    #[test]
    fn request_first_ledger_line_even_when_runtime_missing() {
        let dir = std::env::temp_dir().join("aginx-server-test-turn-noruntime");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("workspaces");
        std::fs::create_dir_all(root.join("小满/sessions")).unwrap();
        let cfg = ServerCfg::for_test(root, "aginx".into(), dir.join("no-such-runtime").to_string_lossy().to_string());
        let done = run_avatar_turn(&cfg, "小满", "你好");
        assert!(!done.ok);
        assert_eq!(done.error.as_ref().unwrap().code, "spawn");
        // request 已记账：模型可见即已记录，哪怕 spawn 当场失败
        let log = crate::ledger::session_log(&cfg.workspaces_root, "小满", "main");
        let first = std::fs::read_to_string(&log).unwrap();
        assert!(matches!(
            serde_json::from_str::<Frame>(first.lines().next().unwrap()).unwrap(),
            Frame::Request(_)
        ));
    }
}
