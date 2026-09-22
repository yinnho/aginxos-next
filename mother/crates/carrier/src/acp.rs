//! stdio ACP 桥 — 把分身暴露到 aginx 网关（agent:// 第一刀）。
//!
//! 网关按 `~/.aginx/agents/<clone>/aginx.toml` 拉起本命令，stdin/stdout 走
//! ndjson JSON-RPC（ACP 标准，协议全文见 aginx 仓 ACP.md）。`initialize` 只
//! 做握手零开销；`session/new` 才进程内 boot kernel（lazy，boot 失败回
//! JSON-RPC error 不崩桥）；`session/prompt` 直投 `kernel.send_message`，
//! 回包走 `agent_message_chunk` + `end_turn`。
//!
//! 远程化身句柄（CARRIER.md §3.3 远程类）：`--clone` 是 `agent remote add`
//! 注册的别名时，本桥变成纯转发——不 boot kernel，`session/prompt` 经
//! carrier-gateway 的 agent:// 外部协议投递到目标网关，chunk 即时转成 ACP
//! 通知，网关收割的 sessionId 存回会话供续接。本机化身和远程化身对使用
//! 者同构。
//!
//! stdout 只允许 ACP 消息（tracing 全走 stderr）。prompt 处理并发化：
//! reader 线程喂数，prompt 每个起 tokio task，`session/cancel` 打标志位让
//! in-flight 轮尽快返回 `cancelled`。

use std::collections::HashMap;
use std::io::{BufRead as _, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use carrier_gateway::agent_client::AgentConn;
use carrier_kernel::kernel::CarrierKernel;
use carrier_memory::session::SessionTicket;
use carrier_types::agent::AgentId;

use crate::remote::RemoteHandle;

/// One ACP session = one agent binding + a cancellation flag. 绑定两态：
/// 本地化身（kernel agent）或远程化身句柄（网关转发）。
struct Session {
    kind: SessionKind,
    cancelled: Arc<AtomicBool>,
}

enum SessionKind {
    Local {
        agent_id: AgentId,
    },
    /// 远程化身：gw_session 是目标网关侧收割的会话 id（首轮 None，
    /// 每轮收割回写——续接链在网关台账）。
    Remote {
        handle: RemoteHandle,
        gw_session: Arc<Mutex<Option<String>>>,
    },
}

struct BridgeState {
    clone: String,
    /// Lazy kernel boot — `initialize` must stay free.
    kernel: Mutex<Option<Arc<CarrierKernel>>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

/// Run the bridge until stdin closes. Never returns normally on I/O death —
/// errors bubble to `main` and kill the process (the gateway respawns on
/// demand).
///
/// stdin sniffing: if the first line is an ACP JSON-RPC message, run the ACP
/// bridge; otherwise run one-shot `ask` mode (the gateway's `PromptAdapter`
/// writes a bare prompt line to stdin and expects stdout lines back). Both
/// modes share the same `aginx.toml` command entry.
pub fn run(clone: String, session: Option<String>) -> anyhow::Result<()> {
    // All logs to stderr — stdout is protocol-only.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // Sniff the first stdin line to pick ACP-bridge vs one-shot-ask mode.
    //
    // NOTE: must NOT hold a `StdinLock` across the reader-thread spawn below —
    // the lock guards the process-global stdin mutex, and `run()` stays alive
    // inside `block_on` forever, so a leaked lock deadlocks the reader thread
    // (first message works via `tx.send(first)`, everything after hangs).
    // `Stdin::read_line` locks-and-releases per call and reads from the same
    // global buffer, so no line is lost.
    let mut first_raw = String::new();
    if std::io::stdin().read_line(&mut first_raw)? == 0 {
        return Ok(()); // EOF before any input
    }
    let first = first_raw.trim_end_matches(['\r', '\n']).to_string();
    let is_acp = first.trim().starts_with('{')
        && serde_json::from_str::<serde_json::Value>(first.trim())
            .map(|v| v.get("jsonrpc").is_some())
            .unwrap_or(false);

    if !is_acp {
        // One-shot ask: remaining lines join the first as the message.
        let mut msg = first;
        for l in std::io::stdin()
            .lock()
            .lines()
            .map_while(Result::ok)
        {
            msg.push('\n');
            msg.push_str(&l);
        }
        return run_ask(&clone, session.as_deref(), &msg);
    }

    let state = Arc::new(BridgeState {
        clone,
        kernel: Mutex::new(None),
        sessions: Mutex::new(HashMap::new()),
    });
    // Serialize stdout writes across concurrent prompt handlers.
    let stdout_lock = Arc::new(Mutex::new(std::io::stdout()));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        tx.send(first).ok();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in stdin.lock().lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        while let Some(line) = rx.recv().await {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let msg: serde_json::Value = match serde_json::from_str(line) {
                Ok(v) => v,
                Err(e) => {
                    emit(
                        &stdout_lock,
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": null,
                            "error": {"code": -32700, "message": format!("Parse error: {e}")},
                        }),
                    );
                    continue;
                }
            };
            let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
            let id = msg.get("id").cloned();
            let params = msg.get("params").cloned().unwrap_or(serde_json::json!({}));

            // Notifications (no id) we act on: session/cancel. Others dropped.
            let Some(id) = id else {
                if method == "session/cancel" {
                    let sid = params
                        .get("sessionId")
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    if let Some(sess) = state.sessions.lock().unwrap().get(sid) {
                        sess.cancelled.store(true, Ordering::Relaxed);
                    }
                }
                continue;
            };

            match method {
                "initialize" => respond(
                    &stdout_lock, &id,
                    serde_json::json!({
                        "protocolVersion": 1,
                        "agentInfo": {
                            "name": state.clone,
                            "title": format!("aginx-carrier · {}", state.clone),
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "agentCapabilities": {
                            "loadSession": false,
                            "promptCapabilities": {
                                "image": false, "audio": false, "embeddedContext": true,
                            },
                        },
                        "authMethods": [],
                    }),
                ),
                "session/new" => match boot_and_register(&state) {
                    Ok(session_id) => respond(
                        &stdout_lock, &id,
                        serde_json::json!({"sessionId": session_id}),
                    ),
                    Err(e) => respond_error(&stdout_lock, &id, -32000, &e),
                },
                "session/prompt" => {
                    let st = Arc::clone(&state);
                    let out = Arc::clone(&stdout_lock);
                    tokio::spawn(async move {
                        handle_prompt(st, out, id, params).await;
                    });
                }
                "session/set_mode" => respond(&stdout_lock, &id, serde_json::Value::Null),
                _ => respond_error(&stdout_lock, &id, -32601, &format!("Method not found: {method}")),
            }
        }
    });
    Ok(())
}

