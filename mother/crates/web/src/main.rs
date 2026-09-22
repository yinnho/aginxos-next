//! aginx-web — AginxBrowser 客户端 CLI（原 agb，M31 D3 批1；D13 改姓 2026-09-09）。
//!
//! 两张脸：人/流程脚本面子命令（navigate/click/eval/type/scroll/wait/
//! back/close/screenshot/search/fetch），机读面 `aginx-web tool <name>`
//! （stdin 收工具入参 JSON，stdout 出 D1 信封；runtime 的 web_bridge 消费）。
//! 配置：AGINXBROWSER_URL（env 或 ~/.aginx/carrier/.env，启动时加载）。

use clap::{Parser, Subcommand};
use serde_json::Value;

#[derive(Parser)]
#[command(
    name = "aginx-web",
    version,
    about = "AginxBrowser 客户端 — browser/search/fetch 工具 CLI",
    long_about = "aginx-web 是浏览器/搜索/抓取工具的 CLI 形态（D3 批1 外置成包）。\n人面子命令直接给参数；机读面 `aginx-web tool <name>` 从 stdin 读 JSON，\nstdout 出 D1 信封（{\"ok\":true,\"data\":…}）。"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 打开 URL 返回页面内容（markdown/html/text，CSS selector 提取）
    Navigate {
        url: String,
        /// 输出格式：markdown | html | text
        #[arg(long, default_value = "markdown")]
        format: String,
        /// CSS selector，只取命中区域
        #[arg(long)]
        selector: Option<String>,
        /// 页面加载后等 JS 渲染的秒数
        #[arg(long)]
        wait_secs: Option<u64>,
        /// 走代理（境外站）
        #[arg(long, default_value_t = false)]
        proxy: bool,
    },
    /// 读页面为纯文本（navigate 的 format=text 别名）
    Read {
        url: String,
        #[arg(long)]
        selector: Option<String>,
    },
    /// 点击元素（JS element.click()），回读点击后页面文本
    Click {
        url: String,
        /// 要点击的元素的 CSS selector
        #[arg(long)]
        selector: String,
        #[arg(long)]
        wait_secs: Option<u64>,
    },
    /// 在页面上执行 JavaScript，返回结果
    Eval {
        url: String,
        /// JS 表达式或 async IIFE
        #[arg(long)]
        script: String,
        #[arg(long)]
        wait_secs: Option<u64>,
    },
    /// 向输入框打字（JS 模拟）
    Type {
        url: String,
        #[arg(long)]
        selector: String,
        #[arg(long)]
        text: String,
    },
    /// 滚动页面
    Scroll {
        url: String,
        /// up | down
        #[arg(long, default_value = "down")]
        direction: String,
        /// 像素（默认 500）
        #[arg(long)]
        amount: Option<u64>,
    },
    /// 等元素出现或等一段时间
    Wait {
        url: String,
        #[arg(long)]
        selector: Option<String>,
        /// 最长等多少毫秒（默认 5000）
        #[arg(long)]
        timeout_ms: Option<u64>,
    },
    /// 兼容面：AginxBrowser 无导航历史，返回提示
    Back { url: String },
    /// 兼容面：AginxBrowser 无状态，无操作
    Close,
    /// 兼容面：轻量引擎不支持截图，报错并指路 navigate
    Screenshot { url: String },
    /// 网页搜索（聚合后端；fetch-top>0 时连正文一起抓）
    Search {
        /// 搜索词
        q: String,
        /// 自动抓前 N 条的正文
        #[arg(long)]
        fetch_top: Option<u64>,
        /// general | news | images …
        #[arg(long)]
        categories: Option<String>,
        #[arg(long)]
        language: Option<String>,
        #[arg(long)]
        max_results: Option<u64>,
        /// 每条正文截断字符数
        #[arg(long)]
        max_chars_per: Option<u64>,
    },
    /// 抓 URL（SSRF 防护 + HTML→Markdown + 截断；风控站自动走 AginxBrowser）
    Fetch {
        url: String,
        /// GET | POST | PUT | PATCH | DELETE
        #[arg(long, default_value = "GET")]
        method: String,
        /// 请求头 k=v（可多次）
        #[arg(long = "header")]
        headers: Vec<String>,
        /// 请求体
        #[arg(long)]
        body: Option<String>,
    },
    /// 机读面：工具名 + stdin JSON 入参 → stdout D1 信封（runtime 桥用）
    Tool {
        /// 工具名（browser_navigate / web_search / web_fetch …）
        name: String,
    },
}

