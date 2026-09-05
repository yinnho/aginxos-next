//! 全链（N5⑤）：假 relay（裸 TCP）× 假 aginx-server（真 std UDS 监听）
//! × 真传输核 run_once。
//!
//! 钉的是 agc 期约的可观察行为：
//! - register 带 token，registered 后连接保持；
//! - initialize → 定向应答（clientId 注入）；
//! - prompt ok → **恰两条裸行**：chunk 在前、endTurn 终帧在后；
//! - unknown_avatar → -32601 且消息提及化身名；
//! - UDS 不通 → -32603；
//! - 闸门超时（server 收了不回）→ -32603；
//! - disconnected → 只记日志，连接不断；
//! - 未实现方法 → -32601。

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use aginx_gateway::agent::AgentBridge;
use aginx_gateway::relay::{self, Bridge, RelayCfg, Wire};

// ---- 假 aginx-server：一线一答，脚本化应答 ------------------------------

enum ServerScript {
    /// 回一个 agio ok 信封（text 来自 server）。
    Ok(&'static str),
    /// 回 unknown_avatar 失败信封。
    UnknownAvatar,
    /// 收下请求但不回（闸门超时路径）。
    Silent,
}

/// 起一个真 std UDS 监听线程。返回 (sock 路径, 收到的请求行回传通道, 句柄)。
/// label 让并行测试的 sock 路径互不冲撞（同进程同 PID）。
fn fake_server(
    label: &str,
    script: ServerScript,
) -> (PathBuf, std::sync::mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
    let dir = std::env::temp_dir().join(format!("aginx-gw-chain-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("aginx.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { break };
            let mut line = String::new();
            if BufReader::new(&mut conn).read_line(&mut line).is_err() {
                break;
            }
            let _ = tx.send(line);
            match script {
                ServerScript::Ok(text) => {
                    let _ = conn.write_all(
                        format!("{{\"ok\":true,\"data\":{{\"avatar\":\"小满\",\"text\":\"{text}\"}}}}\n").as_bytes(),
                    );
                }
                ServerScript::UnknownAvatar => {
                    // 字节串里不能放非 ASCII ——中文化身名走 str 拼接
                    let _ = conn.write_all(
                        b"{\"ok\":false,\"error\":{\"type\":\"not_found\",\"code\":\"unknown_avatar\",\"message\":\"unknown avatar '",
                    );
                    let _ = conn.write_all("路人".as_bytes());
                    let _ = conn.write_all(b"'\"}}\n");
                }
                ServerScript::Silent => {
                    let _ = conn.write_all(b""); // 收下不回
                }
            }
        }
    });
    (sock, rx, handle)
}

// ---- 假 relay：tokio TcpListener，按行应答 ------------------------------

struct FakeRelay {
    addr: std::net::SocketAddr,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
}

impl FakeRelay {
    async fn start() -> FakeRelay {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_writer = Arc::clone(&seen);
        tokio::spawn(async move {
            // 单客户端网关的测试面：一连线一任务
            if let Ok((sock, _)) = listener.accept().await {
                let seen = Arc::clone(&seen_writer);
                tokio::spawn(async move { relay_session(sock, seen).await });
            }
        });
        FakeRelay { addr, seen }
    }

    fn saw(&self, frag: &str) -> bool {
        self.seen.lock().unwrap().iter().any(|l| l.contains(frag))
    }
}