/// Boot the kernel (once, lazy) — `initialize` stays free.
fn ensure_kernel(state: &BridgeState) -> Result<Arc<CarrierKernel>, String> {
    let mut guard = state.kernel.lock().unwrap();
    if guard.is_none() {
        let kernel = CarrierKernel::boot(None).map_err(|e| format!("kernel boot failed: {e}"))?;
        let kernel = Arc::new(kernel);
        // 桥内 kernel 也要挂 self_handle：session/prompt 的轮次要能调跨 agent
        // 工具（clone_install/cron_create…），否则 "Kernel handle not available"
        // 连环失败（run_ask 一直有，这条路径漏了）。
        kernel.set_self_handle();
        *guard = Some(kernel);
    }
    Ok(Arc::clone(guard.as_ref().unwrap()))
}

/// Resolve `--clone` to a session binding and register a session.
///
/// 分派三路：本地化身（aginx.toml 在册）走 lazy boot——与既有行为一致，
/// boot 失败保留原报错路径；远程句柄（且无同名本地化身）不 boot kernel
/// （本机无 brain/无本地化身也能对话远程化身）；两者皆非时落回 lazy boot，
/// 让 "未安装" 类错误原样报告。名字互斥在注册期拦死（remote add 双向查）。
fn boot_and_register(state: &BridgeState) -> Result<String, String> {
    let kind = if carrier_kernel::aginx_net::registration_exists_default(&state.clone) {
        let kernel = ensure_kernel(state)?;
        let entry = kernel
            .registry
            .find_by_name(&state.clone)
            .ok_or_else(|| format!("clone '{}' not installed on this carrier", state.clone))?;
        SessionKind::Local { agent_id: entry.id }
    } else if let Some(handle) = crate::remote::find(&state.clone) {
        SessionKind::Remote {
            handle,
            gw_session: Arc::new(Mutex::new(None)),
        }
    } else {
        let kernel = ensure_kernel(state)?;
        let entry = kernel
            .registry
            .find_by_name(&state.clone)
            .ok_or_else(|| {
                format!(
                    "clone '{}' not installed on this carrier（agent install 装本地化身 / agent remote add 注册远程化身）",
                    state.clone
                )
            })?;
        SessionKind::Local { agent_id: entry.id }
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    state.sessions.lock().unwrap().insert(
        session_id.clone(),
        Arc::new(Session {
            kind,
            cancelled: Arc::new(AtomicBool::new(false)),
        }),
    );
    Ok(session_id)
}

/// Flatten ACP prompt content blocks into the text the kernel turn consumes.
/// We declared image/audio capability false, so anything else is an error.
fn prompt_text(prompt: &serde_json::Value) -> Result<String, String> {
    let blocks = prompt
        .as_array()
        .ok_or_else(|| "prompt must be a content block array".to_string())?;
    let mut out = String::new();
    for b in blocks {
        match b.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                out.push_str(b.get("text").and_then(|t| t.as_str()).unwrap_or(""));
                out.push('\n');
            }
            Some("resource") => {
                let res = b.get("resource").cloned().unwrap_or_default();
                out.push_str(&format!(
                    "[resource {}]\n{}\n",
                    res.get("uri").and_then(|u| u.as_str()).unwrap_or(""),
                    res.get("text").and_then(|t| t.as_str()).unwrap_or(""),
                ));
            }
            Some("resource_link") => {
                out.push_str(&format!(
                    "[file {}]\n",
                    b.get("uri").and_then(|u| u.as_str()).unwrap_or(""),
                ));
            }
            other => return Err(format!("unsupported prompt block type: {other:?}")),
        }
    }
    Ok(out.trim().to_string())
}

