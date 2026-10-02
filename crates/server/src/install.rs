//! install — 助理装卸链（刀④-3 搬家，2026-10-02）。
//!
//! 原居 `aginx-carrier agent install/remove`（kernel clone_install_files
//! 管线：格式校验→落盘→spawn→入网）。引擎商品化（DESIGN.md §八）后新
//! 装助理**不入 kernel 注册表、不起 in-kernel 腿**——格式层落盘
//! `workflows/<名>/` + 两笔对外写（gateway agents toml（codex 形）+
//! workflows.md 名册行），对话直达 `agent://<机>.relay.aginx.net/<名>`
//! （网关 spawn-on-demand codex，人格=工位 AGENTS.md）。kernel 侧
//! clone_install_files（clone-creator [CLONE_INSTALL] 桥）过渡期并存，
//! 随 ④-4 crate 拆迁收口。
//!
//! 相对旧管线的裁剪：agent.toml（kernel manifest）不再写——codex 条目
//! 不读它；flows/self-growth 种子不再落——flow 机制随引擎退役；
//! knowledge/format-spec.md 仍种（clone-creator 读现行格式规则）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use carrier_kernel::gateway_registry;
use carrier_types::config::{KernelConfig, SYSTEM_AGENT};

#[derive(Debug)]
pub struct Installed {
    pub name: String,
    pub display: String,
    pub description: String,
    pub file_count: usize,
    pub warnings: Vec<String>,
}

/// 从本地 tar 包安装（DupHub 拉取腿归 dup/duphub 线，此面只认本地包）。
pub fn install_tar(
    config: &KernelConfig,
    name: &str,
    tar_path: &Path,
) -> Result<Installed, String> {
    let files =
        carrier_clone::tar_source::read_clone_tar(tar_path).map_err(|e| format!("读取安装包失败：{e}"))?;
    install(config, name, files)
}

/// 装链本体：校验→落盘→两写。重装=清旧（`.dup/` 版本历史保留）。
pub fn install(
    config: &KernelConfig,
    name: &str,
    files: BTreeMap<String, Vec<u8>>,
) -> Result<Installed, String> {
    if !carrier_clone::market::valid_clone_name(name) {
        return Err("化身名只允许小写字母、数字与连字符（1-64 位）".into());
    }
    if name == SYSTEM_AGENT {
        return Err("system 是系统本人（家根身份），不能作为助理安装".into());
    }
    let ws = config.effective_workflows_dir().join(name);
    if !ws.starts_with(config.effective_workflows_dir()) {
        return Err("path traversal denied".into());
    }
    if ws.exists() {
        clear_preserving_dup(&ws)?;
    }
    let mut files = files;
    if carrier_clone::manifest::ensure_template_version(&mut files) {
        // DupHub listing 元数据需要 version；缺失补 "1"（不覆盖——防 dup 抖动）
    }
    let errors = carrier_clone::manifest::validate_install_format(&files)
        .map_err(|e| format!("format validation: {e}"))?;
    if !errors.is_empty() {
        return Err(format!(
            "格式校验未通过（{} 项，修复后重新提交）：\n- {}",
            errors.len(),
            errors.join("\n- ")
        ));
    }
    let warnings = carrier_clone::manifest::write_files_to_workspace(&files, &ws).map_err(
        |e| {
            let _ = std::fs::remove_dir_all(&ws);
            format!("落盘失败：{e}")
        },
    )?;
    seed_format_spec(&ws);
    let template = std::fs::read_to_string(ws.join("template.json"))
        .ok()
        .and_then(|s| carrier_clone::parse_template_manifest_lenient(&s));
    let display = template
        .as_ref()
        .map(|t| t.display_name.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| name.to_string());
    let description = template
        .as_ref()
        .map(|t| t.description.trim().to_string())
        .unwrap_or_default();
    gateway_registry::register(config, name, &display, &description)
        .map_err(|e| format!("对外注册表写入失败（agents toml / workflows.md）：{e}"))?;
    Ok(Installed {
        name: name.to_string(),
        display,
        description,
        file_count: files.len(),
        warnings,
    })
}

/// 卸载：工位整删（含 `.dup/`——卸载即断根，重装语义才干净）+ 对外
/// 两笔摘除。册上没有（工位不存在）= 明确报错，不幂等静过。
pub fn remove(config: &KernelConfig, name: &str) -> Result<String, String> {
    if name == SYSTEM_AGENT {
        return Err("system 是系统本人（家根身份），不可卸载".into());
    }
    let ws = config.effective_workflows_dir().join(name);
    if !ws.is_dir() {
        return Err(format!("本机没有叫 {name} 的助理"));
    }
    std::fs::remove_dir_all(&ws).map_err(|e| format!("删除工位失败：{e}"))?;
    gateway_registry::unregister(config, name)
        .map_err(|e| format!("对外注册表摘除失败（agents toml / workflows.md）：{e}"))?;
    Ok(format!(
        "已卸载 {name}（工位已删）；aginx-svc restart aginx-gateway 后网关名册生效"
    ))
}

