//! Outbound reply cleanup: strip side-effect markers, honor no-reply
//! sentinels. （频道推送腿已随 #68 刀4b 退役——本模块只剩文本清洗。）

mod deliver;
mod parse;
mod pipeline;
mod silence;

pub use pipeline::prepare_outbound;
pub use silence::is_no_reply_sentinel;

#[cfg(test)]
mod tests {
    use super::deliver::process_deliver_markers_pub;
    use super::is_no_reply_sentinel;

    #[test]
    fn detects_no_reply_sentinels() {
        // Space form — injected by end_turn.rs (`[no reply needed]`).
        assert!(is_no_reply_sentinel("[no reply needed]"));
        assert!(is_no_reply_sentinel("  [no reply needed]  "));
        // Underscore form — emitted by flows/agents.
        assert!(is_no_reply_sentinel("[no_reply_needed]"));
        assert!(is_no_reply_sentinel("no_reply_needed"));
        // Chinese form.
        assert!(is_no_reply_sentinel("[无需回复]"));
        // Bare token (no brackets).
        assert!(is_no_reply_sentinel("NO_REPLY"));
        assert!(is_no_reply_sentinel("noreply"));
        // Full-width brackets.
        assert!(is_no_reply_sentinel("【无需回复】"));
    }

    #[test]
    fn leaves_real_replies_untouched() {
        // A real reply that merely mentions the phrase must NOT be suppressed.
        assert!(!is_no_reply_sentinel(
            "这是咱们的月票，点开小程序就能看详情"
        ));
        assert!(!is_no_reply_sentinel(
            "Sure, no reply needed from me, but here's the answer: 42"
        ));
        assert!(!is_no_reply_sentinel(""));
        assert!(!is_no_reply_sentinel("ok"));
    }

    #[tokio::test]
    async fn deliver_markers_are_stripped() {
        let cleaned = process_deliver_markers_pub("月票来啦～ [DELIVER:月票] 收到吧").await;
        assert!(cleaned.contains("月票来啦～") && cleaned.contains("收到吧"));
        assert!(!cleaned.contains("DELIVER"), "marker stripped: {cleaned}");
    }

    #[tokio::test]
    async fn bare_marker_strips_to_empty() {
        assert!(process_deliver_markers_pub("[DELIVER:月卡]").await.trim().is_empty());
    }

    #[tokio::test]
    async fn no_markers_unchanged() {
        assert_eq!(process_deliver_markers_pub("just text").await, "just text");
    }
}
