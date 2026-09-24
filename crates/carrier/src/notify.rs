//! `aginx-carrier notify` — iLink 一次性告警（watchdog 的通知原语）。
//!
//! 独立进程直读 weixin_sessions（DB 优先，`senders/` JSON 旁路兜底）拿
//! bot 会话，`send_message_auto` 裸发——**daemon 死了也能发**：daemon
//! 自己的告警不能指望 daemon 活着，这正是它以独立进程存在的原因。
//!
//! 收件人默认 = 唯一未过期 bot 会话的 user_id（扫码绑定的操作者本人，
//! 个人部署即一个）；多会话时用 `--to` 挑，不猜。

use carrier_ilink::models::BotTokenFile;
use carrier_memory::MemorySubstrate;

/// Send one alert text over iLink, bypassing the daemon entirely.
pub fn run(text: String, to: Option<String>) -> anyhow::Result<()> {
    let mut sessions = load_from_db();
    if sessions.is_empty() {
        sessions = carrier_ilink::token::scan_json_token_files();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let alive: Vec<&BotTokenFile> = sessions
        .iter()
        .filter(|tf| tf.user_id.as_deref().is_some_and(|u| !u.is_empty()))
        .filter(|tf| now < tf.expires_at)
        .collect();

    let tf = match to.as_deref() {
        Some(uid) => alive
            .iter()
            .find(|tf| tf.user_id.as_deref() == Some(uid))
            .ok_or_else(|| anyhow::anyhow!("没有 user_id={uid} 的未过期 iLink 会话"))?,
        None => match alive.len() {
            0 => anyhow::bail!("无未过期 iLink 会话（DB 与 senders/ JSON 皆空）——先扫码登录"),
            1 => alive[0],
            _ => anyhow::bail!(
                "多个 iLink 会话，需 --to 指定其一：{}",
                alive
                    .iter()
                    .map(|tf| tf.user_id.clone().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
    };

    let user_id = tf.user_id.clone().expect("上面已滤掉空 user_id");
    // context_token 可选（裸发即协议基线）：有缓存就带上，砸了自动裸发重试。
    let context_token = tf.context_tokens.get(&user_id).cloned();
    let client_id = format!("openclaw-weixin-{}", uuid::Uuid::new_v4().as_simple());
    let http = carrier_ilink::build_http_client();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(carrier_ilink::api::send_message_auto(
        &http,
        &tf.bot_token,
        &tf.baseurl,
        &user_id,
        context_token.as_deref(),
        &client_id,
        &text,
    ))?;
    println!("已发送 → {user_id}（bot {}）", tf.bot_id);
    Ok(())
}

/// DB 直读（不经 daemon）：`~/.aginx/carrier/data/carrier.db` 的
/// weixin_sessions 表。打开/读取失败按空处理，让 JSON 兜底接手。
fn load_from_db() -> Vec<BotTokenFile> {
    let path = dirs::home_dir()
        .map(|h| h.join(".aginx").join("carrier").join("data").join("carrier.db"))
        .expect("无法解析用户主目录");
    let substrate = match MemorySubstrate::open(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("（DB 不可用：{e}；转 senders/ JSON 兜底）");
            return Vec::new();
        }
    };
    match substrate.weixin_store().load_all() {
        Ok(rows) => rows
            .into_iter()
            .map(aginx_carrier::wiring::weixin_row_to_token_file)
            .collect(),
        Err(e) => {
            eprintln!("（DB 读会话失败：{e}；转 senders/ JSON 兜底）");
            Vec::new()
        }
    }
}
