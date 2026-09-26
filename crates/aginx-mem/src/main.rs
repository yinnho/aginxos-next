//! aginx-mem — 记忆工具 CLI（M35；D13 改名 2026-09-26，原 agmem）。
//!
//! 两张脸：人面子命令（kv get/set/list/del、tree search/topic/source/
//! global/drill/leaves、k 知识库面、flow 流程面、evaluate），机读面
//! `aginx-mem tool <name>`（stdin 收工具入参 JSON，stdout 出 D1 信封；
//! runtime 的 agmem_bridge 消费）。身份三元组经 stdin JSON 保留键 `_ctx`
//! 注入——见 lib.rs 头注。knowledge/flow 面要 workspace：桥注入
//! `_ctx.workspace_root`，人面给 --workspace。

use clap::{Parser, Subcommand};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "aginx-mem",
    version,
    about = "记忆工具 CLI — kv 存取 + 记忆树检索 + 知识库/流程",
    long_about = "aginx-mem 是记忆面工具的 CLI 形态（M35 外置成包；D13 改名，原 agmem）。\nkv 按 (agent, owner, user) 三元组隔离；tree 是已摄取对话/邮件/文档的\n回溯索引；k 面管化身 workspace 的知识库；flow 面管流程（skills）。\n人面子命令直接给参数；机读面 `aginx-mem tool <name>` 从 stdin 读 JSON，\nstdout 出 D1 信封（{\"ok\":true,\"data\":…}）。"
)]
struct Cli {
    /// substrate 库路径（缺省 $HOME/.aginx/carrier/data/carrier.db）
    #[arg(long, global = true)]
    db: Option<String>,
    /// knowledge/flow 面的 workspace 根（机读面经 _ctx.workspace_root 注入）
    #[arg(long, global = true)]
    workspace: Option<String>,
    /// 身份三元组：agent（默认 me）
    #[arg(long, global = true, default_value = "me")]
    agent: String,
    /// 身份三元组：owner（默认 default）
    #[arg(long, global = true, default_value = "default")]
    owner: String,
    /// 身份三元组：user（默认 local —— 本机人手，非渠道来信）
    #[arg(long, global = true, default_value = "local")]
    user: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// kv：读一个键
    Get {
        /// 键名
        key: String,
    },
    /// kv：写一个键（值缺省读 stdin；能解析成 JSON 就存 JSON，否则存字符串）
    Set {
        /// 键名
        key: String,
        /// 要存的值（缺省读 stdin）
        value: Option<String>,
    },
    /// kv：列键（可选前缀过滤）
    List {
        /// 前缀（如 entity.）
        prefix: Option<String>,
    },
    /// kv：删一个键
    Del {
        /// 键名
        key: String,
    },
    /// tree：实体搜索（后续 topic 查询的入口）
    Search {
        /// 搜索词
        query: String,
        /// 实体种类过滤（如 person）
        #[arg(long)]
        kind: Option<String>,
        /// 最多返回几条（默认 5）
        #[arg(long)]
        limit: Option<u64>,
    },
    /// tree：按实体查主题子树
    Topic {
        /// search_entities 给出的规范实体 ID
        entity_id: String,
        /// 过滤词
        #[arg(long)]
        query: Option<String>,
        /// 时间窗（天）
        #[arg(long)]
        days: Option<u64>,
    },
    /// tree：按来源查（chat/email/document）
    Source {
        /// 来源 ID
        #[arg(long)]
        source_id: Option<String>,
        /// 来源种类
        #[arg(long)]
        source_kind: Option<String>,
        /// 过滤词
        #[arg(long)]
        query: Option<String>,
        /// 时间窗（天）
        #[arg(long)]
        days: Option<u64>,
    },
    /// tree：全局查
    Global {
        /// 过滤词
        #[arg(long)]
        query: Option<String>,
        /// 时间窗（天）
        #[arg(long)]
        days: Option<u64>,
    },
    /// tree：展开摘要节点的子层
    Drill {
        /// 摘要节点 ID
        node_id: String,
        /// 层数（1–3，默认 1）
        #[arg(long)]
        depth: Option<u64>,
    },
    /// tree：按 chunk ID 取叶子原文
    Leaves {
        /// chunk ID 列表
        chunk_ids: Vec<String>,
    },
    /// 知识库面：化身 workspace 下的 knowledge/*.md（要 --workspace）
    K {
        #[command(subcommand)]
        action: KCmd,
    },
    /// 流程面：化身 workspace 下的 flows/*.md（要 --workspace）
    Flow {
        #[command(subcommand)]
        action: FlowCmd,
    },
    /// 分身质量体检（knowledge/skills/identity 面的确定性打分）
    Evaluate,
    /// 机读面：工具名 + stdin JSON 入参 → stdout D1 信封（runtime 桥用）
    Tool {
        /// 工具名（lib.rs TOOL_NAMES 为准：kv_* / memory_tree /
        /// knowledge_* / clone_evaluate / flow_*）
        name: String,
    },
}

