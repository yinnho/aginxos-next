// aginx-runtime — fast-agi 引擎入口（v0 按需 spawn）。
//
// spawn 契约（server 侧）：
//   aginx-runtime --workspace <化身文件夹> --session <会话id>
//
// stdin/stdout 即协议线（agi crate）：首帧必须是 request，收尾必是
// done，退出码 0=ok / 1=err。会话上下文从化身文件夹的
// sessions/{id}.jsonl 冷恢复（D8：日志=真源）；工具面问路由器 CLI 要。
//
// env：
//   AGINX_BRAIN_URL       默认 https://brain.aginx.net/v1/chat/completions
//   AGINXBRAIN_API_KEY    密钥，只从环境来，不回显不落盘
//   AGINX_BRAIN_MODEL     模态路由标签，默认 chat
//   AGINX_BIN             路由器二进制（工具发现用），默认 PATH 里的 aginx
//   AGINX_CONTEXT_WINDOW  上下文窗口 token 数，默认 128k

use agi::Frame;
use aginx_runtime::agent_loop::{run_turn, TurnConfig};
use aginx_runtime::avatar;
use aginx_runtime::brain::{BrainConfig, HttpBrain};
use aginx_runtime::tools;
use aginx_runtime::transport::{StdioTransport, TurnTransport};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() {
    println!("aginx-runtime — fast-agi engine (one request per spawn, v0)");
    println!();
    println!("usage: aginx-runtime --workspace <dir> --session <id>");
    println!();
    println!("stdin/stdout is the agi frame line. First inbound frame must be");
    println!("request; the process exits after writing exactly one done frame.");
}

fn main() -> ExitCode {
    let mut workspace: Option<PathBuf> = None;
    let mut session: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--workspace" => workspace = args.next().map(PathBuf::from),
            "--session" => session = args.next(),
            "--help" | "-h" => {
                usage();
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("aginx-runtime: unknown argument '{other}'");
                return ExitCode::from(2);
            }
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(run(workspace, session))
}

async fn run(workspace: Option<PathBuf>, session: Option<String>) -> ExitCode {
    let mut io = StdioTransport::new();

    // 首帧：必须是 request。不是 request 或收流 = 契约破裂。
    let request = match io.recv() {
        Ok(Some(Frame::Request(r))) => r,
        Ok(Some(other)) => {
            let _ = io.send(&Frame::Done(agi::Done::err(
                "protocol",
                format!("first frame must be request, got {}", frame_tag(&other)),
            )));
            return ExitCode::FAILURE;
        }
        Ok(None) => {
            eprintln!("aginx-runtime: stdin closed before request");
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("aginx-runtime: bad first frame: {e}");
            return ExitCode::FAILURE;
        }
    };

    // argv 优先（server spawn 契约），缺了退回 request 帧字段 + 默认根。
    let workspace = workspace.unwrap_or_else(|| default_workspace(&request.avatar));
    let session = session.unwrap_or_else(|| request.session.clone());
    let avatar_name = workspace
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| request.avatar.clone());

    let system = avatar::system_prompt(&workspace, &avatar_name);
    let log = workspace.join("sessions").join(format!("{session}.jsonl"));
    let history = avatar::replay_session(&log);
    // server 先记账再 spawn：本轮 request 已在账上，重放即已含本轮文本，
    // 别再叠一份。手工管里没有 pre-log，文本照常从帧上进。
    let user_text = if avatar::trailing_request_logged(&log, &request) {
        String::new()
    } else {
        request.text.clone()
    };

    let tools = match tools::discover_tools(&tools::aginx_bin_from_env()) {
        Ok(t) => t,
        Err(e) => {
            // 工具面挂了不杀对话：裸脑继续，吼一嗓子给人看
            eprintln!("aginx-runtime: tool discovery failed ({e}); continuing without tools");
            Vec::new()
        }
    };

    let mut cfg = TurnConfig::default();
    if let Ok(w) = std::env::var("AGINX_CONTEXT_WINDOW") {
        if let Ok(w) = w.trim().parse::<usize>() {
            cfg.context_window = w;
        }
    }

    let brain = HttpBrain::new(BrainConfig::from_env());
    match run_turn(&brain, &tools, &system, history, &user_text, &mut io, &cfg).await {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("aginx-runtime: frame io failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// 默认化身根：AGINX_HOME 覆写（试跑隔离 ~/.aginx-n 用），否则 ~/.aginx。
fn default_workspace(avatar: &str) -> PathBuf {
    let root = std::env::var("AGINX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".aginx")
        });
    root.join("workspaces").join(avatar)
}

fn frame_tag(f: &Frame) -> &'static str {
    match f {
        Frame::Request(_) => "request",
        Frame::Steer(_) => "steer",
        Frame::ToolCall(_) => "tool_call",
        Frame::ToolResult(_) => "tool_result",
        Frame::Artifact(_) => "artifact",
        Frame::Done(_) => "done",
    }
}
