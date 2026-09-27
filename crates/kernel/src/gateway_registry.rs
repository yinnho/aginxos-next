//! gateway_registry — 安装链三写的对外两笔（刀3，2026-09-27）。
//!
//! 裁决（docs/PLAN-真aginx入机.md 裁定1/裁定4，刀3）：安装 workflows/<名>
//! 助理时除 kernel 注册表（DB，spawn_agent 已写）外同步写两件——
//!   ① 真 aginx 的 agents 条目 `<data>/agents/<名>/aginx.toml`：raw 方言
//!      + `aginx-carrier acp --clone <名>`（crates/carrier/src/acp.rs 双模
//!      桥的裸行模），复刻 Mac 旧母体 clone_install 的「分身装好即入网，
//!      删除分身时自动移除」。写后网关需重启才看得见（AgentManager 启动
//!      期建册，不热扫）。
//!   ② `home/workflows.md` 能力注册表行（裁定1 的唯一真源写腿；读腿=
//!      花名册发现面，刀5）。
//! 卸载对称两删；重装=upsert 覆盖（clone_install_files 重跑即刷新）。
//!
//! 路径规则与生态仓 data_dir() 同律，外加收敛指针：`AGINX_DATA_DIR`
//! 优先，否则跟随 `<home>/gateway-data` 指针（symlink），否则 ~/.aginx
//! ——设备上母体单元带 AGINX_DATA_DIR=/var/lib/aginx/gateway 与网关守护
//! 同世界（pkgs/aginx [service] envs）；首次带 env 写入时在 home 根落
//! 指针自愈（装机收据 #411：装带 env、卸裸跑曾劈叉成两个世界）。
//! host 裸跑无 env 无指针落 ~/.aginx（Mac 同款）。
//! 真身守护不感知本模块存在——它只认目录里的 toml，安装链是唯一写手。

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use carrier_types::config::KernelConfig;

/// home 根的收敛指针名（symlink → 数据根）。
const HOME_POINTER: &str = "gateway-data";

/// workflows.md 固定头（创建时写；后续 upsert 保留原文件只动条目行）。
const ROSTER_HEADER: &str = "# workflows 助理能力注册表\n\
     \n\
     <!-- aginx 安装链自动维护（agent install/remove 同步写）；\n\
     系统智能体读此册知编制、据此派活。行格式：- `名`: 显示名 —— 描述 -->\n";

/// 条目行键（解析侧刀5 对齐）：`- `name`: …`
fn roster_line(name: &str, display: &str, desc: &str) -> String {
    let desc = if desc.is_empty() { "（无描述）" } else { desc };
    format!("- `{}`: {} —— {}", name, display, desc)
}

fn roster_key(name: &str) -> String {
    format!("- `{}`:", name)
}

/// 真 aginx 数据根：env 优先 → home 指针跟随 → ~/.aginx 兜底（生态
/// data_dir 同律 + 收敛指针）。
fn env_data_root() -> Option<PathBuf> {
    std::env::var_os("AGINX_DATA_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 跟随 `<home>/gateway-data` 指针（目标须是现存目录；悬空=没指）。
fn follow_home_pointer(home: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(home.join(HOME_POINTER)).ok()?;
    let t = PathBuf::from(target);
    if t.is_dir() {
        Some(t)
    } else {
        None
    }
}

fn resolve_data_root(home: &Path) -> PathBuf {
    data_root_order(env_data_root().as_deref(), home)
}

/// 解析序（纯函数腿，测试用）：env → home 指针 → ~/.aginx。
fn data_root_order(env: Option<&Path>, home: &Path) -> PathBuf {
    if let Some(r) = env {
        return r.to_path_buf();
    }
    if let Some(r) = follow_home_pointer(home) {
        return r;
    }
    dirs::home_dir()
        .map(|h| h.join(".aginx"))
        .unwrap_or_else(|| PathBuf::from(".aginx"))
}

/// 带 env 写入时落/刷 home 指针（幂等；指错目标则换指）——让后续无 env
/// 的 CLI 面收敛到同一数据世界。
fn ensure_home_pointer(home: &Path, data_root: &Path) {
    let link = home.join(HOME_POINTER);
    match std::fs::read_link(&link) {
        Ok(t) if PathBuf::from(t.clone()) == data_root => return,
        Ok(_) => {
            let _ = std::fs::remove_file(&link);
        }
        Err(_) => {}
    }
    if let Err(e) = std::os::unix::fs::symlink(data_root, &link) {
        tracing::debug!(error = %e, "home 指针落盘失败（无 env 的后续调用仍走 ~/.aginx）");
    }
}

/// toml 字符串字面量（转义反斜杠/引号，压掉换行——描述单行纪律）。
fn toml_str(s: &str) -> String {
    let flat: String = s.chars().filter(|c| *c != '\n' && *c != '\r').collect();
    format!("\"{}\"", flat.replace('\\', "\\\\").replace('"', "\\\""))
}

/// PATH 步进找 aginx-carrier 可执行位（找到给绝对路径，稳定于网关守护
/// 的任意 cwd/PATH；找不到裸名兜底——交给 spawn 的 PATH 解析，装不上
/// 网关条目照样写，坏在对话轮错误路径里现形）。
fn carrier_bin() -> String {
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let cand = dir.join("aginx-carrier");
            if cand.is_file() {
                if let Ok(meta) = std::fs::metadata(&cand) {
                    if meta.permissions().mode() & 0o111 != 0 {
                        return cand.display().to_string();
                    }
                }
            }
        }
    }
    "aginx-carrier".to_string()
}

