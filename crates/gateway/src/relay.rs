//! relay — 第 0 层传输核（N5⑤ 自生态仓 aginx/src/relay/mod.rs 重安置）。
//!
//! 只搬传输：裸 host 拨号（铁律：id 路由是逻辑寻址，永不对
//! `<id>.relay.<domain>` 子域做 DNS，SNI=裸域名）、TLS、register 握手、
//! 心跳、按行读循环、断线由调用方重连。生态线的 AcpHandler/
//! AgentManager/AuthLevel/JWT 一概不带——本仓的收口桥在 agent.rs。
//!
//! wire 形状的权威规范是生态仓 aginx/ACP.md §1+§6（金样本互锁）；
//! tests/golden.rs 钉住本实现与那份文档的样本一致。

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;

/// 行帽 1MiB：本网关只承 UDS 单行轮（一轮一答）。生态线的 128MB
/// 票据单行形状不在 v1 范围——真到那天再抬。
pub const MAX_LINE: usize = 1024 * 1024;

/// 第 0 层帧。`connect`/`connected` 是客户端侧变体：网关不该收到，
/// 但现役 relay 可能旧于 HEAD，解析必须容错（收到只记日志）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum RelayMessage {
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "register")]
    Register { id: String, token: Option<String> },
    #[serde(rename = "registered")]
    Registered { id: String, url: String },
    #[serde(rename = "disconnected")]
    Disconnected { client_id: String },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "data")]
    Data { client_id: String, data: serde_json::Value },
    #[serde(rename = "connect")]
    Connect { target: String, token: Option<String> },
    #[serde(rename = "connected")]
    Connected { client_id: String },
}

/// 收口桥出网帧的两种路由（ACP.md §1.3 数据路由不对称）：
/// - Directed：网关注入 `clientId`，relay 剥掉后定向转发——带响应 id
///   的最终响应（initialize 应答、错误帧）走这条。
/// - Broadcast：裸 JSON-RPC 行直发——chunk 通知与无 id 的终帧走这条；
///   单客户端网关等价于定向。
#[derive(Debug, Clone, PartialEq)]
pub enum Wire {
    Directed(serde_json::Value),
    Broadcast(serde_json::Value),
}

/// 每条入站 data 帧换一批出网帧。实现 owns 轮次（agent.rs 的 110s 闸门）。
#[async_trait::async_trait]
pub trait Bridge: Send + Sync {
    async fn on_data(&self, client_id: &str, data: serde_json::Value) -> Vec<Wire>;
    /// 客户端断连通知。runtime 属 aginx-server，无幽灵进程可杀——
    /// v1 实现只记日志（与生态线的取消名册语义差异在此）。
    async fn on_disconnected(&self, client_id: &str) {
        let _ = client_id;
    }
}

pub struct RelayCfg {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub heartbeat_s: u64,
}

/// 一次连接的完整生命周期：拨号→握手→心跳+读循环→断开即返回。
/// 重连是调用方（main）的事——间隔策略不属于传输核。
pub async fn run_once(
    cfg: &RelayCfg,
    id: &str,
    secret: &str,
    bridge: Arc<dyn Bridge>,
) -> anyhow::Result<()> {
    let tcp = TcpStream::connect((cfg.host.as_str(), cfg.port)).await?;
    let _ = tcp.set_nodelay(true);

    let (reader, writer): (Box<dyn AsyncRead + Unpin + Send>, Box<dyn AsyncWrite + Unpin + Send>) =
        if cfg.tls {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let tls_cfg = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();
            let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_cfg));
            let name = rustls_pki_types::ServerName::try_from(cfg.host.clone())?;
            let tls = connector.connect(name, tcp).await?;
            let (r, w) = tokio::io::split(tls);
            (Box::new(r), Box::new(w))
        } else {
            let (r, w) = tcp.into_split();
            (Box::new(r), Box::new(w))
        };

    let mut reader = BufReader::new(reader);
    let writer: Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>> = Arc::new(Mutex::new(writer));

    handshake(&mut reader, &writer, id, secret).await?;
    message_loop(&mut reader, &writer, cfg.heartbeat_s, bridge).await
}

