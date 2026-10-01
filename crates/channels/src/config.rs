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
    /// v0.1 只接单聊；群消息收到即弃（记日志）。
    #[serde(default = "default_true")]
    pub dm_only: bool,
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
        Self { dm_only: true }
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
                "dm_only = true      # v0.1 只接单聊，群消息收到即弃\n",
            ),
            name = name,
        );
        let _ = std::fs::write(&path, body);
    }
}