async fn handle_prompt(
    state: Arc<BridgeState>,
    out: Arc<Mutex<std::io::Stdout>>,
    id: serde_json::Value,
    params: serde_json::Value,
) {
    let sid = params
        .get("sessionId")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let Some(session) = state.sessions.lock().unwrap().get(&sid).cloned() else {
        respond_error(&out, &id, -32001, "unknown sessionId");
        return;
    };
    session.cancelled.store(false, Ordering::Relaxed);
    let text = match prompt_text(params.get("prompt").unwrap_or(&serde_json::Value::Null)) {
        Ok(t) => t,
        Err(e) => {
            respond_error(&out, &id, -32602, &e);
            return;
        }
    };

    // 远程化身句柄：经网关外部协议转发（agent_client），本机零 kernel 参与。
    if let SessionKind::Remote { handle, gw_session } = &session.kind {
        if params.get("sessionTicket").is_some() {
            // 借用轮的会话真源在分身主人侧——远程句柄不是主人，无从出票/收票。
            respond_error(
                &out,
                &id,
                -32602,
                "远程化身会话不支持借用轮（sessionTicket）——借用请走主人网关的借用通道",
            );
            return;
        }
        handle_remote_prompt(
            Arc::clone(&out),
            id,
            sid,
            handle.clone(),
            Arc::clone(gw_session),
            text,
            Arc::clone(&session.cancelled),
        )
        .await;
        return;
    }

    let SessionKind::Local { agent_id } = session.kind else {
        unreachable!("Remote 已在上分支返回");
    };
    let kernel = Arc::clone(state.kernel.lock().unwrap().as_ref().unwrap());
    let cancelled = Arc::clone(&session.cancelled);

    // 借用机制：`session/prompt` 带 `sessionTicket` → 无状态借用轮（会话票据进/
    // 出，主人服务器零持久化）。不带 ticket → 维持现有持久 session（向后兼容）。
    if let Some(ticket_val) = params.get("sessionTicket").cloned() {
        // 3.2 素材：materials[{name, contentBase64}]——仅本轮有效，轮末销毁。
        let materials = match parse_materials(params.get("materials")) {
            Ok(m) => m,
            Err(e) => {
                respond_error(&out, &id, -32002, &e);
                return;
            }
        };
        let active_flow = params
            .get("activeFlow")
            .and_then(|f| f.as_str())
            .map(|s| s.to_string());
        // 借用者身份：网关把鉴权身份透传下来做准入/配额；本地直连不带。
        let borrower = params
            .get("borrower")
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());
        handle_borrowed_prompt(
            Arc::clone(&out),
            id,
            sid,
            kernel,
            agent_id,
            text,
            ticket_val,
            materials,
            active_flow,
            borrower,
        )
        .await;
        return;
    }

    // Each ACP session runs in its own kernel session (`acp:<id>` label —
    // session isolation refuses unlabeled turns). channel_type keeps the
    // channel-side paths happy without claiming a real channel.
    // kernel_handle 传 self——跨 agent 工具（clone_install 等）依赖它。
    let kh: Arc<dyn carrier_runtime::kernel_handle::KernelHandle> = kernel.clone();
    let turn = kernel.send_message_with_handle(
        agent_id,
        &text,
        Some(kh),
        Some(format!("acp:{sid}")),
        Some("aginx".to_string()),
        None,
        Some("acp".to_string()),
        None,
        None,
    );
    tokio::pin!(turn);

    let result = tokio::select! {
        r = &mut turn => r,
        // Poll the cancel flag while the turn runs.
        _ = wait_cancelled(Arc::clone(&cancelled)) => {
            Err(carrier_types::error::CarrierError::Internal("cancelled".into()).into())
        }
    };

    match result {
        Ok(r) if !r.response.trim().is_empty() => {
            update(
                &out, &sid,
                serde_json::json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": {"type": "text", "text": r.response},
                }),
            );
            respond(&out, &id, serde_json::json!({"stopReason": "end_turn"}));
        }
        // Silent turn (agent chose not to reply) — still a clean end_turn.
        Ok(_) => respond(&out, &id, serde_json::json!({"stopReason": "end_turn"})),
        Err(e) if e.to_string().contains("cancelled") => {
            respond(&out, &id, serde_json::json!({"stopReason": "cancelled"}))
        }
        Err(e) => respond_error(&out, &id, -32002, &format!("agent turn failed: {e}")),
    }
}

