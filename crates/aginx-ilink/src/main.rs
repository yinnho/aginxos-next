//! aginx-ilink — weixin 频道守护 + 频道运维 CLI（#68 ②b 第一频道）。
//!
//! 面（D13：aginx-<domain>-<object>-<verb> 姓下这是频道本体件）：
//! - `daemon`         守护（单元跑的；也可前台手跑排障）
//! - `login [--bind <agent>]`  扫码绑号（终端直接出二维码块，无头机不依赖屏）
//! - `status`         会话一览（账号/绑定/剩余寿命）
//! - `bind <agent>`   换绑（改 bind_agent 字段，不搬家）
//! - `send <uid> <text>`       出站对测

mod acp;
mod config;
mod daemon;

use anyhow::{anyhow, Context, Result};
use carrier_types::channel::Channel;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let out = match cmd {
        "daemon" => daemon::run(),
        "login" => cmd_login(&args[1..]),
        "status" => cmd_status(),
        "bind" => cmd_bind(&args[1..]),
        "send" => cmd_send(&args[1..]),
        _ => {
            eprintln!(
                "aginx-ilink v{ver} — weixin 频道（#68 ②b）\n\
                 用法：aginx-ilink daemon | login [--bind <agent>] | status\n\
                       aginx-ilink bind <agent> | send <uid> <text>\n\
                 环境变量：AGINX_HOME（默认 /home）、AGINX_ILINK_ACP_ADDR（默认 {addr}）、AGINX_ILINK_ACP_TIMEOUT_SECS（默认 {tmo}）",
                 ver = env!("CARGO_PKG_VERSION"),
                 addr = acp::DEFAULT_ADDR,
                 tmo = acp::DEFAULT_TIMEOUT_SECS,
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = out {
        eprintln!("aginx-ilink: {e:#}");
        std::process::exit(1);
    }
}

/// `login [--bind <agent>]`：qr_login 阻塞到扫码完成，会话落频道家
/// （set_sessions_root 后 register→save 即落 `senders/<uid>/session.json`，
/// 绑定=文件字段）。二维码用终端半块字符画——无头机唯一人面是 ssh。
fn cmd_login(args: &[String]) -> Result<()> {
    let bind = flag_value(args, "--bind");
    let root = config::channel_root();
    std::fs::create_dir_all(&root)?;
    config::ChannelConfig::seed_default_file(&root);
    carrier_ilink::token::set_sessions_root(&root);

    let http = carrier_ilink::build_http_client();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let on_qr = |url: &str| {
        println!("微信扫码登录（过期自动换码，共 8 分钟窗）：");
        println!("{url}\n");
        print_terminal_qr(url);
    };
    let tenant = rt.block_on(carrier_ilink::auth::qr_login(
        &http,
        "",
        bind.as_deref(),
        Some(&on_qr),
    ))?;
    let bind_note = bind
        .as_deref()
        .map(|a| format!("绑定字段={a}"))
        .unwrap_or_else(|| "未指名绑定（路由走 channel.toml default_agent）".to_string());
    println!(
        "\nWeChat account linked: {tenant}（会话家 {}，{bind_note}）",
        root.join("senders").display()
    );
    Ok(())
}

/// 终端二维码（qrcodegen EcLevel M，两模块一格半块字符；暗底终端
/// 反色渲染——黑底亮码块）。
fn print_terminal_qr(text: &str) {
    use qrcodegen::{QrCode, QrCodeEcc};
    let Ok(qr) = QrCode::encode_text(text, QrCodeEcc::Medium) else {
        return;
    };
    let size = qr.size();
    // 静区 2 模块
    let quiet = 2i32;
    let top = -quiet;
    let bottom = size + quiet;
    for y in (top..bottom).step_by(2) {
        let mut line = String::new();
        for x in top..bottom {
            // 一格=上下两模块：█(全黑) ▀(上黑) ▄(下黑) 空格(全白)。
            // 反色：模块有=白区。黑底终端里「白区」画亮块。
            let up = (y..y + 2).any(|yy| yy >= 0 && yy < size && qr.get_module(x, yy));
            let down = (y + 1..y + 2).any(|yy| yy >= 0 && yy < size && qr.get_module(x, yy));
            line.push_str(match (up, down) {
                (true, true) => "██",
                (true, false) => "▀▀",
                (false, true) => "▄▄",
                (false, false) => "  ",
            });
        }
        println!("{line}");
    }
}

fn cmd_status() -> Result<()> {
    let root = config::channel_root();
    carrier_ilink::token::set_sessions_root(&root);
    let state = &carrier_ilink::token::WEIXIN_STATE;
    state.load_from_dir();
    let list = state.status_list();
    if list.is_empty() {
        println!("（无会话——aginx-ilink login 扫码起号）");
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&list)?);
    println!("channel.toml default_agent = {:?}", config::ChannelConfig::load(&root).default_agent);
    Ok(())
}

/// `bind <agent>`（单账号）或 `bind <uid> <agent>`：改 session.json 的
/// bind_agent 字段。守护 5s 拾取，无需重启；正在对话的发信人下一条
/// 消息自动开新引擎线程（gw.json agent 不一致即弃旧会话）。
fn cmd_bind(args: &[String]) -> Result<()> {
    let (user_key, agent) = match args.len() {
        1 => (None, args[0].clone()),
        2 => (Some(args[0].clone()), args[1].clone()),
        _ => return Err(anyhow!("用法：bind <agent> | bind <uid> <agent>")),
    };
    if agent.is_empty() {
        return Err(anyhow!("agent 名不能为空"));
    }
    let root = config::channel_root();
    let senders = root.join("senders");
    let mut hits: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&senders) {
        for e in rd.flatten() {
            let p = e.path().join("session.json");
            if p.is_file() {
                hits.push(p);
            }
        }
    }
    if hits.is_empty() {
        return Err(anyhow!("{} 下无会话——先 login", senders.display()));
    }
    let target: Vec<std::path::PathBuf> = match &user_key {
        Some(k) => hits
            .iter()
            .filter(|p| p.parent().and_then(|d| d.file_name()).map(|n| n == k.as_str()).unwrap_or(false))
            .cloned()
            .collect(),
        None => hits.clone(),
    };
    if target.len() != 1 {
        let names: Vec<String> = hits
            .iter()
            .map(|p| p.parent().unwrap().file_name().unwrap().to_string_lossy().to_string())
            .collect();
        return Err(anyhow!(
            "命中 {n} 个会话（{names:?}）——用 bind <uid> <agent> 指名",
            n = target.len(),
            names = names
        ));
    }
    let path = target[0].clone();
    let content = std::fs::read_to_string(&path).context("read session.json")?;
    let mut tf: carrier_ilink::models::BotTokenFile =
        serde_json::from_str(&content).context("parse session.json")?;
    let old = tf.bind_agent.clone();
    tf.bind_agent = Some(agent.clone());
    let json = serde_json::to_string_pretty(&tf)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    println!(
        "{}: bind_agent {:?} → {:?}（字段即绑定，未搬家）",
        path.display(),
        old,
        agent
    );
    Ok(())
}

/// `send <uid> <text>`：出站对测（走 get_session_for_send 三路解析）。
fn cmd_send(args: &[String]) -> Result<()> {
    if args.len() < 2 {
        return Err(anyhow!("用法：send <uid> <text>"));
    }
    let uid = args[0].clone();
    let text = args[1..].join(" ");
    let root = config::channel_root();
    carrier_ilink::token::set_sessions_root(&root);
    let state = &carrier_ilink::token::WEIXIN_STATE;
    state.load_from_dir();
    let watcher = carrier_ilink::SessionWatcher::new();
    watcher
        .send("", &uid, &text)
        .map_err(|e| anyhow!("send: {e}"))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
    }
    None
}
