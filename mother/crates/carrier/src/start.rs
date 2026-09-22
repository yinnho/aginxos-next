//! `aginx-carrier start` — 服务器/守护形态（个人部署）。
//!
//! kernel + iLink 通道 + cron/heartbeat 后台，进程常驻。组装接线与桌面
//! 形态共用 `aginx_carrier::wiring`；HTTP API（AGENT-APP-API 对外契约）
//! 是后续增量，这里先保通道在线。

use tracing::info;

/// Boot the kernel and run channel services until Ctrl-C.
pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let kernel = aginx_carrier::wiring::boot_kernel()?;

    // ── 系统分身：clone-creator（克隆大师）未注册则用内嵌定义层装上。
    // 分身只经它生成，不手工摆文件。──
    aginx_carrier::wiring::seed_system_creator(&kernel).await;
    // ── 系统身份：「我」——主人的统一身份（总管/门面），开箱即聊。──
    aginx_carrier::wiring::seed_system_me(&kernel).await;

    // ── aginx 入网同步：workspace 里已装但 ~/.aginx/agents/ 缺登记的分身
    // 补写 aginx.toml（clone_install 是增量钩子，这里是启动对账）。──
    aginx_carrier::wiring::sync_aginx_registrations(&kernel);

    let cm = aginx_carrier::wiring::boot_channels(&kernel).await?;

    // webhook HTTP 入站通道（daemon 形态专属；uniffi/移动端不起监听）。
    // bind 失败只报错——不拖死 daemon 其余通道。
    if kernel.config.webhook.enabled {
        let kh: std::sync::Arc<dyn carrier_runtime::kernel_handle::KernelHandle> = kernel.clone();
        let cfg = kernel.config.webhook.clone();
        let bridge_tx = cm.bridge_sender();
        tokio::spawn(carrier_webhook::serve(kh, bridge_tx, cfg));
    }

    let tool_count = cm.tool_definitions().len();
    info!(
        tools = tool_count,
        webhook = kernel.config.webhook.enabled,
        "aginx-carrier 守护进程就绪（iLink 通道在线，Ctrl-C 退出）"
    );

    // ── api_tools cron 委托（M34b）：30s 一跳 spawn `aginx-carrier api cron`。
    // 到点判定（minute 粒度）+ 防双发（秒<30 才发）+ 落库全在 CLI 单真源；
    // 守护只出节拍。空载门：先扫 toml 有无 [tool.cron]，没有就不 spawn——
    // 零配置设备不付每 30s 一次的进程代价（功耗线 M23）。──
    tokio::spawn(async {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        interval.tick().await; // skip immediate fire
        loop {
            interval.tick().await;
            let mut tomls = vec![carrier_types::config::home_dir().join("api_tools.toml")];
            let ws_root = carrier_types::config::home_dir().join("workspaces");
            if let Ok(entries) = std::fs::read_dir(&ws_root) {
                for entry in entries.flatten() {
                    let t = entry.path().join("api_tools.toml");
                    if t.is_file() {
                        tomls.push(t);
                    }
                }
            }
            let has_cron = tomls.iter().any(|p| {
                std::fs::read_to_string(p)
                    .map(|c| c.contains("[tool.cron]"))
                    .unwrap_or(false)
            });
            if !has_cron {
                continue;
            }
            let mut cmd = tokio::process::Command::new("aginx-carrier");
            cmd.arg("api").arg("cron").arg("--json");
            for t in &tomls {
                cmd.arg("--toml").arg(t);
            }
            match cmd.output().await {
                Ok(o) => {
                    let out = String::from_utf8_lossy(&o.stdout);
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(out.trim()) {
                        if let Some(fired) = v["data"]["fired"].as_array() {
                            for f in fired {
                                info!(
                                    tool = f["tool"].as_str().unwrap_or("?"),
                                    ok = f["ok"].as_bool().unwrap_or(false),
                                    "api cron 委托一跳"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "api cron 委托 spawn 失败")
                }
            }
        }
    });

    tokio::signal::ctrl_c().await?;
    info!("收到退出信号，bye");
    Ok(())
}