/// 远程化身轮：连目标网关（agent:// 外部协议）→ initialize → prompt。
/// chunk 即时转成 ACP `agent_message_chunk` 通知；网关收割的 sessionId 存回
/// 会话（gw_session）供下轮续接。cancel 旗标经 select 打断等待——连接随轮
/// 结束即弃（一次性连接，不复用）。
#[allow(clippy::too_many_arguments)]
async fn handle_remote_prompt(
    out: Arc<Mutex<std::io::Stdout>>,
    id: serde_json::Value,
    sid: String,
    handle: RemoteHandle,
    gw_session: Arc<Mutex<Option<String>>>,
    text: String,
    cancelled: Arc<AtomicBool>,
) {
    let Some(ep) = crate::remote::endpoint(&handle) else {
        respond_error(
            &out,
            &id,
            -32003,
            &format!(
                "无法解析远程化身地址 {}（需 agent://<id>.relay.<domain> 形态；本机 ~/.aginx/config.toml 缺 [relay] 段时 relay secret 无源）",
                handle.url
            ),
        );
        return;
    };
    let mut conn = match AgentConn::connect(&ep).await {
        Ok(c) => c,
        Err(e) => {
            respond_error(&out, &id, -32003, &format!("连接远程网关失败: {e}"));
            return;
        }
    };
    if let Err(e) = conn.initialize().await {
        respond_error(&out, &id, -32003, &format!("远程网关握手失败: {e}"));
        return;
    }

    let prev_sid = gw_session.lock().unwrap().clone();
    let out2 = Arc::clone(&out);
    let sid2 = sid.clone();
    let fut = conn.prompt(&handle.agent, &text, prev_sid.as_deref(), move |chunk| {
        update(
            &out2,
            &sid2,
            serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": chunk},
            }),
        );
        true
    });
    tokio::pin!(fut);
    let result = tokio::select! {
        r = &mut fut => r,
        _ = wait_cancelled(cancelled) => Err("cancelled".to_string()),
    };

    match result {
        Ok(r) => {
            // 续接链：收割到的网关会话 id 回写（harvest 已过字符集门）。
            if let Some(gsid) = r.session_id {
                *gw_session.lock().unwrap() = Some(gsid);
            }
            respond(&out, &id, serde_json::json!({"stopReason": "end_turn"}));
        }
        Err(e) if e == "cancelled" => {
            respond(&out, &id, serde_json::json!({"stopReason": "cancelled"}))
        }
        Err(e) => respond_error(&out, &id, -32003, &format!("远程轮失败: {e}")),
    }
}

