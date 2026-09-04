// mother — 母体 me（宪法 D11：母体=aginx）。me 不是文件夹，是前台里的
// 一段代码：直接应答，可调 brain，v0 无账本（跟化身对话才有 D8 账）、
// 无工具面。用户跟母体说话 = 跟这台机器本身说话。

use aginx_runtime::brain::{BrainConfig, BrainDriver, CompletionRequest};
use aginx_runtime::message::Message;
use std::sync::OnceLock;

pub fn mother_reply(text: &str, roster: &[String]) -> Result<String, String> {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    let rt = RT.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
    });
    let brain = aginx_runtime::brain::HttpBrain::new(BrainConfig::from_env());
    let system = system_prompt(roster);
    let request = CompletionRequest {
        model: String::new(), // 模型归 brain 端配置
        messages: vec![Message::user(text)],
        tools: Vec::new(),
        max_tokens: 1024,
        temperature: 0.7,
        system: Some(system),
    };
    rt.block_on(brain.complete(request)).map(|r| r.text).map_err(|e| e.to_string())
}

fn system_prompt(roster: &[String]) -> String {
    let names = if roster.is_empty() {
        "（在册化身：无）".to_string()
    } else {
        format!("（在册化身：{}）", roster.join("、"))
    };
    format!(
        "你是 AginxOS 的母体——这台机器操作系统的自我，名字叫 me，站在前台。\
用户直接对你说话时你代表系统作答：简体中文、简短、直接。\
你知道谁在册：{names}。\
用户想跟某个化身说话就点名（aginx agent send <名字> <文本>），\
想回到你这里就说退房词（「再见」「退下」等）。\
化身的事你给转述线索，不替他们作答；你不是任何化身的替身。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// 本地假 brain：吃一个请求、回一个标准 OpenAI 响应。返回其地址。
    fn stub_brain(reply_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 8192];
                let mut got = Vec::new();
                // 读到请求体完整（content-length 满足即止，粗但够用）
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
                let http = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    reply_body.len(),
                    reply_body
                );
                let _ = sock.write_all(http.as_bytes());
            }
        });
        addr
    }

    #[test]
    fn mother_over_stub_brain_answers() {
        std::env::set_var(
            "AGINX_BRAIN_URL",
            format!(
                "http://{}/v1/chat/completions",
                stub_brain(
                    r#"{"choices":[{"message":{"role":"assistant","content":"我在，我是母体。"},"finish_reason":"stop"}]}"#,
                )
            ),
        );
        let reply = mother_reply("你是谁", &["小满".to_string()]).unwrap();
        assert_eq!(reply, "我在，我是母体。");
        std::env::remove_var("AGINX_BRAIN_URL");
        // unreachable → Err（brain 端自带 3 次退避重试，连拒绝秒败，
        // 全程 ~7s——语义已由 runtime crate 的错误折进 done 覆盖，这里
        // 不再跑慢路径）
        assert!(system_prompt(&[]).contains("在册化身：无"));
        assert!(system_prompt(&["小满".to_string()]).contains("小满"));
    }
}
