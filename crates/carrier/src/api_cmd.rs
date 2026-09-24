//! `aginx-carrier api` — 声明式 API 工具面（M34：api_tools → API 命令层）。
//!
//! 执行链自 runtime `api_tools/{provider,cron}.rs` 原样搬来（单真源在此）：
//! resolve 链（参数先经另一 api 工具预解，如地名→坐标）、ctx 注入
//! （sender_id→openid，channel 门 + only_if_absent 让位）、JSON body
//! （缺席/空串省略）、URL 模板 + 查询串、HMAC-SHA256 签名（签一发一，
//! body 原串直发）、error_check、extract（dot-path / transform / derived
//! tiers）。runtime 桥（M34b）spawn `api call` 执行；守护 30s cron 循环
//! 改调 `api cron`——链在 CLI 侧解、零 LLM、单执行路径。
//!
//! 机读面（桥契约）：stdin JSON = 入参 + `_ctx{sender_id, channel_type}`，
//! stdout D1 信封（`--json`）。人面：`--param k=v` 直给，裸结果直出。
//! stdin 只在非 TTY 时读（aterm 交互不被挂起）；机读面靠管道天然非 TTY。
//!
//! 注册面（`api register`）也是 runtime `api_tool_register` 的落盘单真源
//! ——runtime 侧只留动态注册表更新（进程内态），TOML 写入一律走这里。
//! 文件即真源：守护只在启动时读 api_tools.toml，注册后需重启守护生效。

use chrono::Timelike;
use clap::Subcommand;
use carrier_types::api_tool::{ApiToolDef, ApiToolsConfig};
use carrier_types::error::{CarrierError, CarrierResult};
use std::collections::HashSet;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;

/// 桥传来的身份（stdin JSON 保留键 `_ctx`）——只取 provider.rs 用到的两个。
#[derive(Default, Clone)]
struct ApiCtx {
    sender_id: Option<String>,
    channel_type: Option<String>,
}

#[derive(Subcommand)]
pub enum ApiAction {
    /// 列出在册的具名 API 工具（名/描述/方法/端点）
    List {
        /// 额外 api_tools.toml 路径（可重复；后者同名覆盖前者）
        #[arg(long = "toml")]
        toml_paths: Vec<PathBuf>,
        /// 机器可读输出：D1 信封一条
        #[arg(long)]
        json: bool,
    },
    /// 调用具名 API 工具（机读面：stdin JSON 入参 + _ctx；人面：--param）
    Call {
        /// 工具名（api_tools.toml 的 [[tool]].name）
        name: String,
        /// 入参 k=v（值先按 JSON 解析，失败当字符串；可重复）
        #[arg(short = 'p', long = "param")]
        params: Vec<String>,
        /// api_tools.toml 路径（可重复；缺省 = carrier 全局）
        #[arg(long = "toml")]
        toml_paths: Vec<PathBuf>,
        /// 机器可读输出：D1 信封一条
        #[arg(long)]
        json: bool,
    },
    /// 通用直通：任意 METHOD URL，不读 api_tools.toml、不碰 .env
    Raw {
        /// HTTP 方法（GET/POST/PUT/PATCH/DELETE）
        method: String,
        /// 完整 URL
        url: String,
        /// 请求头 k=v（可重复）
        #[arg(long = "header")]
        headers: Vec<String>,
        /// 请求体（JSON 字符串）
        #[arg(long)]
        data: Option<String>,
        /// 机器可读输出：D1 信封一条
        #[arg(long)]
        json: bool,
    },
    /// 注册一个 [[tool]] 定义（stdin 或 --file 给 TOML；写盘单真源）
    Register {
        /// TOML 文件路径（缺省读 stdin）
        #[arg(long)]
        file: Option<PathBuf>,
        /// 写入全局（carrier home）；缺省需 --workspace
        #[arg(long)]
        global: bool,
        /// 写入指定化身工作区
        #[arg(long)]
        workspace: Option<PathBuf>,
        /// 机器可读输出：D1 信封一条
        #[arg(long)]
        json: bool,
    },
    /// cron 一跳：遍历带 [tool.cron] 的工具，到点执行并按 save_to 落库
    Cron {
        /// api_tools.toml 路径（可重复；缺省 = carrier 全局）
        #[arg(long = "toml")]
        toml_paths: Vec<PathBuf>,
        /// save_to 相对路径的基准目录（缺省 = carrier home）
        #[arg(long)]
        home: Option<PathBuf>,
        /// 机器可读输出：D1 信封一条
        #[arg(long)]
        json: bool,
    },
}

pub fn run(action: ApiAction) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        match action {
            ApiAction::List { toml_paths, json } => {
                let tools = load_tools(&toml_paths);
                if json {
                    let arr: Vec<serde_json::Value> = tools
                        .iter()
                        .map(|t| {
                            serde_json::json!({
                                "name": t.name,
                                "description": t.description,
                                "method": t.method,
                                "url": t.url,
                                "params": t.params.len(),
                                "cron": t.cron.is_some(),
                            })
                        })
                        .collect();
                    println!(
                        "{}",
                        aginx_carrier::envelope::ok_meta(
                            serde_json::Value::Array(arr.clone()),
                            serde_json::json!({"count": arr.len()}),
                        )
                    );
                } else if tools.is_empty() {
                    println!("（没有在册 API 工具——api register 注册，或 --toml 指路）");
                } else {
                    println!("{:<24} {:<6} ENDPOINT", "NAME", "METHOD");
                    for t in &tools {
                        println!("{:<24} {:<6} {}", t.name, t.method, t.url);
                    }
                }
            }
            ApiAction::Call {
                name,
                params,
                toml_paths,
                json,
            } => {
                let tools = load_tools(&toml_paths);
                let executor = ApiExecutor::new(tools);
                let Some(config) = executor.find_config(&name).cloned() else {
                    emit_fail(
                        json,
                        "not_found",
                        "api_tool_unknown",
                        &format!("没有在册的 API 工具 '{name}'"),
                        Some("try: api list"),
                    );
                };
                let (args, ctx) = match build_args(params) {
                    Ok(v) => v,
                    Err(e) => {
                        emit_fail(json, "usage", "api_bad_input", &format!("{e}"), None);
                    }
                };
                match executor.execute_api_call(&config, &args, &ctx).await {
                    Ok(out) => emit_ok(json, out),
                    Err(e) => emit_fail(json, "internal", "api_call_fail", &format!("{e}"), None),
                }
            }
            ApiAction::Raw {
                method,
                url,
                headers,
                data,
                json,
            } => match raw_call(&method, &url, &headers, data.as_deref()).await {
                Ok(out) => emit_ok(json, out),
                Err(e) => emit_fail(json, "internal", "api_raw_fail", &format!("{e}"), None),
            },
            ApiAction::Register {
                file,
                global,
                workspace,
                json,
            } => {
                let toml_str = match read_definition(file.as_deref()) {
                    Ok(s) => s,
                    Err(e) => {
                        emit_fail(json, "usage", "api_bad_input", &format!("{e}"), None);
                    }
                };
                let target = if global {
                    WriteTarget::Global
                } else if let Some(ws) = workspace {
                    WriteTarget::Workspace(ws)
                } else {
                    emit_fail(
                        json,
                        "usage",
                        "api_register_target",
                        "注册需要 --global 或 --workspace <dir>",
                        Some("try: api register --global < tool.toml"),
                    );
                };
                match register_tool(&toml_str, target) {
                    Ok(msg) => emit_ok(json, msg),
                    Err(e) => emit_fail(json, "internal", "api_register_fail", &format!("{e}"), None),
                }
            }
            ApiAction::Cron {
                toml_paths,
                home,
                json,
            } => {
                let home = home.unwrap_or_else(carrier_types::config::home_dir);
                let report = cron_tick(&toml_paths, &home).await;
                if json {
                    println!("{}", aginx_carrier::envelope::ok(report));
                } else {
                    let fired = report["fired"].as_array().cloned().unwrap_or_default();
                    if fired.is_empty() {
                        println!("（本轮无到点任务）");
                    }
                    for f in &fired {
                        let tool = f["tool"].as_str().unwrap_or("?");
                        if f["ok"].as_bool().unwrap_or(false) {
                            println!("fired  {}  ok", tool);
                        } else {
                            println!(
                                "fired  {}  FAIL  {}",
                                tool,
                                f["error"].as_str().unwrap_or("?")
                            );
                        }
                    }
                }
            }
        }
        anyhow::Ok(())
    })
}

