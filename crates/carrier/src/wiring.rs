//! 组装接线：kernel boot。

/// `aginx-carrier start` 守护形态已随 #68 刀4 退役（webhook 通道、
/// ChannelManager、插件树一并拆——频道体系整线住 crates/channels）。
/// 这里只剩一次性 CLI（agent/acp/cron 子命令）要的裸 boot 与种子。

use std::sync::Arc;

use carrier_kernel::kernel::CarrierKernel;

/// Boot the kernel: disk restore + self handle + 后台 agent 循环/心跳/cron。
pub fn boot_kernel() -> anyhow::Result<Arc<CarrierKernel>> {
    let kernel = CarrierKernel::boot(None)?;
    let kernel = Arc::new(kernel);
    kernel.set_self_handle();
    kernel.start_background_agents();
    Ok(kernel)
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
        Ok(()) => tracing::info!(path = %path.display(), "已写入 brain.json 骨架（待配置 brain）"),
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