/// 借用轮处理：会话票据进/出，走 `run_borrowed_turn`（无状态，主人服务器零持久化）。
#[allow(clippy::too_many_arguments)]
async fn handle_borrowed_prompt(
    out: Arc<Mutex<std::io::Stdout>>,
    id: serde_json::Value,
    sid: String,
    kernel: Arc<CarrierKernel>,
    agent_id: AgentId,
    text: String,
    ticket_val: serde_json::Value,
    materials: Vec<carrier_kernel::messaging::BorrowedMaterial>,
    active_flow: Option<String>,
    borrower: Option<String>,
) {
    let ticket: SessionTicket = match serde_json::from_value(ticket_val) {
        Ok(t) => t,
        Err(e) => {
            respond_error(&out, &id, -32602, &format!("invalid sessionTicket: {e}"));
            return;
        }
    };

    match kernel
        .run_borrowed_turn(
            agent_id,
            ticket,
            &text,
            Some(Arc::clone(&kernel) as Arc<dyn carrier_runtime::kernel_handle::KernelHandle>),
            active_flow.as_deref(),
            &materials,
            None,
            borrower.as_deref(),
        )
        .await
    {
        Ok(result) => {
            if !result.response.trim().is_empty() {
                update(
                    &out,
                    &sid,
                    serde_json::json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": result.response},
                    }),
                );
            }
            // 返回更新后的票据 + 回传产物——会话与产物的真源都在用户侧，服务器无状态。
            respond(
                &out,
                &id,
                serde_json::json!({
                    "stopReason": "end_turn",
                    "sessionTicket": result.ticket,
                    "files": result.files,
                }),
            );
        }
        Err(e) => respond_error(&out, &id, -32002, &format!("borrowed turn failed: {e}")),
    }
}

/// 解析 3.2 素材参数：`materials: [{name, contentBase64}]`。空/缺省 → 空集。
fn parse_materials(
    val: Option<&serde_json::Value>,
) -> Result<Vec<carrier_kernel::messaging::BorrowedMaterial>, String> {
    use base64::Engine as _;

    let Some(arr) = val else {
        return Ok(Vec::new());
    };
    let arr = arr.as_array().ok_or("materials must be an array")?;
    let mut out = Vec::with_capacity(arr.len());
    for m in arr {
        let name = m
            .get("name")
            .and_then(|s| s.as_str())
            .ok_or("material entry missing name")?
            .to_string();
        let b64 = m
            .get("contentBase64")
            .and_then(|s| s.as_str())
            .ok_or_else(|| format!("material {name:?} missing contentBase64"))?;
        let content = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("material {name:?} base64 decode failed: {e}"))?;
        out.push(carrier_kernel::messaging::BorrowedMaterial { name, content });
    }
    Ok(out)
}

/// A tiny future that resolves once the cancel flag is raised (5ms poll —
/// cancellation is a rare, human-timeout-scale event).
async fn wait_cancelled(flag: Arc<AtomicBool>) {
    loop {
        if flag.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// One-shot `ask` mode — the gateway `PromptAdapter` contract: prompt on
/// stdin, response lines on stdout, exit 0.
///
/// 会话契约（统一 aginx 流程）：stdout 说 claude-stream-json 方言（接入包
/// 声明 `output = "claude-stream-json"`，网关翻译器收割）——每个 TextDelta
/// 一行 `assistant` 事件，轮末一行 `result` 事件带 `session_id` /
/// `num_turns` / `duration_ms` / `is_error`。`--session` 缺省铸造新 uuid；
/// kernel 会话 label = `aginx:<session_id>`——同 id 跨进程续接同一会话
/// （连续性由 carrier 侧 session 存储保证，网关台账只记账目）。
fn run_ask(clone: &str, session: Option<&str>, message: &str) -> anyhow::Result<()> {
    // 远程化身句柄：无本地 kernel，直接走网关外部协议。`--session` 语义
    // 与本地不同——它是网关侧收割的会话 id（result 行原样回吐，下轮喂回
    // 即续接同一条远程会话）。
    if let Some(handle) = crate::remote::find(clone) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return runtime.block_on(run_remote_ask(&handle, session, message));
    }

    // Charset gate（网关侧同款注入门）：非法 id 弃用改铸造，不炸轮。
    let sid = session
        .filter(|s| valid_session_id(s))
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let started = std::time::Instant::now();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let kernel = CarrierKernel::boot(None)
            .map_err(|e| anyhow::anyhow!("kernel boot failed: {e}"))?;
        let kernel = Arc::new(kernel);
        kernel.set_self_handle();
        let kh: Arc<dyn carrier_runtime::kernel_handle::KernelHandle> = kernel.clone();
        let entry = kernel
            .registry
            .find_by_name(clone)
            .ok_or_else(|| anyhow::anyhow!("clone '{clone}' not installed on this carrier"))?;
        let agent_id = entry.id;

        let (mut rx, handle) = kernel
            .send_message_streaming(
                agent_id,
                message,
                Some(kh),
                Some(format!("aginx:{sid}")),
                None,
                None,
                Some("ask".to_string()),
                None,
            )
            .await
            .map_err(|e| anyhow::anyhow!("turn failed to start: {e}"))?;

        use std::io::Write as _;
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        while let Some(ev) = rx.recv().await {
            if let carrier_runtime::llm_driver::StreamEvent::TextDelta { text } = ev {
                writeln!(out, "{}", stream_assistant_line(&text)).ok();
                out.flush().ok();
            }
        }
        let duration_ms = started.elapsed().as_millis() as u64;
        match handle.await {
            Ok(Ok(result)) => {
                // 静默轮也发 result 行——session_id 必须回吐（续接链不断）。
                writeln!(
                    out,
                    "{}",
                    stream_result_line(
                        &sid,
                        "success",
                        result.response.trim(),
                        result.iterations,
                        duration_ms,
                        false,
                    )
                )
                .ok();
                out.flush().ok();
                Ok(())
            }
            Ok(Err(e)) => {
                // 轮失败同样带 session_id：翻译器出干净 error 帧，会话链保留。
                writeln!(
                    out,
                    "{}",
                    stream_result_line(
                        &sid,
                        "error_turn",
                        &format!("agent turn failed: {e}"),
                        0,
                        duration_ms,
                        true,
                    )
                )
                .ok();
                out.flush().ok();
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("agent turn task panicked: {e}")),
        }
    })
}

