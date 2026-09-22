//! `aginx-carrier tool <name>` — 机读面（M33 D3 批3，runtime 桥的目标）。
//!
//! 内核耦合工具（schedule/cron/agent_*/location/system_time）的单真源：
//! 实现自 runtime tools/scheduling.rs / agent_mgmt.rs / misc.rs 原样搬来。
//! 输入 = stdin JSON（工具入参 + 保留键 `_ctx` 身份），输出 = stdout 一条
//! D1 信封（{"ok":true,"data":…} / {"ok":false,"error":…}），rc 0/1。
//! 与 aginx-web/agf 的 `tool` 面同构（M31/M32 先例）。
//!
//! 并发模型与既有 CLI 面一致：一次性裸 kernel boot、不起后台循环
//!（agent_cmd/cron_cmd 同款）。cron create 落 DB，常驻 daemon ≤15s
//! reconcile 采进——DB 即 daemon 总线。agent_send 是内联轮（acp 每
//! prompt 一进程的同款形态）；跨进程递归护栏走 AGINX_AGENT_DEPTH
//! env（桥传 depth+1，本面以该深度 scope 目标轮）。

use std::io::Read;
use std::sync::Arc;

use carrier_kernel::kernel::CarrierKernel;
use carrier_runtime::kernel_handle::KernelHandle;
use carrier_runtime::memory_handle::MemoryHandle as _;
use carrier_types::error::{CarrierError, CarrierResult};

/// 机读面承载的全部工具名（与 runtime 桥 BRIDGE_TOOL_NAMES 一一对应）。
pub const TOOL_NAMES: &[&str] = &[
    "schedule_create",
    "schedule_list",
    "schedule_delete",
    "cron_create",
    "cron_list",
    "cron_cancel",
    "agent_send",
    "agent_spawn",
    "agent_list",
    "agent_kill",
    "agent_restart",
    "location_get",
    "system_time",
];

const SCHEDULES_KEY: &str = "__carrier_schedules";
/// 桥传来的身份 + 递归深度（stdin JSON 保留键 `_ctx`）。
struct ToolCtx {
    caller_agent_id: Option<String>,
    sender_id: Option<String>,
    owner_id: Option<String>,
    depth: u32,
}

pub fn run(name: String) -> anyhow::Result<()> {
    if !TOOL_NAMES.contains(&name.as_str()) {
        print_envelope_err("tool_unknown", &format!("未知工具名 {name}（机读面只认 TOOL_NAMES）"));
        std::process::exit(1);
    }
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    let input: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => {
            print_envelope_err("tool_bad_input", &format!("stdin 不是合法 JSON: {e}"));
            std::process::exit(1);
        }
    };
    let ctx = parse_ctx(&input);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = rt.block_on(dispatch(&name, input, ctx));

    match result {
        Ok(out) => {
            println!("{}", aginx_carrier::envelope::ok(serde_json::Value::String(out)));
        }
        Err(e) => {
            print_envelope_err("tool_fail", &format!("{e}"));
            std::process::exit(1);
        }
    }
    Ok(())
}

fn print_envelope_err(code: &str, msg: &str) {
    println!("{}", aginx_carrier::envelope::fail("tool", code, msg, None));
}

fn parse_ctx(input: &serde_json::Value) -> ToolCtx {
    let c = &input["_ctx"];
    ToolCtx {
        caller_agent_id: c["caller_agent_id"].as_str().map(|s| s.to_string()),
        sender_id: c["sender_id"].as_str().map(|s| s.to_string()),
        owner_id: c["owner_id"].as_str().map(|s| s.to_string()),
        depth: c["depth"].as_u64().unwrap_or(0) as u32,
    }
}

