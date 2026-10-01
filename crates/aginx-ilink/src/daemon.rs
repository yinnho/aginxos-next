//! weixin 频道守护：入站轮询（SessionWatcher）→ 路由 → ACP → 回信。
//!
//! 启动序（顺序有讲究）：
//! 1. 旧世界一次性迁移：`workflows/*/senders/*/session.json` 与家根
//!    `senders/*/session.json`（刀5 形状）里的 weixin 会话搬进频道家
//!    `channels/weixin/senders/<uid>/`——必须在 `set_sessions_root` 之前
//!    走（此后 token 扫描只看频道布局，旧文件成为死数据）。
//! 2. `set_sessions_root` → SessionWatcher 起（内部自带 5s respawn 拾
//!    新扫码，换绑同路拾取——文件字段即真源）。
//! 3. 桥循环：PluginMessage → 选 agent（账号 bind_agent 字段 →
//!    channel.toml default_agent → "system"）→ 每发信人一个串行锁
//!    （跨发信人并行；ACP 回合可能数分钟，不能全局串）。
//!
//! gw.json：`senders/<peer>/gw.json` {agent, session_id}——该发信人当前
//! 对话挂在哪个 agent、哪条引擎会话上；换绑（bind 字段改了）后第一条
//! 消息自动开新线程（agent 不一致即弃旧 sessionId）。

