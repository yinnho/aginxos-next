//! aginx-carrier — 分身 OS（aginx 网络上的一个网站）
//!
//! 从 OpenCarrier 搬运移植的独立仓（非 fork——零共享 git 历史，各自进化）：单操作者、私有化、aginx 原生。
//! 定位与总纲见 aginx 生态 docs/AGINX-CARRIER-VISION.md。

mod acp;
mod agent_cmd;
mod api_cmd;
mod cron_cmd;
mod notify;
mod probe;
mod qrlogin;
mod remote;
mod start;
mod sys_cmd;
mod tool_cmd;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "aginx-carrier",
    version,
    about = "分身 OS — 托管数字分身的 Agent 运行时",
    long_about = "aginx-carrier 是 aginx Agent 互联网上托管分身的运行时。\n自己的分身跟着自己的设备走；用别人的分身时算力在他家、数据在你家。"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 运行时分身守护进程（个人部署形态）
    Start,
    /// 化身管理 CLI 面（AginxOS 融合后唯一管理入口：供 aterm 启动器/脚本调用）
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// 任务面 CLI（CARRIER.md §3.4-3：aclone 的 cron 列表 + 暂停/恢复/删除）
    Cron {
        #[command(subcommand)]
        action: CronAction,
    },
    /// stdio ACP 桥：被 aginx 网关拉起，把分身暴露到 agent:// 网络
    Acp {
        /// 分身名称
        #[arg(long)]
        clone: String,
        /// 续接会话 id（网关 `${SESSION_ID}` 注入；缺省 = 新会话并铸造 id）
        #[arg(long)]
        session: Option<String>,
    },
    /// 系统杂项面（M33：location/time——自 runtime misc 搬来，不 boot kernel）
    Sys {
        #[command(subcommand)]
        action: sys_cmd::SysAction,
    },
    /// 机读面（M33 D3 批3）：stdin JSON（入参+_ctx）→ stdout D1 信封。
    /// runtime 桥 spawn 此面执行内核耦合工具（schedule/cron/agent_*/sys）。
    Tool {
        /// 工具名（tool_cmd::TOOL_NAMES）
        name: String,
    },
    /// 声明式 API 工具面（M34）：api_tools 执行链单真源在此——具名调用
    /// （resolve/HMAC/extract）、通用直通、注册落盘、cron 一跳。
    Api {
        #[command(subcommand)]
        action: api_cmd::ApiAction,
    },
    /// 显示版本与本地数据目录等信息
    Info,
    /// 网关存活探测：TLS 连 relay 握 connected（watchdog 探测原语）
    Probe {
        /// 目标网关 agent:// URL（如 agent://selvkwjv.relay.aginx.net）
        url: String,
    },
    /// iLink 一次性告警发送（daemon 无关，watchdog 通知原语）
    Notify {
        /// 告警正文
        text: String,
        /// 收件人 user_id（缺省 = 唯一未过期 bot 会话的绑定用户）
        #[arg(long)]
        to: Option<String>,
    },
    /// iLink 扫码登录：终端渲染 ASCII 二维码，扫后落分身下 senders/ 会话
    QrLogin {
        /// 本账号的 bot 名（标签；会话文件按 user_id 存）
        #[arg(long, default_value = "main")]
        bot_id: String,
        /// 绑定的分身名（绑定即路由：扫码即绑定，消息直达该分身；必填）
        #[arg(long)]
        bind_agent: String,
    },
    /// 用户侧票据仓库（借用机制的会话真源在用户侧）
    Ticket {
        #[command(subcommand)]
        action: TicketAction,
    },
}

