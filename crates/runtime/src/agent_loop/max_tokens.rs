//! Handler for the MaxTokens stop reason.
//!
//! When the LLM hits its token limit, the agent loop either continues
//! (with a "Please continue" prompt) or returns a partial response
//! if the limit has been hit too many times consecutively.

use super::*;
use crate::hooks::HookRegistry;
use crate::llm_driver::StreamEvent;
use carrier_memory::MemorySubstrate;
use tracing::warn;
use carrier_types::message::{Message, TokenUsage};

/// Maximum consecutive MaxTokens continuations before returning partial response.
/// Raised from 3 to 5 to allow longer-form generation.
pub const MAX_CONTINUATIONS: u32 = 5;

/// Result of handling a MaxTokens stop reason.
pub(in crate::agent_loop) enum MaxTokensAction {
    /// The loop should continue with the appended messages.
    Continue,
    /// The loop should return this result to the caller.
    Complete(AgentLoopResult),
}

/// Handle a `StopReason::MaxTokens` response.
///
/// If consecutive MaxTokens hits have not exceeded `MAX_CONTINUATIONS`, appends
/// "Please continue" to both `session` and `messages` and returns `Continue`.
/// Otherwise, saves the session and returns `Complete` with a partial response.
#[allow(clippy::too_many_arguments)]
pub(in crate::agent_loop) async fn handle_max_tokens(
    response: &CompletionResponse,
    session: &mut Session,
    messages: &mut Vec<Message>,
    memory: &MemorySubstrate,
    _stream_tx: &Option<tokio::sync::mpsc::Sender<StreamEvent>>,
    consecutive_max_tokens: &mut u32,
    hooks: Option<&HookRegistry>,
    agent_id_str: &str,
    manifest: &AgentManifest,
    iteration: u32,
    total_usage: TokenUsage,
    // Macro-based save helper — we pass the session_base_len so we can
    // do the save inline instead of through the macro.
    session_base_len: usize,
) -> MaxTokensAction {
    *consecutive_max_tokens += 1;
    if *consecutive_max_tokens >= MAX_CONTINUATIONS {
        let text = response.text();
        let text = if text.trim().is_empty() {
            "[Partial response — token limit reached with no text output.]".to_string()
        } else {
            text
        };
        // O6: Single-track — sync before save, then push the final response
        super::helpers::sync_loop_messages(messages, session, session_base_len);
        session.messages.push(Message::assistant(&text));
        // Save session (inline version of save_new! macro)
        let new_msgs = &session.messages[session_base_len..];
        if let Err(e) = memory
            .save_session_append_async(
                session.id,
                &session.agent_name,
                new_msgs,
                session.context_window_tokens,
                session.label.as_deref(),
                None,
            )
            .await
        {
            warn!("Failed to save session on max continuations: {e}");
        }
        warn!(
            iteration,
            consecutive_max_tokens, "Max continuations reached , returning partial response"
        );
        // Fire AgentLoopEnd hook
        if let Some(hook_reg) = hooks {
            let ctx = crate::hooks::HookContext {
                agent_name: &manifest.name,
                agent_id: agent_id_str,
                event: carrier_types::agent::HookEvent::AgentLoopEnd,
                data: serde_json::json!({
                    "iterations": iteration + 1,
                    "reason": "max_continuations",
                }),
            };
            let _ = hook_reg.fire(&ctx);
        }
        MaxTokensAction::Complete(AgentLoopResult {
            response: text,
            total_usage,
            iterations: iteration + 1,
            silent: false,
            directives: Default::default(),
            plan: None,
        })
    } else {
        let text = response.text();
        // O6: Single-track — only push to messages
        messages.push(Message::assistant(&text));
        messages.push(Message::user("Please continue."));
        warn!(iteration, "Max tokens hit , continuing");
        MaxTokensAction::Continue
    }
}