#[derive(Subcommand)]
enum KCmd {
    /// 列出知识文件（带 frontmatter 标题）
    Ls,
    /// 读一个知识文件（未知文件名时列出可用清单）
    Read {
        /// 文件名（支持模糊匹配：精确 → 前缀 → 子串）
        filename: String,
    },
    /// 新增知识条目（正文从 stdin 读）
    Add {
        /// 标题（同时是文件名）
        title: String,
    },
    /// 整体替换一个知识文件（完整新内容从 stdin 读，须带 frontmatter）
    Update {
        /// 目标文件（模糊匹配）
        filename: String,
    },
    /// 删除知识文件（版本留痕）
    Rm {
        /// 目标文件（模糊匹配）
        filename: String,
    },
    /// 批量导入（数据从 stdin 读）
    Import {
        /// 数据类型（auto/json/markdown/text…）
        #[arg(long, default_value = "auto")]
        r#type: String,
    },
    /// 体检：frontmatter/索引问题清单
    Lint,
    /// 自修：lint 出的可自动修复项
    Heal,
    /// 重建 MEMORY.md 索引
    Index,
    /// 从原始内容提炼一条知识（正文从 stdin 读）
    Extract {
        /// 标题
        title: String,
    },
}

#[derive(Subcommand)]
enum FlowCmd {
    /// 新建流程（正文从 stdin 读）
    New {
        /// 流程名
        name: String,
        /// 描述（进 frontmatter）
        #[arg(long)]
        desc: Option<String>,
        /// 工具白名单（可多次）
        #[arg(long = "tool")]
        tools: Vec<String>,
    },
    /// 更新流程（--desc/--tool 改元数据；--stdin 换正文）
    Set {
        /// 流程名
        name: String,
        /// 新描述
        #[arg(long)]
        desc: Option<String>,
        /// 工具白名单（可多次；整体替换）
        #[arg(long = "tool")]
        tools: Vec<String>,
        /// 正文从 stdin 读（整体替换）
        #[arg(long)]
        stdin: bool,
    },
    /// 读流程全文
    Cat {
        /// 流程名（支持模糊匹配）
        name: String,
    },
}

fn main() -> anyhow::Result<()> {
    // CLI 人面常接 `| head`：Rust 默认忽略 SIGPIPE，写已关闭管道会以
    // "failed printing to stdout: Broken pipe" panic 收场。恢复默认处置
    // = 安静地死于 SIGPIPE，与普通 CLI 一致（aginx-file 同款，设备实测过）。
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run(cli))
}

fn fallback_of(cli: &Cli) -> aginx_mem::AgmemCtx {
    aginx_mem::AgmemCtx {
        agent_id: cli.agent.clone(),
        owner_id: Some(cli.owner.clone()),
        user_id: Some(cli.user.clone()),
        home_dir: None,
        db_path: None,
        workspace_root: None,
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let db_flag = cli.db.as_ref().map(PathBuf::from);
    let ws_flag = cli.workspace.as_ref().map(PathBuf::from);
    let fallback = fallback_of(&cli);
    match &cli.command {
        Command::Tool { name } => {
            let mut raw = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)?;
            let input: Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("stdin 不是合法 JSON 入参: {e}"))?;
            match aginx_mem::execute_tool(name, &input, db_flag.as_ref(), ws_flag.as_ref(), fallback)
                .await
            {
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
            let input_val = Value::Object(input);
            match aginx_mem::execute_tool(name, &input_val, db_flag.as_ref(), ws_flag.as_ref(), fallback)
                .await
            {
                None => anyhow::bail!("unknown tool: {name}"),
                Some(Ok(data)) => println!("{data}"),
                Some(Err(e)) => aginx_mem::bail_human(&e),
            }
        }
    }
    Ok(())
}