#[derive(Subcommand)]
enum TicketAction {
    /// 列出某分身下的全部票据
    List {
        /// 分身名称
        #[arg(long)]
        clone: String,
    },
    /// 从 JSON 文件导入票据（如 agc --save-ticket 的产出）
    Import {
        /// 分身名称
        #[arg(long)]
        clone: String,
        /// 对话标签（空 = default）
        #[arg(long, default_value = "default")]
        label: String,
        /// 票据 JSON 文件
        #[arg(long)]
        file: String,
    },
    /// 导出票据到 JSON 文件（喂给 agc --ticket）
    Export {
        /// 分身名称
        #[arg(long)]
        clone: String,
        /// 对话标签
        #[arg(long, default_value = "default")]
        label: String,
        /// 输出文件
        #[arg(long)]
        file: String,
    },
    /// 删除一张票据（开新对话）
    Delete {
        /// 分身名称
        #[arg(long)]
        clone: String,
        /// 对话标签
        #[arg(long, default_value = "default")]
        label: String,
    },
}

/// 任务面动作（CARRIER.md §3.4-3；创建一期不要求——化身对话里自建）。
#[derive(Subcommand)]
enum CronAction {
    /// 任务列表（D1 信封一条：data 一元素一任务 id/name/schedule/
    /// next_fire/last_result/enabled + agent/one_shot/late）
    List {
        /// 只看某化身的任务（缺省 = 全部）
        #[arg(long)]
        agent: Option<String>,
    },
    /// 暂停任务（在飞轮跑完，下一槽不再触发）
    Pause {
        /// 任务 id（cron list 的 UUID）
        id: String,
    },
    /// 恢复任务（错误计数清零，重算下一槽）
    Resume {
        /// 任务 id（cron list 的 UUID）
        id: String,
    },
    /// 删除任务
    Remove {
        /// 任务 id（cron list 的 UUID）
        id: String,
    },
    /// 创建任务（M33：打破"创建不走 CLI"一期限制）。任务 JSON 从 stdin 或
    /// --json 读（{name, schedule:{kind:at|every|cron,...}, action:{kind:
    /// system_event|agent_turn|...}, one_shot?...}）；落 DB 后常驻 daemon
    /// ≤15s reconcile 采进。
    Create {
        /// 归属化身（名或 UUID）
        #[arg(long)]
        agent: String,
        /// 任务 JSON（缺省读 stdin）
        #[arg(long)]
        json: Option<String>,
    },
}

/// 化身管理动作（CARRIER.md §3.3 下载类形态的 CLI 面）。
#[derive(Subcommand)]
enum AgentAction {
    /// 安装化身：默认从 DupHub 拉取；--file 从本地 tar 装（AginxOS 手机
    /// 形态，duphub auth 等 M36 sidecar 前的离线路径）。已存在则重装，
    /// .dup/ 历史保留。
    Install {
        /// 化身名（DupHub 名，或 --file 时的本地命名）
        name: String,
        /// 本地包路径：分身定义层平铺 tar（.tar/.tar.gz，与 dup 工作区同构）
        #[arg(long)]
        file: Option<PathBuf>,
        /// 预检不安装：跑安装格式硬闸 + 列出 flows 与 shell 权限预览
        #[arg(long)]
        dry_run: bool,
    },
    /// 列出本机化身（本地 + 远程句柄同构合并）
    List {
        /// 机器可读输出：D1 信封一条（{ok,data,meta}，CARRIER.md §3.4-1）
        #[arg(long)]
        json: bool,
    },
    /// 卸载化身（杀后台/清 cron/删 workspace/离网）
    Remove {
        /// 本机化身名
        name: String,
    },
    /// 更新化身（Hub 最新版本 ≠ 本地版本才重装）
    Update {
        /// 本机化身名
        name: String,
    },
    /// 给化身发一条消息并收回答复（内联跑一轮；机读面 tool agent_send 同源）
    Send {
        /// 目标化身（名或 UUID）
        agent: String,
        /// 消息正文
        #[arg(long)]
        message: String,
        /// 发送者标识（缺省 = cli）
        #[arg(long, default_value = "cli")]
        sender: String,
    },
    /// 强杀化身（取消在跑任务、清后台/调度/能力/事件；注册表保留）
    Kill {
        /// 目标化身（名或 UUID）
        agent: String,
    },
    /// 重启化身（取消在跑任务、状态回 Running——改配置后生效用）
    Restart {
        /// 目标化身（名或 UUID）
        agent: String,
    },
    /// 远程化身句柄：注册别人网关上的分身，列表与对话同构（CARRIER.md §3.3 远程类）
    Remote {
        #[command(subcommand)]
        action: RemoteAction,
    },
}

