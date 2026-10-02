// aginx-server — 前台（刀④-4 裁形，2026-10-02）：UDS 面 + 装卸链 +
// 系统直通条目。
//
// 引擎商品化（DESIGN.md §八）后 server 不再进程内 boot carrier
// kernel——对话真源=网关 agent://（system 与助理都是 gateway 名册里的
// codex 条目，④-2/④-3 定形）；send/create 面随 in-process 引擎退役。
// 剩余职责：
//   ① boot：落系统本人直通条目（ensure_system_entry，codex 形）
//   ② UDS 面：list/status/install/remove（装卸链 ④-3 搬入）
//
// env：
//   AGINX_SOCK         UDS 路径，默认 /run/aginx.sock（host 试跑必设）
//   AGINX_HOME         home 根（workflows/ 的父），默认 ~/.aginx
//   AGINX_DATA_DIR     网关数据根（agents/ 条目落处；缺席走 home 指针）

mod gateway_registry;
mod install;
mod ops;

use carrier_types::config::KernelConfig;
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = Arc::new(ServerCfg::from_env());

    let kernel_cfg = KernelConfig {
        home_dir: cfg.home.clone(),
        data_dir: cfg.home.join("data"),
        workflows_dir: Some(cfg.home.join("workflows")),
        ..KernelConfig::default()
    };
    let kernel_cfg = Arc::new(kernel_cfg);
    // 系统本人直通条目（agent://<机>.relay.aginx.net/system）——boot 期
    // 幂等补写，新机世界第一拍在这里落。
    gateway_registry::ensure_system_entry(&kernel_cfg);

    // 陈旧 socket 文件（上次崩溃留下的）先清；v0 单实例，不做存活探测
    let _ = std::fs::remove_file(&cfg.sock);
    let listener = match UnixListener::bind(&cfg.sock) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("aginx-server: {e}");
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
        let kernel_cfg = Arc::clone(&kernel_cfg);
        std::thread::spawn(move || {
            let _ = handle_conn(&kernel_cfg, stream);
        });
    }
}

fn handle_conn(kernel_cfg: &KernelConfig, stream: std::os::unix::net::UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(&stream);
    let mut line = String::new();
    // 一问一答 v0：读一行（上限粗防：op 行不该超 1 MiB）
    reader.read_line(&mut line)?;
    if line.len() > 1024 * 1024 {
        let _ = writeln!(&mut &stream, "{}", agio::fail(agio::ErrorType::Usage, "bad_request", "op line over 1MiB"));
        return Ok(());
    }
    let resp = ops::handle_line(kernel_cfg, &line);
    let mut w = &stream;
    writeln!(w, "{resp}")?;
    w.flush()
}
