// avatar — 化身文件夹的读面（D5 冷=盘上文件夹）。
//
// runtime 对化身文件夹只读：定义层（SOUL.md = 系统提示）与运行层
// （sessions/{id}.jsonl = D8 会话日志）。追加永远归 server——账本只有
// 一个记账人；冷启动 = 把帧账重放成 Message 列表，这就是「冷启动即
// 恢复」的全部机制。
//
// 日志形状就是 agi::Frame 每行一帧（server 落它经手的所有线上帧）：
//   request/steer → user 轮；tool_call → assistant 调用；
//   tool_result → tool 轮；done(ok, text) → assistant 定稿；
//   artifact / done(err) 记账但不进模型上下文（给审计，不给 brain）。
//
// 重放完跑一遍 repair：崩溃在工具轮中间的会话（有 tool_call 无
// tool_result）补合成结果，孤儿 tool_result（配对的调用不在）剔除——
// OpenAI 形状的 brain 见到悬空调用会拒单。

use crate::message::{args_to_wire_string, Message, MsgToolCall, Role};
use agi::Frame;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// 系统提示：SOUL.md 有则用之（定义层），否则给化身一条素底。
pub fn system_prompt(workspace: &Path, avatar: &str) -> String {
    if let Ok(soul) = std::fs::read_to_string(workspace.join("SOUL.md")) {
        let soul = soul.trim();
        if !soul.is_empty() {
            return soul.to_string();
        }
    }
    format!(
        "你是 AginxOS 上的化身「{avatar}」，一部以 Agent 为用户的手机操作系统里的常驻智能体。\
用简体中文回复，简洁直接。需要动用系统能力时通过工具调用（tool_use）发起，\
不要把调用过程当作文本讲给用户。"
    )
}

/// 会话日志 → Message 列表（冷恢复）。文件不存在 = 新会话，空列表。
pub fn replay_session(log_path: &Path) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut pending_calls: Vec<MsgToolCall> = Vec::new();
    let file = match File::open(log_path) {
        Ok(f) => f,
        Err(_) => return messages,
    };
    let flush =
        |messages: &mut Vec<Message>, pending: &mut Vec<MsgToolCall>| {
            if !pending.is_empty() {
                let calls = std::mem::take(pending);
                messages.push(Message::assistant_tools("", calls));
            }
        };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(frame) = serde_json::from_str::<Frame>(line.trim()) else { continue };
        match frame {
            Frame::Request(r) => {
                flush(&mut messages, &mut pending_calls);
                messages.push(Message::user(r.text));
            }
            Frame::Steer(s) => {
                flush(&mut messages, &mut pending_calls);
                messages.push(Message::user(s.text));
            }
            Frame::ToolCall(c) => {
                pending_calls.push(MsgToolCall {
                    id: c.id,
                    name: c.tool,
                    arguments: args_to_wire_string(&c.args),
                });
            }
            Frame::ToolResult(r) => {
                // tool 轮紧跟调用轮：pending 还开着就先落盘
                flush(&mut messages, &mut pending_calls);
                messages.push(Message::tool_result(&r.id, result_content(&r)));
            }
            Frame::Done(d) => {
                flush(&mut messages, &mut pending_calls);
                if d.ok && !d.text.is_empty() {
                    messages.push(Message::assistant_text(d.text));
                }
            }
            Frame::Artifact(_) => {} // 投影产物：账上有，模型上下文里没有
        }
    }
    flush(&mut messages, &mut pending_calls);
    repair(&mut messages);
    messages
}

/// 工具结果回填 brain 的内容形状：成功给 stdout，失败给 stderr+退出码。
/// 冷恢复重放与实时回账共用这一个折法。
pub(crate) fn result_content(r: &agi::ToolResult) -> String {
    if r.ok {
        if r.out.is_empty() {
            "(no output)".to_string()
        } else {
            r.out.clone()
        }
    } else {
        let mut s = format!("error (exit {})", r.code);
        if !r.err.is_empty() {
            s.push_str(": ");
            s.push_str(&r.err);
        }
        if !r.out.is_empty() {
            s.push_str("\n");
            s.push_str(&r.out);
        }
        s
    }
}

/// 崩溃自愈：悬空 tool_call 补合成结果；孤儿 tool_result 剔除。
fn repair(messages: &mut Vec<Message>) {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut open_ids: Vec<String> = Vec::new();
    let mut pending_assistant: Option<Message> = None;
    let close_group = |out: &mut Vec<Message>, open: &mut Vec<String>| {
        for id in open.drain(..) {
            out.push(Message::tool_result(&id, "(会话在此中断，未收到工具结果)"));
        }
    };
    for m in messages.drain(..) {
        match m.role {
            Role::Assistant if !m.tool_calls.is_empty() => {
                close_group(&mut out, &mut open_ids);
                if let Some(a) = pending_assistant.take() {
                    out.push(a);
                }
                open_ids = m.tool_calls.iter().map(|c| c.id.clone()).collect();
                pending_assistant = Some(m);
            }
            Role::Tool => {
                let Some(id) = m.tool_call_id.clone() else { continue };
                if open_ids.iter().any(|o| *o == id) {
                    open_ids.retain(|o| *o != id);
                    if let Some(a) = pending_assistant.take() {
                        out.push(a);
                    }
                    out.push(m);
                } // 孤儿结果：配对调用不在，丢弃
            }
            _ => {
                close_group(&mut out, &mut open_ids);
                if let Some(a) = pending_assistant.take() {
                    out.push(a);
                }
                out.push(m);
            }
        }
    }
    if let Some(a) = pending_assistant.take() {
        out.push(a);
    }
    close_group(&mut out, &mut open_ids);
    *messages = out;
}

