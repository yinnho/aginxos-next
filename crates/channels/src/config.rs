//! channel.toml — 频道级默认与策略（DESIGN.md §四：一频道一目录）。
//!
//! 绑定真源是各 `senders/<uid>/session.json` 的 `bind_agent` **字段**
//! （换绑=改字段，`aginx-channels bind`）；此文件只兜未绑定会话的默认
//! 路由与群消息策略。手工可改，daemon 启动时读一次。
//!
//! 文件夹即注册表：`/home/channels/<名>/` 每个目录是一个频道，
//! daemon 扫目录起来，不认识的 type 跳过并告警。

use crate::vocab::home_dir;
use std::path::{Path, PathBuf};

/// `/home/channels/`（AGINX_HOME 下）。
pub fn channels_root() -> PathBuf {
    home_dir().join("channels")
}

/// `/home/channels/<name>/`。
pub fn channel_root(name: &str) -> PathBuf {
    channels_root().join(name)
}

/// 目录名合法性：一档 path component，不收 `.`/`..`/空/NUL/斜杠。
pub fn valid_channel_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0')
        && name.chars().all(|c| !c.is_whitespace())
}

#[derive(Debug, serde::Deserialize)]
pub struct ChannelConfig {
    /// 频道类型（=词表里的腿名，与 session.json 的 channel 字段同源）。
    #[serde(rename = "type", default = "default_channel_type")]
    pub channel_type: String,
    /// 未绑定会话的兜底路由（gateway 名册里的 agent 名）。
    #[serde(default = "default_agent")]
    pub default_agent: String,
    #[serde(default)]
    pub policy: Policy,
}

#[derive(Debug, serde::Deserialize)]
pub struct Policy {
    /// 只收绑定号（扫码人）本人的消息，陌生人弃（记日志）。
    ///
    /// 注意 iLink 伴生协议里 group_id **不是群聊判定**：扫码号本人与
    /// bot 的会话本身带 group_id（v0.1.1 首条真考实抓：from=绑定号
    /// 本人、group_id 非空——旧母体世界 is_group 从无人消费）。
    /// 绑定判定见 token.rs `is_bound_sender`。
    #[serde(default = "default_true")]
    pub bound_only: bool,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            channel_type: default_channel_type(),
            default_agent: default_agent(),
            policy: Policy::default(),
        }
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self { bound_only: true }
    }
}

fn default_channel_type() -> String {
    "weixin".to_string()
}
fn default_agent() -> String {
    "system".to_string()
}
fn default_true() -> bool {
    true
}

impl ChannelConfig {
    /// 读 `<root>/channel.toml`；缺文件/坏文件=默认值（warn 不殉启——
    /// 频道守护先起来，配置再补）。
    pub fn load(root: &Path) -> Self {
        let path = root.join("channel.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => cfg,
                Err(e) => {
                    eprintln!("aginx-channels: bad {}: {e} — using defaults", path.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// 首启播种默认 channel.toml（已存在不动——文件即真源）。type 以
    /// 目录名为缺省（目录名=频道名；词表认识的腿才真的起）。
    pub fn seed_default_file(root: &Path, name: &str) {
        let path = root.join("channel.toml");
        if path.exists() || std::fs::create_dir_all(root).is_err() {
            return;
        }
        let body = format!(
            concat!(
                "# aginx-channels 频道配置（一频道一目录，DESIGN.md §四）。绑定真源=",
                "session.json 的\n# bind_agent 字段（换绑=改字段）；本文件只管频道级默认与策略。\n",
                "type = \"{name}\"\n",
                "default_agent = \"system\"\n",
                "\n",
                "[policy]\n",
                "bound_only = true   # 只收绑定号（扫码人）本人的消息，陌生人弃\n",
            ),
            name = name,
        );
        let _ = std::fs::write(&path, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// bound_only 缺省=只收绑定号本人（v0.1.2 起 iLink 政策）。
    #[test]
    fn policy_defaults_to_bound_only() {
        let p: Policy = toml::from_str("").unwrap();
        assert!(p.bound_only);
        assert!(Policy::default().bound_only);
    }

    /// v0.1.1 播种过的存量文件写着旧键 dm_only——改名后该文件必须照常
    /// 起频道（未知键忽略、bound_only 走缺省 true），不殉存量部署。
    #[test]
    fn legacy_dm_only_file_still_loads() {
        let cfg: ChannelConfig = toml::from_str(
            r#"
            type = "weixin"
            default_agent = "system"

            [policy]
            dm_only = true
            "#,
        )
        .unwrap();
        assert!(cfg.policy.bound_only);
        assert_eq!(cfg.channel_type, "weixin");
    }

    /// 显式放开：bound_only = false 收陌生人（对外服务宿主面）。
    #[test]
    fn bound_only_can_be_disabled() {
        let p: Policy = toml::from_str("bound_only = false").unwrap();
        assert!(!p.bound_only);
    }
}
