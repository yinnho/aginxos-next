//! 按键说话（PTT）：音量键 + 电源键，按住音量下=采集、松手=提交。电源键
//! 不动（短按灭屏/长按关机是 M15 语义），音量下键此前一直被 aterm 忽略
//! ——产品唤醒键就选它。evdev 是广播语义：voiced 自己开 fd，与 aterm 读
//! 同一节点互不抢。
//! M42e 产品面补充：音量上键短按=VolUp（音量下键的短按在 daemon 侧按
//! 按住时长判别成音量−，长按仍是 PTT）。
//! 2026-09-04 收据：两键分家——音量下+电源在 qpnp_pon(event1)，音量上在
//! gpio-keys(event0)（/proc/bus/input/devices KEY 位图逐位解出 115 与
//! 114/116，与 HARDWARE.md 2475/5538 收据互证）。只听 event1 时音量上
//! 永远收不到，Ptt 必须 poll 全部节点。

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

pub const PTT_DEV: &str = "/dev/input/event1"; // qpnp_pon: 音量下+电源
pub const VOLUP_DEV: &str = "/dev/input/event0"; // gpio-keys: 音量上
pub const EV_KEY: u16 = 0x01;
pub const KEY_VOLUMEDOWN: u16 = 114;
pub const KEY_VOLUMEUP: u16 = 115;

pub struct Ptt {
    fds: Vec<(&'static str, File)>,
    buf: [u8; 512],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PttEv {
    Down,
    Up,
    VolUp,
}

impl Ptt {
    pub fn open() -> Option<Ptt> {
        let mut fds = Vec::new();
        for dev in [PTT_DEV, VOLUP_DEV] {
            // O_NONBLOCK：主循环 poll 里读，没数据立刻返回
            if let Ok(f) = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(dev)
            {
                fds.push((dev, f));
            }
        }
        if fds.is_empty() {
            None
        } else {
            Some(Ptt { fds, buf: [0; 512] })
        }
    }

    /// 已打开的节点名（启动日志用）。
    pub fn devs(&self) -> String {
        self.fds
            .iter()
            .map(|(d, _)| *d)
            .collect::<Vec<_>>()
            .join("+")
    }

    /// poll 全部按键节点（timeout_ms 是主环的心跳），可读的排干成事件。
    /// 返回后主环照常跑超时逻辑，与旧的单 fd poll 语义一致。
    pub fn wait(&mut self, timeout_ms: i32) -> Vec<PttEv> {
        let mut pfds: Vec<libc::pollfd> = self
            .fds
            .iter()
            .map(|(_, f)| libc::pollfd {
                fd: f.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let rc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
        let mut out = Vec::new();
        if rc > 0 {
            for i in 0..pfds.len() {
                if pfds[i].revents & libc::POLLIN != 0 {
                    out.extend(self.drain(i));
                }
            }
        }
        out
    }

    /// 非阻塞排干第 i 个 fd 的 pending 事件。
    /// 64 位下 input_event = 16(timeval)+2+2+4 = 24 字节无填充。
    fn drain(&mut self, i: usize) -> Vec<PttEv> {
        let mut out = Vec::new();
        loop {
            match self.fds[i].1.read(&mut self.buf) {
                Ok(0) => break,
                Ok(n) => {
                    let mut off = 0;
                    while off + 24 <= n {
                        let ty = u16::from_le_bytes([self.buf[off + 16], self.buf[off + 17]]);
                        let code = u16::from_le_bytes([self.buf[off + 18], self.buf[off + 19]]);
                        let val = i32::from_le_bytes([
                            self.buf[off + 20],
                            self.buf[off + 21],
                            self.buf[off + 22],
                            self.buf[off + 23],
                        ]);
                        if ty == EV_KEY {
                            match (code, val) {
                                (KEY_VOLUMEDOWN, 1) => out.push(PttEv::Down),
                                (KEY_VOLUMEDOWN, 0) => out.push(PttEv::Up),
                                // 只认松手沿：一次点按一个事件，repeat(2)忽略
                                (KEY_VOLUMEUP, 0) => out.push(PttEv::VolUp),
                                _ => {}
                            }
                        }
                        off += 24;
                    }
                }
                Err(_) => break, // EAGAIN
            }
        }
        out
    }
}
