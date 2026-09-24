//! Channel trait — interface for platform message adapters.
//!
//! Each channel (feishu, wecom, weixin, dingtalk) implements this trait
//! to receive inbound messages and send outbound replies.

use tokio::sync::mpsc;

use crate::error::{CarrierError, CarrierResult};
use crate::plugin::PluginMessage;

/// Run async work from a sync [`Channel`] method on a dedicated OS thread.
///
/// Channel `send`/`deliver` are synchronous (object-safe trait) but the real
/// I/O is async. Callers may already be on a Tokio worker (`spawn_blocking` or
/// an axum handler calling through a sync `ChannelDeliverFn`), so nesting
/// `block_on` on the current thread would panic. This helper always detaches
/// onto a new current-thread runtime.
///
/// Prefer this over copy-pasting `thread::spawn` + `Builder::new_current_thread`
/// in each channel crate.
pub fn block_on_detached<F, T>(fut: F) -> CarrierResult<T>
where
    F: std::future::Future<Output = CarrierResult<T>> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt.block_on(fut),
            Err(e) => Err(CarrierError::Internal(format!("runtime: {e}"))),
        };
        let _ = tx.send(result);
    });
    let inner = rx
        .recv()
        .map_err(|e| CarrierError::Internal(format!("deliver/send thread disconnected: {e}")))?;
    inner
}

/// A channel adapter that bridges an external platform to the carrier kernel.
///
/// Implementations handle platform-specific protocols (WebSocket, HTTP callback, etc.)
/// and translate between platform messages and the unified `PluginMessage` format.
pub trait Channel: Send + Sync {
    /// Channel type identifier (e.g. "weixin", "feishu", "wecom", "dingtalk").
    fn channel_type(&self) -> &str;

    /// Human-readable channel name.
    fn name(&self) -> &str;

    /// Bot identifier this channel belongs to.
    fn bot_id(&self) -> &str;

    /// Start receiving messages from the channel.
    ///
    /// Inbound messages are sent through `sender` for routing to agents.
    fn start(&mut self, sender: mpsc::Sender<PluginMessage>) -> CarrierResult<()>;

    /// Send a text message through the channel.
    fn send(&self, bot_id: &str, user_id: &str, text: &str) -> CarrierResult<()>;

    /// Deliver rich content. Each channel picks the highest-fidelity
    /// representation it supports from `content` (miniprogram > video > file >
    /// image > link > text) and sends it to `user_id` on bot `bot_id`.
    ///
    /// The default implementation degrades to plain text (or a formatted link)
    /// via [`send`](Self::send) - so channels that only support text need do
    /// nothing. Channels with richer native forms (weixin-oa miniprogram cards,
    /// wecom kf files/video, weixin iLink images/video) override this to pick
    /// their best form, resolving the sending account by `bot_id` (multi-tenant).
    fn deliver(
        &self,
        content: &crate::content::ContentDescriptor,
        bot_id: &str,
        user_id: &str,
    ) -> CarrierResult<()> {
        let text = content.as_text().ok_or_else(|| {
            CarrierError::InvalidInput(
                "channel has no representation for this content and no text fallback".into(),
            )
        })?;
        self.send(bot_id, user_id, &text)
    }

    /// Stop the channel and release resources.
    fn stop(&mut self);

    /// Whether this channel supports proactive push (sending without an inbound
    /// context). Channels that require a context_token / response_url should
    /// return false; cron and other server-initiated notifications must be
    /// buffered for these channels until the user sends an inbound message.
    fn supports_proactive_push(&self) -> bool {
        false
    }
}