async fn write_line(
    writer: &Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>>,
    v: &serde_json::Value,
) -> anyhow::Result<()> {
    let mut w = writer.lock().await;
    w.write_all(format!("{v}\n").as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

async fn handshake<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    writer: &Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>>,
    id: &str,
    secret: &str,
) -> anyhow::Result<()> {
    write_line(
        writer,
        &serde_json::to_value(RelayMessage::Register {
            id: id.into(),
            token: Some(secret.into()),
        })?,
    )
    .await?;
    let mut line = String::new();
    read_line_capped(reader, MAX_LINE, &mut line).await?;
    match serde_json::from_str::<RelayMessage>(line.trim())? {
        RelayMessage::Registered { id, url } => {
            eprintln!("aginx-gateway: registered id={id} url={url}");
            Ok(())
        }
        RelayMessage::Error { message } => Err(anyhow::anyhow!("registration refused: {message}")),
        other => Err(anyhow::anyhow!("unexpected handshake reply: {other:?}")),
    }
}

async fn message_loop<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    writer: &Arc<Mutex<Box<dyn AsyncWrite + Unpin + Send>>>,
    heartbeat_s: u64,
    bridge: Arc<dyn Bridge>,
) -> anyhow::Result<()> {
    // 心跳：周期 ping。入站 ping 也回 pong（老实现只忽略——协议上
    // 对称更正确，双向都活着连接才被 relay 视作活）。
    let hb_writer = Arc::clone(writer);
    let hb = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(heartbeat_s.max(1)));
        // interval 首拍立即触发——吞掉它，首个 ping 落在一个完整周期后
        // （否则 register 后立刻 ping，心跳语义变"立即+周期"）。
        tick.tick().await;
        loop {
            tick.tick().await;
            if write_line(&hb_writer, &serde_json::json!({"type": "ping"})).await.is_err() {
                break;
            }
        }
    });

    let mut line = String::new();
    loop {
        line.clear();
        match read_line_capped(reader, MAX_LINE, &mut line).await {
            Ok(0) => {
                eprintln!("aginx-gateway: relay closed the connection");
                break;
            }
            Ok(_) => {}
            Err(e) => {
                hb.abort();
                return Err(anyhow::anyhow!("relay read: {e}"));
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<RelayMessage>(trimmed) else {
            eprintln!("aginx-gateway: unparsable relay line ({} bytes)", trimmed.len());
            continue;
        };
        match msg {
            RelayMessage::Ping => {
                let _ = write_line(writer, &serde_json::json!({"type": "pong"})).await;
            }
            RelayMessage::Pong => {}
            RelayMessage::Data { client_id, data } => {
                for wire in bridge.on_data(&client_id, data).await {
                    let v = match wire {
                        Wire::Directed(mut v) => {
                            if let Some(obj) = v.as_object_mut() {
                                obj.insert("clientId".into(), serde_json::json!(client_id));
                            }
                            v
                        }
                        Wire::Broadcast(v) => v,
                    };
                    if let Err(e) = write_line(writer, &v).await {
                        eprintln!("aginx-gateway: write failed: {e}");
                    }
                }
            }
            RelayMessage::Disconnected { client_id } => {
                eprintln!("aginx-gateway: client {client_id} disconnected");
                bridge.on_disconnected(&client_id).await;
            }
            RelayMessage::Error { message } => {
                eprintln!("aginx-gateway: relay error: {message}");
            }
            other => {
                eprintln!("aginx-gateway: ignoring unexpected frame: {other:?}");
            }
        }
    }
    hb.abort();
    Ok(())
}

/// 带帽的按行读：返回读到的字节数（不含换行），0=EOF。超帽报错而非
/// 让恶意长行把内存吃穿——read_line 无界，这里必须有闸。
pub async fn read_line_capped<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    cap: usize,
    out: &mut String,
) -> std::io::Result<usize> {
    let mut buf: Vec<u8> = Vec::with_capacity(512);
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            let n = buf.len();
            *out = String::from_utf8_lossy(&buf).into_owned();
            return Ok(n);
        }
        if let Some(nl) = available.iter().position(|&b| b == b'\n') {
            buf.extend_from_slice(&available[..nl]);
            let n = buf.len();
            reader.consume(nl + 1);
            if n > cap {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("line exceeds {cap}-byte cap"),
                ));
            }
            *out = String::from_utf8_lossy(&buf).into_owned();
            return Ok(n);
        }
        let n = available.len();
        buf.extend_from_slice(available);
        reader.consume(n);
        if buf.len() > cap {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("line exceeds {cap}-byte cap"),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_line_capped_enforces_the_cap() {
        let (mut client, server) = tokio::io::duplex(64);
        let mut server = BufReader::new(server);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let _ = client.write_all(b"short line\n").await;
            let _ = client.write_all(&vec![b'a'; 300]).await;
            let _ = client.flush().await;
        });
        let mut out = String::new();
        assert_eq!(read_line_capped(&mut server, 128, &mut out).await.unwrap(), 10);
        assert_eq!(out, "short line");
        assert!(read_line_capped(&mut server, 128, &mut out).await.is_err());
    }

    #[test]
    fn register_frame_matches_the_acp_shape() {
        let v = serde_json::to_value(RelayMessage::Register {
            id: "qi7o6bj5".into(),
            token: Some("<relay-secret>".into()),
        })
        .unwrap();
        assert_eq!(v, serde_json::json!({"type":"register","id":"qi7o6bj5","token":"<relay-secret>"}));
    }
}
