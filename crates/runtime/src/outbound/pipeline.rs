//! Outbound reply cleanup for the cron path.

use tracing::info;

use super::deliver::process_deliver_markers_pub;
use super::silence::is_no_reply_sentinel;

/// Result of preparing an agent reply for delivery.
pub struct OutboundResult {
    /// Reply text with markers stripped.
    pub cleaned_text: String,
    /// When true, the caller must not deliver `cleaned_text` (empty after
    /// markers, or a no-reply sentinel).
    pub suppress_text_send: bool,
}

/// Strip `[DELIVER:…]` markers, then decide whether the final text should be
/// delivered: empty replies and no-reply sentinels suppress the send.
pub async fn prepare_outbound(response: &str) -> OutboundResult {
    let text = process_deliver_markers_pub(response).await;
    let suppress_text_send = text.trim().is_empty() || is_no_reply_sentinel(&text);
    if suppress_text_send && is_no_reply_sentinel(&text) {
        info!("Outbound suppressing no-reply sentinel — not delivering");
    }
    OutboundResult {
        cleaned_text: text,
        suppress_text_send,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn strips_markers_and_suppresses_sentinels() {
        let out = prepare_outbound("[DELIVER:月票][no reply needed]").await;
        assert!(out.suppress_text_send);
        let out = prepare_outbound("正文 [DELIVER:月票] 收到").await;
        assert!(!out.suppress_text_send);
        assert_eq!(out.cleaned_text, "正文  收到");
    }
}