/// 远程化身的一次性 ask 轮：claude-stream-json 方言同本地（assistant 行
/// 流式 + result 行收尾）。`--session` 喂的是网关收割 id；result 行回吐收割
/// 值——下轮 `--session` 喂它即续接。connect/握手失败在输出任何方言行之前，
/// 直接非 0 退出（stderr 报因）；轮中失败发 error result 行保会话链（同本地）。
async fn run_remote_ask(
    handle: &RemoteHandle,
    session: Option<&str>,
    message: &str,
) -> anyhow::Result<()> {
    let started = std::time::Instant::now();
    // Charset gate 同本地（防注入 resume 参数）。未提供则只铸本地记账 id，
    // 不发给网关（网关侧新会话由它自己开）。
    let provided = session
        .filter(|s| valid_session_id(s))
        .map(str::to_string);
    let local_sid = provided
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let Some(ep) = crate::remote::endpoint(handle) else {
        anyhow::bail!(
            "无法解析远程化身地址 {}（需 agent://<id>.relay.<domain> 形态；本机 ~/.aginx/config.toml 缺 [relay] 段时 relay secret 无源）",
            handle.url
        );
    };
    let mut conn = AgentConn::connect(&ep)
        .await
        .map_err(|e| anyhow::anyhow!("连接远程网关失败: {e}"))?;
    conn.initialize()
        .await
        .map_err(|e| anyhow::anyhow!("远程网关握手失败: {e}"))?;

    use std::io::Write as _;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let r = conn
        .prompt(&handle.agent, message, provided.as_deref(), |chunk| {
            writeln!(out, "{}", stream_assistant_line(chunk)).ok();
            out.flush().ok();
            true
        })
        .await;
    let duration_ms = started.elapsed().as_millis() as u64;

    match r {
        Ok(r) => {
            let sid_out = r.session_id.unwrap_or(local_sid);
            writeln!(
                out,
                "{}",
                stream_result_line(
                    &sid_out,
                    "success",
                    r.text.trim(),
                    r.num_turns.unwrap_or(0) as u32,
                    duration_ms,
                    false,
                )
            )
            .ok();
            out.flush().ok();
            Ok(())
        }
        Err(e) => {
            writeln!(
                out,
                "{}",
                stream_result_line(
                    &local_sid,
                    "error_turn",
                    &format!("remote turn failed: {e}"),
                    0,
                    duration_ms,
                    true,
                )
            )
            .ok();
            out.flush().ok();
            Ok(())
        }
    }
}

/// 网关侧注入门同款：`[A-Za-z0-9_-]`。
fn valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// claude-stream-json `assistant` 事件行：message.content[] 单 text 块。
/// 网关翻译器（aginx 仓 translate.rs）扫 content[] 的 text 块拼 chunk。
fn stream_assistant_line(delta: &str) -> String {
    serde_json::json!({
        "type": "assistant",
        "message": {"content": [{"type": "text", "text": delta}]},
    })
    .to_string()
}

