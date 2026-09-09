// ops — UDS 面的操作层：一行 JSON 请求 → 一行 D1 信封响应。
//
// 客户端（`aginx agent …`，路由器内置）与 server 之间的一问一答协议：
//   → {"op":"send","avatar":"小满"?,"text":"你好"}
//   → {"op":"status"} | {"op":"list"} | {"op":"create","avatar":"…","soul":"…"?}
//   ← agio 信封（ok/data/error），单行，连接即关。
// send 不带 avatar = 住（当前光标）；退房词在 front 层裁决。
// ② send 的三路：目标化身的轮正跑着 → steer 插进下一个工具步边界
// （回执 steered:true 即回，不等轮跑完）；插不进 → 排队当下一回合。

use crate::front::{FrontDesk, SendTarget, SteerOutcome, MOTHER};
use crate::ServerCfg;
use serde_json::{json, Value};
use std::sync::MutexGuard;

pub fn handle_line(desk: &FrontDesk, cfg: &ServerCfg, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => return agio::fail(agio::ErrorType::Usage, "bad_request", &format!("not json: {e}")),
    };
    let op = req["op"].as_str().unwrap_or("");
    match op {
        "send" => op_send(desk, cfg, &req),
        "status" => {
            let roster = desk.roster();
            agio::ok(json!({"cursor": desk.cursor(), "avatars": roster}))
        }
        "list" => agio::ok(json!({"avatars": desk.roster()})),
        "create" => op_create(desk, &req),
        other => agio::fail(
            agio::ErrorType::Usage,
            "bad_op",
            &format!("unknown op '{other}' (send/status/list/create)"),
        ),
    }
}

fn op_send(desk: &FrontDesk, cfg: &ServerCfg, req: &Value) -> Value {
    let text = req["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return agio::fail(agio::ErrorType::Usage, "empty_text", "send needs non-empty text");
    }
    let avatar = req["avatar"].as_str().map(str::trim).filter(|s| !s.is_empty());

    // 前台裁决先行（退房词/点名/住）——光标是前台自己的状态，不需要
    // 轮锁护着；裁决完才知道这条 send 有没有 steer 可插。
    let target = match desk.resolve_send(avatar, &text) {
        Ok(t) => t,
        Err(msg) => {
            return agio::fail_hint(
                agio::ErrorType::NotFound,
                "unknown_avatar",
                &msg,
                "aginx agent create <名字>",
            )
        }
    };

    // ② 三路：能插队就 steer；插不进（母体直答/退房/换化身/轮正好
    // 收尾）排队当下一回合。撞上收口的 steer 拿到 TurnEnded 后回头
    // 再抢锁——那句话重分类为下一回合，不丢。
    loop {
        if let Some(guard) = desk.try_turn() {
            return held_turn(desk, cfg, &target, &text, guard);
        }
        if let SendTarget::Avatar(name) = &target {
            if let Some(rx) = desk.push_steer(name, &text) {
                match rx.recv() {
                    Ok(SteerOutcome::Delivered(turn)) => {
                        return agio::ok(json!({
                            "avatar": name,
                            "steered": true,
                            "turn": turn,
                            "text": "（已插入运行中的回合）",
                        }))
                    }
                    _ => continue, // 轮先收口了：重分类为下一回合
                }
            }
        }
        return held_turn(desk, cfg, &target, &text, desk.turn_lock());
    }
}

/// 跑一个完整回合（锁已到手，guard 只保活着）。母体直答/化身 spawn/
/// 退房回执三臂。
fn held_turn(
    desk: &FrontDesk,
    cfg: &ServerCfg,
    target: &SendTarget,
    text: &str,
    _turn: MutexGuard<'_, ()>,
) -> Value {
    match target {
        SendTarget::Checkout => agio::ok(json!({
            "avatar": MOTHER, "text": "（已回到母体）", "checkout": true,
        })),
        SendTarget::Mother => match crate::mother::mother_reply(text, &desk.roster()) {
            Ok(reply) => agio::ok(json!({"avatar": MOTHER, "text": reply})),
            Err(e) => agio::fail(agio::ErrorType::State, "brain", &format!("母体 brain 调用失败：{e}")),
        },
        SendTarget::Avatar(name) => {
            let done = crate::turn::run_avatar_turn(cfg, desk, name, text);
            if done.ok {
                agio::ok(json!({
                    "avatar": name, "session": crate::front::SESSION_MAIN, "text": done.text,
                }))
            } else {
                let err = done.error.unwrap_or(agi::FrameError {
                    code: "internal".into(),
                    message: "unknown error".into(),
                });
                agio::fail(agio::ErrorType::State, &err.code, &err.message)
            }
        }
    }
}

