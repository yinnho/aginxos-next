//! QR code login flow for iLink Bot.
//!
//! Flow: get_bot_qrcode → poll get_qrcode_status → save token.
//! Handles IDC redirect and QR expiry (max 3 refreshes).

use crate::api;
use crate::models::*;
use crate::token::WEIXIN_STATE;
use reqwest::Client;
use tracing::{info, warn};
use carrier_types::error::{CarrierError, CarrierResult};

/// Maximum QR code refreshes before giving up.
const MAX_QR_REFRESH: u32 = 3;

/// Total QR login timeout in seconds.
const QR_TOTAL_TIMEOUT_SECS: u64 = 480;

/// Poll interval for QR status (seconds).
const QR_POLL_INTERVAL_SECS: u64 = 1;

/// Perform QR code login and return the new tenant name.
///
/// Returns `(bot_id, qr_url)` on first call, then blocks until scanned.
/// This is a blocking function meant to be called from a tool or API handler.
pub async fn qr_login(
    http: &Client,
    bot_id: &str,
    bind_agent: Option<&str>,
    on_qr: Option<&dyn Fn(&str)>,
) -> CarrierResult<String> {
    let mut base_url = ILINK_API_BASE.to_string();
    let mut refresh_count = 0;
    let start = std::time::Instant::now();

    loop {
        if start.elapsed().as_secs() > QR_TOTAL_TIMEOUT_SECS {
            return Err(CarrierError::Internal(
                "QR login timed out (8 minutes)".to_string(),
            ));
        }

        // Step 1: Get QR code
        info!("Fetching QR code from {base_url}");
        let qr_resp = api::get_bot_qrcode_with_base(http, &base_url).await?;
        let qrcode_token = qr_resp.qrcode.clone();
        let qr_url = qr_resp.qrcode_img_content.clone();

        info!(qr_url = %qr_url, "QR code generated, waiting for scan");
        if let Some(cb) = on_qr {
            cb(&qr_url);
        }

        // Step 2: Poll QR status
        let poll_base = base_url.clone();
        loop {
            if start.elapsed().as_secs() > QR_TOTAL_TIMEOUT_SECS {
                return Err(CarrierError::Internal(
                    "QR login timed out during polling".to_string(),
                ));
            }

            match api::get_qrcode_status(http, &poll_base, &qrcode_token).await {
                Ok(status) => match status.status.as_str() {
                    "wait" => {
                        // No scan yet, continue polling
                    }
                    "scaned" => {
                        info!("QR code scanned, waiting for confirmation");
                    }
                    "confirmed" => {
                        let bot_token = status.bot_token.ok_or_else(|| {
                            CarrierError::Internal("confirmed but no bot_token".to_string())
                        })?;
                        let ilink_bot_id = status.ilink_bot_id.ok_or_else(|| {
                            CarrierError::Internal("confirmed but no ilink_bot_id".to_string())
                        })?;
                        let baseurl = status.baseurl.unwrap_or_else(|| ILINK_API_BASE.to_string());
                        let user_id = status.ilink_user_id;

                        // Register the tenant
                        WEIXIN_STATE.register_from_qr(
                            bot_id,
                            &bot_token,
                            &baseurl,
                            &ilink_bot_id,
                            user_id.as_deref(),
                            bind_agent,
                        );

                        info!(tenant = bot_id, "QR login successful");
                        return Ok(format!(
                            "WeChat account linked: {} (bot_id: {})",
                            bot_id, ilink_bot_id
                        ));
                    }
                    "expired" => {
                        refresh_count += 1;
                        if refresh_count >= MAX_QR_REFRESH {
                            return Err(CarrierError::Internal(
                                "QR code expired 3 times, giving up".to_string(),
                            ));
                        }
                        warn!(refresh = refresh_count, "QR expired, refreshing");
                        break; // Back to step 1
                    }
                    "scaned_but_redirect" => {
                        if let Some(host) = &status.redirect_host {
                            info!(host, "IDC redirect, switching base URL");
                            base_url = format!("https://{host}");
                        }
                        continue;
                    }
                    other => {
                        warn!(status = other, "Unknown QR status");
                    }
                },
                Err(e) => {
                    warn!(error = %e, "QR status poll error");
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(QR_POLL_INTERVAL_SECS)).await;
        }
    }
}
