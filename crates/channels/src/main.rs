//! aginx-channels — 频道体系件（#69 改形：channels 唯一家）。
//!
//! 一频道 = 一目录 + 一包（DESIGN.md §四）。本件是全部频道的机身：
//! 守护 + 运维 CLI 一件双面，子命令带频道名（`login weixin`）。
//! 数据与绑定不进包：全部住 `/home/channels/<名>/`。
//!
//! 面：
//! - `daemon`                        守护（单元跑的；也可前台手跑排障）
//! - `login <名> [--bind <agent>]`   扫码绑号（终端出二维码块）
//! - `status [<名>]`                 频道/会话一览（缺省=全部频道）
//! - `bind <名> <agent>` / `bind <名> <uid> <agent>`  换绑（改字段）
//! - `send <名> <uid> <text>`        出站对测

mod acp;
mod config;
mod daemon;
#[allow(dead_code)] // 词表=频道体系契约面：给后续腿（telegram/email…）留的变体与方法
mod vocab;
mod weixin;

use anyhow::{anyhow, Context, Result};
use config::{channel_root, channels_root, valid_channel_name, ChannelConfig};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let out = match cmd {
        "daemon" => daemon::run(),
        "login" => cmd_login(&args[1..]),
        "status" => cmd_status(&args[1..]),
        "bind" => cmd_bind(&args[1..]),
        "send" => cmd_send(&args[1..]),
        _ => {
            eprintln!(
                "aginx-channels v{ver} — 频道体系（一频道一目录一包，DESIGN.md §四）\n\
                 用法：aginx-channels daemon\n\
                       aginx-channels login <名> [--bind <agent>]\n\
                       aginx-channels status [<名>]\n\
                       aginx-channels bind <名> <agent> | bind <名> <uid> <agent>\n\
                       aginx-channels send <名> <uid> <text>\n\
                 词表频道：weixin。数据家 /home/channels/<名>/（AGINX_HOME 下）。\n\
                 环境变量：AGINX_HOME（默认 /home）、AGINX_CHANNELS_ACP_ADDR（默认 {addr}）、AGINX_CHANNELS_ACP_TIMEOUT_SECS（默认 {tmo}）",
                 ver = env!("CARGO_PKG_VERSION"),
                 addr = acp::DEFAULT_ADDR,
                 tmo = acp::DEFAULT_TIMEOUT_SECS,
            );
            std::process::exit(2);
        }
    };
    if let Err(e) = out {
        eprintln!("aginx-channels: {e:#}");
        std::process::exit(1);
    }
}

/// 词表检查：本版认识的协议腿。目录名=频道名；type=腿名。
fn require_known_leg(name: &str) -> Result<()> {
    match name {
        "weixin" => Ok(()),
        other => Err(anyhow!(
            "未知频道 {other:?}——本版词表只有 weixin（新腿=channels 新模块）"
        )),
    }
}

fn take_channel_name(args: &[String], usage: &str) -> Result<String> {
    let name = args
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("用法：{usage}"))?;
    if !valid_channel_name(&name) {
        return Err(anyhow!("频道名 {name:?} 不合法"));
    }
    require_known_leg(&name)?;
    Ok(name)
}