/// claude-stream-json `result` 事件行：网关从本行收割 session_id/num_turns/
/// duration_ms；`is_error:true` 时 `result` 字段作为错误文本出 error 帧。
fn stream_result_line(
    sid: &str,
    subtype: &str,
    result_text: &str,
    num_turns: u32,
    duration_ms: u64,
    is_error: bool,
) -> String {
    serde_json::json!({
        "type": "result",
        "subtype": subtype,
        "session_id": sid,
        "num_turns": num_turns,
        "duration_ms": duration_ms,
        "is_error": is_error,
        "result": result_text,
    })
    .to_string()
}


fn emit(out: &Arc<Mutex<std::io::Stdout>>, value: serde_json::Value) {
    let mut w = out.lock().unwrap();
    let _ = writeln!(w, "{value}");
    let _ = w.flush();
}

fn respond(out: &Arc<Mutex<std::io::Stdout>>, id: &serde_json::Value, result: serde_json::Value) {
    emit(
        out,
        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}),
    );
}

fn respond_error(
    out: &Arc<Mutex<std::io::Stdout>>,
    id: &serde_json::Value,
    code: i64,
    message: &str,
) {
    emit(
        out,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }),
    );
}

fn update(out: &Arc<Mutex<std::io::Stdout>>, session_id: &str, update: serde_json::Value) {
    emit(
        out,
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {"sessionId": session_id, "update": update},
        }),
    );
}

/// ask 模式方言行 + sessionId 注入门单测（会话契约，统一 aginx 流程）。
#[cfg(test)]
mod stream_tests {
    use super::*;

    #[test]
    fn assistant_line_carries_single_text_block() {
        let v: serde_json::Value =
            serde_json::from_str(&stream_assistant_line("你好，世界")).unwrap();
        assert_eq!(v["type"].as_str(), Some("assistant"));
        assert_eq!(v["message"]["content"][0]["type"].as_str(), Some("text"));
        assert_eq!(
            v["message"]["content"][0]["text"].as_str(),
            Some("你好，世界")
        );
    }

    #[test]
    fn result_line_shape_matches_gateway_harvest() {
        let line = stream_result_line("d903c124-abcd_1", "success", "已记", 3, 8123, false);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"].as_str(), Some("result"));
        assert_eq!(v["session_id"].as_str(), Some("d903c124-abcd_1"));
        assert_eq!(v["num_turns"].as_u64(), Some(3));
        assert_eq!(v["duration_ms"].as_u64(), Some(8123));
        assert_eq!(v["is_error"].as_bool(), Some(false));
        assert_eq!(v["result"].as_str(), Some("已记"));
    }

    #[test]
    fn error_result_line_flags_and_keeps_session() {
        let line = stream_result_line("abc-1", "error_turn", "agent turn failed: boom", 0, 5, true);
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["is_error"].as_bool(), Some(true));
        assert_eq!(v["session_id"].as_str(), Some("abc-1"));
        assert!(v["result"].as_str().unwrap().contains("boom"));
    }

    #[test]
    fn session_id_charset_gate() {
        assert!(valid_session_id("b8f713a4-ea3b-4d6c-920b-87dc2a0403f0"));
        assert!(valid_session_id("Ab_0-"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("a;rm -rf /"));
        assert!(!valid_session_id("带中文"));
        assert!(!valid_session_id("with space"));
    }
}

/// ACP.md 金样本互锁测试（协议立法层）。
///
/// ACP.md 住在隔壁 aginx 仓（`../aginx/ACP.md`，本仓 CARGO_MANIFEST_DIR 起
/// `../../../`）——协议权威与三端实现由金样本互锁，文档与实现打架即测试红。
/// 独立 clone（如 GitHub CI）没有该文件时跳过，不阻断。
#[cfg(test)]
mod golden_tests {
    use super::*;

    fn doc() -> Option<String> {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../aginx/ACP.md");
        match std::fs::read_to_string(path) {
            Ok(d) => Some(d),
            Err(_) => {
                eprintln!("SKIP golden_tests: {path} 不存在（独立 clone 无 aginx 仓）");
                None
            }
        }
    }

