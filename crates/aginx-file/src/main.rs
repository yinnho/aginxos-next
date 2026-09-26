//! aginx-file — 文件工具 CLI（M32 D3 批2；D13 改名 2026-09-26，原 agf）。
//!
//! 两张脸：人/流程脚本面子命令（read/ls/write/convert/inspect），机读面
//! `aginx-file tool <name>`（stdin 收工具入参 JSON，stdout 出 D1 信封；
//! runtime 的 agf_bridge 消费）。身份与预解析路径经 stdin JSON 保留键
//! `_ctx` 注入——见 lib.rs 头注。

use clap::{Parser, Subcommand};
use serde_json::Value;

#[derive(Parser)]
#[command(
    name = "aginx-file",
    version,
    about = "文件工具 CLI — 读写/列表/转换/图像信息",
    long_about = "aginx-file 是文件面工具的 CLI 形态（D3 批2 外置成包；D13 改名，原 agf）。\n人面子命令直接给参数（路径按当前目录解析）；\n机读面 `aginx-file tool <name>` 从 stdin 读 JSON，stdout 出 D1 信封\n（{\"ok\":true,\"data\":…}）。"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 读文件内容（文本；文档格式走 markitdown 提取）
    Read {
        /// 文件路径
        path: String,
    },
    /// 列目录内容（文件不带后缀，目录带 /）
    Ls {
        /// 目录路径
        path: String,
    },
    /// 写文件（内容默认从 stdin 读；--content 可直给）
    Write {
        /// 目标文件路径（父目录自动创建）
        path: String,
        /// 要写的内容（缺省时读 stdin）
        #[arg(long)]
        content: Option<String>,
    },
    /// 文档格式转换（pandoc；markdown/html/docx/pdf/…）
    Convert {
        /// 输入文件路径
        input: String,
        /// 目标格式（pdf/docx/html/…）
        format: String,
        /// 输出路径（缺省自动生成 <stem>.<format>）
        #[arg(long)]
        out: Option<String>,
    },
    /// 图像文件信息（格式/尺寸/大小 + base64 预览）
    Inspect {
        /// 图像文件路径
        path: String,
    },
    /// 机读面：工具名 + stdin JSON 入参 → stdout D1 信封（runtime 桥用）
    Tool {
        /// 工具名（file_read / file_write / file_list / file_convert / image_analyze）
        name: String,
    },
}

fn main() -> anyhow::Result<()> {
    // CLI 人面常接 `| head`：Rust 默认忽略 SIGPIPE，写已关闭管道会以
    // "failed printing to stdout: Broken pipe" panic 收场。恢复默认处置
    // = 安静地死于 SIGPIPE，与普通 CLI 一致（设备实测过）。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cli))
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Tool { name } => {
            let mut raw = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)?;
            let input: Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("stdin 不是合法 JSON 入参: {e}"))?;
            match aginx_file::execute_tool(&name, &input).await {
                None => {
                    print_envelope_error(&format!("unknown tool: {name}"));
                    std::process::exit(1);
                }
                Some(Ok(data)) => {
                    println!(
                        "{}",
                        serde_json::json!({"ok": true, "data": data, "meta": {"tool": name}})
                    );
                }
                Some(Err(e)) => {
                    print_envelope_error(&e.to_string());
                    std::process::exit(1);
                }
            }
        }
        human => {
            // 人面：参数拼回工具 JSON 入参，走同一条 execute_tool 单真源。
            let (name, input) = args_to_input(human)?;
            let input_val = serde_json::Value::Object(input);
            match aginx_file::execute_tool(name, &input_val).await {
                None => anyhow::bail!("unknown tool: {name}"),
                Some(Ok(data)) => println!("{data}"),
                Some(Err(e)) => aginx_file::bail_human(&e),
            }
        }
    }
    Ok(())
}

/// 人面子命令 → (工具名, 工具入参 JSON)。与工具 schema 同构，仅做参数搬运。
/// write 的内容缺省读 stdin（sh-first：`… | aginx-file write f` / `aginx-file write f < in`）。
fn args_to_input(cmd: Command) -> anyhow::Result<(&'static str, serde_json::Map<String, Value>)> {
    use serde_json::json;
    let mut m = serde_json::Map::new();
    match cmd {
        Command::Read { path } => {
            m.insert("path".into(), json!(path));
            Ok(("file_read", m))
        }
        Command::Ls { path } => {
            m.insert("path".into(), json!(path));
            Ok(("file_list", m))
        }
        Command::Write { path, content } => {
            let content = match content {
                Some(c) => c,
                None => {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                }
            };
            m.insert("path".into(), json!(path));
            m.insert("content".into(), json!(content));
            Ok(("file_write", m))
        }
        Command::Convert { input, format, out } => {
            m.insert("input_path".into(), json!(input));
            m.insert("output_format".into(), json!(format));
            if let Some(op) = out {
                m.insert("output_path".into(), json!(op));
            }
            Ok(("file_convert", m))
        }
        Command::Inspect { path } => {
            m.insert("path".into(), json!(path));
            Ok(("image_analyze", m))
        }
        // 已在 run() 里分走；穷尽匹配留这层保险
        Command::Tool { .. } => unreachable!("Tool 在 run() 先行分派"),
    }
}

fn print_envelope_error(msg: &str) {
    println!("{}", serde_json::json!({"ok": false, "error": msg}));
    eprintln!("aginx-file: {msg}");
}
