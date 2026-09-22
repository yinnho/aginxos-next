//! HTTP 入站面——`POST /hook/{name}` 事件入口 + `/healthz`。
//!
//! daemon `start` 形态专用（移动端不起监听）。两种应答模式：
//! - 默认（异步）：验 token → 去重 → 构造 PluginMessage 注入 bridge → 202。
//!   轮后台跑，回复文本由 WebhookChannel 落日志。
//! - `?wait=N`（同步）：直调 `KernelHandle::send_to_agent`（uniffi `app:`
//!   前缀同款直调先例），阻塞 ≤ min(N, max_wait_secs) 拿回复；超时 504。
//!   两种模式 session 标签一致（`user:webhook:{name}`），跨模式同会话。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use carrier_runtime::kernel_handle::KernelHandle;
use carrier_types::config::{WebhookConfig, WebhookHook};
use carrier_types::plugin::{PluginContent, PluginMessage};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::dedup::DedupLru;

const DEDUP_TTL: Duration = Duration::from_secs(120);
const DEDUP_MAX: usize = 10_000;

pub struct WebhookState {
    hooks: HashMap<String, WebhookHook>,
    kernel: Arc<dyn KernelHandle>,
    bridge_tx: mpsc::Sender<PluginMessage>,
    max_wait_secs: u64,
    max_body_bytes: usize,
    dedup: DedupLru,
}

/// 起 HTTP 监听。bind 失败只报错不 panic——通道挂掉不该拖死 daemon 其余部分。
pub async fn serve(kernel: Arc<dyn KernelHandle>, bridge_tx: mpsc::Sender<PluginMessage>, cfg: WebhookConfig) {
    let listen = cfg.listen.clone();
    let state = Arc::new(build_state(kernel, bridge_tx, &cfg));
    let app = build_app(state);
    match tokio::net::TcpListener::bind(&listen).await {
        Ok(listener) => {
            info!(%listen, "webhook 入站通道在线");
            if let Err(e) = axum::serve(listener, app).await {
                error!(error = %e, "webhook server 异常退出");
            }
        }
        Err(e) => {
            error!(%listen, error = %e, "webhook bind 失败（其余通道不受影响）");
        }
    }
}

fn build_state(kernel: Arc<dyn KernelHandle>, bridge_tx: mpsc::Sender<PluginMessage>, cfg: &WebhookConfig) -> WebhookState {
    let mut hooks = HashMap::new();
    for h in &cfg.hooks {
        if !valid_hook(h) {
            warn!(hook = %h.name, "webhook hook 配置不合法（name 限 [a-z0-9-]、token ≥16 字符、agent 非空），跳过");
            continue;
        }
        hooks.insert(h.name.clone(), h.clone());
    }
    if hooks.is_empty() {
        warn!("webhook enabled 但无合法 hook（监听仍起，/healthz 可用）");
    }
    WebhookState {
        hooks,
        kernel,
        bridge_tx,
        max_wait_secs: cfg.max_wait_secs.clamp(1, 600),
        max_body_bytes: cfg.max_body_bytes,
        dedup: DedupLru::new(DEDUP_TTL, DEDUP_MAX),
    }
}

