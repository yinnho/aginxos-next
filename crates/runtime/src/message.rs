// message — 会话消息模型（OpenAI 形状的最小集）。
//
// carrier 的 Message/ContentBlock 树有媒体/思考/提供者元数据三层，本仓
// runtime 只吃文本轮：content 是纯文本字符串，tool_calls 挂 assistant，
// tool_call_id 挂 tool 角色。wire 映射见 brain.rs 的 build_oai_request。
//
// 与 D8 的关系：这个模型是「发给 brain 的形状」；会话日志（sessions/
// {id}.jsonl）的真源形状是 agi::Frame，avatar::replay_session 负责把
// 帧账重放成这里的 Message 列表（冷启动���恢复）。

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// assistant 消息上挂的一次工具调用。arguments 是 JSON 字符串
/// （OpenAI 惯例：wire 上 arguments 永远是 string）。
#[derive(Debug, Clone, PartialEq)]
pub struct MsgToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// 仅 assistant：本轮发起的工具调用。
    pub tool_calls: Vec<MsgToolCall>,
    /// 仅 tool：回应的 tool_call.id。
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Message {
        Message { role: Role::User, content: text.into(), tool_calls: Vec::new(), tool_call_id: None }
    }

    pub fn system(text: impl Into<String>) -> Message {
        Message { role: Role::System, content: text.into(), tool_calls: Vec::new(), tool_call_id: None }
    }

    pub fn assistant_text(text: impl Into<String>) -> Message {
        Message {
            role: Role::Assistant,
            content: text.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// assistant 工具调用轮：content 可为空（模型只发起调用不说话）。
    pub fn assistant_tools(text: impl Into<String>, calls: Vec<MsgToolCall>) -> Message {
        Message { role: Role::Assistant, content: text.into(), tool_calls: calls, tool_call_id: None }
    }

    pub fn tool_result(id: &str, content: impl Into<String>) -> Message {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.to_string()),
        }
    }
}

/// 为什么停：决定 DISPATCH 走 end_turn / tool_use / max_tokens 哪条支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

/// 把 agi 帧上的 args（数组=argv / 对象=旗标）编回 OpenAI 的
/// arguments JSON 字符串。重放与实时路径共用。
/// 帧上的 args → wire 上 function.arguments 的 JSON 文本。provider 要求
/// object：数组/标量按 argv 约定包成 {"args": …}，对象原样（--key value
/// 旗标对），null 给空对象。账本保持模型原形，只有 wire 收敛成这一种。
pub fn args_to_wire_string(args: &Value) -> String {
    let wire = match args {
        Value::Object(_) => args.clone(),
        Value::Array(a) => serde_json::json!({"args": a}),
        Value::Null => serde_json::json!({}),
        other => serde_json::json!({"args": [other]}),
    };
    serde_json::to_string(&wire).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_shape() {
        let m = Message::user("你好");
        assert_eq!(m.role, Role::User);
        assert!(m.tool_calls.is_empty() && m.tool_call_id.is_none());

        let a = Message::assistant_tools(
            "",
            vec![MsgToolCall { id: "c1".into(), name: "dev-hello".into(), arguments: "[\"x\"]".into() }],
        );
        assert_eq!(a.role, Role::Assistant);
        assert_eq!(a.tool_calls.len(), 1);

        let t = Message::tool_result("c1", "ok");
        assert_eq!(t.tool_call_id.as_deref(), Some("c1"));
        assert_eq!(t.role, Role::Tool);
    }

    #[test]
    fn args_wire_string_roundtrip() {
        let v = serde_json::json!(["北京", "天气"]);
        let s = args_to_wire_string(&v);
        // 数组按 argv 约定包 {"args": …}——provider 只认 object
        assert_eq!(s, r#"{"args":["北京","天气"]}"#);
        assert_eq!(args_to_wire_string(&serde_json::json!({"b": 1})), r#"{"b":1}"#);
        assert_eq!(args_to_wire_string(&serde_json::Value::Null), "{}");
        assert_eq!(args_to_wire_string(&serde_json::json!(42)), r#"{"args":[42]}"#);
        // wire 形状自己能往返（对象无损、数组有壳）
        let back: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back, serde_json::json!({"args": v}));
    }
}