// ---------------------------------------------------------------------------
// 入参组装：stdin JSON（机读面）+ --param k=v（人面，后者覆盖前者）
// ---------------------------------------------------------------------------

/// 读 stdin（仅非 TTY）拼 --param。返回 (入参对象, _ctx)。
fn build_args(params: Vec<String>) -> CarrierResult<(serde_json::Value, ApiCtx)> {
    let mut args = serde_json::Map::new();
    if !std::io::stdin().is_terminal() {
        let mut raw = String::new();
        std::io::stdin().read_to_string(&mut raw).ok();
        if !raw.trim().is_empty() {
            let v: serde_json::Value = serde_json::from_str(raw.trim())
                .map_err(|e| CarrierError::InvalidInput(format!("stdin 不是合法 JSON: {e}")))?;
            let obj = v.as_object().cloned().ok_or_else(|| {
                CarrierError::InvalidInput("stdin JSON 必须是对象（入参 + _ctx）".into())
            })?;
            args = obj;
        }
    }
    let ctx = ApiCtx {
        sender_id: args.get("_ctx").and_then(|c| c["sender_id"].as_str().map(String::from)),
        channel_type: args
            .get("_ctx")
            .and_then(|c| c["channel_type"].as_str().map(String::from)),
    };
    args.remove("_ctx");

    for p in params {
        let (k, v) = p.split_once('=').ok_or_else(|| {
            CarrierError::InvalidInput(format!("--param 需要 k=v 形态，得到 '{p}'"))
        })?;
        args.insert(k.to_string(), parse_param_value(v));
    }
    Ok((serde_json::Value::Object(args), ctx))
}

/// 人面 `--param` 值：先按 JSON 解析（数字/布尔/对象），失败当字符串。
/// 纯数字但 0 开头（如电话区号）按字符串保形，不进 JSON 数字语义。
fn parse_param_value(v: &str) -> serde_json::Value {
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(v) {
        let looks_zero_padded = parsed.is_number()
            && v.len() > 1
            && v.starts_with('0');
        if !looks_zero_padded {
            return parsed;
        }
    }
    serde_json::Value::String(v.to_string())
}

/// 注册定义的来源：--file 或 stdin（仅非 TTY）。
fn read_definition(file: Option<&std::path::Path>) -> CarrierResult<String> {
    if let Some(f) = file {
        return std::fs::read_to_string(f)
            .map_err(|e| CarrierError::Config(format!("读 {}: {e}", f.display())));
    }
    if std::io::stdin().is_terminal() {
        return Err(CarrierError::InvalidInput(
            "缺 TOML 来源：--file <path> 或管道给 stdin".into(),
        ));
    }
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).ok();
    if raw.trim().is_empty() {
        return Err(CarrierError::InvalidInput("stdin 为空".into()));
    }
    Ok(raw)
}

// ---------------------------------------------------------------------------
// 输出：人面裸文本，机读面 D1 信封（与 sys/tool 面同款）
// ---------------------------------------------------------------------------

fn emit_ok(json: bool, out: String) {
    if json {
        println!("{}", aginx_carrier::envelope::ok(serde_json::Value::String(out)));
    } else {
        println!("{out}");
    }
}

