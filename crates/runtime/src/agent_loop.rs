// agent_loop — 轮状态机（carrier agent_loop 的设计搬入，两处手术的
// 落点都在这层）：
//
//   INIT → [PREPARE_TURN → LLM_CALL → DISPATCH]* → done
//
// 手术一：KernelHandle / ToolModule 链 → fast-agi 帧。工具不在本进程
// 执行：tool_call 帧发去 server（母体 spawn aginx-<tool>，D12），结果
// 以 tool_result 帧回来，折成 OpenAI tool 消息继续下一轮。
// 手术二：会话持久化没了。server 拥有 D8 账本；本函数收「重放好的
// 历史 + 本轮文本」，吐 done 终帧，中途状态一概不落盘。
//
// 搬入的实战纪律（老仓每条都有收据）：
// - 无硬性轮数上限：跑到自然收束或卡死检测（无进展/工具循环）。
// - 文本工具叙述恢复：模型把调用写成 "[Called x]" 文本 → 引导重试，
//   两次后放最后一次「纯自然语言」机会，仍叙述则回退话术，绝不把
//   叙述原文发给用户（08-21 86bus 教训：造 assistant 先例会被复读）。
// - 引导消息只进 system 位，不造 assistant 文本先例。
// - MaxTokens 续写：压 assistant 半成品 + user「继续」，连挂 5 次封顶
//   拿现有文本收尾。

use crate::brain::{BrainDriver, CompletionRequest, CompletionResponse};
use crate::loop_state::*;
use crate::message::{Message, MsgToolCall, Role, StopReason};
use crate::tools::ToolDef;
use crate::transport::TurnTransport;
use agi::{Done, Frame, ToolCall as WireToolCall, ToolResult as WireToolResult};
use serde_json::Value;
use std::collections::HashMap;
use std::io;

/// 文本工具叙述恢复的次数上限，之后给最后一次无工具机会。
pub const MAX_TEXT_RECOVERY_RETRIES: u32 = 2;
/// MaxTokens 连挂上限：续写五次还到不了尾就拿现有文本收尾。
pub const MAX_CONTINUATIONS: u32 = 5;

#[derive(Debug, Clone)]
pub struct TurnConfig {
    pub context_window: usize,
    pub max_output_tokens: u32,
    pub temperature: f32,
}

impl Default for TurnConfig {
    fn default() -> Self {
        TurnConfig {
            context_window: DEFAULT_CONTEXT_WINDOW,
            max_output_tokens: 4096,
            temperature: 0.7,
        }
    }
}

