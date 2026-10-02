//! `[DELIVER:key]` marker stripping.
//!
//! 频道投递腿已随 #68 刀4b 退役（引擎不认识频道）——这里只剩把标记从
//! 回复文本里剥掉，不让原始 marker 泄进卡片/webhook 文本。卡片投递是
//! channels 守护（crates/channels）经 ACP 的自家事。

use tracing::warn;

use super::parse::strip_deliver_markers;

/// Strip `[DELIVER:…]` markers from `response` (no delivery attempt).
pub async fn process_deliver_markers_pub(response: &str) -> String {
    let (count, cleaned) = strip_deliver_markers(response);
    if count > 0 {
        warn!(
            count,
            "DELIVER markers stripped without delivery (channels live in crates/channels)"
        );
    }
    cleaned
}