fn emit_fail(json: bool, etype: &str, code: &str, msg: &str, hint: Option<&str>) -> ! {
    if json {
        println!("{}", aginx_carrier::envelope::fail(etype, code, msg, hint));
    } else {
        eprintln!("Error: {msg}");
        if let Some(h) = hint {
            eprintln!("  {h}");
        }
    }
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// 执行引擎（自 runtime api_tools/provider.rs 逐段搬来；ToolContext 换 ApiCtx）
// ---------------------------------------------------------------------------

/// 装载 api_tools.toml（可重复路径，后者同名覆盖前者——loader.rs 语义）。
fn load_tools(toml_paths: &[PathBuf]) -> Vec<ApiToolDef> {
    let mut paths: Vec<PathBuf> = toml_paths.to_vec();
    if paths.is_empty() {
        paths.push(carrier_types::config::home_dir().join("api_tools.toml"));
    }
    let mut tools: Vec<ApiToolDef> = Vec::new();
    for p in &paths {
        let content = match std::fs::read_to_string(p) {
            Ok(c) => c,
            Err(_) => continue, // 不存在的路径静默跳过（loader.rs 同款）
        };
        match toml::from_str::<ApiToolsConfig>(&content) {
            Ok(config) => {
                for t in config.tool {
                    tools.retain(|e| e.name != t.name);
                    tools.push(t);
                }
            }
            Err(e) => {
                eprintln!("warn: 解析 {} 失败: {e}", p.display());
            }
        }
    }
    tools
}

struct ApiExecutor {
    tools: Vec<ApiToolDef>,
    http: reqwest::Client,
}

impl ApiExecutor {
    fn new(tools: Vec<ApiToolDef>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self { tools, http }
    }

    fn find_config(&self, name: &str) -> Option<&ApiToolDef> {
        self.tools.iter().find(|t| t.name == name)
    }

    fn resolve_auth(config: &ApiToolDef) -> Option<String> {
        if let Some(ref env_name) = config.auth_env {
            // carrier_types::env::get_secret（env/.env 先，agsecretd sidecar
            // 补位 — M36）——只 std::env 会漏 .env 和 sidecar 里的键。
            carrier_types::env::get_secret(env_name).filter(|s| !s.is_empty())
        } else {
            None
        }
    }

    /// Param names that go into the JSON body (not the query string).
    fn body_field_set(config: &ApiToolDef) -> HashSet<String> {
        config
            .body
            .as_ref()
            .map(|b| b.fields.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Derive the URL path component (no scheme/host/query) for HMAC signing.
    fn url_path(url: &str) -> String {
        let after_scheme = match url.find("://") {
            Some(i) => &url[i + 3..],
            None => url,
        };
        let path_start = after_scheme
            .find('/')
            .map(|i| &after_scheme[i..])
            .unwrap_or("");
        match path_start.find('?') {
            Some(i) => path_start[..i].to_string(),
            None => path_start.to_string(),
        }
    }

    /// HMAC-SHA256(secret, msg) -> hex.
    fn hmac_sha256_hex(secret: &str, msg: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac =
            HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
        mac.update(msg.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    /// Render the HMAC sign-string template. `{body}` is replaced LAST so body
    /// content can't be re-interpreted as another placeholder.
    fn render_sign_template(
        template: &str,
        method: &str,
        path: &str,
        timestamp: &str,
        key_id: &str,
        body: &str,
    ) -> String {
        template
            .replace("{method}", method)
            .replace("{path}", path)
            .replace("{timestamp}", timestamp)
            .replace("{key_id}", key_id)
            .replace("{body}", body)
    }

    /// Build the JSON request body from configured body.fields. Absent, null,
    /// or empty-string params are omitted. Returns None when no body is configured.
    fn build_body_str(config: &ApiToolDef, args: &serde_json::Value) -> Option<String> {
        let body_def = config.body.as_ref()?;
        let mut obj = serde_json::Map::new();
        for field in &body_def.fields {
            if let Some(val) = args.get(field) {
                if val.is_null() {
                    continue;
                }
                if let serde_json::Value::String(s) = val {
                    if s.is_empty() {
                        continue;
                    }
                }
                obj.insert(field.clone(), val.clone());
            }
        }
        Some(serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_else(|_| "null".to_string()))
    }

    fn build_url(config: &ApiToolDef, args: &serde_json::Value) -> String {
        let mut url = config.url.clone();
        let body_fields = Self::body_field_set(config);

        for name in config.params.keys() {
            if let Some(val) = args.get(name).and_then(|v| v.as_str()) {
                let placeholder = format!("{{{}}}", name);
                url = url.replace(&placeholder, &urlencoding::encode(val));
            }
        }

        let mut query_parts: Vec<String> = Vec::new();
        for (name, param_def) in &config.params {
            if config.url.contains(&format!("{{{}}}", name)) {
                continue;
            }
            if body_fields.contains(name) {
                continue;
            }
            if let Some(val) = args.get(name) {
                let val_str = match val {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                query_parts.push(format!(
                    "{}={}",
                    urlencoding::encode(name),
                    urlencoding::encode(&val_str)
                ));
            } else if let Some(ref default) = param_def.default {
                let val_str = match default {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    _ => continue,
                };
                query_parts.push(format!(
                    "{}={}",
                    urlencoding::encode(name),
                    urlencoding::encode(&val_str)
                ));
            }
        }

        if let (Some(auth_key), Some(auth_param)) = (Self::resolve_auth(config), &config.auth_param)
        {
            query_parts.push(format!(
                "{}={}",
                urlencoding::encode(auth_param),
                urlencoding::encode(&auth_key)
            ));
        }

        if query_parts.is_empty() {
            url
        } else if url.contains('?') {
            format!("{}&{}", url, query_parts.join("&"))
        } else {
            format!("{}?{}", url, query_parts.join("&"))
        }
    }

    /// Navigate a dot-path into a JSON value: "route.paths[0].distance"
    fn navigate_path<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
        let mut current = root;
        for segment in path.split('.') {
            if segment.is_empty() {
                continue;
            }
            if let Some(bracket) = segment.find('[') {
                let field = &segment[..bracket];
                let idx_str = &segment[bracket + 1..segment.len() - 1];
                if !field.is_empty() {
                    current = current.get(field)?;
                }
                let idx: usize = idx_str.parse().ok()?;
                current = current.get(idx)?;
            } else {
                current = current.get(segment)?;
            }
        }
        Some(current)
    }

    fn apply_transform(value: f64, transform: &str) -> serde_json::Value {
        match transform {
            "divide_1000_round1" => {
                let r = (value / 1000.0 * 10.0).round() / 10.0;
                serde_json::Value::from(
                    serde_json::Number::from_f64(r).unwrap_or_else(|| serde_json::Number::from(0)),
                )
            }
            "divide_60_round" => serde_json::Value::from((value / 60.0).round() as i64),
            "to_int" => serde_json::Value::from(value as i64),
            "round1" => {
                let r = (value * 10.0).round() / 10.0;
                serde_json::Value::from(
                    serde_json::Number::from_f64(r).unwrap_or_else(|| serde_json::Number::from(0)),
                )
            }
            "round0" => serde_json::Value::from(value.round() as i64),
            _ => serde_json::Value::from(value as i64),
        }
    }

    /// Resolve parameters that have a [tool.resolve] config: 满足条件时先调
    /// 指定的另一 api 工具换值（如地名→坐标）。链内递归，全在本进程解。
    async fn resolve_params(
        &self,
        config: &ApiToolDef,
        args: &serde_json::Value,
        ctx: &ApiCtx,
    ) -> CarrierResult<serde_json::Value> {
        if config.resolve.is_empty() {
            return Ok(args.clone());
        }

        let mut resolved = args.clone();

        for (param_name, resolve_def) in &config.resolve {
            let current_val = match resolved.get(param_name).and_then(|v| v.as_str()) {
                Some(v) => v.to_string(),
                None => continue,
            };

            let condition = resolve_def.condition.as_deref().unwrap_or("");
            let should_resolve = match condition {
                "not_coordinates" => !is_coordinates(&current_val),
                "not_empty" => !current_val.is_empty(),
                "" => true,
                _ => true,
            };
            if !should_resolve {
                continue;
            }

            let target_config = match self.find_config(&resolve_def.tool) {
                Some(c) => c.clone(),
                None => {
                    eprintln!(
                        "warn: resolve 目标工具 {} 不在册，参数 {} 原样保留",
                        resolve_def.tool, param_name
                    );
                    continue;
                }
            };

            let mut resolve_args = serde_json::Map::new();
            resolve_args.insert(resolve_def.param.clone(), serde_json::Value::String(current_val));

            match Box::pin(self.execute_api_call(&target_config, &serde_json::Value::Object(resolve_args), ctx))
                .await
            {
                Ok(result_str) => {
                    let result: serde_json::Value =
                        serde_json::from_str(&result_str).unwrap_or(serde_json::Value::Null);
                    if let Some(extracted) = result.get(&resolve_def.extract) {
                        if let Some(s) = extracted.as_str() {
                            resolved[param_name.clone()] = serde_json::Value::String(s.to_string());
                        } else {
                            eprintln!("warn: resolve 提取值不是字符串（参数 {param_name}）");
                        }
                    } else {
                        eprintln!(
                            "warn: resolve 结果缺字段 {}（参数 {param_name}）",
                            resolve_def.extract
                        );
                    }
                }
                Err(e) => {
                    eprintln!("warn: resolve 失败（参数 {param_name}）：{e}，用原值");
                }
            }
        }

        Ok(resolved)
    }

    /// Execute a single API tool call — the whole chain (validate → resolve →
    /// inject → body → url → sign → send → check → extract).
    async fn execute_api_call(
        &self,
        config: &ApiToolDef,
        args: &serde_json::Value,
        ctx: &ApiCtx,
    ) -> CarrierResult<String> {
        for (name, param_def) in &config.params {
            if param_def.required && args.get(name).is_none() && param_def.default.is_none() {
                return Err(CarrierError::InvalidInput(format!(
                    "Missing required parameter: {}",
                    name
                )));
            }
        }

        let mut resolved_args = self.resolve_params(config, args, ctx).await?;

        // 注入上下文字段（sender_id → openid，channel 门）。只填化身没给的。
        if !config.inject.is_empty() {
            for (field, rule) in &config.inject {
                if resolved_args.get(field).is_some() {
                    continue;
                }
                if rule
                    .only_if_absent
                    .iter()
                    .any(|f| resolved_args.get(f).is_some())
                {
                    continue;
                }
                if let Some(ref ch) = rule.channel {
                    if ctx.channel_type.as_deref() != Some(ch.as_str()) {
                        continue;
                    }
                }
                if rule.from == "sender_id" {
                    if let Some(ref sid) = ctx.sender_id {
                        if !sid.is_empty() {
                            resolved_args[field.clone()] = serde_json::Value::String(sid.clone());
                        }
                    }
                }
            }
        }

        // body 只序列化一次：签的就是发的（never re-serialized）。
        let body_str: Option<String> = Self::build_body_str(config, &resolved_args);
        let url = Self::build_url(config, &resolved_args);
        let method = config.method.to_uppercase();

        let mut req = match method.as_str() {
            "POST" => self.http.post(&url),
            "PUT" => self.http.put(&url),
            "PATCH" => self.http.patch(&url),
            "DELETE" => self.http.delete(&url),
            _ => self.http.get(&url),
        };

        for (k, v) in &config.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        if let Some(ref hmac_def) = config.hmac {
            let key_id = carrier_types::env::get_secret(&hmac_def.key_id_env).ok_or_else(|| {
                CarrierError::Internal(format!(
                    "{}: hmac.key_id_env '{}' not configured",
                    config.name, hmac_def.key_id_env
                ))
            })?;
            let secret = carrier_types::env::get_secret(&hmac_def.secret_env).ok_or_else(|| {
                CarrierError::Internal(format!(
                    "{}: hmac.secret_env '{}' not configured",
                    config.name, hmac_def.secret_env
                ))
            })?;
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string();
            let path = Self::url_path(&config.url);
            let body_for_sign = body_str.as_deref().unwrap_or("");
            let sign_string = Self::render_sign_template(
                &hmac_def.sign_template,
                &method,
                &path,
                &timestamp,
                &key_id,
                body_for_sign,
            );
            let signature = Self::hmac_sha256_hex(&secret, &sign_string);
            for (k, v_template) in &hmac_def.headers {
                let v = v_template
                    .replace("{key_id}", &key_id)
                    .replace("{timestamp}", &timestamp)
                    .replace("{signature}", &signature);
                req = req.header(k.as_str(), v.as_str());
            }
        }

        if let Some(ref body_str) = body_str {
            req = req
                .header("Content-Type", "application/json")
                .body(body_str.clone());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CarrierError::Network(format!("{} request failed: {}", config.name, e)))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(CarrierError::Network(format!(
                "{} HTTP error: {}",
                config.name, status
            )));
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| {
            CarrierError::Serialization(format!("{} parse error: {}", config.name, e))
        })?;

        if let Some(ref check) = config.error_check {
            let actual = Self::navigate_path(&body, &check.field)
                .map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            if actual != check.expect {
                return Err(CarrierError::Network(format!(
                    "{} API error: {}='{}', expected='{}'",
                    config.name, check.field, actual, check.expect
                )));
            }
        }

        if config.extract.is_empty() {
            return Ok(
                serde_json::to_string_pretty(&body).unwrap_or_else(|_| body.to_string()),
            );
        }

        let mut extracted = serde_json::Map::new();
        for (name, def) in &config.extract {
            if def.derived.unwrap_or(false) {
                continue;
            }
            if let Some(ref path) = def.path {
                if let Some(raw) = Self::navigate_path(&body, path) {
                    let num = match raw {
                        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0),
                        serde_json::Value::String(s) => s.parse::<f64>().unwrap_or(0.0),
                        _ => {
                            extracted.insert(name.clone(), raw.clone());
                            continue;
                        }
                    };
                    if let Some(ref transform) = def.transform {
                        extracted.insert(name.clone(), Self::apply_transform(num, transform));
                    } else if let Some(ref t) = def.r#type {
                        match t.as_str() {
                            "int" => {
                                extracted.insert(name.clone(), serde_json::Value::from(num as i64));
                            }
                            "float" => {
                                let n = serde_json::Number::from_f64(num)
                                    .unwrap_or_else(|| serde_json::Number::from(0));
                                extracted.insert(name.clone(), serde_json::Value::from(n));
                            }
                            _ => {
                                extracted.insert(name.clone(), raw.clone());
                            }
                        }
                    } else {
                        extracted.insert(name.clone(), raw.clone());
                    }
                }
            }
        }

        // Derived fields（tier 分档）
        for (name, def) in &config.extract {
            if !def.derived.unwrap_or(false) {
                continue;
            }
            if let Some(ref tiers) = def.tiers {
                if let Some(ref from) = def.from {
                    if let Some(from_val) = extracted.get(from) {
                        let num = from_val.as_f64().unwrap_or(0.0);
                        for tier in tiers {
                            if let Some(le) = tier.le {
                                if num <= le {
                                    extracted.insert(
                                        name.clone(),
                                        serde_json::Value::String(tier.value.clone()),
                                    );
                                    break;
                                }
                            } else {
                                extracted.insert(
                                    name.clone(),
                                    serde_json::Value::String(tier.value.clone()),
                                );
                            }
                        }
                    }
                }
            }
        }

        let result = serde_json::Value::Object(extracted);
        Ok(serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()))
    }
}

/// Check if a string looks like coordinates (contains comma, no CJK chars).
fn is_coordinates(s: &str) -> bool {
    s.contains(',') && !s.chars().any(|c| c > '\u{4e00}' && c < '\u{9fff}')
}

/// 把错误正文截到首行 + N 字符（信封/日志里不塞整页 HTML）。
fn truncate_for_error(s: &str, max: usize) -> String {
    let first_line = s.lines().next().unwrap_or("");
    let cut = first_line.chars().take(max).collect::<String>();
    if first_line.chars().count() > max {
        format!("{cut}…")
    } else {
        cut
    }
}

// ---------------------------------------------------------------------------
// 通用直通（不读 toml、不碰 .env）
// ---------------------------------------------------------------------------

async fn raw_call(
    method: &str,
    url: &str,
    headers: &[String],
    data: Option<&str>,
) -> CarrierResult<String> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| CarrierError::Network(format!("HTTP client: {e}")))?;
    let m = method.to_uppercase();
    let mut req = match m.as_str() {
        "POST" => http.post(url),
        "PUT" => http.put(url),
        "PATCH" => http.patch(url),
        "DELETE" => http.delete(url),
        "GET" => http.get(url),
        other => {
            return Err(CarrierError::InvalidInput(format!(
                "不支持的 HTTP 方法：{other}"
            )))
        }
    };
    for h in headers {
        let (k, v) = h.split_once('=').ok_or_else(|| {
            CarrierError::InvalidInput(format!("--header 需要 k=v 形态，得到 '{h}'"))
        })?;
        req = req.header(k, v);
    }
    if let Some(d) = data {
        req = req
            .header("Content-Type", "application/json")
            .body(d.to_string());
    }
    let resp = req
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("request failed: {e}")))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| CarrierError::Serialization(format!("read body: {e}")))?;
    if !status.is_success() {
        return Err(CarrierError::Network(format!(
            "HTTP error: {status}: {}",
            truncate_for_error(&text, 300)
        )));
    }
    // JSON 响应美化直出，其他原样。
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::to_string_pretty(&v).unwrap_or(text))
    } else {
        Ok(text)
    }
}