/// 跑一轮。所有出口（含错误）都以 done 终帧收尾，返回进程退出码
/// （0=ok，1=err）；唯一能穿透上来的是写帧本身的 io 失败。
pub async fn run_turn<B: BrainDriver, T: TurnTransport>(
    brain: &B,
    tools: &[ToolDef],
    system: &str,
    history: Vec<Message>,
    user_text: &str,
    io: &mut T,
    cfg: &TurnConfig,
) -> io::Result<i32> {
    let mut messages = history;
    // 空串 = 本轮文本已在重放历史里（server 先记账再 spawn），不叠份
    if !user_text.is_empty() {
        messages.push(Message::user(user_text));
    }
    let mut state = LoopState::new(cfg.context_window);

    loop {
        // ---- PREPARE_TURN ----
        prepare_turn(&mut messages, &mut state);

        // ---- LLM_CALL ----
        let request = CompletionRequest {
            model: String::new(), // 模型归 brain 端配置（HttpBrain 覆写）
            messages: messages.clone(),
            tools: tools.to_vec(),
            max_tokens: cfg.max_output_tokens,
            temperature: cfg.temperature,
            system: Some(system.to_string()),
        };
        let response = match brain.complete(request).await {
            Ok(r) => r,
            Err(e) => return finish(io, Done::err("brain", e.to_string())),
        };
        state.total_input_tokens += response.usage.input_tokens;
        state.total_output_tokens += response.usage.output_tokens;
        state.context_tokens_used_estimate = response.usage.input_tokens as usize;
        state.context_pressure =
            ContextPressure::from_usage_pct(state.context_usage_pct());

        // ---- 文本工具叙述恢复 ----
        if response.stop_reason == StopReason::EndTurn && response.tool_calls.is_empty() {
            match handle_narration(&response.text, &mut state, &mut messages) {
                Recovery::Proceed => {}
                Recovery::Retry => {
                    state.idle_streak = 0; // 主动恢复算进展
                    state.iteration += 1;
                    continue;
                }
                Recovery::Fallback => {
                    return finish(io, Done::ok(NARRATION_FALLBACK_REPLY));
                }
            }
        }

        // ---- DISPATCH ----
        let stop = response.stop_reason;
        let made_progress = match stop {
            StopReason::EndTurn => {
                // 收束：定稿文本（剥叙述残留）→ done。
                let text = strip_tool_call_artifacts(&response.text);
                let silent = is_silent(&text);
                messages.push(Message::assistant_text(text.clone()));
                let final_text = if silent { String::new() } else { text };
                return finish(io, Done::ok(final_text));
            }
            StopReason::ToolUse => {
                match dispatch_tool_use(&response, &mut messages, &mut state, io).await? {
                    ToolUseOutcome::Finished(code) => return Ok(code),
                    ToolUseOutcome::Continued => state.tools_this_iter > 0,
                }
            }
            StopReason::MaxTokens => {
                state.consecutive_max_tokens += 1;
                if state.consecutive_max_tokens > MAX_CONTINUATIONS {
                    // 续写五次还到不了尾：拿现有文本收尾，别无限烧
                    return finish(io, Done::ok(response.text));
                }
                if !response.text.is_empty() {
                    messages.push(Message::assistant_text(response.text.clone()));
                }
                messages.push(Message::user("继续输出，从上次中断处接着写，不要重复已有内容。"));
                true // 生成中算进展
            }
        };

        state.iteration += 1;
        if let Some(streak) = state.record_iteration_progress(made_progress) {
            return finish(
                io,
                Done::err(
                    "loop_stuck",
                    format!("agent 连续 {streak} 轮无进展（无成功工具调用、无最终答案），判定卡死，终止本轮"),
                ),
            );
        }
    }
}

fn finish<T: TurnTransport>(io: &mut T, done: Done) -> io::Result<i32> {
    io.send(&Frame::Done(done.clone()))?;
    Ok(if done.ok { 0 } else { 1 })
}

// ---------------------------------------------------------------------------
// PREPARE_TURN
// ---------------------------------------------------------------------------

fn prepare_turn(messages: &mut Vec<Message>, state: &mut LoopState) {
    // 历史裁剪：超了从最前头成对裁（保 user/assistant 轮界）
    if messages.len() > MAX_HISTORY_MESSAGES {
        let excess = messages.len() - MAX_HISTORY_MESSAGES;
        let drain = if excess % 2 == 0 { excess } else { excess + 1 };
        messages.drain(..drain.min(messages.len()));
    }
    // 上下文护栏：超长工具结果就地截断（CLI 倾倒巨量输出是上下文炸弹）
    for m in messages.iter_mut() {
        if m.role == Role::Tool && m.content.chars().count() > TOOL_RESULT_MAX_CHARS {
            let kept: String = m.content.chars().take(TOOL_RESULT_MAX_CHARS).collect();
            m.content = format!("{kept}…[已截断]");
        }
    }
    // 状态注入：模型每轮都看得见全局（上一条已是状态就不重复压）
    let should = !messages
        .last()
        .is_some_and(|m| m.role == Role::System && m.content.starts_with("📊 Turn"));
    if should {
        messages.push(Message::system(state.build_status_message()));
    }
}

// ---------------------------------------------------------------------------
// DISPATCH: ToolUse
// ---------------------------------------------------------------------------

/// dispatch 的出口：Continued = 继续循环；Finished = done 已发，就此收工
/// （循环检测/对端断流在工具轮中途杀掉了本轮）。
enum ToolUseOutcome {
    Continued,
    Finished(i32),
}