async fn dispatch(name: &str, input: serde_json::Value, ctx: ToolCtx) -> CarrierResult<String> {
    match name {
        // 杂项：不 boot kernel。
        "location_get" => crate::sys_cmd::location_get().await,
        "system_time" => Ok(crate::sys_cmd::system_time()),

        // 内核耦合：裸 boot 一次性进程（agent_cmd 先例：CLI 面兜底写骨架）。
        // 包成 Arc<dyn KernelHandle> 再调——具体 kernel 上同名固有方法
        // （kill_agent(AgentId) 等）会遮蔽 trait 版，dyn 面只留 trait 契约。
        _ => {
            aginx_carrier::wiring::seed_brain_skeleton_if_missing();
            let kernel = Arc::new(CarrierKernel::boot(None)?);
            let kh: Arc<dyn KernelHandle> = kernel.clone();
            let mem = carrier_kernel::handle::MemorySubstrateHandle::new(kernel.memory.clone());
            run_kernel_tool(name, &input, &kh, &mem, &ctx).await
        }
    }
}

async fn run_kernel_tool(
    name: &str,
    input: &serde_json::Value,
    kh: &Arc<dyn KernelHandle>,
    mem: &carrier_kernel::handle::MemorySubstrateHandle,
    ctx: &ToolCtx,
) -> CarrierResult<String> {
    match name {
        // --- schedule_*：化身 kv 笔记本（无消费者触发，纯记录面） ---
        "schedule_create" => schedule_create(input, mem, ctx),
        "schedule_list" => schedule_list(mem, ctx),
        "schedule_delete" => schedule_delete(input, mem, ctx),

        // --- cron_*：kernel 调度器（DB 即 daemon 总线） ---
        "cron_create" => {
            let aid = ctx.require_caller("cron_create")?;
            kh
                .cron_create(
                    &aid,
                    ctx.owner_id.as_deref(),
                    ctx.sender_id.as_deref(),
                    input.clone(),
                )
                .await
        }
        "cron_list" => {
            let aid = ctx.require_caller("cron_list")?;
            let jobs = kh.cron_list(&aid, ctx.owner_id.as_deref()).await?;
            serde_json::to_string_pretty(&jobs).map_err(|e| {
                CarrierError::Serialization(format!("Failed to serialize cron jobs: {e}"))
            })
        }
        "cron_cancel" => {
            let aid = ctx.require_caller("cron_cancel")?;
            let job_id = str_param(input, "job_id", "cron_cancel")?;
            // Ownership check: verify this job belongs to the caller
            let jobs = kh.cron_list(&aid, ctx.owner_id.as_deref()).await?;
            let owned = jobs
                .iter()
                .any(|j| j.get("id").and_then(|v| v.as_str()) == Some(job_id.as_str()));
            if !owned {
                return Err(CarrierError::InvalidInput(
                    "Cron job not found or does not belong to you".to_string(),
                ));
            }
            kh.cron_cancel(&job_id).await?;
            Ok(format!("Cron job '{job_id}' cancelled."))
        }

        // --- agent_*：内联轮 / 注册表操作 ---
        "agent_send" => {
            let agent_id = str_param(input, "agent_id", "agent_send")?;
            let message = str_param(input, "message", "agent_send")?;
            let depth = ctx.depth;
            if depth >= carrier_runtime::tool_runner::max_agent_call_depth() {
                return Err(CarrierError::Internal(format!(
                    "Agent call depth exceeded (max {}). Use the task queue instead.",
                    carrier_runtime::tool_runner::max_agent_call_depth()
                )));
            }
            // 以桥传来的深度 scope 目标轮：其内的工具调用读到的就是它，
            // 再 send 时桥会读到 depth 并传 depth+1——护栏跨进程不丢。
            carrier_runtime::tool_runner::scope_agent_call_depth(depth, async {
                kh
                    .send_to_agent(
                        &agent_id,
                        &message,
                        ctx.sender_id.as_deref(),
                        None,
                        ctx.caller_agent_id.as_deref(),
                        ctx.owner_id.as_deref(),
                        None,
                    )
                    .await
            })
            .await
        }
        "agent_spawn" => {
            let manifest_toml = str_param(input, "manifest_toml", "agent_spawn")?;
            let (id, agent_name) = kh
                .spawn_agent(&manifest_toml, ctx.caller_agent_id.as_deref())
                .await?;
            Ok(format!(
                "Agent spawned successfully.\n  ID: {id}\n  Name: {agent_name}"
            ))
        }
        "agent_list" => {
            let agents = kh.list_agents();
            if agents.is_empty() {
                return Ok("No agents currently running.".to_string());
            }
            let mut output = format!("Running agents ({}):\n", agents.len());
            for a in &agents {
                output.push_str(&format!(
                    "  - {} (id: {}, state: {}, modality: {}, model: {})\n",
                    a.name, a.id, a.state, a.modality, a.model
                ));
            }
            Ok(output)
        }
        "agent_kill" => {
            let target_id = str_param(input, "agent_id", "agent_kill")?;
            kh.kill_agent(&target_id)?;
            Ok(format!("Agent {target_id} killed successfully."))
        }
        "agent_restart" => {
            let target_id = str_param(input, "agent_id", "agent_restart")?;
            kh.restart_agent(&target_id)?;
            Ok(format!("Agent {target_id} restarted successfully."))
        }

        _ => Err(CarrierError::Internal(format!(
            "tool face: unhandled tool {name}"
        ))),
    }
}

