//! `[DELIVER:key]` marker processing — resolve content.toml + channel deliver.

use tracing::{info, warn};

use super::parse::parse_deliver_markers;
use super::types::ChannelDeliverFn;

/// Process `[DELIVER:key]` / `[DELIVER:key|field=value|...]` markers without a
/// live `PluginBridgeManager` (used by the cron delivery path). Mirrors the
/// bridge method: resolve each key from `config`, apply overrides, deliver via
/// `deliver_fn`, strip markers. On failure with no remaining text, falls back
/// to the content's text representation.
///
/// `config` is `None` when the agent has no `content.toml`; markers are still
/// stripped so they don't leak to the user.
pub async fn process_deliver_markers_pub(
    deliver_fn: Option<ChannelDeliverFn>,
    config: Option<&carrier_types::content::ContentConfig>,
    channel_type: &str,
    bot_id: &str,
    sender_id: &str,
    response: &str,
) -> String {
    let (markers, cleaned) = parse_deliver_markers(response);
    if markers.is_empty() {
        return response.to_string();
    }
    let Some(deliver_fn) = deliver_fn else {
        warn!("DELIVER markers present but no channel_deliver_fn configured");
        // Still strip the markers so they don't leak to the user.
        return cleaned;
    };
    let mut fallback_text: Option<String> = None;
    'marker_loop: for marker in &markers {
        let mut desc = match config.and_then(|c| c.get(&marker.key)) {
            Some(d) => d.clone(),
            None => {
                warn!(key = %marker.key, "DELIVER marker: content key not found in content.toml");
                continue;
            }
        };
        for (field, value) in &marker.overrides {
            // Apply overrides directly on ContentDescriptor (no identity wrapper).
            if let Err(e) = desc.apply_override(field, value) {
                warn!(
                    key = %marker.key,
                    field = %field,
                    error = %e,
                    "DELIVER marker: bad override field, skipping this delivery"
                );
                if fallback_text.is_none() {
                    fallback_text = desc.as_text();
                }
                continue 'marker_loop;
            }
        }
        let desc_text = desc.as_text();
        let channel_type = channel_type.to_string();
        let bot_id = bot_id.to_string();
        let sender_id = sender_id.to_string();
        let key_owned = marker.key.clone();
        let channel_for_log = channel_type.clone();
        let deliver_fn = deliver_fn.clone();
        let result = tokio::task::spawn_blocking(move || {
            deliver_fn(&channel_type, &bot_id, &sender_id, &desc)
        })
        .await;
        match result {
            Ok(Ok(())) => info!(
                key = %key_owned,
                channel = %channel_for_log,
                "DELIVER: content delivered"
            ),
            Ok(Err(e)) => {
                warn!(
                    key = %key_owned,
                    channel = %channel_for_log,
                    error = %e,
                    "DELIVER: channel deliver failed; will fall back to text if reply empty"
                );
                if fallback_text.is_none() {
                    fallback_text = desc_text;
                }
            }
            Err(e) => warn!(key = %key_owned, error = %e, "DELIVER: spawn_blocking join failed"),
        }
    }
    if cleaned.trim().is_empty() {
        if let Some(t) = fallback_text {
            return t;
        }
    }
    cleaned
}