/// 重装清旧：保留 `.dup/`（dup-push 版本历史），其余全清。
fn clear_preserving_dup(ws: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(ws).map_err(|e| format!("读取旧工位失败：{e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy() == ".dup" {
            continue;
        }
        let p: PathBuf = entry.path();
        let _ = if p.is_dir() {
            std::fs::remove_dir_all(&p)
        } else {
            std::fs::remove_file(&p)
        };
    }
    Ok(())
}

/// 种 knowledge/format-spec.md（系统件，带版本戳）——clone-creator 与
/// 任何代理读现行格式规则，不靠各自定义层里的陈旧副本。
fn seed_format_spec(ws: &Path) {
    let spec_path = ws.join("knowledge/format-spec.md");
    if let Some(parent) = spec_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let stamped = format!(
        "<!-- clone-format-spec {} (system-seeded; do not edit) -->\n{}",
        carrier_clone::CLONE_FORMAT_SPEC_VERSION, carrier_clone::CLONE_FORMAT_SPEC
    );
    let _ = std::fs::write(spec_path, stamped);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立 home 世界：workflows 显式、数据根走 gateway-data 指针收敛
    /// （无 env 依赖，不落 ~/.aginx，测试间零竞态）。
    fn config(tag: &str) -> (KernelConfig, PathBuf) {
        let home =
            std::env::temp_dir().join(format!("aginx-install-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join("data")).unwrap();
        std::os::unix::fs::symlink(home.join("data"), home.join("gateway-data")).unwrap();
        let mut cfg = KernelConfig::default();
        cfg.home_dir = home.clone();
        cfg.workflows_dir = Some(home.join("workflows"));
        (cfg, home)
    }

    fn bundle() -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        files.insert(
            "template.json".to_string(),
            "{\"name\":\"note-taker\",\"display_name\":\"记事佬\",\"description\":\"记事\",\"version\":\"1\"}"
                .as_bytes()
                .to_vec(),
        );
        files.insert("AGENTS.md".to_string(), "你是记事佬。".as_bytes().to_vec());
        files
    }

    /// ④-3 主线：装=落盘+codex 形条目+名册行；重装=.dup 保留；卸=三删。
    #[test]
    fn install_remove_roundtrip() {
        let (cfg, home) = config("rt");
        let r = install(&cfg, "note-taker", bundle()).unwrap();
        assert_eq!(r.display, "记事佬");
        assert_eq!(r.file_count, 2);
        assert!(home.join("workflows/note-taker/AGENTS.md").is_file());
        assert!(home.join("workflows/note-taker/knowledge/format-spec.md").is_file());
        let toml =
            std::fs::read_to_string(home.join("data/agents/note-taker/aginx.toml")).unwrap();
        assert!(toml.contains("agent_type = \"codex\""));
        assert!(!toml.contains("acp"), "助理条目不再走 carrier acp 桥");
        let roster = std::fs::read_to_string(home.join("workflows.md")).unwrap();
        assert!(roster.contains("`note-taker`: 记事佬"));
        // 重装：.dup/ 版本历史保留
        std::fs::create_dir_all(home.join("workflows/note-taker/.dup")).unwrap();
        std::fs::write(home.join("workflows/note-taker/.dup/keep"), b"x").unwrap();
        install(&cfg, "note-taker", bundle()).unwrap();
        assert!(home.join("workflows/note-taker/.dup/keep").is_file());
        // 卸载：三删（工位/条目/名册行）
        remove(&cfg, "note-taker").unwrap();
        assert!(!home.join("workflows/note-taker").exists());
        assert!(!home.join("data/agents/note-taker").exists());
        assert!(!std::fs::read_to_string(home.join("workflows.md"))
            .unwrap()
            .contains("note-taker"));
        // 册上没有=报错；system 不可装不可卸
        assert!(remove(&cfg, "note-taker").is_err());
        assert!(install(&cfg, SYSTEM_AGENT, bundle()).is_err());
        assert!(remove(&cfg, SYSTEM_AGENT).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 格式门：skills/ 死布局拒装且不落盘。
    #[test]
    fn install_rejects_dead_format() {
        let (cfg, home) = config("dead");
        let mut files = bundle();
        files.insert("skills/x/SKILL.md".to_string(), b"---\nname: x\n---".to_vec());
        let err = install(&cfg, "note-taker", files).unwrap_err();
        assert!(err.contains("skills"), "err: {err}");
        assert!(!home.join("workflows/note-taker").exists(), "拒装不落盘");
        let _ = std::fs::remove_dir_all(&home);
    }
}
