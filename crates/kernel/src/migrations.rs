//! migrations — boot 期一次性世界搬家（刀5 no-me，收据 #413）。
//!
//! 「me」是老世界的母体名。09-27 裁决：系统即智能体、不应有 me——系统
//! 本人改名 `system`（家根身份不变）。旧库/旧绑定还带着 me，这里在
//! kernel boot（注册表恢复后）幂等搬家：
//!
//! - agents 表：me 条目原 uuid 改名 system（manifest 同步换名/身份）。
//!   uuid 不变 ⇒ cron 任务、sessions、flow_runs、事件账全部免搬。
//! - 复活体（装机收据 #413 抓到）：system 已在册而 me 又在册——旧世界
//!   的 `<workflows>/me/` 残目录让 host reconcile 又 spawn 出一只新 me
//!   （新 uuid）。搬家后摘除复活体（registry + DB；复活体是零历史幽灵，
//!   调度器/能力表里的悬空注册无害，随进程消亡）。
//! - 目录世界：`<workflows>/me/` 里有旧世界的 senders/（微信来信人的
//!   会话史与晨报产物，sender 历史富集的面）——并入家根 senders/
//!   （文件级 union，家根已有的赢），然后把残目录整棵清走。这步必须删：
//!   目录在，me 就每次 boot 复活（与「只搬不删」的 N5 纪律相反，但残
//!   目录本身就是 bug 的根）。
//!
//! 纪律与 N5 迁移器同款：新名已在不抢、失败只告警不挡启动（下次 boot
//! 重试）；senders 并入失败时残目录不清（等下次重试）。

use crate::kernel::CarrierKernel;
use carrier_types::config::{LEGACY_SYSTEM_AGENT_ME, SYSTEM_AGENT};

/// me → system 世界搬家。在 boot 注册表恢复之后调用；幂等。
pub fn migrate_legacy_me(kernel: &CarrierKernel) {
    let system_present = kernel.registry.find_by_name(SYSTEM_AGENT).is_some();

    match kernel.registry.find_by_name(LEGACY_SYSTEM_AGENT_ME) {
        // 经典迁移：me 在册、system 不在 → 原 uuid 改名
        Some(entry) if !system_present => rename_me_to_system(kernel, entry.id),
        // 复活体：system 已在册、me 又在册（残目录 respawn 的零历史幽灵）
        Some(entry) => {
            let id = entry.id;
            if let Err(e) = kernel.registry.remove(id) {
                tracing::warn!(error = %e, "刀5 迁移：复活 me 摘除失败（下次 boot 重试）");
            } else if let Err(e) = kernel.memory.remove_agent(id) {
                tracing::warn!(error = %e, "刀5 迁移：复活 me DB 清除失败（下次 boot 重试）");
            } else {
                tracing::info!(id = %id, "刀5 迁移：摘除复活体 me（system 已在册，残目录产物）");
            }
        }
        None => {}
    }

    // 目录世界：workflows/me 残树（senders 并家根 + 清走）
    migrate_legacy_home(kernel);
}

fn rename_me_to_system(kernel: &CarrierKernel, id: carrier_types::agent::AgentId) {
    // registry + DB：摘下→改名换身份→回注册（name_index 键是 entry.name，
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
    tracing::info!(id = %id, "刀5 迁移完成：me → system（uuid 保持，家根身份不变）");
}

/// 旧世界母体目录 `<workflows>/me/`：senders 并入家根，残树清走。
/// 并入失败不清树（下次 boot 重试）。
fn migrate_legacy_home(kernel: &CarrierKernel) {
    let legacy = kernel
        .config
        .effective_workflows_dir()
        .join(LEGACY_SYSTEM_AGENT_ME);
    if !legacy.is_dir() {
        return;
    }
    let senders_src = legacy.join("senders");
    if senders_src.is_dir() {
        let senders_dst = kernel.config.home_dir.join("senders");
        if let Err(e) = merge_tree(&senders_src, &senders_dst) {
            tracing::warn!(error = %e, "刀5 迁移：senders 并家根失败（残目录保留，下次 boot 重试）");
            return;
        }
        tracing::info!(
            src = %senders_src.display(),
            dst = %senders_dst.display(),
            "刀5 迁移：旧 me senders 并入家根（union，家根已有的赢）"
        );
    }
    match std::fs::remove_dir_all(&legacy) {
        Ok(()) => tracing::info!(path = %legacy.display(), "刀5 迁移：残目录 workflows/me 清走（防复活）"),
        Err(e) => tracing::warn!(error = %e, "刀5 迁移：残目录清走失败（下次 boot 重试）"),
    }
}