/// ① agents 条目落盘（upsert：目录存在即覆盖重写）。
pub fn write_entry(root: &Path, name: &str, display: &str, desc: &str) -> std::io::Result<()> {
    let dir = root.join("agents").join(name);
    std::fs::create_dir_all(&dir)?;
    let toml = format!(
        "# 由 AginxOS 安装链自动写（agent install/remove 维护）——真 aginx 的\n\
         # 对外直通条目：agent://<机>.relay.aginx.net/{name}。raw 方言=carrier\n\
         # 双模桥裸行模（crates/carrier/src/acp.rs）。\n\
         id = {}\n\
         name = {}\n\
         agent_type = \"aginx-carrier\"\n\
         description = {}\n\
         output = \"raw\"\n\
         timeout = 300\n\
         \n\
         [command]\n\
         path = {}\n\
         args = [\"acp\", \"--clone\", {}]\n\
         \n\
         [session]\n\
         resume_args = [\"--session\", \"${{SESSION_ID}}\"]\n",
        toml_str(name),
        toml_str(display),
        toml_str(desc),
        toml_str(&carrier_bin()),
        toml_str(name),
        name = name,
    );
    std::fs::write(dir.join("aginx.toml"), toml)
}

/// ① 的删除（幂等：不存在=成功）。
pub fn remove_entry(root: &Path, name: &str) -> std::io::Result<()> {
    let dir = root.join("agents").join(name);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// ② workflows.md 行 upsert（按键 `- `name`:` 替换；全册按名排序，重写
/// 保持确定性）。文件不存在带头创建。
pub fn roster_upsert(home_root: &Path, name: &str, display: &str, desc: &str) -> std::io::Result<()> {
    let path = home_root.join("workflows.md");
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .filter(|l| !l.starts_with(&roster_key(name)))
        .collect();
    lines.push(roster_line(name, display, desc));
    // 只排序条目行（`- \`开头），头注与空行保位。
    let mut entries: Vec<String> = lines.iter().filter(|l| l.starts_with("- `")).cloned().collect();
    let rest: Vec<String> = lines
        .into_iter()
        .filter(|l| !l.starts_with("- `"))
        .collect();
    entries.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    let mut out = String::new();
    if !path.exists() {
        out.push_str(ROSTER_HEADER);
    }
    for l in &rest {
        out.push_str(l);
        out.push('\n');
    }
    for l in &entries {
        out.push_str(l);
        out.push('\n');
    }
    std::fs::write(&path, out)
}

/// ② 的行删除（幂等）。
pub fn roster_remove(home_root: &Path, name: &str) -> std::io::Result<()> {
    let path = home_root.join("workflows.md");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let filtered: String = content
        .lines()
        .filter(|l| !l.starts_with(&roster_key(name)))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&path, filtered)
}