/// 人面子命令 → (工具名, 工具入参 JSON)。与工具 schema 同构，仅做参数搬运。
/// set 的值缺省读 stdin（sh-first：`… | aginx-mem set k` / `aginx-mem set k < in`）；
/// 能解析成 JSON 就按 JSON 存（数字/对象原样），否则存裸字符串。
fn args_to_input(cmd: &Command) -> anyhow::Result<(&'static str, serde_json::Map<String, Value>)> {
    use serde_json::json;
    let mut m = serde_json::Map::new();
    match cmd {
        Command::Get { key } => {
            m.insert("key".into(), json!(key));
            Ok(("kv_get", m))
        }
        Command::Set { key, value } => {
            let raw = match value {
                Some(v) => v.clone(),
                None => {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
                    buf
                }
            };
            let parsed: Value = serde_json::from_str(&raw).unwrap_or(Value::String(raw));
            m.insert("key".into(), json!(key));
            m.insert("value".into(), parsed);
            Ok(("kv_set", m))
        }
        Command::List { prefix } => {
            if let Some(p) = prefix {
                m.insert("prefix".into(), json!(p));
            }
            Ok(("kv_list", m))
        }
        Command::Del { key } => {
            m.insert("key".into(), json!(key));
            Ok(("kv_delete", m))
        }
        Command::Search {
            query,
            kind,
            limit,
        } => {
            m.insert("mode".into(), json!("search_entities"));
            m.insert("query".into(), json!(query));
            if let Some(k) = kind {
                m.insert("kinds".into(), json!([k]));
            }
            if let Some(l) = limit {
                m.insert("limit".into(), json!(l));
            }
            Ok(("memory_tree", m))
        }
        Command::Topic {
            entity_id,
            query,
            days,
        } => {
            m.insert("mode".into(), json!("query_topic"));
            m.insert("entity_id".into(), json!(entity_id));
            if let Some(q) = query {
                m.insert("query".into(), json!(q));
            }
            if let Some(d) = days {
                m.insert("time_window_days".into(), json!(d));
            }
            Ok(("memory_tree", m))
        }
        Command::Source {
            source_id,
            source_kind,
            query,
            days,
        } => {
            m.insert("mode".into(), json!("query_source"));
            if let Some(s) = source_id {
                m.insert("source_id".into(), json!(s));
            }
            if let Some(s) = source_kind {
                m.insert("source_kind".into(), json!(s));
            }
            if let Some(q) = query {
                m.insert("query".into(), json!(q));
            }
            if let Some(d) = days {
                m.insert("time_window_days".into(), json!(d));
            }
            Ok(("memory_tree", m))
        }
        Command::Global { query, days } => {
            m.insert("mode".into(), json!("query_global"));
            if let Some(q) = query {
                m.insert("query".into(), json!(q));
            }
            if let Some(d) = days {
                m.insert("time_window_days".into(), json!(d));
            }
            Ok(("memory_tree", m))
        }
        Command::Drill { node_id, depth } => {
            m.insert("mode".into(), json!("drill_down"));
            m.insert("node_id".into(), json!(node_id));
            if let Some(d) = depth {
                m.insert("max_depth".into(), json!(d));
            }
            Ok(("memory_tree", m))
        }
        Command::Leaves { chunk_ids } => {
            m.insert("mode".into(), json!("fetch_leaves"));
            m.insert("chunk_ids".into(), json!(chunk_ids));
            Ok(("memory_tree", m))
        }
        Command::K { action } => match action {
            KCmd::Ls => Ok(("knowledge_list", m)),
            KCmd::Read { filename } => {
                m.insert("filename".into(), json!(filename));
                Ok(("knowledge_read", m))
            }
            KCmd::Add { title } => {
                let content = read_stdin()?;
                m.insert("title".into(), json!(title));
                m.insert("content".into(), json!(content));
                Ok(("knowledge_add", m))
            }
            KCmd::Update { filename } => {
                let content = read_stdin()?;
                m.insert("filename".into(), json!(filename));
                m.insert("content".into(), json!(content));
                Ok(("knowledge_update", m))
            }
            KCmd::Rm { filename } => {
                m.insert("filename".into(), json!(filename));
                Ok(("knowledge_remove", m))
            }
            KCmd::Import { r#type } => {
                let data = read_stdin()?;
                m.insert("data".into(), json!(data));
                m.insert("data_type".into(), json!(r#type));
                Ok(("knowledge_import", m))
            }
            KCmd::Lint => Ok(("knowledge_lint", m)),
            KCmd::Heal => Ok(("knowledge_heal", m)),
            KCmd::Index => Ok(("knowledge_index", m)),
            KCmd::Extract { title } => {
                let content = read_stdin()?;
                m.insert("title".into(), json!(title));
                m.insert("content".into(), json!(content));
                Ok(("knowledge_extract", m))
            }
        },
        Command::Flow { action } => match action {
            FlowCmd::New {
                name,
                desc,
                tools,
            } => {
                let body = read_stdin()?;
                m.insert("name".into(), json!(name));
                if let Some(d) = desc {
                    m.insert("description".into(), json!(d));
                }
                if !tools.is_empty() {
                    m.insert("tools".into(), json!(tools));
                }
                m.insert("body".into(), json!(body));
                Ok(("flow_create", m))
            }
            FlowCmd::Set {
                name,
                desc,
                tools,
                stdin,
            } => {
                m.insert("name".into(), json!(name));
                if let Some(d) = desc {
                    m.insert("description".into(), json!(d));
                }
                if !tools.is_empty() {
                    m.insert("tools".into(), json!(tools));
                }
                if *stdin {
                    let body = read_stdin()?;
                    m.insert("body".into(), json!(body));
                }
                Ok(("flow_update", m))
            }
            FlowCmd::Cat { name } => {
                m.insert("name".into(), json!(name));
                Ok(("flow_load", m))
            }
        },
        Command::Evaluate => Ok(("clone_evaluate", m)),
        // 已在 run() 里分走；穷尽匹配留这层保险
        Command::Tool { .. } => unreachable!("Tool 在 run() 先行分派"),
    }
}

/// 正文类参数统一从 stdin 读（sh-first：`… | aginx-mem k add 标题`）。
fn read_stdin() -> anyhow::Result<String> {
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
    Ok(buf)
}

fn print_envelope_error(msg: &str) {
    println!("{}", serde_json::json!({"ok": false, "error": msg}));
    eprintln!("aginx-mem: {msg}");
}