async fn dispatch_tool_use<T: TurnTransport>(
    response: &CompletionResponse,
    messages: &mut Vec<Message>,
    state: &mut LoopState,
    io: &mut T,
) -> io::Result<ToolUseOutcome> {
    let calls = &response.tool_calls;
    // assistant 调用轮先进历史（含模型随调用的旁白）
    messages.push(Message::assistant_tools(
        response.text.clone(),
        calls
            .iter()
            .map(|c| MsgToolCall {
                id: c.id.clone(),
                name: c.name.clone(),
                arguments: serde_json::to_string(&c.input).unwrap_or_else(|_| "{}".into()),
            })
            .collect(),
    ));

    // 循环检测 + 发帧。检测在执行前计：被拒/失败的调用也计数——锤一个
    // 不存在的命令正是该断的循环。
    for tc in calls {
        let key = (tc.name.clone(), tool_input_hash(&tc.input));
        state.recent_tool_calls.push(key.clone());
        let recent_len = state.recent_tool_calls.len();
        if recent_len > 32 {
            state.recent_tool_calls.drain(..recent_len - 32);
        }
        if let Some((name, _)) = detect_tool_loop(&state.recent_tool_calls, LOOP_DETECTION_WINDOW) {
            let msg = format!(
                "工具 `{name}` 连续 {LOOP_DETECTION_WINDOW} 次同参调用，判定卡死，终止本轮"
            );
            let code = finish(io, Done::err("loop_stuck", msg))?;
            return Ok(ToolUseOutcome::Finished(code));
        }
        let n = state.tool_call_counts.entry(key).or_insert(0);
        *n += 1;
        if *n >= CUMULATIVE_BREAK_AT {
            let msg = format!(
                "工具 `{}` 同参数调用累计 {} 次（两次提醒后仍在重复），判定轮换式循环，终止本轮",
                tc.name, CUMULATIVE_BREAK_AT
            );
            let code = finish(io, Done::err("loop_stuck", msg))?;
            return Ok(ToolUseOutcome::Finished(code));
        }
        if *n == CUMULATIVE_ESCALATE_AT {
            messages.push(Message::system(format!(
                "⚠️ 你已经用完全相同的参数调用 `{}` {} 次了。同样的调用不会得到不同结果。\
换一种做法，或基于已有信息给出结论。",
                tc.name, CUMULATIVE_ESCALATE_AT
            )));
        } else if *n == CUMULATIVE_REMIND_AT {
            messages.push(Message::system(format!(
                "💡 工具 `{}` 已用相同参数调用 {} 次（参数：{}）。确认这不是重复操作；\
如是分页/批量请改变参数，否则别再调它。",
                tc.name,
                CUMULATIVE_REMIND_AT,
                tool_args_preview(&tc.input, 80)
            )));
        }
        io.send(&Frame::ToolCall(WireToolCall {
            id: tc.id.clone(),
            tool: tc.name.clone(),
            args: normalize_frame_args(&tc.input),
        }))?;
    }

    // 等回账：全部 id 收齐才继续。steer 缓存到下一轮注入。
    let mut results: HashMap<String, WireToolResult> = HashMap::new();
    let mut steer_queue: Vec<String> = Vec::new();
    loop {
        if results.len() == calls.len() {
            break;
        }
        match io.recv() {
            Ok(Some(Frame::ToolResult(r))) => {
                if calls.iter().any(|c| c.id == r.id) && !results.contains_key(&r.id) {
                    results.insert(r.id.clone(), r);
                } // 未知 id / 重复回账：忽略
            }
            Ok(Some(Frame::Steer(s))) => steer_queue.push(s.text),
            Ok(Some(_)) => {} // 杂帧：v0 不认，吞掉
            Ok(None) => {
                let code = finish(
                    io,
                    Done::err("protocol", "server closed the stream before tool results arrived"),
                )?;
                return Ok(ToolUseOutcome::Finished(code));
            }
            Err(e) => {
                let code = finish(io, Done::err("protocol", e.to_string()))?;
                return Ok(ToolUseOutcome::Finished(code));
            }
        }
    }

    // 回账折成 tool 消息（形状与冷恢复重放共用一个折法）
    for tc in calls {
        let r = &results[&tc.id];
        state.error_tracker.record(&tc.name, r.ok);
        if r.ok {
            state.tools_this_iter += 1;
            state.any_tools_executed = true;
        }
        messages.push(Message::tool_result(&tc.id, crate::avatar::result_content(r)));
    }
    for s in steer_queue {
        messages.push(Message::user(s));
    }
    state.consecutive_max_tokens = 0;
    Ok(ToolUseOutcome::Continued)
}