// ---------------------------------------------------------------------------
// 注册落盘（自 runtime api_tools/register.rs 搬来——写盘单真源）
// ---------------------------------------------------------------------------

enum WriteTarget {
    Global,
    Workspace(PathBuf),
}

fn register_tool(toml_str: &str, target: WriteTarget) -> CarrierResult<String> {
    let config: ApiToolsConfig = toml::from_str(toml_str)
        .map_err(|e| CarrierError::Serialization(format!("Invalid TOML: {e}")))?;

    if config.tool.is_empty() {
        return Err(CarrierError::InvalidInput(
            "No [[tool]] block found in definition".into(),
        ));
    }

    let tool_def = &config.tool[0];
    if tool_def.name.is_empty() {
        return Err(CarrierError::InvalidInput("Tool name is required".into()));
    }
    if tool_def.url.is_empty() {
        return Err(CarrierError::InvalidInput("Tool url is required".into()));
    }
    if !tool_def.url.starts_with("https://") && !tool_def.url.starts_with("http://") {
        return Err(CarrierError::InvalidInput(
            "Tool url must start with http:// or https://".into(),
        ));
    }

    let tool_name = tool_def.name.clone();
    let (toml_path, scope) = match target {
        WriteTarget::Global => (
            carrier_types::config::home_dir().join("api_tools.toml"),
            "global",
        ),
        WriteTarget::Workspace(ws) => (ws.join("api_tools.toml"), "workspace"),
    };

    // 同名去重：改定义 = 先移除旧块再追加（幂等注册）。
    remove_tool_block(&toml_path, &tool_name)?;
    let serialized = serialize_tool(tool_def);
    let content = std::fs::read_to_string(&toml_path).unwrap_or_default();
    let new_content = if content.trim().is_empty() {
        serialized
    } else {
        format!("{}\n\n{}", content.trim_end(), serialized)
    };
    std::fs::write(&toml_path, new_content).map_err(CarrierError::Io)?;

    Ok(format!(
        "✅ API tool '{}' registered ({scope}: {}). 文件即真源——守护启动时读，注册后需重启守护生效。",
        tool_name,
        toml_path.display()
    ))
}

