//! 按键说话（PTT）：音量键 + 电源键，按住音量下=采集、松手=提交。电源键
//! 不动（短按灭屏/长按关机是 M15 语义），音量下键此前一直被 aterm 忽略
//! ——产品唤醒键就选它。evdev 是广播语义：aginx-voice 自己开 fd，与 aterm 读
//! 同一节点互不抢。
//! M42e 产品面补充：音量上键短按=VolUp（音量下键的短按在 daemon 侧按
//! 按住时长判别成音量−，长按仍是 PTT）。
//! 2026-09-04 收据：两键分家——PTT 键与音量上键**在不同节点**（位图逐位
//! 解出键码，与 HARDWARE.md 2475/5538 收据互证）。只听单节点时音量上
//! 永远收不到，Ptt 必须 poll 全部节点。
//! D14：节点与键码不再是本文件的常量——唯一合法来源是
//! device.toml 的 [input.ptt]（hwd 读）。

use std::fs::File;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;

pub const EV_KEY: u16 = 0x01;

pub struct Ptt {
    fds: Vec<(&'static str, File)>,
    /// 每个 fd 盯的键码（来自 [input.ptt]，与 fds 按位配对）：
    /// [0]=PTT 节点盯音量下，[1]=音量上节点盯音量上。drain 按下标判角色。
    keys: Vec<u16>,
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
        let p = hwd::load_or_exit();
        let ipt = &p.input.ptt;
        let mut fds = Vec::new();
        let mut keys = Vec::new();
        for (dev, key) in [
            (&*ipt.device, ipt.key_volume_down),
            (&*ipt.volume_up_device, ipt.key_volume_up),
        ] {
            // O_NONBLOCK：主循环 poll 里读，没数据立刻返回
            if let Ok(f) = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(dev)
            {
                fds.push((dev, f));
                keys.push(key);
            }
        }
        if fds.is_empty() {
            None
        } else {
            Some(Ptt { fds, keys, buf: [0; 512] })
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
                        if ty == EV_KEY && code == self.keys[i] {
                            match (i, val) {
                                (0, 1) => out.push(PttEv::Down),
                                (0, 0) => out.push(PttEv::Up),
                                // 只认松手沿：一次点按一个事件，repeat(2)忽略
                                (1, 0) => out.push(PttEv::VolUp),
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
