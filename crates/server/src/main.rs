// aginx-server — 母体（宪法 D4/D10/D11）：前台登记（进/住/切/退）、会话
// 光标、请求路由、D8 会话账、fast-agi spawn 端。
//
// 入口 = UDS（v0 不开 TCP）：每条连线一问一答——读一行 op JSON，回一行
// D1 信封，连接即关。化身轮次在连线线程里同步跑完（前台一次一轮，
// turn 锁在 front 层）；真 brain 一轮可以跑几分钟，客户端就等几分钟，
// 这正是语音/终端对话的产品形状。
//
// env：
//   AGINX_SOCK         UDS 路径，默认 /run/aginx.sock（host 试跑必设）
//   AGINX_HOME         化身根的父目录，默认 ~/.aginx（试跑隔离用 ~/.aginx-n）
//   AGINX_BIN          路由器二进制（工具派发用），默认 PATH 里的 aginx
//   AGINX_RUNTIME_BIN  fast-agi 引擎，默认 PATH 里的 aginx-runtime
//   AGINXBRAIN_API_KEY / AGINX_BRAIN_URL / AGINX_BRAIN_MODEL  透传给 runtime

mod front;
mod ledger;
mod mother;
mod ops;
mod turn;

use front::FrontDesk;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;

pub struct ServerCfg {
    pub workspaces_root: PathBuf,
    pub sock: PathBuf,
    pub aginx_bin: String,
    pub runtime_bin: String,
}

impl ServerCfg {
    pub fn from_env() -> ServerCfg {
        let home = std::env::var("AGINX_HOME").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".aginx")
        });
        ServerCfg {
            workspaces_root: home.join("workspaces"),
            sock: PathBuf::from(std::env::var("AGINX_SOCK").unwrap_or_else(|_| "/run/aginx.sock".into())),
            aginx_bin: std::env::var("AGINX_BIN").unwrap_or_else(|_| "aginx".into()),
            runtime_bin: std::env::var("AGINX_RUNTIME_BIN").unwrap_or_else(|_| "aginx-runtime".into()),
        }
    }

    #[cfg(test)]
    pub fn for_test(workspaces_root: PathBuf, aginx_bin: String, runtime_bin: String) -> ServerCfg {
        ServerCfg { workspaces_root, sock: PathBuf::from("/tmp/aginx-server-test.sock"), aginx_bin, runtime_bin }
    }
}

fn frame_tag(f: &agi::Frame) -> &'static str {
    match f {
        agi::Frame::Request(_) => "request",
        agi::Frame::Steer(_) => "steer",
        agi::Frame::ToolCall(_) => "tool_call",
        agi::Frame::ToolResult(_) => "tool_result",
        agi::Frame::Artifact(_) => "artifact",
        agi::Frame::Done(_) => "done",
    }
}

fn main() {
    let cfg = std::sync::Arc::new(ServerCfg::from_env());
    let desk = std::sync::Arc::new(FrontDesk::new(cfg.workspaces_root.clone()));

    // 陈旧 socket 文件（上次崩溃留下的）先清；v0 单实例，不做存活探测
    let _ = std::fs::remove_file(&cfg.sock);
    let listener = match UnixListener::bind(&cfg.sock) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("aginx-server: cannot bind {}: {e}", cfg.sock.display());
            eprintln!("aginx-server: host 试跑请设 AGINX_SOCK（/run 在 mac 上不存在）");
            std::process::exit(1);
        }
    };
    eprintln!(
        "aginx-server: listening on {} (workspaces: {})",
        cfg.sock.display(),
        cfg.workspaces_root.display()
    );

    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let desk = std::sync::Arc::clone(&desk);
        let cfg = std::sync::Arc::clone(&cfg);
        std::thread::spawn(move || {
            let _ = handle_conn(&desk, &cfg, stream);
        });
    }
}

fn handle_conn(desk: &FrontDesk, cfg: &ServerCfg, stream: std::os::unix::net::UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    // 一问一答 v0：读一行（上限粗防：op 行不该超 1 MiB）
    reader.read_line(&mut line)?;
    if line.len() > 1024 * 1024 {
        let _ = writeln!(&mut &stream, "{}", agio::fail(agio::ErrorType::Usage, "bad_request", "op line over 1MiB"));
        return Ok(());
    }
    let resp = ops::handle_line(desk, cfg, &line);
    let mut w = &stream;
    writeln!(w, "{resp}")?;
    w.flush()
}
