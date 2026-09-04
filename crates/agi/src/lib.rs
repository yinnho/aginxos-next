// agi — fast-agi v0：服务器 ↔ runtime 实例之间的执行协议（宪法 D5）。
//
// 一行一帧，JSONL，走 stdio：服务器的 stdin 是 runtime 的 stdin，反之亦然。
// 方向表（v0 按需 spawn；协议按常驻设计——常驻 worker 只是把「读文件冷恢复」
// 换成「收增量事件」，帧集不变）：
//
//   server → runtime : request / steer / tool_result
//   runtime → server : tool_call / artifact / done
//
// 帧法：
// - done 是唯一终帧：正常与错误都折在它的 ok/text/error 里，流上没有
//   第二种收尾方式；done 之后进程退出（v0）。
// - tool_call.id ↔ tool_result.id 配对；工具执行在服务器侧（D12：外件
//   一律 CLI，母体唯一外部接口 = spawn），runtime 只发起与收账。
// - artifact 是投影产物（D6）：流式增量，屏每变一次重投影一次；v0 只有
//   文本增量，富产物类型后续加 kind。
// - request 只带本轮文本；会话上下文由 runtime 从化身文件夹的
//   sessions/{id}.jsonl 冷恢复（D8：会话日志 = 真源，冷启动即恢复）。
// - steer 是主动输入中途插入（语音打断、追加指令），v0 定义不使用。
//
// 帧判别字段是 "t"（紧凑、人读 JSONL 时一眼可分）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, Write};

/// 一行的最大字节数：artifact 增量可以肥，但不能无边。
pub const MAX_LINE: usize = 16 * 1024 * 1024;

// ---------------- frames ----------------

/// server → runtime：一轮请求。runtime 以化身文件夹 + session 为上下文
/// 跑一轮，收尾必须给 done。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub avatar: String,
    pub session: String,
    pub text: String,
}

/// server → runtime：中途插入的主动输入（v0 定义不使用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Steer {
    pub text: String,
}

/// runtime → server：要一个工具（外件 CLI）执行。args 是命令行尾巴：
/// JSON 数组 = argv 原样；JSON 对象 = 展平成 --key value 旗标。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub tool: String,
    pub args: Value,
}

/// server → runtime：一次工具执行的回账。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub ok: bool,
    pub code: i32,
    /// stdout（成功载荷；D1 信封的 data 或裸文本）
    pub out: String,
    /// stderr（人类诊断）
    pub err: String,
}

/// runtime → server：投影产物增量（D6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    /// 文本增量：服务器拼起来投影，done.text 是收尾定稿
    Text,
    /// 图片（路径或 base64，v0 未用）
    Image,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub kind: ArtifactKind,
    pub data: String,
}

/// done 里的错误形状（错误折进 done，不设独立 error 帧）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameError {
    /// 稳定码：brain_unreachable / tool_failed / bad_state / internal…
    pub code: String,
    pub message: String,
}

/// runtime → server：终帧。ok=true 时 text 是本轮定稿回复；ok=false 时
/// error 说明死因。done 之后流即结束。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Done {
    pub ok: bool,
    pub text: String,
    pub error: Option<FrameError>,
}

impl Done {
    pub fn ok(text: impl Into<String>) -> Done {
        Done { ok: true, text: text.into(), error: None }
    }

    pub fn err(code: &str, message: impl Into<String>) -> Done {
        Done { ok: false, text: String::new(), error: Some(FrameError {
            code: code.to_string(),
            message: message.into(),
        }) }
    }
}

/// 一帧。Wire 形状：`{"t":"request",…}` 单行 JSON。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Frame {
    Request(Request),
    Steer(Steer),
    ToolCall(ToolCall),
    ToolResult(ToolResult),
    Artifact(Artifact),
    Done(Done),
}

// ---------------- io ----------------