#[cfg(test)]
mod tests {
    use super::*;
    use agi::{Done, Request, ToolCall, ToolResult};
    use serde_json::json;

    fn log(dir: &Path, frames: &[Frame]) -> std::path::PathBuf {
        let p = dir.join("s.jsonl");
        let mut s = String::new();
        for f in frames {
            s.push_str(&agi::encode(f));
        }
        std::fs::write(&p, s).unwrap();
        p
    }

    #[test]
    fn replay_full_turn() {
        let dir = std::env::temp_dir().join("aginx-runtime-test-avatar-full");
        std::fs::create_dir_all(&dir).unwrap();
        let p = log(
            &dir,
            &[
                Frame::Request(Request { avatar: "小满".into(), session: "s".into(), text: "打个招呼".into() }),
                Frame::ToolCall(ToolCall { id: "c1".into(), tool: "dev-hello".into(), args: json!(["世界"]) }),
                Frame::ToolResult(ToolResult { id: "c1".into(), ok: true, code: 0, out: "hello 世界".into(), err: String::new() }),
                Frame::Done(Done::ok("你好，世界")),
            ],
        );
        let m = replay_session(&p);
        assert_eq!(m.len(), 4);
        assert_eq!(m[0].role, Role::User);
        assert_eq!(m[1].tool_calls[0].name, "dev-hello");
        assert_eq!(m[1].tool_calls[0].arguments, r#"["世界"]"#);
        assert_eq!(m[2].tool_call_id.as_deref(), Some("c1"));
        assert_eq!(m[2].content, "hello 世界");
        assert_eq!(m[3].content, "你好，世界");
    }

    #[test]
    fn replay_failed_tool_result_and_error_done() {
        let dir = std::env::temp_dir().join("aginx-runtime-test-avatar-fail");
        std::fs::create_dir_all(&dir).unwrap();
        let p = log(
            &dir,
            &[
                Frame::Request(Request { avatar: "a".into(), session: "s".into(), text: "x".into() }),
                Frame::ToolCall(ToolCall { id: "c1".into(), tool: "nope".into(), args: json!([]) }),
                Frame::ToolResult(ToolResult { id: "c1".into(), ok: false, code: 127, out: String::new(), err: "unknown command".into() }),
                Frame::Done(Done::err("brain", "dial timeout")),
            ],
        );
        let m = replay_session(&p);
        // 失败 done 不进上下文：只剩 user + assistant调用 + tool 结果
        assert_eq!(m.len(), 3);
        assert!(m[2].content.starts_with("error (exit 127)"));
        assert!(m[2].content.contains("unknown command"));
    }

    #[test]
    fn replay_repairs_dangling_tool_call() {
        let dir = std::env::temp_dir().join("aginx-runtime-test-avatar-dangling");
        std::fs::create_dir_all(&dir).unwrap();
        // 崩在工具轮中间：有 tool_call 无 tool_result
        let p = log(
            &dir,
            &[
                Frame::Request(Request { avatar: "a".into(), session: "s".into(), text: "查一下".into() }),
                Frame::ToolCall(ToolCall { id: "c1".into(), tool: "web-search".into(), args: json!(["北京"]) }),
            ],
        );
        let m = replay_session(&p);
        assert_eq!(m.len(), 3);
        assert_eq!(m[2].role, Role::Tool);
        assert_eq!(m[2].tool_call_id.as_deref(), Some("c1"));
        assert!(m[2].content.contains("中断"));
    }

    #[test]
    fn replay_drops_orphan_tool_result() {
        let dir = std::env::temp_dir().join("aginx-runtime-test-avatar-orphan");
        std::fs::create_dir_all(&dir).unwrap();
        let p = log(
            &dir,
            &[
                Frame::Request(Request { avatar: "a".into(), session: "s".into(), text: "x".into() }),
                Frame::ToolResult(ToolResult { id: "ghost".into(), ok: true, code: 0, out: "?".into(), err: String::new() }),
            ],
        );
        let m = replay_session(&p);
        assert_eq!(m.len(), 1); // 只剩 user
    }

    #[test]
    fn replay_steer_is_user_turn_and_artifact_skipped() {
        let dir = std::env::temp_dir().join("aginx-runtime-test-avatar-steer");
        std::fs::create_dir_all(&dir).unwrap();
        let p = log(
            &dir,
            &[
                Frame::Request(Request { avatar: "a".into(), session: "s".into(), text: "北京天气".into() }),
                Frame::Artifact(agi::Artifact { kind: agi::ArtifactKind::Text, data: "正在查".into() }),
                Frame::Steer(agi::Steer { text: "改成上海".into() }),
            ],
        );
        let m = replay_session(&p);
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].content, "改成上海");
        assert_eq!(m[1].role, Role::User);
    }

    #[test]
    fn soul_md_overrides_default_prompt() {
        let dir = std::env::temp_dir().join("aginx-runtime-test-avatar-soul");
        // 临时目录跨次运行留残（上回写进的 SOUL.md），先清场再验
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(system_prompt(&dir, "小满").contains("小满"));
        std::fs::write(dir.join("SOUL.md"), "你是测试化身，只会说测试话。\n").unwrap();
        assert_eq!(system_prompt(&dir, "小满"), "你是测试化身，只会说测试话。");
    }
}
