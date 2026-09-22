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
use crate::host::Mother;
use serde_json::{json, Value};
use std::sync::MutexGuard;

pub fn handle_line(desk: &FrontDesk, mother: &Mother, line: &str) -> Value {
    let req: Value = match serde_json::from_str(line.trim()) {
        Ok(v) => v,
        Err(e) => return agio::fail(agio::ErrorType::Usage, "bad_request", &format!("not json: {e}")),
    };
    let op = req["op"].as_str().unwrap_or("");
    match op {
        "send" => op_send(desk, mother, &req),
        "status" => {
            let roster = desk.roster();
            agio::ok(json!({"cursor": desk.cursor(), "avatars": roster}))
        }
        "list" => agio::ok(json!({"avatars": desk.roster()})),
        "create" => op_create(desk, mother, &req),
        other => agio::fail(
            agio::ErrorType::Usage,
            "bad_op",
            &format!("unknown op '{other}' (send/status/list/create)"),
        ),
    }
}

fn op_send(desk: &FrontDesk, mother: &Mother, req: &Value) -> Value {
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

    // ② 三路：能插队就 steer；插不进（母体直答/退房/换化���/轮正好
    // 收尾）排队当下一回合。撞上收口的 steer 拿到 TurnEnded 后回头
    // 再抢锁——那句话重分类为下一回合，不丢。
    loop {
        if let Some(guard) = desk.try_turn() {
            return held_turn(desk, mother, &target, &text, guard);
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
        return held_turn(desk, mother, &target, &text, desk.turn_lock());
    }
}

/// 跑一个完整回合（锁已到手，guard 只保活着）。直调后两臂同路：
/// 母体与助理都是 kernel 里的一名 agent（me 的 workspace=home 根），
/// 谁说话都记 D8 账。退房回执不进 kernel——光标回家就是回执。
fn held_turn(
    desk: &FrontDesk,
    mother: &Mother,
    target: &SendTarget,
    text: &str,
    _turn: MutexGuard<'_, ()>,
) -> Value {
    match target {
        SendTarget::Checkout => agio::ok(json!({
            "avatar": MOTHER, "text": "（已回到母体）", "checkout": true,
        })),
        SendTarget::Mother => respond(mother.run_turn(desk, MOTHER, text), MOTHER),
        SendTarget::Avatar(name) => respond(mother.run_turn(desk, name, text), name),
    }
}

/// Done → agio 信封。ok 走 data（avatar+session+text），失败拆
/// FrameError 进 error。
fn respond(done: agi::Done, avatar: &str) -> Value {
    if done.ok {
        agio::ok(json!({
            "avatar": avatar, "session": crate::front::SESSION_MAIN, "text": done.text,
        }))
    } else {
        let err = done.error.unwrap_or(agi::FrameError {
            code: "internal".into(),
            message: "unknown error".into(),
        });
        agio::fail(agio::ErrorType::State, &err.code, &err.message)
    }
}

fn op_create(desk: &FrontDesk, mother: &Mother, req: &Value) -> Value {
    let Some(name) = req["avatar"].as_str().map(str::trim).filter(|s| !s.is_empty()) else {
        return agio::fail(agio::ErrorType::Usage, "bad_request", "create needs avatar name");
    };
    let soul = req["soul"].as_str();
    match desk.create_avatar(name, soul) {
        Ok(ws) => {
            // 前台文件夹已建；kernel 里同步 spawn（幂等）——从此这个
            // 名字在册，send 直达。
            if let Err(e) = mother.spawn_workflow(name) {
                return agio::fail(agio::ErrorType::State, "kernel", &e);
            }
            agio::ok(json!({"avatar": name, "workspace": ws.display().to_string()}))
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            agio::fail(agio::ErrorType::State, "exists", &format!("avatar '{name}' already exists"))
        }
        Err(e) => agio::fail(agio::ErrorType::Io, "io", &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::agent_log;
    use crate::testkit::{boot_host, later, now, oai, read_frames, stub_brain};
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn send_checkout_and_avatar_lifecycle() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-life-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let addr = stub_brain(vec![
            now(oai("在的", None, "stop")),
            now(oai("继续着呢", None, "stop")),
        ]);
        let (desk, mother) = boot_host(&dir, &addr);

        // 开机：点名不存在 → NotFound + hint
        let r = handle_line(&desk, &mother, r#"{"op":"send","avatar":"小满","text":"在吗"}"#);
        assert_eq!(r["ok"], json!(false));
        assert_eq!(r["error"]["code"], json!("unknown_avatar"));

        // create → 在册（前台文件夹 + kernel 注册）
        let r = handle_line(&desk, &mother, r#"{"op":"create","avatar":"小满","soul":"你是小满"}"#);
        assert_eq!(r["ok"], json!(true));
        let r = handle_line(&desk, &mother, r#"{"op":"list"}"#);
        assert_eq!(r["data"]["avatars"], json!(["小满"]));

        // 点名 → 真轮直答 → 光标落在化身
        let r = handle_line(&desk, &mother, r#"{"op":"send","avatar":"小满","text":"在吗"}"#);
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["text"], json!("在的"));
        assert_eq!(r["data"]["avatar"], json!("小满"));
        let r = handle_line(&desk, &mother, r#"{"op":"status"}"#);
        assert_eq!(r["data"]["cursor"], json!("小满"));

        // 住：不点名给光标
        let r = handle_line(&desk, &mother, r#"{"op":"send","text":"继续"}"#);
        assert_eq!(r["data"]["avatar"], json!("小满"));
        assert_eq!(r["data"]["text"], json!("继续着呢"));

        // 退房词 → 回母体
        let r = handle_line(&desk, &mother, r#"{"op":"send","text":"再见"}"#);
        assert_eq!(r["data"]["checkout"], json!(true));
        let r = handle_line(&desk, &mother, r#"{"op":"status"}"#);
        assert_eq!(r["data"]["cursor"], json!(MOTHER));
    }

    #[test]
    fn bad_lines_and_ops_get_usage_envelopes() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // 三条坏行都到不了 brain：stub 空转即可
        let (desk, mother) = boot_host(&dir, &stub_brain(vec![]));
        let r = handle_line(&desk, &mother, "not json");
        assert_eq!(r["error"]["type"], json!("usage"));
        let r = handle_line(&desk, &mother, r#"{"op":"zzz"}"#);
        assert_eq!(r["error"]["code"], json!("bad_op"));
        let r = handle_line(&desk, &mother, r#"{"op":"send","text":"  "}"#);
        assert_eq!(r["error"]["code"], json!("empty_text"));
    }

    /// D16 派活端到端：不点名 + 光标在母体 + 册上有人 → 派给在册化身，
    /// 回执 avatar 是化身，光标随迁。
    #[test]
    fn unnamed_send_delegates_to_first_avatar() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-delegate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let addr = stub_brain(vec![now(oai("查到了", None, "stop"))]);
        let (desk, mother) = boot_host(&dir, &addr);
        let r = handle_line(&desk, &mother, r#"{"op":"create","avatar":"小喜","soul":"你是小喜"}"#);
        assert_eq!(r["ok"], json!(true));
        let r = handle_line(&desk, &mother, r#"{"op":"send","text":"南京天气怎么样"}"#);
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["avatar"], json!("小喜")); // 派给化身，不是 me
        assert_eq!(r["data"]["text"], json!("查到了"));
        let r = handle_line(&desk, &mother, r#"{"op":"status"}"#);
        assert_eq!(r["data"]["cursor"], json!("小喜")); // 光标随迁
    }

    /// ② 运行中插入：一轮卡在第二个工具步前（假 brain 把第二响应拖
    /// 1s——drain 发生在工具结果之后、下一次 brain 调用之前，窗口由
    /// 延迟撑开），0.3s 进队的 steer 在第二个工具步边界被收走：回执
    /// steered:true 即回（不等轮跑完），账序 request → tc → tr → tc →
    /// tr → steer → done（管序=账序）。工具用两个不同的未知名——
    /// 账面照样 call/result 成对，且不触发同名工具连败护栏。
    #[test]
    fn steer_lands_at_tool_step_boundary() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-steer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let addr = stub_brain(vec![
            now(oai("我查", Some(r#"[{"id":"c1","type":"function","function":{"name":"no_such_tool_a","arguments":"{}"}}]"#), "tool_calls")),
            later(1000, oai("再查", Some(r#"[{"id":"c2","type":"function","function":{"name":"no_such_tool_b","arguments":"{}"}}]"#), "tool_calls")),
            now(oai("一轮完成", None, "stop")),
        ]);
        let (desk, mother) = boot_host(&dir, &addr);
        let r = handle_line(&desk, &mother, r#"{"op":"create","avatar":"小满"}"#);
        assert_eq!(r["ok"], json!(true));
        let mother = std::sync::Arc::new(mother);

        let (d2, m2) = (desk.clone(), mother.clone());
        let t1 = std::thread::spawn(move || {
            handle_line(&d2, &m2, r#"{"op":"send","avatar":"小满","text":"报个状态"}"#)
        });

        // 0.3s：首轮工具步已过（drain 空手），第二响应还在路上——
        // 这条 send 不排队，进队等下一个边界
        std::thread::sleep(Duration::from_millis(300));
        let r = handle_line(&desk, &mother, r#"{"op":"send","avatar":"小满","text":"改成上海"}"#);
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["steered"], json!(true));
        assert_eq!(r["data"]["turn"], json!(1));
        assert_eq!(r["data"]["avatar"], json!("小满"));

        let r1 = t1.join().unwrap();
        assert_eq!(r1["data"]["text"], json!("一轮完成"));

        // 账序：request → tc c1 → tr c1 → tc c2 → tr c2 → steer → done
        let frames = read_frames(&agent_log(&dir, "小满"));
        assert_eq!(frames.len(), 7, "frames: {frames:?}");
        assert!(matches!(&frames[0], agi::Frame::Request(r) if r.text == "报个状态" && r.turn == 1));
        assert!(matches!(&frames[1], agi::Frame::ToolCall(c) if c.id == "c1"));
        assert!(matches!(&frames[2], agi::Frame::ToolResult(tr) if tr.id == "c1"));
        assert!(matches!(&frames[3], agi::Frame::ToolCall(c) if c.id == "c2"));
        assert!(matches!(&frames[4], agi::Frame::ToolResult(tr) if tr.id == "c2"));
        assert!(matches!(&frames[5], agi::Frame::Steer(s) if s.text == "改成上海"));
        assert!(matches!(&frames[6], agi::Frame::Done(d) if d.ok && d.turn == Some(1)));
    }

    /// ② 插不进就重分类：轮无工具步边界、先收口了（首响应拖 1s、直答），
    /// 排队的 steer 拿 TurnEnded 后回头抢锁，那句话成为下一回合——
    /// 不丢话，账上无 steer 帧。
    #[test]
    fn steer_missed_turn_becomes_next_turn() {
        let dir = std::env::temp_dir().join(format!("aginx-server-test-ops-reclass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let addr = stub_brain(vec![
            later(1000, oai("答", None, "stop")),
            now(oai("答", None, "stop")),
        ]);
        let (desk, mother) = boot_host(&dir, &addr);
        let r = handle_line(&desk, &mother, r#"{"op":"create","avatar":"小满"}"#);
        assert_eq!(r["ok"], json!(true));
        let mother = std::sync::Arc::new(mother);

        let (d2, m2) = (desk.clone(), mother.clone());
        let t1 = std::thread::spawn(move || {
            handle_line(&d2, &m2, r#"{"op":"send","avatar":"小满","text":"报个状态"}"#)
        });

        std::thread::sleep(Duration::from_millis(300));
        let started = Instant::now();
        let r = handle_line(&desk, &mother, r#"{"op":"send","avatar":"小满","text":"改成上海"}"#);
        // 不是 steered：是它自己跑完的一轮
        assert_eq!(r["ok"], json!(true), "reclassified turn envelope: {r}");
        assert!(r["data"]["steered"].is_null());
        assert_eq!(r["data"]["text"], json!("答"));
        // 排队等到了第一轮收口才开跑（TurnEnded → 重分类 → 抢锁）
        assert!(started.elapsed() >= Duration::from_millis(650));

        let r1 = t1.join().unwrap();
        assert_eq!(r1["data"]["text"], json!("答"));

        // 账上两轮各自收口，全程没有 steer 帧
        let frames = read_frames(&agent_log(&dir, "小满"));
        assert_eq!(frames.len(), 4);
        assert!(matches!(&frames[0], agi::Frame::Request(r) if r.text == "报个状态" && r.turn == 1));
        assert!(matches!(&frames[1], agi::Frame::Done(d) if d.turn == Some(1)));
        assert!(matches!(&frames[2], agi::Frame::Request(r) if r.text == "改成上海" && r.turn == 2));
        assert!(matches!(&frames[3], agi::Frame::Done(d) if d.turn == Some(2)));
        assert!(!frames.iter().any(|f| matches!(f, agi::Frame::Steer(_))));
    }
}
