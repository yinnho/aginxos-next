//! Outbound reply processing: markers, silence, content registry, and pipeline.
//!
//! Agent replies may embed side-effect markers (`[DELIVER]`, `[NOTIFY]`) and
//! no-reply sentinels. This module owns the parsing and processing of those
//! markers so both the interactive bridge path and the cron delivery path
//! share one implementation via [`prepare_outbound`].
//!
//! `plugin::bridge` re-exports the public API for backward-compatible import
//! paths (`runtime::plugin::bridge::process_*`).

mod content;
mod deliver;
mod notify;
mod parse;
mod pipeline;
mod silence;
mod types;

pub use content::ContentRegistry;
pub use deliver::process_deliver_markers_pub;
pub use pipeline::{prepare_outbound, OutboundCtx, OutboundResult};
pub use silence::{is_no_reply_sentinel, sanitize_wechat_text};
pub use types::{ChannelDeliverFn, ChannelSendFn, NotifyTarget};

#[cfg(test)]
mod tests {
    use super::is_no_reply_sentinel;
    use super::parse::parse_deliver_markers;

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

    #[test]
    fn deliver_markers_are_stripped_and_keys_extracted() {
        let (markers, cleaned) = parse_deliver_markers("月票来啦～ [DELIVER:月票] 收到吧");
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].key, "月票");
        assert!(markers[0].overrides.is_empty());
        assert!(!cleaned.contains("DELIVER"), "marker stripped: {cleaned}");
        assert!(cleaned.contains("月票来啦～") && cleaned.contains("收到吧"));

        // Bare marker only -> empty cleaned text, key extracted.
        let (markers, cleaned) = parse_deliver_markers("[DELIVER:月卡]");
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].key, "月卡");
        assert!(cleaned.trim().is_empty());

        // No markers -> unchanged.
        let (none, same) = parse_deliver_markers("just text");
        assert!(none.is_empty());
        assert_eq!(same, "just text");
    }

    #[test]
    fn deliver_marker_overrides_are_parsed() {
        let (markers, cleaned) = parse_deliver_markers(
            "[DELIVER:charter-card|miniprogram.appid=wxabc|miniprogram.pagepath=pages/x?token=a=b|miniprogram.title=包车]",
        );
        assert_eq!(markers.len(), 1);
        let m = &markers[0];
        assert_eq!(m.key, "charter-card");
        assert_eq!(m.overrides.len(), 3);
        assert_eq!(
            m.overrides[0],
            ("miniprogram.appid".to_string(), "wxabc".to_string())
        );
        assert_eq!(
            m.overrides[1],
            (
                "miniprogram.pagepath".to_string(),
                "pages/x?token=a=b".to_string()
            )
        );
        assert_eq!(
            m.overrides[2],
            ("miniprogram.title".to_string(), "包车".to_string())
        );
        assert!(cleaned.trim().is_empty());
    }

    #[test]
    fn deliver_marker_escapes_preserved_in_values() {
        // A pagepath containing `|` and `]` must be escaped so it does not end
        // the marker or split into another field; the parser resolves the
        // escapes back to the literal characters.
        let (markers, cleaned) = parse_deliver_markers(
            "[DELIVER:card|miniprogram.title=a\\|b\\]c|miniprogram.pagepath=p?x=1]",
        );
        assert_eq!(markers.len(), 1);
        let m = &markers[0];
        assert_eq!(m.overrides.len(), 2);
        assert_eq!(
            m.overrides[0],
            ("miniprogram.title".to_string(), "a|b]c".to_string())
        );
        assert_eq!(
            m.overrides[1],
            ("miniprogram.pagepath".to_string(), "p?x=1".to_string())
        );
        assert!(cleaned.trim().is_empty());
    }

    #[test]
    fn deliver_overrides_apply_to_descriptor() {
        use carrier_types::content::ContentDescriptor;
        let mut desc = ContentDescriptor::default();
        desc.apply_override("text", "fallback").unwrap();
        desc.apply_override("miniprogram.appid", "wxapp").unwrap();
        desc.apply_override("miniprogram.pagepath", "pages/x")
            .unwrap();
        desc.apply_override("miniprogram.title", "title").unwrap();
        desc.apply_override("miniprogram.thumb_media_id", "thumb")
            .unwrap();
        assert_eq!(desc.text.as_deref(), Some("fallback"));
        let mp = desc.miniprogram.as_ref().unwrap();
        assert_eq!(mp.appid, "wxapp");
        assert_eq!(mp.pagepath, "pages/x");
        assert_eq!(mp.title, "title");
        assert_eq!(mp.thumb_media_id.as_deref(), Some("thumb"));
        assert!(mp.is_complete());
    }

    #[test]
    fn deliver_override_unknown_field_errors() {
        use carrier_types::content::ContentDescriptor;
        let mut desc = ContentDescriptor::default();
        // Typo: app_id (underscore) instead of appid - must error, not silently
        // drop, so the marker handler can skip delivery instead of sending a
        // card with the wrong/empty appid.
        let err = desc.apply_override("miniprogram.app_id", "wxapp");
        assert!(err.is_err());
        assert!(desc.miniprogram.is_none());
    }

    #[test]
    fn empty_or_sentinel_should_suppress() {
        // Mirrors prepare_outbound suppress rules (unit-level without async).
        assert!(is_no_reply_sentinel("[no reply needed]"));
        assert!("".trim().is_empty() || is_no_reply_sentinel(""));
        assert!(!"hello".trim().is_empty() && !is_no_reply_sentinel("hello"));
    }
}