use crate::acp::AcpClient;
use crate::config::{channel_root, ChannelConfig};
use anyhow::Result;
use carrier_ilink::channel::SessionWatcher;
use carrier_types::channel::Channel;
use carrier_types::plugin::{PluginContent, PluginMessage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

/// 每发信人的引擎对话状态（落在 senders/<peer>/gw.json）。
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct GwState {
    agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
}

pub fn run() -> Result<()> {
    let root = channel_root();
    std::fs::create_dir_all(&root)?;
    ChannelConfig::seed_default_file(&root);

    let migrated = migrate_legacy_sessions(&root);
    if migrated > 0 {
        info!(count = migrated, root = %root.display(), "migrated legacy weixin sessions into the channel home");
    }

    // 从这里起，本进程的会话世界=频道布局（绑定=字段）。
    carrier_ilink::token::set_sessions_root(&root);
    let config = ChannelConfig::load(&root);
    info!(root = %root.display(), channel = %config.channel_type, default_agent = %config.default_agent, "aginx-ilink daemon starting");

    // Channel trait 的入站口是 tokio mpsc（channel.rs 线权）；桥线程从
    // async 上下文收，转发进 std 世界的处理线程（ACP 是阻塞 IO）。
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()?;
    rt.block_on(async move { bridge_loop(root, config).await })
}

async fn bridge_loop(root: std::path::PathBuf, config: ChannelConfig) -> Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<PluginMessage>(1024);
    let mut watcher = SessionWatcher::new();
    if let Err(e) = watcher.start(tx) {
        anyhow::bail!("weixin SessionWatcher failed to start: {e}");
    }

    let acp = Arc::new(AcpClient::new());
    let locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>> = Arc::new(Mutex::new(HashMap::new()));
    let root = Arc::new(root);
    let config = Arc::new(config);

    while let Some(msg) = rx.recv().await {
        if config.policy.dm_only && msg.is_group {
            info!(sender = %msg.sender_id, "dropped group message (dm_only)");
            continue;
        }
        let lock = {
            let mut map = locks.lock().unwrap();
            map.entry(msg.sender_id.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let acp = acp.clone();
        let cfg = config.clone();
        let root = root.clone();
        let sender_id = msg.sender_id.clone();
        std::thread::Builder::new()
            .name(format!("ilink-bridge-{sender_id}"))
            .spawn(move || {
                let _guard = lock.lock().unwrap();
                if let Err(e) = bridge_one(&root, &cfg, &acp, msg) {
                    error!(sender = %sender_id, error = %e, "bridge turn failed");
                }
            })
            .ok();
    }
    Ok(())
}

/// 单条入站消息的完整回合（调用方已持有该发信人的串行锁）。
fn bridge_one(
    root: &std::path::Path,
    config: &ChannelConfig,
    acp: &AcpClient,
    msg: PluginMessage,
) -> Result<()> {
    let text = match &msg.content {
        PluginContent::Text(t) => t.clone(),
        PluginContent::Voice { .. } => "（语音消息：本频道 v0.1 只接文字）".to_string(),
        PluginContent::Image { .. } => "（图片消息：本频道 v0.1 只接文字）".to_string(),
        PluginContent::File { filename, .. } => {
            format!("（收到文件 {filename}：本频道 v0.1 只接文字）")
        }
        PluginContent::Video { .. } => "（视频消息：本频道 v0.1 只接文字）".to_string(),
        other => format!("（暂不支持的消息类型 {other:?}）"),
    };

    // 路由：账号 bind_agent 字段 → channel.toml default_agent。
    let account_bind = carrier_ilink::token::WEIXIN_STATE
        .bots
        .iter()
        .find(|e| e.value().bot_id == msg.bot_id)
        .and_then(|e| e.value().bind_agent.clone())
        .filter(|a| !a.is_empty());
    let agent = account_bind
        .clone()
        .unwrap_or_else(|| config.default_agent.clone());

    let gw_path = gw_json_path(root, &msg.sender_id);
    let mut gw = read_gw(&gw_path);
    // 换绑后第一条消息：agent 变了，旧引擎会话不是这条腿的上下文，弃。
    if !gw.agent.is_empty() && gw.agent != agent {
        info!(sender = %msg.sender_id, from = %gw.agent, to = %agent, "rebound — starting a fresh engine session");
        gw = GwState::default();
    }

    info!(sender = %msg.sender_id, agent = %agent, session = ?gw.session_id, "bridge turn");
    let reply = match acp.prompt(&agent, &text, gw.session_id.as_deref()) {
        Ok(r) => {
            gw.agent = agent;
            gw.session_id = r.session_id;
            write_gw(&gw_path, &gw);
            r.text
        }
        Err(e) => {
            warn!(sender = %msg.sender_id, agent = %agent, error = %e, "ACP turn failed — replying honestly");
            format!("（频道桥开小差：{e}）")
        }
    };

    if reply.trim().is_empty() {
        info!(sender = %msg.sender_id, "engine reply empty — nothing to send");
        return Ok(());
    }
    let watcher = SessionWatcher::new();
    watcher
        .send(&msg.bot_id, &msg.sender_id, &reply)
        .map_err(|e| anyhow::anyhow!("weixin send failed: {e}"))
}

fn gw_json_path(root: &std::path::Path, sender: &str) -> PathBuf {
    root.join("senders").join(sender).join("gw.json")
}

fn read_gw(path: &std::path::Path) -> GwState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_gw(path: &std::path::Path, gw: &GwState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(gw) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

/// 一次性旧世界迁移（daemon 每次启动跑，幂等：目标在=只清源）。
/// 扫两处 legacy 布局：`workflows/*/senders/*/session.json`（分身轴）
/// 与 `senders/*/session.json`（家根，刀5 v0.1.12 搬家后的落点）。
/// 绑定语义随迁翻转：legacy 目录=真源 → 频道字段=真源（迁移时把
/// 目录名写进 bind_agent 字段，文件自带绑定）。
fn migrate_legacy_sessions(root: &std::path::Path) -> usize {
    let home = carrier_types::config::home_dir();
    let mut moved = 0;

    // (path, dir_asserted_agent) 收集
    let mut legacy: Vec<(PathBuf, Option<String>)> = Vec::new();
    if let Ok(workflows) = std::fs::read_dir(home.join("workflows")) {
        for agent_entry in workflows.flatten() {
            let agent = agent_entry.file_name().to_string_lossy().to_string();
            if let Ok(senders) = std::fs::read_dir(agent_entry.path().join("senders")) {
                for s in senders.flatten() {
                    let p = s.path().join("session.json");
                    if p.is_file() {
                        legacy.push((p, Some(agent.clone())));
                    }
                }
            }
        }
    }
    if let Ok(senders) = std::fs::read_dir(home.join("senders")) {
        for s in senders.flatten() {
            let p = s.path().join("session.json");
            if p.is_file() {
                legacy.push((p, None)); // 家根无分身可依，信文件字段
            }
        }
    }

    for (path, dir_agent) in legacy {
        let Ok(content) = std::fs::read_to_string(&path) else { continue };
        let Ok(mut tf) = serde_json::from_str::<carrier_ilink::models::BotTokenFile>(&content)
        else {
            continue;
        };
        if tf.channel != "weixin" {
            continue;
        }
        // 目录断言优先（legacy 不变式），家根信字段本身。
        if let Some(a) = dir_agent {
            tf.bind_agent = Some(a);
        }
        let key = tf.user_id.clone().unwrap_or_else(|| tf.bot_id.clone());
        let dest = root.join("senders").join(&key).join("session.json");
        if !dest.exists() {
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let Ok(json) = serde_json::to_string_pretty(&tf) else {
                warn!(path = %path.display(), "migration serialize failed — keeping source");
                continue;
            };
            let tmp = dest.with_extension("json.tmp");
            if std::fs::write(&tmp, json).is_err() || std::fs::rename(&tmp, &dest).is_err() {
                warn!(path = %path.display(), "migration write failed — keeping source");
                let _ = std::fs::remove_file(&tmp);
                continue;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600));
            }
        }
        // 频道世界接管；旧文件退场（防 legacy 扫描复活双份）。
        let _ = std::fs::remove_file(&path);
        moved += 1;
        info!(src = %path.display(), dest = %dest.display(), "session migrated to channel home");
    }
    moved
}