fn op_create(desk: &FrontDesk, req: &Value) -> Value {
    let Some(name) = req["avatar"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return agio::fail(agio::ErrorType::Usage, "bad_request", "create needs avatar name");
    };
    let soul = req["soul"].as_str();
    match desk.create_avatar(name, soul) {
        Ok(ws) => agio::ok(json!({"avatar": name, "workspace": ws.display().to_string()})),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            agio::fail(agio::ErrorType::State, "exists", &format!("avatar '{name}' already exists"))
        }
        Err(e) => agio::fail(agio::ErrorType::Io, "io", &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fake_runtime(dir: &std::path::Path) -> std::path::PathBuf {
        let p = dir.join("fake-runtime.sh");
        std::fs::write(
            &p,
            concat!(
                "#!/bin/sh\n",
                "read -r req\n",
                "printf '%s\\n' '{\"t\":\"done\",\"ok\":true,\"text\":\"化身回话\",\"error\":null}'\n",
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn desk_cfg(name: &str) -> (FrontDesk, ServerCfg, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("workspaces");
        std::fs::create_dir_all(&root).unwrap();
        (
            FrontDesk::new(root.clone()),
            ServerCfg::for_test(root, "aginx".into(), fake_runtime(&dir).to_string_lossy().to_string()),
            dir,
        )
    }

    #[test]
    fn send_checkout_and_motherless_avatar_lifecycle() {
        let (desk, cfg, _dir) = desk_cfg("life");

        // 开机：光标=me；点名不存在 → NotFound + hint
        let r = handle_line(&desk, &cfg, r#"{"op":"send","avatar":"小满","text":"在吗"}"#);
        assert_eq!(r["ok"], json!(false));
        assert_eq!(r["error"]["code"], json!("unknown_avatar"));

        // create → 在册
        let r = handle_line(&desk, &cfg, r#"{"op":"create","avatar":"小满","soul":"你是小满"}"#);
        assert_eq!(r["ok"], json!(true));
        let r = handle_line(&desk, &cfg, r#"{"op":"list"}"#);
        assert_eq!(r["data"]["avatars"], json!(["小满"]));

        // 点名 → 假 runtime 应答 → 光标落在化身
        let r = handle_line(&desk, &cfg, r#"{"op":"send","avatar":"小满","text":"在吗"}"#);
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["text"], json!("化身回话"));
        assert_eq!(r["data"]["avatar"], json!("小满"));
        let r = handle_line(&desk, &cfg, r#"{"op":"status"}"#);
        assert_eq!(r["data"]["cursor"], json!("小满"));

        // 住：不点名给光标
        let r = handle_line(&desk, &cfg, r#"{"op":"send","text":"继续"}"#);
        assert_eq!(r["data"]["avatar"], json!("小满"));

        // 退房词 → 回母体
        let r = handle_line(&desk, &cfg, r#"{"op":"send","text":"再见"}"#);
        assert_eq!(r["data"]["checkout"], json!(true));
        let r = handle_line(&desk, &cfg, r#"{"op":"status"}"#);
        assert_eq!(r["data"]["cursor"], json!("me"));
    }

    #[test]
    fn bad_lines_and_ops_get_usage_envelopes() {
        let (desk, cfg, _dir) = desk_cfg("bad");
        let r = handle_line(&desk, &cfg, "not json");
        assert_eq!(r["error"]["type"], json!("usage"));
        let r = handle_line(&desk, &cfg, r#"{"op":"zzz"}"#);
        assert_eq!(r["error"]["code"], json!("bad_op"));
        let r = handle_line(&desk, &cfg, r#"{"op":"send","text":"  "}"#);
        assert_eq!(r["error"]["code"], json!("empty_text"));
    }

    /// D16 派活端到端：不点名 + 光标在母体 + 册上有人 → 假 runtime 应答
    /// （不是 mother_reply 的 brain 单发），回执 avatar 是化身，光标随迁。
    #[test]
    fn unnamed_send_delegates_to_first_avatar() {
        let (desk, cfg, _dir) = desk_cfg("delegate");
        let r = handle_line(&desk, &cfg, r#"{"op":"create","avatar":"小喜","soul":"你是小喜"}"#);
        assert_eq!(r["ok"], json!(true));
        let r = handle_line(&desk, &cfg, r#"{"op":"send","text":"南京天气怎么样"}"#);
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["avatar"], json!("小喜")); // 派给化身，不是 me
        assert_eq!(r["data"]["text"], json!("化身回话"));
        let r = handle_line(&desk, &cfg, r#"{"op":"status"}"#);
        assert_eq!(r["data"]["cursor"], json!("小喜")); // 光标随迁
    }

    fn make_exec(p: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// ② 运行中插入：一轮正卡在工具步上，后到的 send 插进同一轮——回执
    /// steered:true 即回（不等轮跑完），账上 steer 落在 tool_result 之后、
    /// done 之前（管序=账序）。假 runtime 不读 steer 行：写进 pipe 缓冲
    /// 即成功，服务端账序才是断言对象。
    #[test]
    fn steer_lands_at_tool_step_boundary() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-steer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("workspaces");
        std::fs::create_dir_all(&root).unwrap();
        // 睡 2s 留出 steer 窗口；一轮一工具，收完回账就 done
        let p = dir.join("fake-runtime-steer.sh");
        std::fs::write(
            &p,
            concat!(
                "#!/bin/sh\n",
                "read -r req\n",
                "sleep 2\n",
                "printf '%s\\n' '{\"t\":\"tool_call\",\"id\":\"c1\",\"tool\":\"dev-hello\",\"args\":[]}'\n",
                "read -r res\n",
                "printf '%s\\n' '{\"t\":\"done\",\"ok\":true,\"text\":\"一轮完成\",\"error\":null}'\n",
            ),
        )
        .unwrap();
        make_exec(&p);

        let desk = std::sync::Arc::new(FrontDesk::new(root.clone()));
        let cfg = std::sync::Arc::new(ServerCfg::for_test(
            root,
            "aginx".into(),
            p.to_string_lossy().to_string(),
        ));
        desk.create_avatar("小满", None).unwrap();

        let (d2, c2) = (desk.clone(), cfg.clone());
        let t1 = std::thread::spawn(move || {
            handle_line(&d2, &c2, r#"{"op":"send","avatar":"小满","text":"报个状态"}"#)
        });

        // 0.5s：轮在跑（begin_turn 早于此，首个工具步边界晚于此），
        // 这条 send 不排队——拿到 steered 即回
        std::thread::sleep(std::time::Duration::from_millis(500));
        let r = handle_line(&desk, &cfg, r#"{"op":"send","avatar":"小满","text":"改成上海"}"#);
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["steered"], json!(true));
        assert_eq!(r["data"]["turn"], json!(1));
        assert_eq!(r["data"]["avatar"], json!("小满"));

        let r1 = t1.join().unwrap();
        assert_eq!(r1["data"]["text"], json!("一轮完成"));

        // 账序：request → tool_call → tool_result → steer → done
        let log = crate::ledger::session_log(&cfg.workspaces_root, "小满", "main");
        let lines: Vec<agi::Frame> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 5);
        assert!(matches!(&lines[0], agi::Frame::Request(r) if r.text == "报个状态" && r.turn == 1));
        assert!(matches!(&lines[1], agi::Frame::ToolCall(c) if c.id == "c1"));
        assert!(matches!(&lines[2], agi::Frame::ToolResult(tr) if tr.id == "c1"));
        assert!(matches!(&lines[3], agi::Frame::Steer(s) if s.text == "改成上海"));
        assert!(matches!(&lines[4], agi::Frame::Done(d) if d.ok && d.turn == Some(1)));
    }

    /// ② 插不进就重分类：轮无工具步边界、先收口了，排队的 steer 拿
    /// TurnEnded 后回头抢锁，那句话成为下一回合——不丢话，账上无 steer 帧。
    #[test]
    fn steer_missed_turn_becomes_next_turn() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-reclass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let root = dir.join("workspaces");
        std::fs::create_dir_all(&root).unwrap();
        // 睡 1s 直答：没有工具步，steer 没有落脚点
        let p = dir.join("fake-runtime-reclass.sh");
        std::fs::write(
            &p,
            concat!(
                "#!/bin/sh\n",
                "read -r req\n",
                "sleep 1\n",
                "printf '%s\\n' '{\"t\":\"done\",\"ok\":true,\"text\":\"答\",\"error\":null}'\n",
            ),
        )
        .unwrap();
        make_exec(&p);

        let desk = std::sync::Arc::new(FrontDesk::new(root.clone()));
        let cfg = std::sync::Arc::new(ServerCfg::for_test(
            root,
            "aginx".into(),
            p.to_string_lossy().to_string(),
        ));
        desk.create_avatar("小满", None).unwrap();

        let (d2, c2) = (desk.clone(), cfg.clone());
        let t1 = std::thread::spawn(move || {
            handle_line(&d2, &c2, r#"{"op":"send","avatar":"小满","text":"报个状态"}"#)
        });

        std::thread::sleep(std::time::Duration::from_millis(300));
        let started = std::time::Instant::now();
        let r = handle_line(&desk, &cfg, r#"{"op":"send","avatar":"小满","text":"改成上海"}"#);
        // 不是 steered：是它自己跑完的一轮
        assert_eq!(r["ok"], json!(true));
        assert!(r["data"]["steered"].is_null());
        assert_eq!(r["data"]["text"], json!("答"));
        // 排队等到了第一轮收口才开跑（TurnEnded → 重分类 → 抢锁）
        assert!(started.elapsed() >= std::time::Duration::from_millis(700));

        let r1 = t1.join().unwrap();
        assert_eq!(r1["data"]["text"], json!("答"));

        // 账上两轮各自收口，全程没有 steer 帧
        let log = crate::ledger::session_log(&cfg.workspaces_root, "小满", "main");
        let lines: Vec<agi::Frame> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        assert!(matches!(&lines[0], agi::Frame::Request(r) if r.text == "报个状态" && r.turn == 1));
        assert!(matches!(&lines[1], agi::Frame::Done(d) if d.turn == Some(1)));
        assert!(matches!(&lines[2], agi::Frame::Request(r) if r.text == "改成上海" && r.turn == 2));
        assert!(matches!(&lines[3], agi::Frame::Done(d) if d.turn == Some(2)));
        assert!(!lines.iter().any(|f| matches!(f, agi::Frame::Steer(_))));
    }
}
