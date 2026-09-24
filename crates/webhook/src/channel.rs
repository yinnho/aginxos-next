//! WebhookChannel——`Channel` trait 的出站侧落点。
//!
//! webhook 的"出站"没有真实去处：异步轮的 HTTP 早已 202 返回，agent 的
//! 回复文本写日志供运营查（agent 靠工具做事：发微信/写文件）。注册这个
//! 通道是为了让 bridge 的 `send_response` 能按 channel_type 找到落点，
//! 不然每次异步轮结束都刷 "Channel not found" 错误日志。

use carrier_types::channel::Channel;
use carrier_types::error::CarrierResult;
use tokio::sync::mpsc;
use tracing::info;

pub struct WebhookChannel;

/// 回复日志截断（字符数非字节数——中文切片 panic 坑）。
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

impl Channel for WebhookChannel {
    fn channel_type(&self) -> &str {
        "webhook"
    }

    fn name(&self) -> &str {
        "webhook"
    }

    fn bot_id(&self) -> &str {
        ""
    }

    fn start(&mut self, _sender: mpsc::Sender<carrier_types::plugin::PluginMessage>) -> CarrierResult<()> {
        // webhook 模式：入站来自 HTTP 监听（server.rs 直接注入 bridge），
        // 通道自身无轮询可起。
        Ok(())
    }

    fn send(&self, bot_id: &str, user_id: &str, text: &str) -> CarrierResult<()> {
        info!(
            bot = bot_id,
            user = user_id,
            reply = %truncate_chars(text, 500),
            "webhook 回复（无出站去处，全文进日志）"
        );
        Ok(())
    }

    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_is_char_safe() {
        let s = "中文回复".repeat(200);
        let out = truncate_chars(&s, 500);
        assert_eq!(out.chars().count(), 500);
        // 短文本原样。
        assert_eq!(truncate_chars("ok", 500), "ok");
    }
}
