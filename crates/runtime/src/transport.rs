// transport — fast-agi 的两端 IO 面。
//
// 把「发一帧 / 收一帧」抽成 trait，run_turn 只认这个面：生产走 stdio
// （server spawn 的管道两端），测试走内存队列——同一段状态机不换姿势
// 就能整轮驱动。

use agi::{Frame, FrameReader, RecvError};
use std::collections::VecDeque;
use std::io::{self, BufReader};

pub trait TurnTransport {
    /// runtime → server。行锁步协议：对方在等这一行。
    fn send(&mut self, f: &Frame) -> io::Result<()>;

    /// server → runtime。Ok(None) = 对端收流。
    fn recv(&mut self) -> Result<Option<Frame>, RecvError>;
}

/// 生产传输：stdin 收、stdout 发。
pub struct StdioTransport {
    rd: FrameReader<BufReader<std::io::Stdin>>,
    wr: std::io::Stdout,
}

impl StdioTransport {
    pub fn new() -> StdioTransport {
        StdioTransport {
            rd: FrameReader::new(BufReader::new(std::io::stdin())),
            wr: std::io::stdout(),
        }
    }
}

impl Default for StdioTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnTransport for StdioTransport {
    fn send(&mut self, f: &Frame) -> io::Result<()> {
        agi::write(&mut self.wr, f)
    }

    fn recv(&mut self) -> Result<Option<Frame>, RecvError> {
        self.rd.next()
    }
}

/// 测试传输：sent 收集 runtime→server 帧；inbox 预埋 server→runtime 帧
/// （recv 逐个吐，空了 = EOF）。
#[derive(Default)]
pub struct MemTransport {
    pub sent: Vec<Frame>,
    pub inbox: VecDeque<Frame>,
}

impl TurnTransport for MemTransport {
    fn send(&mut self, f: &Frame) -> io::Result<()> {
        self.sent.push(f.clone());
        Ok(())
    }

    fn recv(&mut self) -> Result<Option<Frame>, RecvError> {
        Ok(self.inbox.pop_front())
    }
}