/// 读帧失败：坏行（超长/非 UTF-8/非 JSON）或底层 io 错误。干净的 EOF
/// 不是错误——`FrameReader::next` 把它表达为 `Ok(None)`。
#[derive(Debug)]
pub enum RecvError {
    Bad(String),
    Io(std::io::Error),
}

impl From<std::io::Error> for RecvError {
    fn from(e: std::io::Error) -> Self {
        RecvError::Io(e)
    }
}

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecvError::Bad(m) => write!(f, "bad frame: {m}"),
            RecvError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for RecvError {}

/// 单帧编码为一行 JSON（带结尾换行）。
pub fn encode(f: &Frame) -> String {
    let mut s = serde_json::to_string(f).expect("frame json");
    s.push('\n');
    s
}

/// 写一帧并冲刷。协议是行锁步的：对方在等你这一行。
pub fn write<W: Write>(w: &mut W, f: &Frame) -> std::io::Result<()> {
    w.write_all(encode(f).as_bytes())?;
    w.flush()
}

/// 流式读帧器：一行一帧，行有上限。
pub struct FrameReader<R: BufRead> {
    r: R,
    line: Vec<u8>,
}

impl<R: BufRead> FrameReader<R> {
    pub fn new(r: R) -> FrameReader<R> {
        FrameReader { r, line: Vec::new() }
    }

    /// 读下一帧；Ok(None) = 对端收流。
    pub fn next(&mut self) -> Result<Option<Frame>, RecvError> {
        let n = read_line_bounded(&mut self.r, &mut self.line)?;
        if n == 0 {
            return Ok(None);
        }
        let t = std::str::from_utf8(&self.line[..n])
            .map_err(|_| RecvError::Bad("invalid utf-8".into()))?
            .trim_end_matches(['\n', '\r']);
        if t.is_empty() {
            return Err(RecvError::Bad("empty line".into()));
        }
        serde_json::from_str(t)
            .map(Some)
            .map_err(|e| RecvError::Bad(format!("bad frame json: {e}")))
    }
}

