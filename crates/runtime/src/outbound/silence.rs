//! No-reply sentinels for outbound replies.
//!
//! # Silence contract
//!
//! There is **one** intentional-silence signal for channel delivery:
//!
//! | Layer | Responsibility |
//! |-------|----------------|
//! | **Upstream** (agent loop `end_turn`, multi-step `run_flow`) | Detect `[[silent]]` and whole-text sentinels; set `AgentLoopResult { silent: true, response: "" }`. Session history may store `"[no reply needed]"` for prune/audit. |
//! | **Downstream** ([`prepare_outbound`]) | Safety net: if cleaned text is empty or still a whole-text sentinel, set `suppress_text_send` so the literal marker never reaches users (or 45015 on OA 客服). |
//!
//! Whole-text matching only (after trim / bracket strip). A real reply that
//! merely *contains* the phrase is not suppressed.
//!
//! New silence dialects should extend [`is_no_reply_sentinel`] (and thus both
//! layers), not invent a third path.

/// Does this agent reply text mean "no reply should be sent to the user"?
///
/// Agents/flows signal an intentional no-reply by emitting a sentinel token as
/// the *entire* reply (`[no reply needed]`, `[no_reply_needed]`, `[无需回复]`,
/// `NO_REPLY`, …). When the WeChat OA 客服消息 48h window is closed — which is
/// the common case for event-triggered turns (card clicks, page views) that
/// don't carry a real user message — shipping that sentinel via the customer-
/// send API yields error 45015 and spams the logs (~dozens/day). Suppressing
/// it here also stops the literal marker from ever reaching a user on any
/// channel.
///
/// Used by:
/// - agent-loop / flow **upstream** (set `silent` + empty response)
/// - outbound **downstream** safety net (interactive + cron)
pub fn is_no_reply_sentinel(text: &str) -> bool {
    let t = text.trim();
    let inner = t
        .trim_start_matches(['[', '【'])
        .trim_end_matches([']', '】'])
        .trim()
        .to_lowercase();
    // Normalize underscores to spaces so `[no_reply_needed]` == `[no reply needed]`.
    let inner = inner.replace('_', " ");
    matches!(
        inner.as_str(),
        "no reply needed" | "no reply" | "noreply" | "no reply required" | "无需回复" | "无需答复"
    )
}
