//! `aginx-carrier probe` — 网关存活探测（heartbeat watchdog 的探测原语）。
//!
//! 走真链路而非查本机进程：TLS 连 relay → `{"type":"connect"}` 握手 →
//! 等 `connected`。该应答只能由目标网关经 relay 隧道发回，一次握手同时
//! 证明 relay 活、网关注册在册、隧道畅通。复用 carrier-gateway 的
//! agent_client（agent:// 客户端先例），零协议重复。
//!
//! relay secret 取本机 `~/.aginx/config.toml` 的 `[relay]` 段（与网关
//! 同机同用户的网级凭证）。退出码：0 在线 / 1 不在线（原因进 Err 文本，
//! watchdog 脚本可直接当告警正文）。

use carrier_gateway::agent_client::{AgentConn, AgentEndpoint};

/// Probe one `agent://<id>.relay.<domain>` gateway end-to-end.
/// Down gateway → Err（main 转 exit 1）。
pub fn run(url: String) -> anyhow::Result<()> {
    let ep = AgentEndpoint::from_url_with_local_secret(&url).ok_or_else(|| {
        anyhow::anyhow!(
            "无法解析 {url}（需 agent://<id>.relay.<domain> 形态），或本机 \
             ~/.aginx/config.toml 缺 [relay] 段（relay secret 无源）"
        )
    })?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    match runtime.block_on(AgentConn::connect(&ep)) {
        Ok(_conn) => {
            println!(
                "ok {}（{}:{} 握手 connected）",
                ep.target, ep.tls_domain, ep.port
            );
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("down {} — {e}", ep.target)),
    }
}