fn build_app(state: Arc<WebhookState>) -> Router {
    // body 上限给个下地板，防配置写 0 把合法事件全拒。
    let body_limit = state.max_body_bytes.max(1024);
    Router::new()
        .route("/hook/{name}", post(handle_hook))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

fn valid_hook(h: &WebhookHook) -> bool {
    !h.name.is_empty()
        && h.name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && h.token.len() >= 16
        && !h.agent.is_empty()
}

#[derive(Deserialize)]
struct HookQuery {
    wait: Option<u64>,
    token: Option<String>,
}

async fn handle_hook(
    State(st): State<Arc<WebhookState>>,
    Path(name): Path<String>,
    Query(q): Query<HookQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(hook) = st.hooks.get(&name).cloned() else {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "unknown hook"}))).into_response();
    };
    match extract_token(&headers, &q) {
        Some(t) if constant_time_eq(&t, &hook.token) => {}
        _ => {
            return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error": "bad token"}))).into_response();
        }
    }

    // 去重：delivery id 有则挡重放（GitHub 重试、网关重发）。先标记后入队：
    // 失败路径（bridge 关闭）抑制重试是可接受的——daemon 都在停了。
    let delivery_id = first_header(&headers, &["x-webhook-id", "x-github-delivery", "x-request-id"]);
    if let Some(id) = &delivery_id {
        if !st.dedup.check_and_mark(&format!("{name}:{id}")) {
            info!(hook = %name, id = %id, "webhook 重复投递已丢弃");
            return (StatusCode::OK, Json(serde_json::json!({"status": "duplicate"}))).into_response();
        }
    }

    let event = first_header(&headers, &["x-webhook-event", "x-github-event"]);
    let body_text = String::from_utf8_lossy(&body);
    let text = build_text(&name, event.as_deref(), &body_text);
    let sender_id = format!("webhook:{name}");

    if let Some(wait) = q.wait.filter(|w| *w > 0) {
        // 同步模式：直调 kernel（不走桥——HTTP 响应本身就是回复去处）。
        let cap = wait.min(st.max_wait_secs).clamp(1, 600);
        let fut = st.kernel.send_to_agent(
            &hook.agent,
            &text,
            Some(&sender_id),
            Some(&name),
            None,
            Some(&name),
            Some("webhook"),
        );
        match tokio::time::timeout(Duration::from_secs(cap), fut).await {
            Ok(Ok(reply)) => (StatusCode::OK, Json(serde_json::json!({"reply": reply}))).into_response(),
            Ok(Err(e)) => {
                (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error": e.to_string()}))).into_response()
            }
            Err(_) => (
                StatusCode::GATEWAY_TIMEOUT,
                Json(serde_json::json!({"error": format!("agent turn exceeded {cap}s")})),
            )
                .into_response(),
        }
    } else {
        // 异步模式：注入 bridge（DirectBind + 种路由在 wiring 完成）。
        let mut metadata = HashMap::new();
        if let Some(ev) = &event {
            metadata.insert("event".to_string(), serde_json::json!(ev));
        }
        if let Some(id) = &delivery_id {
            metadata.insert("delivery_id".to_string(), serde_json::json!(id));
        }
        let msg = PluginMessage {
            channel_type: "webhook".to_string(),
            platform_message_id: delivery_id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            sender_id,
            sender_name: name.clone(),
            bot_id: name.clone(),
            content: PluginContent::Text(text),
            timestamp_ms: now_ms(),
            is_group: false,
            thread_id: None,
            metadata,
        };
        match st.bridge_tx.send(msg).await {
            Ok(()) => (StatusCode::ACCEPTED, Json(serde_json::json!({"status": "accepted"}))).into_response(),
            Err(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({"error": "bridge queue closed"}))).into_response()
            }
        }
    }
}

/// 进 agent 的文本：一行头部（hook 名 + 可选事件类型）+ 原文 body。
/// 头部让 agent 无需看 HTTP 头就知道事件从哪来。
fn build_text(hook: &str, event: Option<&str>, body: &str) -> String {
    match event {
        Some(e) => format!("[hook:{hook} event={e}]\n{body}"),
        None => format!("[hook:{hook}]\n{body}"),
    }
}