/// 读到 `\n` 或 EOF，带字节数上限（std 的 read_line 无界）。
/// 返回读到的字节数；buffer 只含本行。
fn read_line_bounded<R: BufRead>(r: &mut R, out: &mut Vec<u8>) -> Result<usize, RecvError> {
    out.clear();
    loop {
        let available = match r.fill_buf() {
            Ok(a) => a,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(RecvError::Io(e)),
        };
        if available.is_empty() {
            break; // EOF
        }
        match available.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                out.extend_from_slice(&available[..=pos]);
                r.consume(pos + 1);
                break;
            }
            None => {
                let len = available.len();
                out.extend_from_slice(available);
                r.consume(len);
            }
        }
        if out.len() > MAX_LINE {
            return Err(RecvError::Bad(format!("line over {MAX_LINE} bytes")));
        }
    }
    Ok(out.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 一轮的规范帧序：request → tool_call → tool_result → artifact×2 →
    /// done。金样：wire 上的字节形状逐行断言，协议改动会在这里炸。
    fn canonical_turn() -> Vec<Frame> {
        vec![
            Frame::Request(Request {
                avatar: "小满".into(),
                session: "s-20260905-01".into(),
                text: "北京今天天气怎么样".into(),
            }),
            Frame::ToolCall(ToolCall {
                id: "c1".into(),
                tool: "web-search".into(),
                args: json!(["北京", "天气"]),
            }),
            Frame::ToolResult(ToolResult {
                id: "c1".into(),
                ok: true,
                code: 0,
                out: "{\"ok\":true,\"data\":\"晴 26C\"}".into(),
                err: String::new(),
            }),
            Frame::Artifact(Artifact { kind: ArtifactKind::Text, data: "今天北京".into() }),
            Frame::Artifact(Artifact { kind: ArtifactKind::Text, data: "晴，26 度。".into() }),
            Frame::Done(Done::ok("今天北京晴，26 度。")),
        ]
    }

    #[test]
    fn golden_wire_bytes() {
        let lines: Vec<String> = canonical_turn()
            .iter()
            .map(|f| encode(f).trim_end().to_string())
            .collect();
        assert_eq!(
            lines,
            vec![
                r#"{"t":"request","avatar":"小满","session":"s-20260905-01","text":"北京今天天气怎么样"}"#,
                r#"{"t":"tool_call","id":"c1","tool":"web-search","args":["北京","天气"]}"#,
                r#"{"t":"tool_result","id":"c1","ok":true,"code":0,"out":"{\"ok\":true,\"data\":\"晴 26C\"}","err":""}"#,
                r#"{"t":"artifact","kind":"text","data":"今天北京"}"#,
                r#"{"t":"artifact","kind":"text","data":"晴，26 度。"}"#,
                r#"{"t":"done","ok":true,"text":"今天北京晴，26 度。","error":null}"#,
            ]
        );
    }

    #[test]
    fn roundtrip_canonical_turn() {
        let mut wire = String::new();
        for f in &canonical_turn() {
            wire.push_str(&encode(f));
        }
        let mut rd = FrameReader::new(wire.as_bytes());
        for want in canonical_turn() {
            assert_eq!(rd.next().unwrap(), Some(want));
        }
        assert_eq!(rd.next().unwrap(), None);
    }

    #[test]
    fn roundtrip_all_frames_and_shapes() {
        let frames = vec![
            Frame::Steer(Steer { text: "等等，改成上海".into() }),
            Frame::ToolCall(ToolCall {
                id: "c9".into(),
                tool: "file-write".into(),
                args: json!({"path": "/tmp/a.txt", "content": "hi"}),
            }),
            Frame::ToolResult(ToolResult {
                id: "c9".into(),
                ok: false,
                code: 2,
                out: String::new(),
                err: "usage: need <path>".into(),
            }),
            Frame::Artifact(Artifact { kind: ArtifactKind::Image, data: "/tmp/shot.png".into() }),
            Frame::Done(Done::err("brain_unreachable", "dial tcp: timeout")),
            Frame::Done(Done { ok: true, text: String::new(), error: None }),
        ];
        let mut wire = String::new();
        for f in &frames {
            wire.push_str(&encode(f));
        }
        let mut rd = FrameReader::new(wire.as_bytes());
        for want in frames {
            assert_eq!(rd.next().unwrap(), Some(want));
        }
        assert_eq!(rd.next().unwrap(), None);
    }

    #[test]
    fn error_done_shape() {
        let f = Frame::Done(Done::err("tool_failed", "web-search rc=1"));
        assert_eq!(
            encode(&f).trim_end(),
            r#"{"t":"done","ok":false,"text":"","error":{"code":"tool_failed","message":"web-search rc=1"}}"#
        );
    }

    #[test]
    fn bad_lines_are_rejected_not_panics() {
        for bad in ["not json", r#"{"t":"nope"}"#, "\n"] {
            let mut rd = FrameReader::new(bad.as_bytes());
            assert!(rd.next().is_err(), "should reject {bad:?}");
        }
        // 空输入 = 干净 EOF，不是错误
        let mut rd = FrameReader::new("".as_bytes());
        assert_eq!(rd.next().unwrap(), None);
    }

    #[test]
    fn oversized_line_is_rejected() {
        // 16MiB+1 无换行的一行必须被拒，不能吃到内存无界
        let big = vec![b'a'; MAX_LINE + 1];
        let mut rd = FrameReader::new(&big[..]);
        assert!(matches!(rd.next(), Err(RecvError::Bad(_))));
    }

    #[test]
    fn crlf_stripped() {
        let mut wire = String::new();
        for f in canonical_turn() {
            let mut l = encode(&f);
            l.pop();
            l.push_str("\r\n");
            wire.push_str(&l);
        }
        let mut rd = FrameReader::new(wire.as_bytes());
        for want in canonical_turn() {
            assert_eq!(rd.next().unwrap(), Some(want));
        }
    }
}