/// 从既有 api_tools.toml 移除同名 [[tool]] 块（块 = `[[tool]]` 到下一个
/// `[[tool]]` 或文件尾；块名取块内第一个 `name = ` 行——serialize_tool
/// 恒把 name 放块首）。重注册 = 换定义，不叠块。
fn remove_tool_block(path: &std::path::Path, name: &str) -> CarrierResult<()> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Ok(()), // 文件不存在 = 无旧块
    };
    let lines: Vec<&str> = content.lines().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "[[tool]]" {
            // 块界：下一个 [[tool]] 或文件尾
            let mut j = i + 1;
            let mut block_name: Option<String> = None;
            while j < lines.len() && lines[j].trim() != "[[tool]]" {
                if block_name.is_none() {
                    if let Some(rest) = lines[j].trim().strip_prefix("name = ") {
                        block_name = Some(rest.trim().trim_matches('"').to_string());
                    }
                }
                j += 1;
            }
            if block_name.as_deref() == Some(name) {
                i = j; // 旧块整块丢弃
                continue;
            }
        }
        out.push(lines[i]);
        i += 1;
    }
    let mut new_content = out.join("\n");
    if !new_content.is_empty() {
        new_content.push('\n');
    }
    std::fs::write(path, new_content).map_err(CarrierError::Io)?;
    Ok(())
}