/// 每条假 relay 连线：读 register → 回 registered → 固定脚本 → 关写侧
/// 排空到 EOF。读侧不按固定条数——`read_turn` 读到该轮收口（终帧
/// stopReason 或带 id 的错误帧）为止：ok 轮两行、错误轮一行，同一
/// 脚本两种轮都走得通。
async fn relay_session(sock: tokio::net::TcpStream, seen: Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (mut r, mut w) = sock.into_split();
    let mut reader = BufReader::new(r);
    let mut line = String::new();

    // register → registered
    read_line_into(&mut reader, &mut line, &seen).await;
    let reg: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(reg["type"], "register", "第一帧必须是 register");
    assert_eq!(reg["token"], "test-secret", "单门：token 必带");
    w.write_all(b"{\"type\":\"registered\",\"id\":\"gw-test\",\"url\":\"agent://gw-test.relay.aginx.net\"}\n")
        .await
        .unwrap();

    // initialize → 一条定向应答
    w.write_all(b"{\"type\":\"data\",\"client_id\":\"c_1\",\"data\":{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"0.1.0\"}}}\n").await.unwrap();
    read_line_into(&mut reader, &mut line, &seen).await;

    // prompt ok 轮
    feed_prompt(&mut w, 2, "小满", "用一句话介绍AginxOS").await;
    read_turn(&mut reader, &mut line, &seen, 2).await;

    // prompt unknown avatar 轮
    feed_prompt(&mut w, 3, "路人", "hi").await;
    read_turn(&mut reader, &mut line, &seen, 3).await;

    // disconnected：网关只记日志不该断——再喂一轮证明连接还活。
    w.write_all(b"{\"type\":\"disconnected\",\"client_id\":\"c_9\"}\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.write_all(b"{\"type\":\"data\",\"client_id\":\"c_1\",\"data\":{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"listAgents\",\"params\":{}}}\n").await.unwrap();
    read_turn(&mut reader, &mut line, &seen, 4).await;

    // 入站 ping → 网关应 pong（读写双向仍通的活证）；关写侧排空到 EOF。
    w.write_all(b"{\"type\":\"ping\"}\n").await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    w.shutdown().await.unwrap();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => seen.lock().unwrap().push(line.clone()),
        }
    }
}

async fn read_line_into<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut String,
    seen: &Arc<std::sync::Mutex<Vec<String>>>,
) {
    use tokio::io::AsyncBufReadExt;
    line.clear();
    let n = tokio::time::timeout(Duration::from_secs(10), reader.read_line(line))
        .await
        .expect("gateway reply within 10s")
        .unwrap();
    assert!(n > 0, "gateway closed mid-script");
    seen.lock().unwrap().push(line.clone());
}

/// 读到该轮收口：终帧（result.stopReason）或带该 id 的错误帧。
async fn read_turn<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut String,
    seen: &Arc<std::sync::Mutex<Vec<String>>>,
    id: u64,
) {
    loop {
        read_line_into(reader, line, seen).await;
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let final_ = v.pointer("/result/stopReason").is_some();
        let error_mine = v.get("error").is_some() && v["id"].as_u64() == Some(id);
        if final_ || error_mine {
            return;
        }
    }
}

/// 喂 prompt（中文一律 str 分段写——字节串不容非 ASCII）。
async fn feed_prompt(w: &mut tokio::net::tcp::OwnedWriteHalf, id: u64, agent: &str, message: &str) {
    use tokio::io::AsyncWriteExt;
    w.write_all(b"{\"type\":\"data\",\"client_id\":\"c_1\",\"data\":{\"jsonrpc\":\"2.0\",\"id\":").await.unwrap();
    w.write_all(id.to_string().as_bytes()).await.unwrap();
    w.write_all(b",\"method\":\"prompt\",\"params\":{\"agent\":\"").await.unwrap();
    w.write_all(agent.as_bytes()).await.unwrap();
    w.write_all(b"\",\"message\":\"").await.unwrap();
    w.write_all(message.as_bytes()).await.unwrap();
    w.write_all(b"\"}}}\n").await.unwrap();
}

/// 返回 Result（不内部 unwrap）：测试侧 `gw.await.unwrap().unwrap()`
/// 同时捏 JoinError 与运行错。
async fn run_gateway(relay_addr: std::net::SocketAddr, server_sock: PathBuf, gate: Duration) -> anyhow::Result<()> {
    let cfg = RelayCfg {
        host: relay_addr.ip().to_string(),
        port: relay_addr.port(),
        tls: false,
        heartbeat_s: 3600, // 测试期不发心跳（专测见下）
    };
    let bridge = Arc::new(AgentBridge {
        sock: server_sock,
        turn_timeout: gate,
    });
    relay::run_once(&cfg, "gw-test", "test-secret", bridge).await
}

