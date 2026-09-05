//! aginx-gateway bin — 见 src/lib.rs 的 crate 头。日志走 stderr
//! （svc.d 单元捕获到 /var/log/aginx/）；n5.sh K 段断言 "registered" 行。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aginx_gateway::agent::{self, AgentBridge};
use aginx_gateway::{config, relay, secret};

fn main() {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("aginx-gateway: config error: {e}");
            std::process::exit(1);
        }
    };
    // id 是设备身份：env 带（svc.d env_file=/etc/aginx/env 注入），
    // 属设备状态走 state tar——配置文件里故意没有它。
    let Ok(id) = std::env::var("AGINX_GATEWAY_ID") else {
        eprintln!("aginx-gateway: AGINX_GATEWAY_ID is not set — put it in /etc/aginx/env (device state)");
        std::process::exit(1);
    };
    let id = id.trim().to_string();
    if id.is_empty() {
        eprintln!("aginx-gateway: AGINX_GATEWAY_ID is empty");
        std::process::exit(1);
    }
    let sock = std::env::var("AGINX_SOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(agent::SERVER_SOCK));
    let secret_sock = std::env::var("AGINX_SECRET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(secret::SIDECAR_SOCK));

    eprintln!(
        "aginx-gateway: id={id} relay={}:{} tls={} heartbeat={}s reconnect={}s turn-gate={}s",
        cfg.host, cfg.port, cfg.tls, cfg.heartbeat_s, cfg.reconnect_s, cfg.turn_timeout_s
    );

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        let bridge = Arc::new(AgentBridge {
            sock,
            turn_timeout: Duration::from_secs(cfg.turn_timeout_s.max(1)),
        });
        let relay_cfg = relay::RelayCfg {
            host: cfg.host.clone(),
            port: cfg.port,
            tls: cfg.tls,
            heartbeat_s: cfg.heartbeat_s,
        };
        loop {
            let key = wait_for_secret(&secret_sock).await;
            match relay::run_once(&relay_cfg, &id, &key, Arc::clone(&bridge) as Arc<dyn relay::Bridge>).await {
                Ok(()) => eprintln!("aginx-gateway: connection ended cleanly"),
                Err(e) => eprintln!("aginx-gateway: relay error: {e:#}"),
            }
            eprintln!("aginx-gateway: reconnecting in {}s", cfg.reconnect_s);
            tokio::time::sleep(Duration::from_secs(cfg.reconnect_s.max(1))).await;
        }
    });
}

/// 钥匙未就绪就等（不退出）：sidecar 可能晚起、运维可能正在灌注。
/// 日志只打一次"等待中"，避免每个重试周期刷屏。
async fn wait_for_secret(secret_sock: &std::path::Path) -> String {
    let mut announced = false;
    loop {
        if let Some(k) = secret::resolve(secret_sock) {
            if announced {
                eprintln!("aginx-gateway: relay secret is available now");
            }
            return k;
        }
        if !announced {
            eprintln!(
                "aginx-gateway: waiting for relay secret (env {} or sidecar {})",
                secret::ENV_VAR,
                secret::SCOPE
            );
            announced = true;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