    fn golden(doc: &str, name: &str) -> serde_json::Value {
        let marker = format!("<!-- golden: {name} -->");
        let start = doc
            .find(&marker)
            .unwrap_or_else(|| panic!("ACP.md 缺金样本标记: {name}"));
        let rest = &doc[start + marker.len()..];
        let fence = rest
            .find("```json")
            .unwrap_or_else(|| panic!("金样本 {name} 后缺 ```json 围栏"));
        let body = &rest[fence + "```json".len()..];
        let end = body
            .find("```")
            .unwrap_or_else(|| panic!("金样本 {name} 围栏未闭合"));
        serde_json::from_str(body[..end].trim())
            .unwrap_or_else(|e| panic!("金样本 {name} 不是合法 JSON: {e}"))
    }

    /// 票据 wire 形状 camelCase：入↔出 roundtrip 必须逐字节等——锁死
    /// SessionTicket 的 rename_all 与字段名（turnSummaries/contextWindowTokens）。
    #[test]
    fn ticket_roundtrip_identical() {
        let Some(doc) = doc() else { return };
        let sample = golden(&doc, "ticket_v1");
        let ticket: SessionTicket =
            serde_json::from_value(sample.clone()).expect("ticket_v1 应可解析为 SessionTicket");
        let back = serde_json::to_value(&ticket).expect("SessionTicket 应可序列化");
        assert_eq!(back, sample, "票据 wire 形状漂移：文档样本与 serde 不一致");
        assert_eq!(ticket.version, SessionTicket::CURRENT_VERSION);
    }

    /// session/prompt（内部层）的借用五件套参数名 + prompt 块形状。
    #[test]
    fn internal_session_prompt_params_shape() {
        let Some(doc) = doc() else { return };
        let v = golden(&doc, "internal_session_prompt_borrowed_request");
        let params = v.get("params").expect("params 必填");
        for key in ["sessionId", "prompt", "sessionTicket", "materials", "activeFlow", "borrower"] {
            assert!(params.get(key).is_some(), "session/prompt 缺参数 {key}");
        }
        let block = params.pointer("/prompt/0").expect("prompt[0] 必填");
        assert_eq!(block.get("type").and_then(|t| t.as_str()), Some("text"));
        let ticket_val = params.get("sessionTicket").unwrap().clone();
        serde_json::from_value::<SessionTicket>(ticket_val)
            .expect("session/prompt 内嵌票据应可解析");
    }

    /// 素材条目形状：parse_materials 期望 {name, contentBase64}（后者可 base64 解码）。
    #[test]
    fn material_entry_shape() {
        let Some(doc) = doc() else { return };
        let v = golden(&doc, "material_entry");
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["contentBase64", "name"]);
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(v.get("contentBase64").unwrap().as_str().unwrap())
            .expect("金样本 contentBase64 应可解码");
    }

    /// 产物条目形状：BorrowedOutputFile 序列化键 = {name, contentBase64}。
    #[test]
    fn output_file_serialization_keys() {
        let Some(doc) = doc() else { return };
        let sample = golden(&doc, "output_file_entry");
        let f = carrier_kernel::messaging::BorrowedOutputFile {
            name: sample.get("name").unwrap().as_str().unwrap().to_string(),
            content_base64: sample.get("contentBase64").unwrap().as_str().unwrap().to_string(),
        };
        let back = serde_json::to_value(&f).unwrap();
        let mut keys: Vec<&str> = back.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["contentBase64", "name"]);
    }

    /// 桥最终响应（内部层）：stopReason 用 ACP 词汇 end_turn + 票据/产物在场。
    #[test]
    fn internal_final_result_shape() {
        let Some(doc) = doc() else { return };
        let v = golden(&doc, "internal_final_result_borrowed");
        assert_eq!(
            v.pointer("/result/stopReason").and_then(|s| s.as_str()),
            Some("end_turn")
        );
        serde_json::from_value::<SessionTicket>(
            v.pointer("/result/sessionTicket").unwrap().clone(),
        )
        .expect("最终响应内嵌票据应可解析");
        let file = v.pointer("/result/files/0").expect("files[0] 必填");
        assert!(file.get("name").is_some() && file.get("contentBase64").is_some());
    }

    /// session/update 通知形状：桥只发 agent_message_chunk。
    #[test]
    fn session_update_notification_shape() {
        let Some(doc) = doc() else { return };
        let v = golden(&doc, "internal_session_update_notification");
        assert_eq!(
            v.get("method").and_then(|m| m.as_str()),
            Some("session/update")
        );
        assert_eq!(
            v.pointer("/params/update/sessionUpdate").and_then(|s| s.as_str()),
            Some("agent_message_chunk")
        );
        assert!(v.pointer("/params/update/content/text")
            .and_then(|t| t.as_str())
            .is_some());
    }
}
