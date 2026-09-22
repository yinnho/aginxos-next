// testkit — server 测试共器（main.rs 里 #[cfg(test)] 门控，不进产物）。
//
// 假 brain 是原生 TcpListener：一连接一应答、content-length 满足即答、
// 应答前可延迟。延迟不是化妆——steer 测试断言的就是时序：中途输入
// 何时进队 vs 何时被工具步边界收走，窗口由应答延迟撑开。

use crate::front::FrontDesk;
use crate::host::Mother;
use agi::Frame;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;

/// 假 brain 的一条应答：先睡 delay_ms 再回 body。
pub struct Reply {
    pub delay_ms: u64,
    pub body: String,
}

/// 立即应答。
pub fn now(body: String) -> Reply {
    Reply { delay_ms: 0, body }
}

/// 延迟应答——把下一个工具步边界拖到 steer 进队之后。
pub fn later(delay_ms: u64, body: String) -> Reply {
    Reply { delay_ms, body }
}

/// 起一个假 brain，返回地址。script 按序消费：第 N 次 agent 轮的 brain
/// 调用拿到第 N 条应答；耗尽后 listener 线程退场（再来的连接被拒，测试
/// 自然红）。kernel 的内部调用（轮末 turn-summary、compactor 摘要——
/// system 里有 "conversation summarizer"）走固定应答车道，不吃 script：
/// 它们是每轮真实发生的隐藏消费者，若与 agent 轮共吃一份序，应答错位、
/// 测试非确定。
pub fn stub_brain(script: Vec<Reply>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        let mut script = script.into_iter();
        loop {
            let Ok((mut sock, _)) = listener.accept() else { return };
            let mut buf = [0u8; 8192];
            let mut got = Vec::new();
            loop {
                let n = sock.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let head = String::from_utf8_lossy(&got).to_string();
                if let Some(cl) = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("content-length:"))
                    .and_then(|l| l.split(':').nth(1))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                {
                    if let Some(p) = head.find("\r\n\r\n") {
                        if got.len() >= p + 4 + cl {
                            break;
                        }
                    }
                }
            }
            let head = String::from_utf8_lossy(&got).to_string();
            let body = if head.contains("conversation summarizer") {
                // 内部车道：合法的 INTENT/OUTCOME/FACTS 摘要（parse 侧
                // 认这个格式；FACTS: NONE → 不写知识抽屉）
                oai("INTENT: 测试轮\\nOUTCOME: 测试轮\\nFACTS: NONE", None, "stop")
            } else {
                let Some(rep) = script.next() else { return };
                if rep.delay_ms > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(rep.delay_ms));
                }
                rep.body
            };
            let http = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(http.as_bytes());
        }
    });
    addr
}

/// 标准 OpenAI chat completion 响应体（stream:false 形状）。tool_calls
/// 传 JSON 数组字面量，finish 用 "tool_calls" / "stop"。
pub fn oai(content: &str, tool_calls: Option<&str>, finish: &str) -> String {
    let calls = match tool_calls {
        Some(c) => format!(",\"tool_calls\":{c}"),
        None => String::new(),
    };
    format!(
        r#"{{"choices":[{{"message":{{"role":"assistant","content":"{content}"{calls}}},"finish_reason":"{finish}"}}]}}"#
    )
}

/// 测试宿主：临时 home + 指向假 brain 的 brain.json + FrontDesk +
/// Mother::boot。返回的 desk 与 Mother 内观察者同源——steer 测试必须
/// 用这一份（排队进一个前台、收账的是另一个，就全错了）。
pub fn boot_host(dir: &Path, brain_addr: &str) -> (Arc<FrontDesk>, Mother) {
    std::fs::create_dir_all(dir).unwrap();
    std::env::set_var("AGINX_TEST_BRAIN_KEY", "test-key");
    std::fs::write(
        dir.join("brain.json"),
        serde_json::json!({
            "base_url": format!("http://{brain_addr}/v1/chat/completions"),
            "api_key_env": "AGINX_TEST_BRAIN_KEY",
            "default_modality": "chat",
            "modalities": { "chat": { "description": "test" } },
        })
        .to_string(),
    )
    .unwrap();
    let desk = Arc::new(FrontDesk::new(dir.join("workflows")));
    let mother = Mother::boot(dir.to_path_buf(), Arc::clone(&desk)).expect("test mother boots");
    (desk, mother)
}

/// 读一本账：一行一帧。
pub fn read_frames(log: &Path) -> Vec<Frame> {
    std::fs::read_to_string(log)
        .unwrap_or_else(|e| panic!("read ledger {}: {e}", log.display()))
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}