#[tokio::test]
async fn full_chain_ok_errors_and_liveness() {
    let (server_sock, server_rx, _server) = fake_server("ok", ServerScript::Ok("AginxOS 是给 Agent 的手机操作系统"));
    let relay_listener = FakeRelay::start().await;
    let gw = tokio::spawn(run_gateway(relay_listener.addr, server_sock.clone(), Duration::from_secs(30)));

    // 等全链走完（假 relay 脚本会自己关）：run_once 返回即完整。
    gw.await.unwrap().unwrap();

    // server 侧收到的请求形状
    let req = server_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let v: Value = serde_json::from_str(req.trim()).unwrap();
    assert_eq!(v["op"], "send");
    assert_eq!(v["avatar"], "小满");
    assert_eq!(v["text"], "用一句话介绍AginxOS");

    // 网关侧出线形状（按内容找行，不钉位置）
    let seen = relay_listener.seen.lock().unwrap().clone();
    let find = |frag: &str| {
        seen.iter()
            .map(|s| s.trim())
            .find(|s| s.contains(frag))
            .unwrap_or_else(|| panic!("no line contains {frag}: {seen:?}"))
            .to_string()
    };

    // initialize 定向应答：clientId 注入 + protocolVersion 整数
    let init: Value = serde_json::from_str(&find("\"id\":1")).unwrap();
    assert_eq!(init["clientId"], "c_1");
    assert_eq!(init["result"]["protocolVersion"], 1);

    // ok 轮：chunk 广播裸行在前、endTurn 终帧在后（顺序是契约）
    let chunk: Value = serde_json::from_str(&find("\"method\":\"chunk\"")).unwrap();
    assert_eq!(chunk["params"]["text"], "AginxOS 是给 Agent 的手机操作系统");
    assert!(chunk.get("id").is_none(), "chunk 无 id");
    assert!(chunk.get("clientId").is_none(), "chunk 广播裸行");
    let final_: Value = serde_json::from_str(&find("\"stopReason\"")).unwrap();
    assert_eq!(final_["result"]["stopReason"], "endTurn");
    assert!(final_.get("id").is_none(), "终帧无 id");
    let ci = seen.iter().position(|l| l.contains("\"method\":\"chunk\"")).unwrap();
    let fi = seen.iter().position(|l| l.contains("\"stopReason\"")).unwrap();
    assert!(ci < fi, "chunk 必须先于终帧");

    // 未实现方法 → -32601 定向（id 4）
    let na: Value = serde_json::from_str(&find("\"id\":4")).unwrap();
    assert_eq!(na["error"]["code"], -32601);

    // 入站 ping 得到 pong：双向仍通
    assert!(relay_listener.saw("\"type\":\"pong\""), "入站 ping 必须回 pong");
}

#[tokio::test]
async fn unknown_avatar_maps_to_32601_with_the_name() {
    let (server_sock, server_rx, _server) = fake_server("ua", ServerScript::UnknownAvatar);
    let relay_listener = FakeRelay::start().await;
    let gw = tokio::spawn(run_gateway(relay_listener.addr, server_sock, Duration::from_secs(30)));
    gw.await.unwrap().unwrap();
    let _ = server_rx;

    // 假 relay 脚本同一条链：第 5 条出线是 unknown_avatar 应答
    let seen = relay_listener.seen.lock().unwrap().clone();
    let err_line = seen
        .iter()
        .map(|s| s.trim())
        .find(|s| s.contains("\"id\":3"))
        .expect("error frame for id 3");
    let v: Value = serde_json::from_str(err_line).unwrap();
    assert_eq!(v["error"]["code"], -32601);
    let msg = v["error"]["message"].as_str().unwrap();
    assert!(msg.contains("路人"), "错误必须提及化身名: {msg}");
}

