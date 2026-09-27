//! migrations — boot 期一次性世界搬家（刀5 no-me，收据 #413）。
//!
//! 「me」是老世界的母体名。09-27 裁决：系统即智能体、不应有 me——系统
//! 本人改名 `system`（家根身份不变）。旧库/旧绑定还带着 me，这里在
//! kernel boot（注册表恢复后）幂等搬家：
//!
//! - agents 表：me 条目原 uuid 改名 system（manifest 同步换名/身份）。
//!   uuid 不变 ⇒ cron 任务、sessions、flow_runs、事件账全部免搬。
//! - weixin_sessions：bind_agent 'me' → 'system'（SenderRouter 是 boot
//!   期从这张表播种的内存表，改表即改路由）。
//!
//! 纪律与 N5 迁移器同款：只搬不删、新名已在不抢（system 在册=迁移完
//! 成，直接返回）、失败只告警不挡启动（下次 boot 重试）。

use crate::kernel::CarrierKernel;
use carrier_types::config::{LEGACY_SYSTEM_AGENT_ME, SYSTEM_AGENT};

/// me → system 世界搬家。在 boot 注册表恢复之后调用；幂等。
pub fn migrate_legacy_me(kernel: &CarrierKernel) {
    if kernel.registry.find_by_name(SYSTEM_AGENT).is_some() {
        return; // 新世界（或已迁移）
    }
    let Some(entry) = kernel.registry.find_by_name(LEGACY_SYSTEM_AGENT_ME) else {
        return; // 裸库/新装：无 me 无事可做
    };
    let id = entry.id;

    // ① registry + DB：摘下→改名换身份→回注册（name_index 键是 entry.name，
    // 得走 remove+register；uuid/id 不变）。update_manifest 不动 name 列。
    let mut entry = match kernel.registry.remove(id) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(error = %e, "刀5 迁移：registry 摘除失败（下次 boot 重试）");
            return;
        }
    };
    entry.name = SYSTEM_AGENT.to_string();
    entry.manifest.name = SYSTEM_AGENT.to_string();
    entry.manifest.display_name = "系统".to_string();
    entry.manifest.description = "系统本体 — OS 进程即智能体；对主人是总管，对外是门面（家根身份，无 workflows 也是智能体）".to_string();
    if let Err(e) = kernel.registry.register(entry.clone()) {
        tracing::warn!(error = %e, "刀5 迁移：registry 回注册失败（下次 boot 重试）");
        return;
    }
    if let Err(e) = kernel.memory.save_agent(&entry) {
        tracing::warn!(error = %e, "刀5 迁移：DB 重存失败（下次 boot 重试）");
        return;
    }

    // ② weixin 绑定行：名字搬家（路由 boot 播种自这张表）
    match kernel.memory.weixin_store().rename_bind_agent(LEGACY_SYSTEM_AGENT_ME, SYSTEM_AGENT) {
        Ok(n) if n > 0 => tracing::info!(rows = n, "刀5 迁移：weixin 绑定行 me → system"),
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "刀5 迁移：weixin 绑定行改名失败（下次 boot 重试）"),
    }

    tracing::info!(id = %id, "刀5 迁移完成：me → system（uuid 保持，家根身份不变）");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::CarrierKernel;
    use carrier_types::agent::AgentManifest;
    use carrier_types::config::KernelConfig;

    fn boot_scratch(tag: &str) -> CarrierKernel {
        let home = std::env::temp_dir().join(format!("aginx-mig-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        // 最小 brain.json（boot 硬依赖；brain=OpenAI 格式 API 单端点扁平
        // 形状，只认 base_url——见 carrier_types::brain::BrainConfig）
        let brain = serde_json::json!({ "base_url": "http://127.0.0.1:1/v1/chat" });
        std::fs::write(home.join("brain.json"), brain.to_string()).unwrap();
        let config = KernelConfig {
            home_dir: home.clone(),
            data_dir: home.join("data"),
            ..Default::default()
        };
        CarrierKernel::boot_with_config(config).expect("boot")
    }

    fn spawn(kernel: &CarrierKernel, name: &str) -> carrier_types::agent::AgentId {
        kernel
            .spawn_agent(AgentManifest {
                name: name.to_string(),
                ..Default::default()
            })
            .expect("spawn")
    }

    #[test]
    fn me_renamed_idempotent_and_untouched_without_me() {
        let k = boot_scratch("me");
        let me_id = spawn(&k, LEGACY_SYSTEM_AGENT_ME);
        spawn(&k, "codex-x"); // 旁观者不动
        migrate_legacy_me(&k);
        let sys = k.registry.find_by_name(SYSTEM_AGENT).expect("system 在册");
        assert_eq!(sys.id, me_id, "uuid 保持");
        assert_eq!(sys.manifest.display_name, "系统");
        assert!(k.registry.find_by_name(LEGACY_SYSTEM_AGENT_ME).is_none());
        assert!(k.registry.find_by_name("codex-x").is_some(), "旁观者不动");
        // 幂等：再跑不炸不抢
        spawn(&k, "later");
        migrate_legacy_me(&k);
        assert_eq!(k.registry.find_by_name(SYSTEM_AGENT).unwrap().id, me_id);
        let _ = std::fs::remove_dir_all(k.config.home_dir.clone());
    }

    #[test]
    fn no_me_is_noop() {
        let k = boot_scratch("bare");
        migrate_legacy_me(&k);
        assert!(k.registry.find_by_name(SYSTEM_AGENT).is_none());
        let _ = std::fs::remove_dir_all(k.config.home_dir.clone());
    }
}