/// Serialize an ApiToolDef back to a TOML [[tool]] block（自 register.rs 原样）。
fn serialize_tool(tool: &ApiToolDef) -> String {
    let mut out = String::new();
    out.push_str("[[tool]]\n");
    out.push_str(&format!("name = \"{}\"\n", tool.name));
    out.push_str(&format!(
        "description = \"{}\"\n",
        tool.description.replace('"', "\\\"")
    ));
    out.push_str(&format!("url = \"{}\"\n", tool.url));
    out.push_str(&format!("method = \"{}\"\n", tool.method));

    if let Some(ref auth_env) = tool.auth_env {
        out.push_str(&format!("auth_env = \"{}\"\n", auth_env));
    }
    if let Some(ref auth_param) = tool.auth_param {
        out.push_str(&format!("auth_param = \"{}\"\n", auth_param));
    }

    if !tool.params.is_empty() {
        out.push_str("\n[tool.params]\n");
        for (name, param) in &tool.params {
            let mut parts = Vec::new();
            if param.required {
                parts.push("required = true".to_string());
            }
            parts.push(format!("type = \"{}\"", param.r#type));
            if !param.description.is_empty() {
                parts.push(format!(
                    "description = \"{}\"",
                    param.description.replace('"', "\\\"")
                ));
            }
            if let Some(ref default) = param.default {
                match default {
                    serde_json::Value::String(s) => parts.push(format!("default = \"{}\"", s)),
                    serde_json::Value::Number(n) => parts.push(format!("default = {}", n)),
                    serde_json::Value::Bool(b) => parts.push(format!("default = {}", b)),
                    _ => {}
                }
            }
            out.push_str(&format!("{} = {{ {} }}\n", name, parts.join(", ")));
        }
    }

    if !tool.extract.is_empty() {
        out.push_str("\n[tool.extract]\n");
        for (name, def) in &tool.extract {
            let mut parts = Vec::new();
            if let Some(ref path) = def.path {
                parts.push(format!("path = \"{}\"", path));
            }
            if let Some(ref transform) = def.transform {
                parts.push(format!("transform = \"{}\"", transform));
            }
            if let Some(ref t) = def.r#type {
                parts.push(format!("type = \"{}\"", t));
            }
            if def.derived.unwrap_or(false) {
                parts.push("derived = true".to_string());
            }
            if let Some(ref from) = def.from {
                parts.push(format!("from = \"{}\"", from));
            }
            if let Some(ref tiers) = def.tiers {
                let tier_strs: Vec<String> = tiers
                    .iter()
                    .map(|t| match t.le {
                        Some(le) => format!("{{ le = {}, value = \"{}\" }}", le, t.value),
                        None => format!("{{ value = \"{}\" }}", t.value),
                    })
                    .collect();
                parts.push(format!("tiers = [\n  {},\n]", tier_strs.join(",\n  ")));
            }
            out.push_str(&format!("{} = {{ {} }}\n", name, parts.join(", ")));
        }
    }

    if let Some(ref check) = tool.error_check {
        out.push_str("\n[tool.error_check]\n");
        out.push_str(&format!("field = \"{}\"\n", check.field));
        out.push_str(&format!("expect = \"{}\"\n", check.expect));
    }

    if !tool.headers.is_empty() {
        out.push_str("\n[tool.headers]\n");
        for (k, v) in &tool.headers {
            out.push_str(&format!("{} = \"{}\"\n", k, escape_toml_string(v)));
        }
    }

    if let Some(ref body) = tool.body {
        out.push_str("\n[tool.body]\n");
        let fields: Vec<String> = body
            .fields
            .iter()
            .map(|f| format!("\"{}\"", escape_toml_string(f)))
            .collect();
        out.push_str(&format!("fields = [{}]\n", fields.join(", ")));
    }

    if let Some(ref hmac) = tool.hmac {
        out.push_str("\n[tool.hmac]\n");
        out.push_str(&format!("key_id_env = \"{}\"\n", hmac.key_id_env));
        out.push_str(&format!("secret_env = \"{}\"\n", hmac.secret_env));
        out.push_str(&format!(
            "sign_template = \"{}\"\n",
            escape_toml_string(&hmac.sign_template)
        ));
        out.push_str(&format!("algorithm = \"{}\"\n", hmac.algorithm));
        if !hmac.headers.is_empty() {
            out.push_str("\n[tool.hmac.headers]\n");
            for (k, v) in &hmac.headers {
                out.push_str(&format!(
                    "\"{}\" = \"{}\"\n",
                    escape_toml_string(k),
                    escape_toml_string(v)
                ));
            }
        }
    }

    if !tool.inject.is_empty() {
        for (field, rule) in &tool.inject {
            out.push_str(&format!("\n[tool.inject.{}]\n", field));
            out.push_str(&format!("from = \"{}\"\n", rule.from));
            if let Some(ref ch) = rule.channel {
                out.push_str(&format!("channel = \"{}\"\n", ch));
            }
            if !rule.only_if_absent.is_empty() {
                let fs: Vec<String> = rule
                    .only_if_absent
                    .iter()
                    .map(|f| format!("\"{}\"", f))
                    .collect();
                out.push_str(&format!("only_if_absent = [{}]\n", fs.join(", ")));
            }
        }
    }

    // [tool.cron]（原 register.rs 的 serialize_tool 漏了这节——注册 cron 工具
    // 会静默丢调度，注册后永不发射；注册面成为唯一写手后这是硬伤）。
    if let Some(ref cron) = tool.cron {
        out.push_str("\n[tool.cron]\n");
        out.push_str(&format!("schedule = \"{}\"\n", cron.schedule));
        if let Some(ref save_to) = cron.save_to {
            out.push_str(&format!("save_to = \"{}\"\n", save_to));
        }
        if let Some(ref table) = cron.table {
            out.push_str(&format!("table = \"{}\"\n", table));
        }
    }

    out
}

fn escape_toml_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

// ---------------------------------------------------------------------------
// cron 一跳（自 runtime api_tools/cron.rs 搬来；minute 粒度 × 30s 节拍 →
// 用「秒 < 30 才发」保证每分钟至多一发）
// ---------------------------------------------------------------------------

async fn cron_tick(toml_paths: &[PathBuf], home: &std::path::Path) -> serde_json::Value {
    let tools = load_tools(toml_paths);
    let now = chrono::Local::now();
    let mut fired: Vec<serde_json::Value> = Vec::new();
    let mut due_skipped = 0usize;

    for tool in &tools {
        let Some(ref cron) = tool.cron else { continue };
        if !is_due(&cron.schedule, &now) {
            continue;
        }
        if now.second() >= 30 {
            due_skipped += 1; // 本分钟已发过（或留给下一跳）——防双发
            continue;
        }
        let name = tool.name.clone();
        let t = tool.clone();
        let home = home.to_path_buf();
        let outcome = execute_cron_api_call(&t, &home).await;
        let entry = match outcome {
            Ok(()) => serde_json::json!({"tool": name, "ok": true}),
            Err(e) => serde_json::json!({"tool": name, "ok": false, "error": e.to_string()}),
        };
        fired.push(entry);
    }

    serde_json::json!({"fired": fired, "due_skipped": due_skipped})
}

async fn execute_cron_api_call(tool: &ApiToolDef, home_dir: &std::path::Path) -> CarrierResult<()> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| CarrierError::Network(format!("HTTP client: {e}")))?;

    let url = build_cron_url(tool);
    let mut req = http.get(&url);
    for (k, v) in &tool.headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("Request: {e}")))?;
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| CarrierError::Serialization(format!("Parse: {e}")))?;

    if let Some(ref check) = tool.error_check {
        let actual = navigate(&body, &check.field)
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_default();
        if actual != check.expect {
            return Err(CarrierError::Network(format!(
                "API error: {}='{}'",
                check.field, actual
            )));
        }
    }

    if let Some(save_to) = tool.cron.as_ref().and_then(|c| c.save_to.as_deref()) {
        let db_path = save_to.strip_prefix("sqlite:").unwrap_or(save_to);
        let table = tool
            .cron
            .as_ref()
            .and_then(|c| c.table.clone())
            .unwrap_or_else(|| format!("api_cron_{}", tool.name));
        store_to_sqlite(home_dir, db_path, &table, &tool.name, &body)?;
    }
    Ok(())
}