impl ToolCtx {
    /// cron_* 的原版报错文案（kernel 侧历史形态）。
    fn require_caller(&self, tool: &str) -> CarrierResult<String> {
        self.caller_agent_id
            .clone()
            .ok_or(CarrierError::Internal(format!(
                "Agent ID required for {tool}"
            )))
    }

    /// schedule_* 的原版报错文案（memory 侧历史形态）。
    fn require_caller_mem(&self, tool: &str) -> CarrierResult<String> {
        self.caller_agent_id
            .clone()
            .ok_or(CarrierError::Internal(format!(
                "No agent context for {tool}"
            )))
    }
}

fn str_param(input: &serde_json::Value, key: &str, _tool: &str) -> CarrierResult<String> {
    input[key]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| CarrierError::InvalidInput(format!("Missing '{key}' parameter")))
}

// ---------------------------------------------------------------------------
// schedule_*（自 runtime tools/scheduling.rs 原样搬来，kv 改走本地 boot 的
// MemorySubstrateHandle——同一个 carrier.db，daemon 侧可见）
// ---------------------------------------------------------------------------

fn schedule_create(
    input: &serde_json::Value,
    mem: &carrier_kernel::handle::MemorySubstrateHandle,
    ctx: &ToolCtx,
) -> CarrierResult<String> {
    let aid = ctx.require_caller_mem("schedule_create")?;
    let description = str_param(input, "description", "schedule_create")?;
    let schedule_str = str_param(input, "schedule", "schedule_create")?;
    let agent = input["agent"].as_str().unwrap_or("");

    let cron_expr = parse_schedule_to_cron(&schedule_str)?;
    let schedule_id = uuid::Uuid::new_v4().to_string();

    let entry = serde_json::json!({
        "id": schedule_id,
        "description": description,
        "schedule_input": schedule_str,
        "cron": cron_expr,
        "agent": agent,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "enabled": true,
    });

    let mut schedules: Vec<serde_json::Value> = match mem.kv_get(&aid, "", "", SCHEDULES_KEY)? {
        Some(serde_json::Value::Array(arr)) => arr,
        _ => Vec::new(),
    };

    schedules.push(entry);
    mem.kv_set(&aid, "", "", SCHEDULES_KEY, serde_json::Value::Array(schedules))?;

    Ok(format!(
        "Schedule created:\n  ID: {schedule_id}\n  Description: {description}\n  Cron: {cron_expr}\n  Original: {schedule_str}"
    ))
}

fn schedule_list(
    mem: &carrier_kernel::handle::MemorySubstrateHandle,
    ctx: &ToolCtx,
) -> CarrierResult<String> {
    let aid = ctx.require_caller_mem("schedule_list")?;

    let schedules: Vec<serde_json::Value> = match mem.kv_get(&aid, "", "", SCHEDULES_KEY)? {
        Some(serde_json::Value::Array(arr)) => arr,
        _ => Vec::new(),
    };

    if schedules.is_empty() {
        return Ok("No scheduled tasks.".to_string());
    }

    let mut output = format!("Scheduled tasks ({}):\n\n", schedules.len());
    for s in &schedules {
        let enabled = s["enabled"].as_bool().unwrap_or(true);
        let status = if enabled { "active" } else { "paused" };
        output.push_str(&format!(
            "  [{status}] {} — {}\n    Cron: {} | Agent: {}\n    Created: {}\n\n",
            s["id"].as_str().unwrap_or("?"),
            s["description"].as_str().unwrap_or("?"),
            s["cron"].as_str().unwrap_or("?"),
            s["agent"].as_str().unwrap_or("(self)"),
            s["created_at"].as_str().unwrap_or("?"),
        ));
    }

    Ok(output)
}