/// 文件级 union 合并：src 里有而 dst 没有的补过去；已有的以 dst 为准。
fn merge_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for ent in std::fs::read_dir(src)? {
        let ent = ent?;
        let from = ent.path();
        let to = dst.join(ent.file_name());
        if from.is_dir() {
            merge_tree(&from, &to)?;
        } else if !to.exists() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
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

    /// 复活体（装机收据 #413）：system 已在册、me 又在册（残目录 respawn）
    /// → me 摘除（registry+DB）、system 原样。
    #[test]
    fn respawned_me_is_removed_when_system_present() {
        let k = boot_scratch("ghost");
        spawn(&k, SYSTEM_AGENT);
        spawn(&k, LEGACY_SYSTEM_AGENT_ME);
        spawn(&k, "codex-x");
        migrate_legacy_me(&k);
        assert!(k.registry.find_by_name(LEGACY_SYSTEM_AGENT_ME).is_none(), "复活体摘除");
        assert!(k.registry.find_by_name(SYSTEM_AGENT).is_some(), "system 不动");
        assert!(k.registry.find_by_name("codex-x").is_some(), "旁观者不动");
        // DB 侧也清了：重开一个 kernel（同 data_dir）不复原 me
        let home = k.config.home_dir.clone();
        let data = k.config.data_dir.clone();
        let config = KernelConfig {
            home_dir: home.clone(),
            data_dir: data,
            ..KernelConfig::default()
        };
        let k2 = CarrierKernel::boot_with_config(config).expect("reboot");
        assert!(k2.registry.find_by_name(LEGACY_SYSTEM_AGENT_ME).is_none(), "DB 无 me");
        assert!(k2.registry.find_by_name(SYSTEM_AGENT).is_some());
        let _ = std::fs::remove_dir_all(home);
    }

    /// 目录世界：workflows/me 残树 → senders 并家根（union）+ 残树清走。
    #[test]
    fn legacy_home_dir_senders_merged_and_tree_removed() {
        let k = boot_scratch("home");
        let wf = k.config.effective_workflows_dir();
        let legacy = wf.join(LEGACY_SYSTEM_AGENT_ME);
        std::fs::create_dir_all(legacy.join("senders/front/sessions")).unwrap();
        std::fs::create_dir_all(legacy.join("flows")).unwrap();
        std::fs::write(legacy.join("senders/front/sessions/a.jsonl"), "旧账\n").unwrap();
        std::fs::write(legacy.join("AGENT.json"), "{}").unwrap();
        // 家根已有的同名文件赢（union）
        std::fs::create_dir_all(k.config.home_dir.join("senders/front/sessions")).unwrap();
        std::fs::write(k.config.home_dir.join("senders/front/sessions/a.jsonl"), "新账\n").unwrap();

        migrate_legacy_me(&k);

        assert!(!legacy.exists(), "残树清走");
        assert_eq!(
            std::fs::read_to_string(k.config.home_dir.join("senders/front/sessions/a.jsonl")).unwrap(),
            "新账\n",
            "家根已有的赢"
        );
        let _ = std::fs::remove_dir_all(k.config.home_dir.clone());
    }

    /// merge_tree：多级目录、缺的补、有的保。
    #[test]
    fn merge_tree_union_rules() {
        let tmp = std::env::temp_dir().join(format!("aginx-mig-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let src = tmp.join("src/a/b");
        let dst = tmp.join("dst/a");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("new.txt"), "n").unwrap();
        std::fs::write(src.join("old.txt"), "src版").unwrap();
        std::fs::write(dst.join("old.txt"), "dst版").unwrap();
        merge_tree(&tmp.join("src"), &tmp.join("dst")).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("b/new.txt")).unwrap(), "n", "缺的补");
        assert_eq!(std::fs::read_to_string(dst.join("old.txt")).unwrap(), "dst版", "有的保");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