fn build_cron_url(tool: &ApiToolDef) -> String {
    let mut url = tool.url.clone();
    let mut query_parts: Vec<String> = Vec::new();
    for (name, param_def) in &tool.params {
        let placeholder = format!("{{{}}}", name);
        if url.contains(&placeholder) {
            if let Some(ref default) = param_def.default {
                let val_str = json_to_str(default);
                url = url.replace(&placeholder, &urlencoding::encode(&val_str));
            }
        } else if let Some(ref default) = param_def.default {
            query_parts.push(format!(
                "{}={}",
                urlencoding::encode(name),
                urlencoding::encode(&json_to_str(default))
            ));
        }
    }
    if let (Some(ref auth_env), Some(ref auth_param)) = (&tool.auth_env, &tool.auth_param) {
        // carrier_types::env::get_secret 先读 env/.env，sidecar 补位（M36）；
        // 只 std::env 会漏 .env 和 sidecar 的键。
        if let Some(key) = carrier_types::env::get_secret(auth_env) {
            if !key.is_empty() {
                query_parts.push(format!(
                    "{}={}",
                    urlencoding::encode(auth_param),
                    urlencoding::encode(&key)
                ));
            }
        }
    }
    if query_parts.is_empty() {
        url
    } else if url.contains('?') {
        format!("{}&{}", url, query_parts.join("&"))
    } else {
        format!("{}?{}", url, query_parts.join("&"))
    }
}

fn json_to_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// Check if a cron expression is due at the current time.
fn is_due(schedule: &str, now: &chrono::DateTime<chrono::Local>) -> bool {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return false;
    }
    let minute = now.format("%M").to_string().parse::<u32>().unwrap_or(0);
    let hour = now.format("%H").to_string().parse::<u32>().unwrap_or(0);
    let dom = now.format("%d").to_string().parse::<u32>().unwrap_or(0);
    let month = now.format("%m").to_string().parse::<u32>().unwrap_or(0);
    let dow = now.format("%w").to_string().parse::<u32>().unwrap_or(0);
    cron_match(parts[0], minute)
        && cron_match(parts[1], hour)
        && cron_match(parts[2], dom)
        && cron_match(parts[3], month)
        && cron_match(parts[4], dow)
}

fn cron_match(field: &str, value: u32) -> bool {
    if field == "*" {
        return true;
    }
    if let Some(n_str) = field.strip_prefix("*/") {
        if let Ok(n) = n_str.parse::<u32>() {
            if n > 0 {
                // （cron.rs 原文 is_multiple_of；% 同义且不抬 MSRV）
                return value % n == 0;
            }
        }
        return false;
    }
    for part in field.split(',') {
        if let Ok(v) = part.trim().parse::<u32>() {
            if v == value {
                return true;
            }
        }
    }
    false
}

fn navigate<'a>(root: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path.split('.') {
        if segment.is_empty() {
            continue;
        }
        if let Some(bracket) = segment.find('[') {
            let field = &segment[..bracket];
            let idx_str = &segment[bracket + 1..segment.len() - 1];
            if !field.is_empty() {
                current = current.get(field)?;
            }
            let idx: usize = idx_str.parse().ok()?;
            current = current.get(idx)?;
        } else {
            current = current.get(segment)?;
        }
    }
    Some(current)
}