fn schedule_delete(
    input: &serde_json::Value,
    mem: &carrier_kernel::handle::MemorySubstrateHandle,
    ctx: &ToolCtx,
) -> CarrierResult<String> {
    let aid = ctx.require_caller_mem("schedule_delete")?;
    let id = str_param(input, "id", "schedule_delete")?;

    let mut schedules: Vec<serde_json::Value> = match mem.kv_get(&aid, "", "", SCHEDULES_KEY)? {
        Some(serde_json::Value::Array(arr)) => arr,
        _ => Vec::new(),
    };

    let before = schedules.len();
    schedules.retain(|s| s["id"].as_str() != Some(id.as_str()));

    if schedules.len() == before {
        return Err(CarrierError::InvalidInput(format!(
            "Schedule '{id}' not found."
        )));
    }

    mem.kv_set(&aid, "", "", SCHEDULES_KEY, serde_json::Value::Array(schedules))?;
    Ok(format!("Schedule '{id}' deleted."))
}

// ---------------------------------------------------------------------------
// 自然语言 → cron（自 runtime tools/scheduling.rs 逐字节搬来）
// ---------------------------------------------------------------------------

/// Parse a natural language schedule into a cron expression.
pub(crate) fn parse_schedule_to_cron(input: &str) -> CarrierResult<String> {
    let input = input.trim().to_lowercase();

    // If it already looks like a cron expression (5 space-separated fields), pass through
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() == 5
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_digit() || "*/,-".contains(c)))
    {
        return Ok(input);
    }

    // Natural language patterns
    if let Some(rest) = input.strip_prefix("every ") {
        if rest == "minute" || rest == "1 minute" {
            return Ok("* * * * *".to_string());
        }
        if let Some(mins) = rest.strip_suffix(" minutes") {
            let n: u32 = mins
                .trim()
                .parse()
                .map_err(|_| CarrierError::InvalidInput(format!("Invalid number in '{input}'")))?;
            if n == 0 || n > 59 {
                return Err(CarrierError::InvalidInput(format!(
                    "Minutes must be 1-59, got {n}"
                )));
            }
            return Ok(format!("*/{n} * * * *"));
        }
        if rest == "hour" || rest == "1 hour" {
            return Ok("0 * * * *".to_string());
        }
        if let Some(hrs) = rest.strip_suffix(" hours") {
            let n: u32 = hrs
                .trim()
                .parse()
                .map_err(|_| CarrierError::InvalidInput(format!("Invalid number in '{input}'")))?;
            if n == 0 || n > 23 {
                return Err(CarrierError::InvalidInput(format!(
                    "Hours must be 1-23, got {n}"
                )));
            }
            return Ok(format!("0 */{n} * * *"));
        }
        if rest == "day" || rest == "1 day" {
            return Ok("0 0 * * *".to_string());
        }
        if rest == "week" || rest == "1 week" {
            return Ok("0 0 * * 0".to_string());
        }
    }

    // "daily at Xam/pm"
    if let Some(time_str) = input.strip_prefix("daily at ") {
        let hour = parse_time_to_hour(time_str)?;
        return Ok(format!("0 {hour} * * *"));
    }

    // "weekdays at Xam/pm"
    if let Some(time_str) = input.strip_prefix("weekdays at ") {
        let hour = parse_time_to_hour(time_str)?;
        return Ok(format!("0 {hour} * * 1-5"));
    }

    // "weekends at Xam/pm"
    if let Some(time_str) = input.strip_prefix("weekends at ") {
        let hour = parse_time_to_hour(time_str)?;
        return Ok(format!("0 {hour} * * 0,6"));
    }

    // "hourly" / "daily" / "weekly" / "monthly"
    match input.as_str() {
        "hourly" => return Ok("0 * * * *".to_string()),
        "daily" => return Ok("0 0 * * *".to_string()),
        "weekly" => return Ok("0 0 * * 0".to_string()),
        "monthly" => return Ok("0 0 1 * *".to_string()),
        _ => {}
    }

    Err(CarrierError::InvalidInput(format!(
        "Could not parse schedule '{input}'. Try: 'every 5 minutes', 'daily at 9am', 'weekdays at 6pm', or a cron expression like '0 */5 * * *'"
    )))
}