#[tokio::test]
async fn unreachable_server_is_32603() {
    // 指向不存在的 UDS 路径：server 永远不可达
    let dir = std::env::temp_dir().join(format!("aginx-gw-nosrv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dead_sock = dir.join("no-server.sock");
    let relay_listener = FakeRelay::start().await;
    let gw = tokio::spawn(run_gateway(relay_listener.addr, dead_sock, Duration::from_secs(5)));
    gw.await.unwrap().unwrap();

    let seen = relay_listener.seen.lock().unwrap().clone();
    let err_line = seen.iter().map(|s| s.trim()).find(|s| s.contains("\"id\":2")).expect("error frame for the ok-turn id 2");
    let v: Value = serde_json::from_str(err_line).unwrap();
    assert_eq!(v["error"]["code"], -32603, "UDS 不通=内部错误码");
}

#[tokio::test]
async fn turn_gate_fires_before_the_client_timeout() {
    // server 收下不回：闸门 0.2s 必须先触发（agc 120s 的微缩模型）。
    let (server_sock, server_rx, _server) = fake_server("gate", ServerScript::Silent);
    let relay_listener = FakeRelay::start().await;
    let t0 = std::time::Instant::now();
    let gw = tokio::spawn(run_gateway(relay_listener.addr, server_sock, Duration::from_millis(200)));
    // 假 relay 会在读到错误帧后继续走完脚本再关——不等它了，等网关任务。
    let _ = tokio::time::timeout(Duration::from_secs(10), gw).await;
    let elapsed = t0.elapsed();
    assert!(elapsed < Duration::from_secs(5), "闸门必须秒级收口: {elapsed:?}");
    let _ = server_rx;

    let seen = relay_listener.seen.lock().unwrap().clone();
    let err_line = seen.iter().map(|s| s.trim()).find(|s| s.contains("\"id\":2")).expect("gate error frame");
    let v: Value = serde_json::from_str(err_line).unwrap();
    assert_eq!(v["error"]["code"], -32603);
    // 消息不钉字样：connect 成功时 std 侧 read_timeout 先炸（"uds read:
    // …"），spawn 挂死才是 "turn gate … exceeded"——两条路都在闸门内
    // 收口，elapsed 断言才是契约。
    assert!(!v["error"]["message"].as_str().unwrap().is_empty());
}

// ---- 纯桥件小测（不起 relay）-------------------------------------------

#[tokio::test]
async fn bridge_prompt_requires_a_message() {
    let dir = std::env::temp_dir().join(format!("aginx-gw-bridge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let b = AgentBridge { sock: dir.join("unused.sock"), turn_timeout: Duration::from_secs(1) };
    let frames = b
        .on_data(
            "c_1",
            json!({"jsonrpc":"2.0","id":9,"method":"prompt","params":{"agent":"小满"}}),
        )
        .await;
    assert_eq!(frames.len(), 1);
    match &frames[0] {
        Wire::Directed(v) => assert_eq!(v["error"]["code"], -32602),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn reply_line_cap_rejects_huge_udS_replies() {
    // 直接打 uds_roundtrip 的帽：假 server 回 2MiB 一行 → Other（帽）。
    let dir = std::env::temp_dir().join(format!("aginx-gw-cap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("cap.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    std::thread::spawn(move || {
        let (mut conn, _) = listener.accept().unwrap();
        let mut sink = [0u8; 512];
        let _ = conn.read(&mut sink); // 吃请求
        let mut big = vec![b'x'; 2 * 1024 * 1024];
        big.push(b'\n');
        let _ = conn.write_all(&big);
    });
    // uds_roundtrip 是私有 fn——经桥走一轮（帽在 envelope 解析前触发）。
    let b = AgentBridge { sock, turn_timeout: Duration::from_secs(5) };
    let frames = b
        .on_data("c_1", json!({"jsonrpc":"2.0","id":1,"method":"prompt","params":{"message":"hi"}}))
        .await;
    match &frames[0] {
        Wire::Directed(v) => assert_eq!(v["error"]["code"], -32603),
        other => panic!("{other:?}"),
    }
}