fn store_to_sqlite(
    home_dir: &std::path::Path,
    db_path: &str,
    table: &str,
    tool_name: &str,
    body: &serde_json::Value,
) -> CarrierResult<()> {
    let full_path = if db_path.starts_with('/') {
        std::path::PathBuf::from(db_path)
    } else {
        home_dir.join(db_path)
    };
    let conn = rusqlite::Connection::open(&full_path)
        .map_err(|e| CarrierError::Internal(format!("SQLite open: {e}")))?;
    conn.execute(
        &format!(
            "CREATE TABLE IF NOT EXISTS {} (id INTEGER PRIMARY KEY AUTOINCREMENT, tool_name TEXT NOT NULL, raw_response TEXT, fetched_at TEXT DEFAULT (datetime('now','localtime')))",
            table
        ),
        [],
    )
    .map_err(|e| CarrierError::Internal(format!("Create table: {e}")))?;
    let raw = serde_json::to_string(body).unwrap_or_default();
    conn.execute(
        &format!(
            "INSERT INTO {} (tool_name, raw_response) VALUES (?1, ?2)",
            table
        ),
        rusqlite::params![tool_name, raw],
    )
    .map_err(|e| CarrierError::Internal(format!("Insert: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 标准 HMAC-SHA256 测试向量（自 provider.rs 原样）。
    #[test]
    fn hmac_sha256_known_vector() {
        let sig = ApiExecutor::hmac_sha256_hex(
            "key",
            "The quick brown fox jumps over the lazy dog",
        );
        assert_eq!(
            sig,
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }

    #[test]
    fn url_path_strips_scheme_host_query() {
        assert_eq!(
            ApiExecutor::url_path("https://chuxing.86bus.com/api/ai/orders"),
            "/api/ai/orders"
        );
        assert_eq!(ApiExecutor::url_path("https://host.com/a/b?x=1&y=2"), "/a/b");
        assert_eq!(ApiExecutor::url_path("https://host.com/"), "/");
        assert_eq!(ApiExecutor::url_path("https://host.com"), "");
    }

    /// body 里字面 `{key_id}` 必须原样存活（{body} 最后替换，不被再解释）。
    #[test]
    fn render_sign_template_body_literal_not_reinterpreted() {
        let rendered = ApiExecutor::render_sign_template(
            "{method}\n{body}",
            "POST",
            "",
            "",
            "REAL_AK",
            "x{key_id}y",
        );
        assert_eq!(rendered, "POST\nx{key_id}y");
    }

    /// 与 charter_sign（weixin-oa/tools.rs）同签名字节——86bus 后端继续认。
    #[test]
    fn charter_signature_matches_charter_sign_pattern() {
        let secret = "test-secret";
        let (method, path, timestamp) = ("POST", "/api/ai/orders", "1700000000");
        let body = r#"{"username":"张三","phone":"138","person_num":5,"start_point":"A","end_point":"B","go_time":"2026-08-11 08:00"}"#;
        let sign_str = format!("{method}\n{path}\n{timestamp}\n{body}");
        let rendered = ApiExecutor::render_sign_template(
            "{method}\n{path}\n{timestamp}\n{body}",
            method,
            path,
            timestamp,
            "",
            body,
        );
        let actual_hex = ApiExecutor::hmac_sha256_hex(secret, &rendered);
        assert_eq!(sign_str, rendered);
        assert_eq!(
            actual_hex,
            hmac_reference(secret, &sign_str),
            "api 面签名必须与旧 charter 路径逐字节一致"
        );
    }

    fn hmac_reference(secret: &str, msg: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        type HmacSha256 = Hmac<Sha256>;
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(msg.as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    fn parse_tool(toml_str: &str) -> ApiToolDef {
        toml::from_str::<ApiToolsConfig>(toml_str)
            .unwrap()
            .tool
            .into_iter()
            .next()
            .unwrap()
    }

    fn charter_test_config() -> ApiToolDef {
        parse_tool(
            r#"
[[tool]]
name = "charter_create_order"
description = "test"
url = "https://chuxing.86bus.com/api/ai/orders"
method = "POST"
[tool.body]
fields = ["username", "phone", "person_num", "start_point", "end_point", "go_time", "back_time", "remark"]
[tool.params]
username = { type = "string", description = "x" }
phone = { type = "string", description = "x" }
person_num = { type = "integer", description = "x" }
start_point = { type = "string", description = "x" }
end_point = { type = "string", description = "x" }
go_time = { type = "string", description = "x" }
back_time = { type = "string", description = "x" }
remark = { type = "string", description = "x" }
"#,
        )
    }

    #[test]
    fn build_body_str_omits_absent_and_empty() {
        let cfg = charter_test_config();
        let args = serde_json::json!({
            "username": "张三",
            "phone": "13800000000",
            "person_num": 5,
            "start_point": "南京南站",
            "end_point": "禄口机场",
            "go_time": "2026-08-11 08:00",
            "remark": ""
        });
        let body = ApiExecutor::build_body_str(&cfg, &args).unwrap();
        assert!(!body.contains("back_time"));
        assert!(!body.contains("remark"));
        assert!(body.contains("username"));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["username"], "张三");
        assert_eq!(v["person_num"], 5);
        assert!(v.get("back_time").is_none());
    }

    #[test]
    fn build_body_str_none_when_no_body_config() {
        let cfg = parse_tool(
            r#"
[[tool]]
name = "t"
description = "x"
url = "https://example.com/api"
method = "GET"
"#,
        );
        let args = serde_json::json!({"q": "hi"});
        assert!(ApiExecutor::build_body_str(&cfg, &args).is_none());
    }

    /// toml 装载 + 同名覆盖（loader.rs 语义在 CLI 侧的保持）。
    #[test]
    fn load_tools_later_path_overrides_by_name() {
        let dir = std::env::temp_dir().join(format!("aginx-api-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.toml");
        let b = dir.join("b.toml");
        std::fs::write(
            &a,
            "[[tool]]\nname = \"t\"\ndescription = \"from-a\"\nurl = \"https://a.example\"\nmethod = \"GET\"\n",
        )
        .unwrap();
        std::fs::write(
            &b,
            "[[tool]]\nname = \"t\"\ndescription = \"from-b\"\nurl = \"https://b.example\"\nmethod = \"POST\"\n[[tool]]\nname = \"u\"\ndescription = \"only-b\"\nurl = \"https://b.example/u\"\nmethod = \"GET\"\n",
        )
        .unwrap();

        let tools = load_tools(&[a.clone(), b.clone()]);
        assert_eq!(tools.len(), 2);
        let t = tools.iter().find(|t| t.name == "t").unwrap();
        assert_eq!(t.description, "from-b");
        assert_eq!(t.method, "POST");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// is_coordinates 门（resolve 链的 not_coordinates 条件）。
    #[test]
    fn is_coordinates_gate() {
        assert!(is_coordinates("118.76,32.06"));
        assert!(!is_coordinates("南京南站"));
        assert!(!is_coordinates("118.76"));
    }

    /// cron 匹配（原语义）+ minute×30s 防双发的秒门由 cron_tick 组合。
    #[test]
    fn cron_match_semantics() {
        assert!(cron_match("*", 33));
        assert!(cron_match("*/5", 30));
        assert!(!cron_match("*/5", 33));
        assert!(cron_match("0,30", 30));
        assert!(!cron_match("0,30", 15));
    }

    /// 注册幂等：同名重注册不产生重复块。
    #[test]
    fn register_is_idempotent_by_name() {
        let dir = std::env::temp_dir().join(format!("aginx-api-reg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let toml_path = dir.join("api_tools.toml");

        let def = "[[tool]]\nname = \"t\"\ndescription = \"v1\"\nurl = \"https://a.example\"\nmethod = \"GET\"\n";
        register_tool(def, WriteTarget::Workspace(dir.clone())).unwrap();
        let def2 = "[[tool]]\nname = \"t\"\ndescription = \"v2\"\nurl = \"https://a.example\"\nmethod = \"GET\"\n";
        register_tool(def2, WriteTarget::Workspace(dir.clone())).unwrap();
        // 另一个名字不冲突共存
        let def3 = "[[tool]]\nname = \"z\"\ndescription = \"v1\"\nurl = \"https://z.example\"\nmethod = \"GET\"\n";
        register_tool(def3, WriteTarget::Workspace(dir.clone())).unwrap();

        let content = std::fs::read_to_string(&toml_path).unwrap();
        assert_eq!(content.matches("[[tool]]").count(), 2);
        assert!(!content.contains("v1\"\nurl = \"https://a"));
        assert!(content.contains("v2"));

        let parsed = toml::from_str::<ApiToolsConfig>(&content).unwrap();
        assert_eq!(parsed.tool.len(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// serialize_tool 往返：序列化再解析回等价定义（关键字段）。
    #[test]
    fn serialize_tool_roundtrip() {
        let cfg = charter_test_config();
        let ser = serialize_tool(&cfg);
        let back = parse_tool(&ser);
        assert_eq!(back.name, cfg.name);
        assert_eq!(back.url, cfg.url);
        assert_eq!(back.method, cfg.method);
        assert_eq!(back.body.as_ref().unwrap().fields, cfg.body.as_ref().unwrap().fields);
        assert_eq!(back.params.len(), cfg.params.len());
    }

    /// [tool.cron] 必须过序列化往返——注册 cron 工具丢调度 = 永不发射
    /// （原 register.rs 就栽在这）。
    #[test]
    fn serialize_tool_keeps_cron_section() {
        let cfg = parse_tool(
            r#"
[[tool]]
name = "ip_city_cron"
description = "snapshots"
url = "http://ip-api.com/json/"
method = "GET"
[tool.cron]
schedule = "* * * * *"
save_to = "sqlite:data/m34-cron.db"
table = "m34_ip_city"
"#,
        );
        let ser = serialize_tool(&cfg);
        assert!(ser.contains("[tool.cron]"), "cron section must be serialized");
        let back = parse_tool(&ser);
        let cron = back.cron.expect("cron survives roundtrip");
        assert_eq!(cron.schedule, "* * * * *");
        assert_eq!(cron.save_to.as_deref(), Some("sqlite:data/m34-cron.db"));
        assert_eq!(cron.table.as_deref(), Some("m34_ip_city"));
    }
}