/// `login <名> [--bind <agent>]`：qr_login 阻塞到扫码完成，会话落频道家
/// （set_sessions_root 后 register→save 即落 `senders/<uid>/session.json`，
/// 绑定=文件字段）。二维码用终端半块字符画——无头机唯一人面是 ssh。
fn cmd_login(args: &[String]) -> Result<()> {
    let name = take_channel_name(args, "login <名> [--bind <agent>]")?;
    let bind = flag_value(args, "--bind");
    let root = channel_root(&name);
    std::fs::create_dir_all(&root)?;
    ChannelConfig::seed_default_file(&root, &name);
    weixin::token::set_sessions_root(&root);

    let http = weixin::build_http_client();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let on_qr = |url: &str| {
        println!("微信扫码登录（过期自动换码，共 8 分钟窗）：");
        println!("{url}\n");
        print_terminal_qr(url);
    };
    // qr_login 的返回已是完整成功语——原样打，别再拼一遍。
    let tenant = rt.block_on(weixin::auth::qr_login(
        &http,
        "",
        bind.as_deref(),
        Some(&on_qr),
    ))?;
    let bind_note = bind
        .as_deref()
        .map(|a| format!("绑定字段={a}"))
        .unwrap_or_else(|| "未指名绑定（路由走 channel.toml default_agent）".to_string());
    println!("\n{tenant}（会话家 {}，{bind_note}）", root.join("senders").display());
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

/// `status [<名>]`：缺省列全部频道目录；weixin 加会话一览
/// （账号/绑定/剩余寿命）。
fn cmd_status(args: &[String]) -> Result<()> {
    let names: Vec<String> = match args.first() {
        Some(n) => {
            if !valid_channel_name(n) {
                return Err(anyhow!("频道名 {n:?} 不合法"));
            }
            vec![n.clone()]
        }
        None => {
            let mut out = Vec::new();
            if let Ok(rd) = std::fs::read_dir(channels_root()) {
                for e in rd.flatten() {
                    if e.path().is_dir() {
                        let n = e.file_name().to_string_lossy().to_string();
                        if !n.starts_with('.') {
                            out.push(n);
                        }
                    }
                }
            }
            out.sort();
            out
        }
    };
    if names.is_empty() {
        println!("（无频道目录——{} 下空；login <名> 起号）", channels_root().display());
        return Ok(());
    }
    for name in names {
        let root = channel_root(&name);
        let config = ChannelConfig::load(&root);
        println!("[{name}] type={} default_agent={}", config.channel_type, config.default_agent);
        if config.channel_type == "weixin" {
            weixin::token::set_sessions_root(&root);
            let state = &weixin::token::WEIXIN_STATE;
            state.load_from_dir();
            let list = state.status_list();
            if list.is_empty() {
                println!("  （无会话——aginx-channels login {name} 扫码起号）");
            } else {
                println!("{}", serde_json::to_string_pretty(&list)?);
            }
        } else {
            println!("  （{} 腿未装机——只有目录与配置）", config.channel_type);
        }
    }
    Ok(())
}

/// `bind <名> <agent>`（单账号）或 `bind <名> <uid> <agent>`：改
/// session.json 的 bind_agent 字段。守护 5s 拾取，无需重启；正在对话
/// 的发信人下一条消息自动开新引擎线程（gw.json agent 不一致即弃旧会话）。
fn cmd_bind(args: &[String]) -> Result<()> {
    let (name, rest) = match args.len() {
        0 => return Err(anyhow!("用法：bind <名> <agent> | bind <名> <uid> <agent>")),
        _ => (args[0].clone(), &args[1..]),
    };
    if !valid_channel_name(&name) {
        return Err(anyhow!("频道名 {name:?} 不合法"));
    }
    let (user_key, agent) = match rest.len() {
        1 => (None, rest[0].clone()),
        2 => (Some(rest[0].clone()), rest[1].clone()),
        _ => return Err(anyhow!("用法：bind <名> <agent> | bind <名> <uid> <agent>")),
    };
    if agent.is_empty() {
        return Err(anyhow!("agent 名不能为空"));
    }
    let root = channel_root(&name);
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
            .filter(|p| {
                p.parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n == k.as_str())
                    .unwrap_or(false)
            })
            .cloned()
            .collect(),
        None => hits.clone(),
    };
    if target.len() != 1 {
        let names: Vec<String> = hits
            .iter()
            .map(|p| {
                p.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        return Err(anyhow!(
            "命中 {n} 个会话（{names:?}）——用 bind <名> <uid> <agent> 指名",
            n = target.len(),
            names = names
        ));
    }
    let path = target[0].clone();
    let content = std::fs::read_to_string(&path).context("read session.json")?;
    let mut json: serde_json::Value = serde_json::from_str(&content).context("parse session.json")?;
    let old = json
        .get("bind_agent")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    json["bind_agent"] = serde_json::json!(agent.clone());
    let out = serde_json::to_string_pretty(&json)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, out)?;
    std::fs::rename(&tmp, &path)?;
    println!(
        "[{name}] {}: bind_agent {:?} → {:?}（字段即绑定，未搬家）",
        path.display(),
        old,
        agent
    );
    Ok(())
}

/// `send <名> <uid> <text>`：出站对测（weixin 走 get_session_for_send
/// 三路解析；iLink 出站是关系账本 best-effort，HTTP 200 ≠ 送达）。
fn cmd_send(args: &[String]) -> Result<()> {
    if args.len() < 3 {
        return Err(anyhow!("用法：send <名> <uid> <text>"));
    }
    let name = take_channel_name(args, "send <名> <uid> <text>")?;
    let uid = args[1].clone();
    let text = args[2..].join(" ");
    let root = channel_root(&name);
    weixin::token::set_sessions_root(&root);
    let state = &weixin::token::WEIXIN_STATE;
    state.load_from_dir();
    let watcher = weixin::SessionWatcher::new();
    use crate::vocab::Channel;
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