fn main() -> anyhow::Result<()> {
    // CLI 人面常接 `| head`：Rust 默认忽略 SIGPIPE，写已关闭管道会以
    // "failed printing to stdout: Broken pipe" panic 收场（设备实测）。
    // 恢复默认处置 = 安静地死于 SIGPIPE，与普通 CLI 一致。桥路径不受
    // 影响（runtime 全量消费 stdout）。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    // 与 kernel 同一份 .env 加载（AGINXBROWSER_URL 等优先级一致）
    carrier_types::dotenv::load_dotenv();
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
            match aginx_web::execute_tool(&name, &input).await {
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
            let (name, input) = args_to_input(human);
            let input_val = serde_json::Value::Object(input);
            match aginx_web::execute_tool(name, &input_val).await {
                None => anyhow::bail!("unknown tool: {name}"),
                Some(Ok(data)) => println!("{data}"),
                Some(Err(e)) => aginx_web::bail_human(&e),
            }
        }
    }
    Ok(())
}

/// 人面子命令 → (工具名, 工具入参 JSON)。与工具 schema 同构，仅做参数搬运。
fn args_to_input(cmd: Command) -> (&'static str, serde_json::Map<String, Value>) {
    use serde_json::json;
    let mut m = serde_json::Map::new();
    match cmd {
        Command::Navigate {
            url,
            format,
            selector,
            wait_secs,
            proxy,
        } => {
            m.insert("url".into(), json!(url));
            m.insert("format".into(), json!(format));
            if let Some(v) = selector {
                m.insert("selector".into(), json!(v));
            }
            if let Some(v) = wait_secs {
                m.insert("wait_secs".into(), json!(v));
            }
            m.insert("use_proxy".into(), json!(proxy));
            ("browser_navigate", m)
        }
        Command::Read { url, selector } => {
            m.insert("url".into(), json!(url));
            m.insert("format".into(), json!("text"));
            if let Some(v) = selector {
                m.insert("selector".into(), json!(v));
            }
            ("browser_read_page", m)
        }
        Command::Click {
            url,
            selector,
            wait_secs,
        } => {
            m.insert("url".into(), json!(url));
            m.insert("selector".into(), json!(selector));
            if let Some(v) = wait_secs {
                m.insert("wait_secs".into(), json!(v));
            }
            ("browser_click", m)
        }
        Command::Eval {
            url,
            script,
            wait_secs,
        } => {
            m.insert("url".into(), json!(url));
            m.insert("script".into(), json!(script));
            if let Some(v) = wait_secs {
                m.insert("wait_secs".into(), json!(v));
            }
            ("browser_evaluate", m)
        }
        Command::Type {
            url,
            selector,
            text,
        } => {
            m.insert("url".into(), json!(url));
            m.insert("selector".into(), json!(selector));
            m.insert("text".into(), json!(text));
            ("browser_type", m)
        }
        Command::Scroll {
            url,
            direction,
            amount,
        } => {
            m.insert("url".into(), json!(url));
            m.insert("direction".into(), json!(direction));
            if let Some(v) = amount {
                m.insert("amount".into(), json!(v));
            }
            ("browser_scroll", m)
        }
        Command::Wait {
            url,
            selector,
            timeout_ms,
        } => {
            m.insert("url".into(), json!(url));
            if let Some(v) = selector {
                m.insert("selector".into(), json!(v));
            }
            if let Some(v) = timeout_ms {
                m.insert("timeout_ms".into(), json!(v));
            }
            ("browser_wait", m)
        }
        Command::Back { url } => {
            m.insert("url".into(), json!(url));
            ("browser_back", m)
        }
        Command::Screenshot { url } => {
            m.insert("url".into(), json!(url));
            ("browser_screenshot", m)
        }
        Command::Close => ("browser_close", m),
        Command::Search {
            q,
            fetch_top,
            categories,
            language,
            max_results,
            max_chars_per,
        } => {
            m.insert("q".into(), json!(q));
            if let Some(v) = fetch_top {
                m.insert("fetch_top".into(), json!(v));
            }
            if let Some(v) = categories {
                m.insert("categories".into(), json!(v));
            }
            if let Some(v) = language {
                m.insert("language".into(), json!(v));
            }
            if let Some(v) = max_results {
                m.insert("max_results".into(), json!(v));
            }
            if let Some(v) = max_chars_per {
                m.insert("max_chars_per".into(), json!(v));
            }
            ("web_search", m)
        }
        Command::Fetch {
            url,
            method,
            headers,
            body,
        } => {
            m.insert("url".into(), json!(url));
            m.insert("method".into(), json!(method));
            if !headers.is_empty() {
                let mut h = serde_json::Map::new();
                for kv in headers {
                    if let Some((k, v)) = kv.split_once('=') {
                        h.insert(k.to_string(), json!(v));
                    }
                }
                m.insert("headers".into(), Value::Object(h));
            }
            if let Some(v) = body {
                m.insert("body".into(), json!(v));
            }
            ("web_fetch", m)
        }
        // 已在 run() 里分走；穷尽匹配留这层保险
        Command::Tool { .. } => unreachable!("Tool 在 run() 先行分派"),
    }
}

fn print_envelope_error(msg: &str) {
    println!("{}", serde_json::json!({"ok": false, "error": msg}));
    eprintln!("aginx-web: {msg}");
}
