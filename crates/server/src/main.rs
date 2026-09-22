// aginx-server — 母体（宪法 D4/D10/D11）：前台登记（进/住/切/退）、会话
// 光标、请求路由、D8 会话账、kernel 直调宿主。
//
// 入口 = UDS（v0 不开 TCP）：每条连线一问一答——读一行 op JSON，回一行
// D1 信封，连接即关。化身轮次在连线线程里同步跑完（前台一次一轮，
// turn 锁在 front 层）；真 brain 一轮可以跑几分钟，客户端就等几分钟，
// 这正是语音/终端对话的产品形状。
//
// 刀2（直调）：server 进程内 boot carrier kernel（host.rs），轮到时
// 直调 send_message——OS 进程=agent 进程，盒内零 hop。aginx-runtime
// 子进程与它的 env（AGINX_RUNTIME_BIN）退役。
//
// env：
//   AGINX_SOCK         UDS 路径，默认 /run/aginx.sock（host 试跑必设）
//   AGINX_HOME         home 根（workflows/ 与 sessions/ 的父），默认 ~/.aginx
//   AGINX_BRAIN_URL / AGINXBRAIN_API_KEY  brain.json 缺席时的一次性桥
//                      （host.rs；此后文件是真源）

mod front;
mod ledger;
mod host;
mod ops;
#[cfg(test)]
mod testkit;

use front::FrontDesk;
use host::Mother;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ServerCfg {
    pub home: PathBuf,
    pub sock: PathBuf,
}

impl ServerCfg {
    pub fn from_env() -> ServerCfg {
        let home = std::env::var("AGINX_HOME").map(PathBuf::from).unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join(".aginx")
        });
        ServerCfg {
            home,
            sock: PathBuf::from(std::env::var("AGINX_SOCK").unwrap_or_else(|_| "/run/aginx.sock".into())),
        }
    }
}

fn main() {
    let cfg = Arc::new(ServerCfg::from_env());
    // kernel 内部多处读全局 home_dir()（AGINX_HOME env）——boot 前把
    // resolved home 写回 env，保证进程内全局一致（edition 2021 安全）。
    std::env::set_var("AGINX_HOME", &cfg.home);

    let desk = Arc::new(FrontDesk::new(cfg.home.join("workflows")));
    let mother = match Mother::boot(cfg.home.clone(), Arc::clone(&desk)) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("aginx-server: {e}");
            std::process::exit(1);
        }
    };
    let mother = Arc::new(mother);

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
        "aginx-server: listening on {} (home: {})",
        cfg.sock.display(),
        cfg.home.display()
    );

    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let desk = Arc::clone(&desk);
        let mother = Arc::clone(&mother);
        std::thread::spawn(move || {
            let _ = handle_conn(&desk, &mother, stream);
        });
    }
}

fn handle_conn(desk: &FrontDesk, mother: &Mother, stream: std::os::unix::net::UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    // 一问一答 v0：读一行（上限粗防：op 行不该超 1 MiB）
    reader.read_line(&mut line)?;
    if line.len() > 1024 * 1024 {
        let _ = writeln!(&mut &stream, "{}", agio::fail(agio::ErrorType::Usage, "bad_request", "op line over 1MiB"));
        return Ok(());
    }
    let resp = ops::handle_line(desk, mother, &line);
    let mut w = &stream;
    writeln!(w, "{resp}")?;
    w.flush()
}
