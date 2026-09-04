// ops — UDS 面的操作层：一行 JSON 请求 → 一行 D1 信封响应。
//
// 客户端（`aginx agent …`，路由器内置）与 server 之间的一问一答协议：
//   → {"op":"send","avatar":"小满"?,"text":"你好"}
//   → {"op":"status"} | {"op":"list"} | {"op":"create","avatar":"…","soul":"…"?}
//   ← agio 信封（ok/data/error），单行，连接即关。
// send 不带 avatar = 住（当前光标）；退房词在 front 层裁决。

use crate::front::{FrontDesk, SendTarget, MOTHER};
use crate::ServerCfg;
use serde_json::{json, Value};

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

    // 前台一次一轮：排队等（后来的连线阻塞在这里，语音/CLI 一视同仁）
    let _turn = desk.turn_lock();

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
    match target {
        SendTarget::Checkout => agio::ok(json!({
            "avatar": MOTHER, "text": "（已回到母体）", "checkout": true,
        })),
        SendTarget::Mother => match crate::mother::mother_reply(&text, &desk.roster()) {
            Ok(reply) => agio::ok(json!({"avatar": MOTHER, "text": reply})),
            Err(e) => agio::fail(agio::ErrorType::State, "brain", &format!("母体 brain 调用失败：{e}")),
        },
        SendTarget::Avatar(name) => {
            let done = crate::turn::run_avatar_turn(cfg, &name, &text);
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
}
