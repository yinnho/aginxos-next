//! 组装接线：kernel boot + webhook 通道。
//!
//! `aginx-carrier start`（守护形态）使用。webui 已退役（2026-08-30
//! AginxOS 融合）；一次性 CLI（agent 子命令）走裸 boot 不进这里。
//! weixin 通道已随 #69 改形退役——频道体系整线住 crates/channels。

use std::sync::Arc;

use carrier_kernel::kernel::CarrierKernel;
use carrier_runtime::channel_manager::ChannelManager;
use carrier_runtime::kernel_handle::KernelHandle;
use tracing::info;

/// Boot the kernel: disk restore + self handle + 后台 agent 循环/心跳/cron。
pub fn boot_kernel() -> anyhow::Result<Arc<CarrierKernel>> {
    let kernel = CarrierKernel::boot(None)?;
    let kernel = Arc::new(kernel);
    kernel.set_self_handle();
    kernel.start_background_agents();
    Ok(kernel)
}

/// Wire up the channels（opencarrier run_daemon 通道段的 webhook 子集）：
/// sender router、cron 投递/通知存储、webhook 注册、出站 send/deliver/probe
/// 与工具分发器注入。
///
/// 调用方负责 `cm.start().await`（start 守护形态与桌面形态时机一致，
/// 但保持显式以便宿主插自己的启动钩子）。
pub async fn boot_channels(kernel: &Arc<CarrierKernel>) -> anyhow::Result<ChannelManager> {
    let kh: Arc<dyn KernelHandle> = kernel.clone();
    let mut cm = ChannelManager::new(kh);

    // Sender-based routing: 绑定即路由的内存路由表。真源在会话里
    // （weixin 的 bind_agent）与 config.toml（webhook），启动时种入。
    let sender_router = Arc::new(carrier_runtime::plugin::router::SenderRouter::new());
    cm.set_sender_router(sender_router.clone());
    info!("Sender-based routing enabled");

    // Cron 投递存储（last-channel 追踪）+ 通知路由存储。
    {
        let store = Arc::new(kernel.memory.cron_delivery().clone());
        cm.set_cron_delivery(store);
    }
    {
        let store = Arc::new(kernel.memory.notify_store().clone());
        cm.set_notify_store(store);
    }

    cm.start().await;

    // webhook 入站通道：出站侧注册（异步轮回复的日志归宿，防 bridge 报
    // Channel-not-found）+ 路由种入。HTTP 监听在 start.rs（daemon 形态专属，
    // 移动端不起监听）。send_fn 捕获同一 channels map，start 后注册对出站
    // 查表可见；WebhookChannel::start 是 noop，不被 start() 调到也成立。
    if kernel.config.webhook.enabled {
        cm.register("webhook", Box::new(carrier_webhook::WebhookChannel));
        for hook in &kernel.config.webhook.hooks {
            cm.set_sender_route(&hook.name, &hook.agent);
            info!(hook = %hook.name, agent = %hook.agent, "webhook route seeded");
        }
    }

    // 出站通道：send（cron 主动推送探针）+ deliver（富媒体投递）注入 kernel。
    {
        let send_fn = cm.make_channel_send_fn();
        *kernel.channel_send_fn.write().unwrap() = Some(send_fn);
        let deliver_fn = cm.make_channel_deliver_fn();
        *kernel.channel_deliver_fn.write().unwrap() = Some(deliver_fn);
        let probe = cm.make_supports_proactive_fn();
        *kernel.channel_supports_proactive_fn.write().unwrap() = Some(probe);
    }
    // 工具分发器注入 kernel（agent 工具调用走通道工具）。
    {
        let dispatcher = cm.tool_dispatcher();
        let mut guard = kernel
            .plugins
            .plugin_tool_dispatcher
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *guard = Some(dispatcher);
    }

    Ok(cm)
}

/// 首启兜底：`~/.aginx/carrier/brain.json` 不存在则写骨架——kernel boot 硬
/// 要求 brain 可加载；base_url 为空时由宿主（AginxOS 设置面）引导补齐。
/// 从 web.rs 收编（web 子命令退役，2026-08-30）。
pub fn seed_brain_skeleton_if_missing() {
    let path = carrier_types::config::home_dir().join("brain.json");
    if path.exists() {
        return;
    }
    let skeleton = serde_json::json!({
        "base_url": "",
        "api_key_env": "AGINXBRAIN_API_KEY",
        "default_modality": "chat",
        "modalities": { "chat": { "description": "默认对话" } }
    });
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, serde_json::to_string_pretty(&skeleton).unwrap_or_default()) {
        Ok(()) => info!(path = %path.display(), "已写入 brain.json 骨架（待配置 brain）"),
        Err(e) => {
            // boot 会再报一次更具体的错，这里只提示来源
            eprintln!("brain.json 骨架写入失败（{e}）；若已有配置可忽略");
        }
    }
}

/// 系统身份种子：系统本人（"system"）住在家根——系统即智能体（09-27
/// 裁决），人格就是家目录本身（SOUL.md / MEMORY.md 在 {AGINX_HOME} 根上，
/// docs/FS.md）。不走 clone 安装管线：那会清空重装目录，家根绝不能进。
///
/// 未注册时：建家目录 + sessions/，再以 workspace=家根 spawn。家根人格
/// 文件不在这里种——出厂树随镜像烤进 /home（结构刀②③④，真源=仓里
/// `home/` 整树），这里只管注册。已注册即跳过；失败只告警不挡启动，
/// 重启重试。刀5 no-me：老 seed_system_me（"me"）退场，boot 迁移器
/// （kernel::migrations）负责把旧 me 世界搬过来。
pub async fn seed_system_agent(kernel: &Arc<CarrierKernel>) {
    use carrier_types::agent::AgentManifest;

    if kernel
        .registry
        .find_by_name(carrier_types::config::SYSTEM_AGENT)
        .is_some()
    {
        return;
    }

    let home = kernel.config.home_dir.clone();
    if let Err(e) = std::fs::create_dir_all(&home) {
        tracing::warn!(error = %e, "system 种子失败：家目录不可建（不影响启动，重启重试）");
        return;
    }

    if let Err(e) = std::fs::create_dir_all(home.join("sessions")) {
        tracing::warn!(error = %e, "system sessions 目录创建失败");
    }

    let manifest = AgentManifest {
        name: carrier_types::config::SYSTEM_AGENT.to_string(),
        display_name: "系统".to_string(),
        description: "系统本体 — OS 进程即智能体；对主人是总管，对外是门面（家根身份，无 workflows 也是智能体）".to_string(),
        workspace: Some(home),
        generate_identity_files: false,
        ..Default::default()
    };
    match kernel.spawn_agent(manifest) {
        Ok(id) => {
            tracing::info!(id = %id, "系统本人已种子：system（workspace=家根）");
            // 刀5 A 路：系统直通条目随种子落（boot 闸只认在册，新世界第一
            // 拍在这里补）
            carrier_kernel::gateway_registry::ensure_system_entry(&kernel.config);
        }
        Err(e) => tracing::warn!(error = ?e, "system 种子失败（不影响启动，重启重试）"),
    }
}