/// 模型工具入参 → 帧上的 args 约定：argv schema 下模型给的是
/// {"args": [...]}，独键且全字符串数组就解成数组原样传；其余形状
/// （对象=旗标）原样透传。
fn normalize_frame_args(input: &Value) -> Value {
    if let Value::Object(m) = input {
        if m.len() == 1 {
            if let Some(Value::Array(a)) = m.get("args") {
                if a.iter().all(|v| v.is_string()) {
                    return Value::Array(a.clone());
                }
            }
        }
    }
    input.clone()
}

// ---------------------------------------------------------------------------
// 文本工具叙述恢复（carrier text_tool_recovery 搬入）
// ---------------------------------------------------------------------------

const NARRATION_MARKERS: &[&str] = &["[Called ", "[调用 ", "[执行 "];

pub const NARRATION_FALLBACK_REPLY: &str =
    "抱歉，这轮我想调用的工具一直没能正确执行，请稍后重发一次消息，或换个说法告诉我。";

enum Recovery {
    /// 正常收束，往下走 DISPATCH
    Proceed,
    /// 已注入引导，重跑本轮
    Retry,
    /// 恢复放弃：用回退话术直接收尾
    Fallback,
}

fn handle_narration(text: &str, state: &mut LoopState, messages: &mut Vec<Message>) -> Recovery {
    let mentions = detect_text_tool_mentions(text);
    if mentions.is_empty() {
        return Recovery::Proceed;
    }
    if state.text_recovery_final {
        // 最后一次无工具机会仍在叙述——绝不把叙述原文发给用户
        return Recovery::Fallback;
    }
    if state.text_recovery_retries >= MAX_TEXT_RECOVERY_RETRIES {
        state.text_recovery_final = true;
        messages.push(Message::system(
            "多次尝试后你仍用文本描述工具调用而非结构化 tool_use。本轮不要再调用任何工具，\
直接用自然语言回复用户；禁止输出 [Called ...]、[调用 ...] 或『我需要调用工具：…』这类文本。",
        ));
        return Recovery::Retry;
    }
    state.text_recovery_retries += 1;
    // 08-21 86bus 教训：引导只进 system 位，不造 assistant 文本先例——
    // 模型放弃工具后会逐字复读注入的先例当最终答案。
    messages.push(Message::system(format!(
        "你刚才把工具调用（{}）写成了文本，用户会直接看到这段原始文本。\
这些工具已在你的可用工具列表中，请直接用 tool_use 发起结构化调用并带上完整参数。\
禁止输出 [Called ...]、[调用 ...] 或『我需要调用工具：…』这类文本。",
        mentions.join("、")
    )));
    Recovery::Retry
}

/// 模型在正文里叙述的工具调用：`[Called x]` / `[调用 x]` / `[执行 x]`。
/// 去重、按首见顺序返回名字。
pub fn detect_text_tool_mentions(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for marker in NARRATION_MARKERS {
        let mut search_from = 0;
        while let Some(pos) = text[search_from..].find(marker) {
            let abs = search_from + pos;
            let after = &text[abs + marker.len()..];
            match after.find(']') {
                Some(close) => {
                    let name = after[..close].trim().trim_matches(',').to_string();
                    if !name.is_empty() && seen.insert(name.clone()) {
                        out.push(name);
                    }
                    search_from = abs + marker.len() + close + 1;
                }
                None => break,
            }
        }
    }
    out
}

/// 从定稿文本里剥掉叙述残留，用户不该看到调用语法。
pub fn strip_tool_call_artifacts(text: &str) -> String {
    let mut result = text.to_string();
    for marker in NARRATION_MARKERS {
        let mut search_from = 0;
        while let Some(pos) = result[search_from..].find(marker) {
            let abs = search_from + pos;
            let after = &result[abs + marker.len()..];
            if let Some(close) = after.find(']') {
                result.replace_range(abs..abs + marker.len() + close + 1, "");
                search_from = abs; // 文本移位后原地重扫
            } else {
                break;
            }
        }
    }
    result
}