/// Parse a time string like "9am", "6pm", "14:00", "9:30am" into an hour (0-23).
fn parse_time_to_hour(s: &str) -> CarrierResult<u32> {
    let s = s.trim().to_lowercase();

    // Handle "9am", "6pm", "12pm", "12am"
    if let Some(h) = s.strip_suffix("am") {
        let hour: u32 = h
            .trim()
            .parse()
            .map_err(|_| CarrierError::InvalidInput(format!("Invalid time: {s}")))?;
        return match hour {
            12 => Ok(0),
            1..=11 => Ok(hour),
            _ => Err(CarrierError::InvalidInput(format!("Invalid hour: {hour}"))),
        };
    }
    if let Some(h) = s.strip_suffix("pm") {
        let hour: u32 = h
            .trim()
            .parse()
            .map_err(|_| CarrierError::InvalidInput(format!("Invalid time: {s}")))?;
        return match hour {
            12 => Ok(12),
            1..=11 => Ok(hour + 12),
            _ => Err(CarrierError::InvalidInput(format!("Invalid hour: {hour}"))),
        };
    }

    // Handle "14:00" or "9:30"
    if let Some((h, _m)) = s.split_once(':') {
        let hour: u32 = h
            .trim()
            .parse()
            .map_err(|_| CarrierError::InvalidInput(format!("Invalid time: {s}")))?;
        if hour > 23 {
            return Err(CarrierError::InvalidInput(format!(
                "Hour must be 0-23, got {hour}"
            )));
        }
        return Ok(hour);
    }

    // Plain number
    let hour: u32 = s
        .parse()
        .map_err(|_| CarrierError::InvalidInput(format!("Invalid time: {s}")))?;
    if hour > 23 {
        return Err(CarrierError::InvalidInput(format!(
            "Hour must be 0-23, got {hour}"
        )));
    }
    Ok(hour)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_cover_all_thirteen() {
        assert_eq!(TOOL_NAMES.len(), 13);
        // 桥面（M33b）与此处一一对应——名字拼错会在设备上表现为静默 unknown。
        for n in [
            "schedule_create",
            "schedule_list",
            "schedule_delete",
            "cron_create",
            "cron_list",
            "cron_cancel",
            "agent_send",
            "agent_spawn",
            "agent_list",
            "agent_kill",
            "agent_restart",
            "location_get",
            "system_time",
        ] {
            assert!(TOOL_NAMES.contains(&n), "missing {n}");
        }
    }

    #[test]
    fn cron_passthrough_and_natural_language() {
        assert_eq!(parse_schedule_to_cron("0 */5 * * *").unwrap(), "0 */5 * * *");
        assert_eq!(parse_schedule_to_cron("every 5 minutes").unwrap(), "*/5 * * * *");
        assert_eq!(parse_schedule_to_cron("every minute").unwrap(), "* * * * *");
        assert_eq!(parse_schedule_to_cron("daily at 9am").unwrap(), "0 9 * * *");
        assert_eq!(parse_schedule_to_cron("daily at 12pm").unwrap(), "0 12 * * *");
        assert_eq!(parse_schedule_to_cron("weekdays at 6pm").unwrap(), "0 18 * * 1-5");
        assert_eq!(parse_schedule_to_cron("hourly").unwrap(), "0 * * * *");
        assert!(parse_schedule_to_cron("sometime soon").is_err());
        assert!(parse_schedule_to_cron("every 0 minutes").is_err());
    }
}