/// 安装链入口：①+② 一并写。失败不回滚安装（对外注册表是外围面，
/// 调用方 warn 降级）。
pub fn register(config: &KernelConfig, name: &str, display: &str, desc: &str) -> std::io::Result<()> {
    let root = resolve_data_root(&config.home_dir);
    if let Some(env_root) = env_data_root() {
        ensure_home_pointer(&config.home_dir, &env_root);
    }
    write_entry(&root, name, display, desc)?;
    roster_upsert(&config.home_dir, name, display, desc)
}

/// 卸载链入口：①+② 一并摘。
pub fn unregister(config: &KernelConfig, name: &str) -> std::io::Result<()> {
    remove_entry(&resolve_data_root(&config.home_dir), name)?;
    roster_remove(&config.home_dir, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aginx-gwreg-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pointer_convergence_env_wins_then_home_pointer() {
        let home = tmp("ptr-home");
        let data = tmp("ptr-data");
        std::fs::create_dir_all(data.join("agents")).unwrap();
        // 无 env 无指针 → ~/.aginx 兜底（与生态 data_dir 同律）
        assert_eq!(
            data_root_order(None, &home),
            dirs::home_dir().unwrap().join(".aginx")
        );
        // 落指针后无 env 也收敛到数据根
        ensure_home_pointer(&home, &data);
        assert_eq!(data_root_order(None, &home), data);
        // env 永远优先（指针不劫持显式 env）
        let env_root = tmp("ptr-env");
        std::fs::create_dir_all(&env_root).unwrap();
        assert_eq!(data_root_order(Some(&env_root), &home), env_root);
        // 换指（指针目标变更=重指）
        let data2 = tmp("ptr-data2");
        std::fs::create_dir_all(data2.join("agents")).unwrap();
        ensure_home_pointer(&home, &data2);
        assert_eq!(data_root_order(None, &home), data2);
        // 悬空指针=没指（目标目录消失）
        let _ = std::fs::remove_dir_all(&data2);
        assert_eq!(
            data_root_order(None, &home),
            dirs::home_dir().unwrap().join(".aginx")
        );
        for d in [home, data, env_root] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn entry_roundtrip_and_remove_idempotent() {
        let d = tmp("entry");
        write_entry(&d, "ai-writer", "AI Writer", "写东西").unwrap();
        let toml = std::fs::read_to_string(d.join("agents/ai-writer/aginx.toml")).unwrap();
        assert!(toml.contains("id = \"ai-writer\""));
        assert!(toml.contains("output = \"raw\""));
        assert!(toml.contains("args = [\"acp\", \"--clone\", \"ai-writer\"]"));
        assert!(toml.contains("resume_args = [\"--session\", \"${SESSION_ID}\"]"));
        // 描述带引号也稳
        write_entry(&d, "quo'te", "带\"引\"号", "desc \\ slash").unwrap();
        let t2 = std::fs::read_to_string(d.join("agents/quo'te/aginx.toml")).unwrap();
        assert!(t2.contains("\\\"引\\\""));
        // upsert 覆盖 + 删除幂等
        write_entry(&d, "ai-writer", "AI Writer 2", "改").unwrap();
        remove_entry(&d, "ai-writer").unwrap();
        remove_entry(&d, "ai-writer").unwrap();
        assert!(!d.join("agents/ai-writer").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn roster_upsert_replace_and_remove() {
        let d = tmp("roster");
        roster_upsert(&d, "b-agent", "B", "第二个").unwrap();
        roster_upsert(&d, "a-agent", "A", "第一个").unwrap();
        let m = std::fs::read_to_string(d.join("workflows.md")).unwrap();
        let a = m.find("- `a-agent`:").expect("a 在册");
        let b = m.find("- `b-agent`:").expect("b 在册");
        assert!(a < b, "条目行按名排序：\n{m}");
        assert!(m.contains("能力注册表"), "新建带头");
        // upsert 换描述不增行
        roster_upsert(&d, "a-agent", "A", "改描述").unwrap();
        let m2 = std::fs::read_to_string(d.join("workflows.md")).unwrap();
        assert_eq!(m2.matches("- `a-agent`").count(), 1);
        assert!(m2.contains("改描述"));
        // 删行 + 幂等
        roster_remove(&d, "b-agent").unwrap();
        roster_remove(&d, "b-agent").unwrap();
        let m3 = std::fs::read_to_string(d.join("workflows.md")).unwrap();
        assert!(!m3.contains("`b-agent`"));
        assert!(m3.contains("`a-agent`"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