#[derive(Subcommand)]
enum RemoteAction {
    /// 注册远程化身（agent:// 网关地址）
    Add {
        /// 本地别名（对使用者即化身名）
        name: String,
        /// 网关地址：agent://<id>.relay.<domain>[:port][/分身名]
        url: String,
        /// 显示名（缺省 = 别名）
        #[arg(long)]
        display_name: Option<String>,
        /// 访客 token（私有网关准入；public 网关可省）
        #[arg(long)]
        token: Option<String>,
    },
    /// 移除远程化身句柄（只删本机注册，不影响对方网关）
    Remove {
        /// 本地别名
        name: String,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Start => start::run()?,
        Command::Agent { action } => agent_cmd::run(action)?,
        Command::Cron { action } => cron_cmd::run(action)?,
        Command::Acp { clone, session } => acp::run(clone, session)?,
        Command::Sys { action } => sys_cmd::run(action)?,
        Command::Tool { name } => tool_cmd::run(name)?,
        Command::Api { action } => api_cmd::run(action)?,
        Command::Probe { url } => probe::run(url)?,
        Command::Notify { text, to } => notify::run(text, to)?,
        Command::QrLogin { bot_id, bind_agent } => {
            qrlogin::run(bot_id, bind_agent)?
        }
        Command::Info => {
            let data_dir = dirs::home_dir()
                .map(|h| h.join(".aginx").join("carrier"))
                .expect("无法解析用户主目录");
            println!("aginx-carrier {}", env!("CARGO_PKG_VERSION"));
            println!("数据目录: {}", data_dir.display());
        }
        Command::Ticket { action } => ticket_cmd(action)?,
    }
    Ok(())
}

/// 用户侧票据仓库 CLI。个人安装版"同一台机器既是主人又是用户"，直接复用
/// 本机 carrier.db（v33 borrow_tickets 表）；App/远程宿主走 lib API。
fn ticket_cmd(action: TicketAction) -> anyhow::Result<()> {
    use carrier_memory::MemorySubstrate;

    let substrate = MemorySubstrate::open(
        &dirs::home_dir()
            .map(|h| h.join(".aginx").join("carrier").join("data").join("carrier.db"))
            .expect("无法解析用户主目录"),
    )?;
    let store = substrate.tickets();
    match action {
        TicketAction::List { clone } => {
            let entries = store.list(&clone)?;
            if entries.is_empty() {
                println!("（无票据）");
                return Ok(());
            }
            for e in entries {
                println!(
                    "{:<16} {:>4} msgs  {}",
                    e.label, e.message_count, e.updated_at
                );
            }
        }
        TicketAction::Import { clone, label, file } => {
            let raw = std::fs::read_to_string(&file)?;
            let ticket: carrier_memory::session::SessionTicket = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("{file} 不是合法票据 JSON: {e}"))?;
            store.save(&clone, &label, &ticket)?;
            println!("已存入 {} / {}（{} msgs）", clone, label, ticket.messages.len());
        }
        TicketAction::Export { clone, label, file } => {
            let ticket = store
                .load(&clone, &label)?
                .ok_or_else(|| anyhow::anyhow!("没有 {} / {} 的票据", clone, label))?;
            std::fs::write(&file, serde_json::to_string_pretty(&ticket)?)?;
            println!("已导出到 {}（{} msgs）", file, ticket.messages.len());
        }
        TicketAction::Delete { clone, label } => {
            if store.delete(&clone, &label)? {
                println!("已删除 {} / {}", clone, label);
            } else {
                println!("没有 {} / {} 的票据", clone, label);
            }
        }
    }
    Ok(())
}