/// 有意的沉默：`[[silent]]` 或整段就是无回复哨兵（含中文变体）。
/// 整段匹配才算——正文里顺带提到这些词不沉默。
pub fn is_silent(text: &str) -> bool {
    let t = text.trim();
    if t == "[[silent]]" {
        return true;
    }
    let inner = t
        .trim_start_matches(['[', '【'])
        .trim_end_matches([']', '】'])
        .trim()
        .to_lowercase()
        .replace('_', " ");
    matches!(
        inner.as_str(),
        "no reply needed" | "no reply" | "noreply" | "no reply required" | "无需回复" | "无需答复"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{BrainConfig, BrainError, HttpBrain};
    use crate::message::Role;
    use crate::transport::MemTransport;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    // ------------------------------------------------------------------
    // 假 brain：脚本化响应 + 请求捕获
    // ------------------------------------------------------------------

    struct FakeBrain {
        script: Mutex<VecDeque<CompletionResponse>>,
        seen: Mutex<Vec<Vec<Message>>>,
    }

    impl FakeBrain {
        fn new(script: Vec<CompletionResponse>) -> FakeBrain {
            FakeBrain {
                script: Mutex::new(script.into()),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl BrainDriver for FakeBrain {
        async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, BrainError> {
            self.seen.lock().unwrap().push(request.messages.clone());
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| BrainError::Http("script exhausted".into()))
        }
    }

    fn tool_call(id: &str, name: &str, input: Value) -> CompletionResponse {
        CompletionResponse::tool_use(vec![crate::brain::RespToolCall {
            id: id.into(),
            name: name.into(),
            input,
        }])
    }

    fn result_frame(id: &str, ok: bool, out: &str) -> Frame {
        Frame::ToolResult(WireToolResult {
            id: id.into(),
            ok,
            code: if ok { 0 } else { 1 },
            out: out.into(),
            err: if ok { String::new() } else { "boom".into() },
        })
    }

    fn dev_hello_tool() -> ToolDef {
        ToolDef::new("dev-hello", "smoke face", None)
    }

    // ------------------------------------------------------------------
    // 状态机单测
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn tool_turn_round_trip() {
        let brain = FakeBrain::new(vec![
            tool_call("c1", "dev-hello", json!({"args": ["世界"]})),
            CompletionResponse::end_turn("你好，世界"),
        ]);
        let mut io = MemTransport::default();
        io.inbox.push_back(result_frame("c1", true, "hello 世界"));
        let code = run_turn(
            &brain,
            &[dev_hello_tool()],
            "sys",
            vec![],
            "打个招呼",
            &mut io,
            &TurnConfig::default(),
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            io.sent,
            vec![
                Frame::ToolCall(WireToolCall { id: "c1".into(), tool: "dev-hello".into(), args: json!(["世界"]) }),
                Frame::Done(Done::ok("你好，世界")),
            ]
        );
        // 第二次 brain 调用里：user 问题 + assistant 调用 + tool 结果都在
        let second = &brain.seen.lock().unwrap()[1];
        assert!(second.iter().any(|m| m.role == Role::User && m.content == "打个招呼"));
        assert!(second.iter().any(|m| m.role == Role::Assistant && m.tool_calls.len() == 1));
        assert!(second.iter().any(|m| m.role == Role::Tool && m.content == "hello 世界"));
    }

    #[tokio::test]
    async fn narration_recovered_then_clean_answer() {
        let brain = FakeBrain::new(vec![
            CompletionResponse::end_turn("我需要调用工具：[Called dev-hello] 来打招呼"),
            CompletionResponse::end_turn("你好，世界"),
        ]);
        let mut io = MemTransport::default();
        let code = run_turn(&brain, &[dev_hello_tool()], "sys", vec![], "打招呼", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(io.sent, vec![Frame::Done(Done::ok("你好，世界"))]);
        // 引导消息注入了 system 位（guard 显式成块：两个 let 借用会各自
        // 持锁到函数尾，同线程二次 lock 直接死锁）
        {
            let seen = brain.seen.lock().unwrap();
            let first = &seen[0];
            assert!(first.iter().any(|m| m.role == Role::User && m.content == "打招呼"));
            let second = &seen[1];
            assert!(second.iter().any(|m| m.role == Role::System && m.content.contains("写成了文本")));
        }
    }

    #[tokio::test]
    async fn narration_fallback_after_exhausted_retries() {
        // 两次重试 + 最后一次无工具机会，全部叙述 → 回退话术
        let mut script = Vec::new();
        for _ in 0..(MAX_TEXT_RECOVERY_RETRIES + 2) {
            script.push(CompletionResponse::end_turn("[调用 dev-hello] 然后回复"));
        }
        let brain = FakeBrain::new(script);
        let mut io = MemTransport::default();
        let code = run_turn(&brain, &[dev_hello_tool()], "sys", vec![], "x", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(io.sent, vec![Frame::Done(Done::ok(NARRATION_FALLBACK_REPLY))]);
    }

    #[tokio::test]
    async fn cumulative_repeat_breaks_turn() {
        // 轮换式重复：两个输入轮着调（躲开 4 连硬循环），单个 (名,参)
        // 累计到 8 断轮——这正是累积检测存在的理由
        let mut script = VecDeque::new();
        for i in 0..20 {
            let arg = if i % 2 == 0 { "a" } else { "b" };
            script.push_back(tool_call("c1", "dev-hello", json!({"args": [arg]})));
        }
        let brain = FakeBrain {
            script: Mutex::new(script),
            seen: Mutex::new(Vec::new()),
        };
        let mut io = MemTransport::default();
        for _ in 0..20 {
            io.inbox.push_back(result_frame("c1", true, "ok"));
        }
        let code = run_turn(&brain, &[dev_hello_tool()], "sys", vec![], "x", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 1);
        let done = io.sent.last().unwrap();
        let Frame::Done(d) = done else { panic!("last frame must be done") };
        assert!(!d.ok);
        assert_eq!(d.error.as_ref().unwrap().code, "loop_stuck");
        assert!(d.error.as_ref().unwrap().message.contains("累计"));
        // 提醒与加重语气的 system 注入过
        let seen = brain.seen.lock().unwrap();
        let flat: Vec<&Message> = seen.iter().flat_map(|v| v.iter()).collect();
        assert!(flat.iter().any(|m| m.role == Role::System && m.content.contains("💡")));
        assert!(flat.iter().any(|m| m.role == Role::System && m.content.contains("⚠️")));
    }

    #[tokio::test]
    async fn all_failing_tools_trip_no_progress() {
        // 同名不同参（躲开循环检测），结果全失败 → 3 轮空转判卡死
        let mut script = VecDeque::new();
        for i in 0..10 {
            script.push_back(tool_call(&format!("c{i}"), "dev-hello", json!({"args": [i]})));
        }
        let brain = FakeBrain { script: Mutex::new(script), seen: Mutex::new(Vec::new()) };
        let mut io = MemTransport::default();
        for i in 0..10 {
            io.inbox.push_back(result_frame(&format!("c{i}"), false, ""));
        }
        let code = run_turn(&brain, &[dev_hello_tool()], "sys", vec![], "x", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 1);
        let Frame::Done(d) = io.sent.last().unwrap() else { panic!() };
        assert_eq!(d.error.as_ref().unwrap().code, "loop_stuck");
        assert!(d.error.as_ref().unwrap().message.contains("无进展"));
    }

    #[tokio::test]
    async fn max_tokens_continues_then_finishes() {
        let partial = CompletionResponse {
            text: "前半段".into(),
            thinking: String::new(),
            tool_calls: vec![],
            stop_reason: StopReason::MaxTokens,
            usage: Default::default(),
        };
        let brain = FakeBrain::new(vec![
            partial,
            CompletionResponse {
                text: "后半段".into(),
                thinking: String::new(),
                tool_calls: vec![],
                stop_reason: StopReason::MaxTokens,
                usage: Default::default(),
            },
            CompletionResponse::end_turn("整段完成"),
        ]);
        let mut io = MemTransport::default();
        let code = run_turn(&brain, &[dev_hello_tool()], "sys", vec![], "写长文", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(io.sent, vec![Frame::Done(Done::ok("整段完成"))]);
        // 第三次调用包含续写 nudges 和 assistant 半成品
        let third = &brain.seen.lock().unwrap()[2];
        assert!(third.iter().any(|m| m.role == Role::User && m.content.starts_with("继续输出")));
        assert!(third.iter().any(|m| m.role == Role::Assistant && m.content == "前半段"));
    }

    #[tokio::test]
    async fn steer_injected_during_tool_wait() {
        let brain = FakeBrain::new(vec![
            tool_call("c1", "dev-hello", json!({"args": ["a"]})),
            CompletionResponse::end_turn("按上海办"),
        ]);
        let mut io = MemTransport::default();
        io.inbox.push_back(Frame::Steer(agi::Steer { text: "改成上海".into() }));
        io.inbox.push_back(result_frame("c1", true, "ok"));
        let code = run_turn(&brain, &[dev_hello_tool()], "sys", vec![], "北京天气", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 0);
        let second = &brain.seen.lock().unwrap()[1];
        assert!(second.iter().any(|m| m.role == Role::User && m.content == "改成上海"));
    }

    #[tokio::test]
    async fn server_closed_mid_tool_is_protocol_error() {
        let brain = FakeBrain::new(vec![tool_call("c1", "dev-hello", json!({}))]);
        let mut io = MemTransport::default(); // 无回账 → EOF
        let code = run_turn(&brain, &[dev_hello_tool()], "sys", vec![], "x", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 1);
        let Frame::Done(d) = io.sent.last().unwrap() else { panic!() };
        assert_eq!(d.error.as_ref().unwrap().code, "protocol");
    }

    #[tokio::test]
    async fn brain_error_folds_into_done() {
        struct DeadBrain;
        #[async_trait]
        impl BrainDriver for DeadBrain {
            async fn complete(&self, _r: CompletionRequest) -> Result<CompletionResponse, BrainError> {
                Err(BrainError::Http("dial tcp: refused".into()))
            }
        }
        let mut io = MemTransport::default();
        let code = run_turn(&DeadBrain, &[], "sys", vec![], "x", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 1);
        let Frame::Done(d) = &io.sent[0] else { panic!() };
        assert_eq!(d.error.as_ref().unwrap().code, "brain");
        assert!(d.error.as_ref().unwrap().message.contains("refused"));
    }

    #[tokio::test]
    async fn history_trims_and_giant_tool_results_guarded() {
        // 40 条历史 + 500 字符超限工具结果：裁到 30 内、工具结果截断
        let mut history: Vec<Message> = Vec::new();
        for i in 0..20 {
            history.push(Message::user(format!("q{i}")));
            history.push(Message::assistant_text(format!("a{i}")));
        }
        history.push(Message::tool_result("ghost", "x".repeat(50_000)));
        let brain = FakeBrain::new(vec![CompletionResponse::end_turn("ok")]);
        let mut io = MemTransport::default();
        let code = run_turn(&brain, &[], "sys", history, "新问题", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 0);
        let seen = brain.seen.lock().unwrap();
        let first = &seen[0];
        assert!(first.len() <= MAX_HISTORY_MESSAGES + 3, "trim + status + user 应有界: {}", first.len());
        let giant = first.iter().find(|m| m.role == Role::Tool).unwrap();
        assert!(giant.content.chars().count() <= TOOL_RESULT_MAX_CHARS + 10);
        assert!(giant.content.ends_with("…[已截断]"));
    }

    // ------------------------------------------------------------------
    // 纯函数
    // ------------------------------------------------------------------

    #[test]
    fn silence_detection() {
        assert!(is_silent("[[silent]]"));
        assert!(is_silent("[no reply needed]"));
        assert!(is_silent("【无需回复】"));
        assert!(is_silent("NO_REPLY"));
        assert!(!is_silent("我来回答 no reply needed 这个问题"));
        assert!(!is_silent("你好"));
    }

    #[test]
    fn narration_detection_and_strip() {
        assert_eq!(
            detect_text_tool_mentions("先 [Called web-search] 再 [调用 file-read]"),
            vec!["web-search", "file-read"]
        );
        assert!(detect_text_tool_mentions("正常回复").is_empty());
        assert_eq!(
            strip_tool_call_artifacts("先[Called web-search]后[执行 cam-shot]完"),
            "先后完"
        );
    }

    #[test]
    fn frame_args_normalization() {
        assert_eq!(normalize_frame_args(&json!({"args": ["a", "b"]})), json!(["a", "b"]));
        // 旗标对象原样透传
        assert_eq!(normalize_frame_args(&json!({"path": "/x"})), json!({"path": "/x"}));
        // 混入第二个键不拆
        assert_eq!(
            normalize_frame_args(&json!({"args": ["a"], "why": "x"})),
            json!({"args": ["a"], "why": "x"})
        );
        // 非字符串元素不拆
        assert_eq!(normalize_frame_args(&json!({"args": [1]})), json!({"args": [1]}));
    }

    // ------------------------------------------------------------------
    // 本地假 OpenAI server：真 HttpBrain + 真状态机全链
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn stub_brain_full_chain_over_http() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let tool_resp = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"c1","type":"function","function":{"name":"dev-hello","arguments":"{\"args\":[\"世界\"]}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":12,"completion_tokens":6}}"#.to_string();
        let final_resp = r#"{"choices":[{"message":{"role":"assistant","content":"你好，世界（来自假 brain）"},"finish_reason":"stop"}],"usage":{"prompt_tokens":30,"completion_tokens":9}}"#.to_string();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut auths = Vec::new();
            let mut bodies = Vec::new();
            for resp in [tool_resp, final_resp] {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut req: Vec<u8> = Vec::new();
                let mut buf = [0u8; 16384];
                loop {
                    let n = sock.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    req.extend_from_slice(&buf[..n]);
                    if let Some(h) = find_subslice(&req, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&req[..h]).to_string();
                        let cl = head
                            .lines()
                            .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                            .and_then(|l| l.split(':').nth(1))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if req.len() >= h + 4 + cl {
                            bodies.push(String::from_utf8_lossy(&req[h + 4..]).to_string());
                            break;
                        }
                    }
                }
                let head = String::from_utf8_lossy(&req).to_string();
                if let Some(a) = head.lines().find(|l| l.to_ascii_lowercase().starts_with("authorization:")) {
                    auths.push(a.splitn(2, ':').nth(1).unwrap().trim().to_string());
                }
                let http = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    resp.len(),
                    resp
                );
                sock.write_all(http.as_bytes()).await.unwrap();
            }
            (auths, bodies)
        });

        let brain = HttpBrain::new(BrainConfig {
            base_url: format!("http://{addr}/v1/chat/completions"),
            api_key: Some("sk-stub-test".into()),
            model: "chat".into(),
        });
        let mut io = MemTransport::default();
        io.inbox.push_back(result_frame("c1", true, "hello from the aginx command universe: 世界"));
        let code = run_turn(&brain, &[dev_hello_tool()], "你是化身", vec![], "打个招呼", &mut io, &TurnConfig::default())
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            io.sent,
            vec![
                Frame::ToolCall(WireToolCall { id: "c1".into(), tool: "dev-hello".into(), args: json!(["世界"]) }),
                Frame::Done(Done::ok("你好，世界（来自假 brain）")),
            ]
        );
        let (auths, bodies) = server.await.unwrap();
        assert_eq!(auths, vec!["Bearer sk-stub-test", "Bearer sk-stub-test"]);
        // 第一个请求体是合法 OpenAI 形状：system 头、工具面、user 问题
        let v: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(v["model"], json!("chat"));
        assert_eq!(v["messages"][0]["role"], json!("system"));
        assert_eq!(v["tools"][0]["function"]["name"], json!("dev-hello"));
        assert!(bodies[0].contains("打个招呼"));
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }
}