fn first_header(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| headers.get(*n).and_then(|v| v.to_str().ok()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn extract_token(headers: &HeaderMap, q: &HookQuery) -> Option<String> {
    if let Some(v) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        if let Some(t) = v.strip_prefix("Bearer ") {
            return Some(t.trim().to_string());
        }
    }
    if let Some(v) = headers.get("x-webhook-token").and_then(|v| v.to_str().ok()) {
        return Some(v.trim().to_string());
    }
    q.token.clone()
}

/// 恒时比较（长度先泄——惯例可接受，与 subtle 默认行为一致）。
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use carrier_types::error::CarrierError;

    /// 记录 send_to_agent 调用参数的最小 stub（其余方法全按"不可用"报错）。
    struct StubKernel {
        calls: std::sync::Mutex<Vec<(String, String, String, String)>>, // (agent, text, sender_id, owner)
        reply: String,
        delay: Option<Duration>,
    }

    impl StubKernel {
        fn new(reply: &str) -> Self {
            Self { calls: std::sync::Mutex::new(Vec::new()), reply: reply.to_string(), delay: None }
        }
        fn calls(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl KernelHandle for StubKernel {
        async fn spawn_agent(&self, _m: &str, _p: Option<&str>) -> carrier_types::error::CarrierResult<(String, String)> {
            Err(CarrierError::Internal("stub".into()))
        }
        async fn send_to_agent(
            &self,
            agent_id: &str,
            message: &str,
            sender_id: Option<&str>,
            _sn: Option<&str>,
            _c: Option<&str>,
            owner_id: Option<&str>,
            channel_type: Option<&str>,
        ) -> carrier_types::error::CarrierResult<String> {
            assert_eq!(channel_type, Some("webhook"));
            if let Some(d) = self.delay {
                tokio::time::sleep(d).await;
            }
            self.calls.lock().unwrap().push((
                agent_id.to_string(),
                message.to_string(),
                sender_id.unwrap_or_default().to_string(),
                owner_id.unwrap_or_default().to_string(),
            ));
            Ok(self.reply.clone())
        }
        fn list_agents(&self) -> Vec<carrier_runtime::kernel_handle::AgentInfo> {
            Vec::new()
        }
        fn kill_agent(&self, _a: &str) -> carrier_types::error::CarrierResult<()> {
            Ok(())
        }
        fn restart_agent(&self, _a: &str) -> carrier_types::error::CarrierResult<()> {
            Ok(())
        }
        fn find_agents(&self, _q: &str) -> Vec<carrier_runtime::kernel_handle::AgentInfo> {
            Vec::new()
        }
        async fn task_post(&self, _t: &str, _d: &str, _a: Option<&str>, _c: Option<&str>) -> carrier_types::error::CarrierResult<String> {
            Err(CarrierError::Internal("stub".into()))
        }
        async fn task_claim(&self, _a: &str) -> carrier_types::error::CarrierResult<Option<serde_json::Value>> {
            Ok(None)
        }
        async fn task_complete(&self, _t: &str, _r: &str) -> carrier_types::error::CarrierResult<()> {
            Ok(())
        }
        async fn task_list(&self, _s: Option<&str>) -> carrier_types::error::CarrierResult<Vec<serde_json::Value>> {
            Ok(Vec::new())
        }
        async fn publish_event(&self, _t: &str, _p: serde_json::Value) -> carrier_types::error::CarrierResult<()> {
            Ok(())
        }
    }

    fn test_cfg() -> WebhookConfig {
        WebhookConfig {
            enabled: true,
            listen: "127.0.0.1:0".to_string(),
            max_wait_secs: 5,
            max_body_bytes: 1024 * 64,
            hooks: vec![WebhookHook {
                name: "ci".to_string(),
                agent: "travel-planner".to_string(),
                token: "tok-0123456789abcdef".to_string(),
            }],
        }
    }

    async fn spawn_server(cfg: &WebhookConfig) -> (String, Arc<StubKernel>, mpsc::Receiver<PluginMessage>) {
        let kernel: Arc<StubKernel> = Arc::new(StubKernel::new("暗号回执 ok"));
        let (tx, rx) = mpsc::channel(32);
        let state = Arc::new(build_state(kernel.clone(), tx, cfg));
        let app = build_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}"), kernel, rx)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn async_mode_injects_plugin_message() {
        let (base, _kernel, mut rx) = spawn_server(&test_cfg()).await;
        let resp = reqwest::Client::new()
            .post(format!("{base}/hook/ci"))
            .header("Authorization", "Bearer tok-0123456789abcdef")
            .header("X-GitHub-Event", "push")
            .body("build green")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 202);
        let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
        assert_eq!(msg.channel_type, "webhook");
        assert_eq!(msg.sender_id, "webhook:ci");
        assert_eq!(msg.bot_id, "ci");
        assert!(matches!(
            &msg.content,
            PluginContent::Text(t) if t == "[hook:ci event=push]\nbuild green"
        ));
        assert_eq!(msg.metadata.get("event").unwrap(), "push");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_mode_returns_reply() {
        let (base, kernel, _rx) = spawn_server(&test_cfg()).await;
        let resp = reqwest::Client::new()
            .post(format!("{base}/hook/ci?wait=5"))
            .header("X-Webhook-Token", "tok-0123456789abcdef")
            .body("问暗号")
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v: serde_json::Value = resp.json().await.unwrap();
        assert_eq!(v["reply"], "暗号回执 ok");
        // 直调参数：agent/sender/owner 全对。
        let calls = kernel.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "travel-planner");
        assert_eq!(calls[0].2, "webhook:ci");
        assert_eq!(calls[0].3, "ci");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bad_token_and_unknown_hook() {
        let (base, kernel, _rx) = spawn_server(&test_cfg()).await;
        let client = reqwest::Client::new();
        let r1 = client.post(format!("{base}/hook/ci")).body("x").send().await.unwrap();
        assert_eq!(r1.status(), 401);
        let r2 = client
            .post(format!("{base}/hook/ci"))
            .header("Authorization", "Bearer wrong-token-1234567")
            .body("x")
            .send()
            .await
            .unwrap();
        assert_eq!(r2.status(), 401);
        let r3 = client
            .post(format!("{base}/hook/nope"))
            .header("X-Webhook-Token", "tok-0123456789abcdef")
            .body("x")
            .send()
            .await
            .unwrap();
        assert_eq!(r3.status(), 404);
        assert_eq!(kernel.calls(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn duplicate_delivery_id_dropped() {
        let (base, _kernel, mut rx) = spawn_server(&test_cfg()).await;
        let client = reqwest::Client::new();
        let r1 = client
            .post(format!("{base}/hook/ci"))
            .header("X-Webhook-Token", "tok-0123456789abcdef")
            .header("X-GitHub-Delivery", "d-123")
            .body("evt")
            .send()
            .await
            .unwrap();
        assert_eq!(r1.status(), 202);
        let r2 = client
            .post(format!("{base}/hook/ci"))
            .header("X-Webhook-Token", "tok-0123456789abcdef")
            .header("X-GitHub-Delivery", "d-123")
            .body("evt")
            .send()
            .await
            .unwrap();
        assert_eq!(r2.status(), 200); // 重放幂等成功（duplicate）
        assert_eq!(
            r2.json::<serde_json::Value>().await.unwrap()["status"],
            "duplicate"
        );
        let first = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap();
        assert!(first.is_some());
        let second = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await;
        assert!(second.is_err(), "重放不该产生第二条消息");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn sync_timeout_504() {
        let cfg = test_cfg();
        let kernel = Arc::new(StubKernel {
            calls: std::sync::Mutex::new(Vec::new()),
            reply: String::new(),
            delay: Some(Duration::from_secs(3)),
        });
        let (tx, _rx) = mpsc::channel(32);
        let state = Arc::new(build_state(kernel, tx, &cfg));
        let app = build_app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let r = reqwest::Client::new()
            .post(format!("http://{addr}/hook/ci?wait=1"))
            .header("X-Webhook-Token", "tok-0123456789abcdef")
            .body("slow")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 504);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_body_413() {
        let (base, _kernel, _rx) = spawn_server(&test_cfg()).await;
        let big = "x".repeat(1024 * 128); // 超过 64KiB 上限
        let r = reqwest::Client::new()
            .post(format!("{base}/hook/ci"))
            .header("X-Webhook-Token", "tok-0123456789abcdef")
            .body(big)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 413);
    }

    #[test]
    fn helpers() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert_eq!(build_text("ci", Some("push"), "b"), "[hook:ci event=push]\nb");
        assert_eq!(build_text("ci", None, "b"), "[hook:ci]\nb");
    }
}
