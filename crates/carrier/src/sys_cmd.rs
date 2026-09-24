//! `aginx-carrier sys` — 系统杂项面（M33 D3 批3）。
//!
//! location_get / system_time 自 runtime tools/misc.rs 原样搬来（单真源
//! 在此）：位置走 ip-api.com（无键、限速），时间走本地钟。不 boot
//! kernel——两个工具都不碰化身态。

use clap::Subcommand;
use carrier_types::error::{CarrierError, CarrierResult};

#[derive(Subcommand)]
pub enum SysAction {
    /// IP 地理位置查询（ip-api.com；机读面 tool location_get 同源）
    Location {
        /// 机器可读输出：D1 信封一条
        #[arg(long)]
        json: bool,
    },
    /// 当前日期时间（UTC/本地/时区/unix epoch；机读面 tool system_time 同源）
    Time {
        /// 机器可读输出：D1 信封一条
        #[arg(long)]
        json: bool,
    },
}

pub fn run(action: SysAction) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match action {
            SysAction::Location { json } => {
                let out = location_get().await?;
                if json {
                    println!("{}", aginx_carrier::envelope::ok(serde_json::Value::String(out)));
                } else {
                    println!("{out}");
                }
            }
            SysAction::Time { json } => {
                let out = system_time();
                if json {
                    println!("{}", aginx_carrier::envelope::ok(serde_json::Value::String(out)));
                } else {
                    println!("{out}");
                }
            }
        }
        anyhow::Ok(())
    })
}

// ---------------------------------------------------------------------------
// 实现（自 runtime tools/misc.rs 逐字节搬来）
// ---------------------------------------------------------------------------

pub(crate) async fn location_get() -> CarrierResult<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| CarrierError::Network(format!("Failed to create HTTP client: {e}")))?;
    // ip-api.com 免费档只开明文 HTTP（HTTPS 全局 403——上机实测 2026-09-02）。
    let resp = client
        .get("http://ip-api.com/json/?fields=status,message,country,regionName,city,zip,lat,lon,timezone,isp,query")
        .header("User-Agent", format!("OpenCarrier/{}", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("Location request failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(CarrierError::Network(format!(
            "Location API returned {}",
            resp.status()
        )));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| {
        CarrierError::Serialization(format!("Failed to parse location response: {e}"))
    })?;
    if body["status"].as_str() != Some("success") {
        let msg = body["message"].as_str().unwrap_or("Unknown error");
        return Err(CarrierError::Network(format!(
            "Location lookup failed: {msg}"
        )));
    }
    let result = serde_json::json!({
        "lat": body["lat"],
        "lon": body["lon"],
        "city": body["city"],
        "region": body["regionName"],
        "country": body["country"],
        "zip": body["zip"],
        "timezone": body["timezone"],
        "isp": body["isp"],
        "ip": body["query"],
    });
    serde_json::to_string_pretty(&result)
        .map_err(|e| CarrierError::Serialization(format!("Serialize error: {e}")))
}

pub(crate) fn system_time() -> String {
    let now_utc = chrono::Utc::now();
    let now_local = chrono::Local::now();
    let result = serde_json::json!({
        "utc": now_utc.to_rfc3339(),
        "local": now_local.to_rfc3339(),
        "unix_epoch": now_utc.timestamp(),
        "timezone": now_local.format("%Z").to_string(),
        "utc_offset": now_local.format("%:z").to_string(),
        "date": now_local.format("%Y-%m-%d").to_string(),
        "time": now_local.format("%H:%M:%S").to_string(),
        "day_of_week": now_local.format("%A").to_string(),
    });
    serde_json::to_string_pretty(&result).unwrap_or_else(|_| now_utc.to_rfc3339())
}
