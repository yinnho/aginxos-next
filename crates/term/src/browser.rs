//! v4⑥ 结果页活体化：term 内嵌 CDP 面板客户端单元（触摸→CDP→DRM 进程内
//! 交帧）。对端宇宙只有一个本机进程——aginxbrowser 引擎 :8089，故 RFC6455
//! 手写（无 TLS/DNS；vstream.py 同款 80 行客户机对这台引擎全线跑通过），
//! 按 Rust 习惯补齐分片累积、半写缓冲、逐帧 ack。
//!
//! 线序（P0 设备收据）：GET /json/version → webSocketDebuggerUrl → WS →
//! Target.createTarget → Target.attachToTarget{flatten} →
//! Emulation.setDeviceMetricsOverride{[panel] 尺寸，D14} → Page.navigate(data:URL)
//! → Page.getLayoutMetrics → Page.startScreencast{jpeg}。screencastFrame
//! 到达先 ack 后解码（ack 压着引擎 ~150ms 帧预算，解码 ~70ms 串进去掉帧）。
//! damage-gated：静页一生只发首帧（签名=dom epoch/layout rev/scroll/
//! viewport，心跳 evaluate 不产生 damage），且首帧紧跟 startScreencast
//! 响应同批冲出——Setup→Live 过渡轮必须以真 can_present 解码，错失的
//! 帧由 Live 看门狗重臂换回。
//!
//! 状态机：Dial（有界阻塞 ~1.5s）→ Setup（稳态每轮至多一 op，等 id 配对，
//! 总预算 4s）→ Live；任何错 → Failed（文本面兜底，下次 result 沿重建）。
//! 本模块不碰 DRM——解码出的 Bitmap 交回主循环按 门（Idle&&result&&!blanked）
//! 直写 back_buf。

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

const ENGINE_PORT: u16 = 8089;
const HEARTBEAT: Duration = Duration::from_millis(300);
/// 帧饥饿看门狗阈值：Live 后这么久仍一帧未解（如过渡帧恰逢合眼被让路），
/// 重发 startScreencast 换新帧。已解出过一帧后静默是正常态（缓存帧持续
/// 在屏），不再重臂。
const REARM_AFTER: Duration = Duration::from_millis(1500);
const DIAL_BUDGET: Duration = Duration::from_millis(1500);
const SETUP_BUDGET: Duration = Duration::from_millis(4000);
const IN_BUF_CAP: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Dial,
    Setup,
    Live,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SetupStage {
    CreateTarget,
    Attach,
    Metrics,
    Navigate,
    Layout,
    Screencast,
}

pub struct Browser {
    state: State,
    sock: Option<TcpStream>,
    out: Vec<u8>,
    inp: Vec<u8>,
    frag: Vec<u8>,
    frag_op: Option<u8>,
    seq: u64,
    next_id: u64,
    wait: Option<(u64, Instant)>,
    setup: Option<SetupStage>,
    html: String,
    /// 面板尺寸（D14）：构造时从 [panel] 拿到，视口覆盖/布局/解码缩放/
    /// screencast 上限全部走这两个字段——本模块不碰 hwd，参数化后 host
    /// 测试喂 fixture 即纯内存。
    pw: u32,
    ph: u32,
    target_id: Option<String>,
    session_id: Option<String>,
    scroll: i64,
    max_off: i64,
    last_beat: Instant,
    /// 最近一帧「到达」时刻（未解出过帧时=构造时刻）。只作重臂节流基准；
    /// 解码与否由 can_present 门管，缓存帧不受影响。
    last_frame: Instant,
    /// Live 期间是否解出过至少一帧。
    frame_ok: bool,
    fresh: Option<aginx_img::Bitmap>,
}

impl Browser {
    /// 携带整页 HTML 进入 Dial。网络动作发生在后续 pump 里（Dial 一次性
    /// 有界阻塞 ~1.5s，主循环可接受）。产品入口：panel 唯一来源
    /// device.toml [panel]（D14，无默认）。
    pub fn start(html: &str) -> Browser {
        let p = &hwd::load_or_exit().panel;
        Browser::start_panel(html, p.width, p.height)
    }

    /// host 测试/显式尺寸入口（render_html(md,w,h) 同款切法）。
    pub fn start_panel(html: &str, pw: u32, ph: u32) -> Browser {
        Browser {
            state: State::Dial,
            sock: None,
            out: Vec::new(),
            inp: Vec::new(),
            frag: Vec::new(),
            frag_op: None,
            seq: 0,
            next_id: 1,
            wait: None,
            setup: None,
            html: html.to_string(),
            pw,
            ph,
            target_id: None,
            session_id: None,
            scroll: 0,
            max_off: 0,
            last_beat: Instant::now(),
            last_frame: Instant::now(),
            frame_ok: false,
            fresh: None,
        }
    }

    /// 主循环只认 pump/fd/scroll_by/teardown；state 供测试与现场对账。
    #[allow(dead_code)]
    pub fn state(&self) -> State {
        self.state
    }

    /// poll 集：Setup|Live 才有 WS fd。
    pub fn fd(&self) -> Option<std::os::fd::RawFd> {
        if matches!(self.state, State::Setup | State::Live) {
            self.sock.as_ref().map(|s| {
                use std::os::fd::AsRawFd;
                s.as_raw_fd()
            })
        } else {
            None
        }
    }

    pub fn wants_write(&self) -> bool {
        !self.out.is_empty()
    }

    /// 泵一轮。can_present=false 时照常收帧+ack 但不解码（Eye/灭屏/非结果
    /// 面期间让路）。返回本轮解码出的最新帧（有即应直写 back_buf）。
    pub fn pump(&mut self, now: Instant, can_present: bool) -> Option<aginx_img::Bitmap> {
        match self.state {
            State::Dial => {
                self.dial();
                None
            }
            State::Setup => {
                // 真 can_present：引擎把首帧紧跟 startScreencast 响应同批冲出，
                // 这一轮 drain 里状态已翻 Live——写死 false 会把静页一生唯一
                // 的一帧吃掉（零帧、永远光标面的现场雷）。
                self.live_io(now, can_present);
                if self.state == State::Setup {
                    self.setup_tick(now);
                }
                self.take_fresh()
            }
            State::Live => {
                self.live_io(now, can_present);
                if self.state == State::Live {
                    if now.duration_since(self.last_beat) >= HEARTBEAT {
                        self.last_beat = now;
                        self.send("Runtime.evaluate", serde_json::json!({"expression": "0"}));
                        self.flush_out();
                    }
                    self.maybe_rearm(now, can_present);
                }
                self.take_fresh()
            }
            State::Failed => None,
        }
    }

    /// 触摸滚动：dy 为手指位移（向下为正→内容上卷），排队 scrollTo。
    pub fn scroll_by(&mut self, dy: isize) {
        if self.state != State::Live {
            return;
        }
        self.scroll = (self.scroll - dy as i64).clamp(0, self.max_off.max(0));
        self.send(
            "Runtime.evaluate",
            serde_json::json!({"expression": format!("window.scrollTo(0,{})", self.scroll)}),
        );
        self.flush_out();
    }

    /// 退场即关：closeTarget 尽力送达（引擎侧 target 不关会 RSS 爬坡），
    /// 短阻塞冲刷后弃线。任何状态都可调。
    pub fn teardown(&mut self) {
        if matches!(self.state, State::Setup | State::Live) {
            eprintln!("aginx-term: browser teardown");
            if let Some(tid) = self.target_id.clone() {
                let _ = self.session_id.take(); // closeTarget 走 browser 域
                let _ = self.send("Target.closeTarget", serde_json::json!({"targetId": tid}));
                let key = mask_key(self.seq);
                self.seq += 1;
                client_frame(&mut self.out, 0x8, &[], key);
                if self.flush_out_blocking() {
                    self.out.clear();
                }
            }
        }
        self.sock = None;
        self.inp.clear();
        self.frag.clear();
        self.frag_op = None;
        self.state = State::Failed;
        self.fresh = None;
    }

    fn take_fresh(&mut self) -> Option<aginx_img::Bitmap> {
        self.fresh.take()
    }

    fn fail(&mut self, why: &str) {
        eprintln!("aginx-term: browser fail: {why}");
        // 尽力送达 closeTarget：fail 的常见病因（cdp error / navigate
        // errorText / setup timeout）里 WS 还活着，滞留 target 会让引擎
        // RSS 爬坡；硬失败（socket 已死）由 TCP 断开兜底。
        if self.sock.is_some() {
            if let Some(tid) = self.target_id.clone() {
                let _ = self.session_id.take();
                let _ = self.send("Target.closeTarget", serde_json::json!({"targetId": tid}));
                if self.flush_out_blocking() {
                    self.out.clear();
                }
            }
        }
        self.sock = None;
        self.out.clear();
        self.inp.clear();
        self.frag.clear();
        self.frag_op = None;
        self.wait = None;
        self.setup = None;
        self.state = State::Failed;
    }

    fn dial(&mut self) {
        match self.try_dial() {
            Ok(s) => {
                self.sock = Some(s);
                self.state = State::Setup;
                self.setup = Some(SetupStage::CreateTarget);
                self.last_beat = Instant::now();
            }
            Err(e) => self.fail(&e),
        }
    }

    fn try_dial(&mut self) -> Result<TcpStream, String> {
        let body = http_get("/json/version")?;
        let (host, port, path) = ws_endpoint(&body)?;
        ws_dial(&host, port, &path, self.seq)
    }

    fn send(&mut self, method: &str, params: serde_json::Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let mut msg = serde_json::json!({"id": id, "method": method, "params": params});
        if let Some(sid) = &self.session_id {
            msg["sessionId"] = serde_json::Value::String(sid.clone());
        }
        let key = mask_key(self.seq);
        self.seq += 1;
        client_frame(&mut self.out, 0x1, msg.to_string().as_bytes(), key);
        id
    }

    fn live_io(&mut self, now: Instant, can_present: bool) {
        if !self.flush_out() {
            return self.fail("ws write");
        }
        match read_sock(&mut self.sock, &mut self.inp) {
            false => return self.fail("ws read/eof"),
            true => {}
        }
        self.drain(now, can_present);
    }

    fn setup_tick(&mut self, now: Instant) {
        if let Some((_, t0)) = self.wait {
            if now.duration_since(t0) > SETUP_BUDGET {
                return self.fail("setup timeout");
            }
            return; // 等响应，下轮再来（稳态每轮至多一 op）
        }
        match self.setup {
            Some(SetupStage::CreateTarget) => {
                let id = self.send("Target.createTarget", serde_json::json!({"url": "about:blank"}));
                self.wait = Some((id, now));
            }
            Some(SetupStage::Attach) => {
                let tid = self.target_id.clone().unwrap_or_default();
                let id = self.send(
                    "Target.attachToTarget",
                    serde_json::json!({"targetId": tid, "flatten": true}),
                );
                self.wait = Some((id, now));
            }
            Some(SetupStage::Metrics) => {
                let id = self.send(
                    "Emulation.setDeviceMetricsOverride",
                    serde_json::json!({
                        "width": self.pw, "height": self.ph,
                        "deviceScaleFactor": 1, "mobile": true,
                    }),
                );
                self.wait = Some((id, now));
            }
            Some(SetupStage::Navigate) => {
                let id = self.send("Page.navigate", serde_json::json!({"url": data_url(&self.html)}));
                self.wait = Some((id, now));
            }
            Some(SetupStage::Layout) => {
                let id = self.send("Page.getLayoutMetrics", serde_json::json!({}));
                self.wait = Some((id, now));
            }
            Some(SetupStage::Screencast) => {
                let id = self.arm_screencast();
                self.wait = Some((id, now));
            }
            None => {}
        }
        self.flush_out();
    }

    fn advance_setup(&mut self, resp: &serde_json::Value) {
        let next = match self.setup {
            Some(SetupStage::CreateTarget) => {
                match resp["result"]["targetId"].as_str() {
                    Some(t) => self.target_id = Some(t.to_string()),
                    None => return self.fail("createTarget no targetId"),
                }
                Some(SetupStage::Attach)
            }
            Some(SetupStage::Attach) => {
                match resp["result"]["sessionId"].as_str() {
                    Some(s) => self.session_id = Some(s.to_string()),
                    None => return self.fail("attach no sessionId"),
                }
                Some(SetupStage::Metrics)
            }
            Some(SetupStage::Metrics) => Some(SetupStage::Navigate),
            Some(SetupStage::Navigate) => {
                if let Some(e) = resp["result"]["errorText"].as_str() {
                    return self.fail(&format!("navigate: {e}"));
                }
                Some(SetupStage::Layout)
            }
            Some(SetupStage::Layout) => {
                let h = resp["result"]["cssContentSize"]["height"]
                    .as_f64()
                    .unwrap_or(self.ph as f64);
                self.max_off = ((h - self.ph as f64).max(0.0)) as i64;
                Some(SetupStage::Screencast)
            }
            Some(SetupStage::Screencast) => {
                self.setup = None;
                self.state = State::Live;
                self.last_beat = Instant::now();
                self.scroll = 0;
                eprintln!("aginx-term: browser live");
                None
            }
            None => None,
        };
        self.setup = next;
    }

    fn drain(&mut self, now: Instant, can_present: bool) {
        let msgs = take_frames(&mut self.inp, &mut self.frag, &mut self.frag_op);
        for (opcode, payload) in msgs {
            match opcode {
                0x8 => {
                    self.fail("ws close");
                    return;
                }
                0x9 => {
                    let key = mask_key(self.seq);
                    self.seq += 1;
                    client_frame(&mut self.out, 0xA, &payload, key);
                }
                0xA => {}
                0x1 => {
                    let txt = String::from_utf8_lossy(&payload).to_string();
                    self.handle_msg(&txt, now, can_present);
                    if self.state == State::Failed {
                        return;
                    }
                }
                _ => {}
            }
        }
    }

    fn handle_msg(&mut self, txt: &str, now: Instant, can_present: bool) {
        let v: serde_json::Value = match serde_json::from_str(txt) {
            Ok(v) => v,
            Err(_) => return,
        };
        if let Some(id) = v["id"].as_u64() {
            let mine = matches!(self.wait, Some((w, _)) if w == id);
            if mine {
                self.wait = None;
                if v.get("error").is_some() {
                    let m = v["error"]["message"].as_str().unwrap_or("?");
                    return self.fail(&format!("cdp error: {m}"));
                }
                self.advance_setup(&v);
            }
            return;
        }
        if v["method"].as_str() == Some("Page.screencastFrame") {
            self.on_frame(&v, now, can_present);
        }
    }

    fn on_frame(&mut self, v: &serde_json::Value, now: Instant, can_present: bool) {
        self.last_frame = now;
        // 诊断期：到达即记（含 can_present），分辨「帧没来/来了没让解/解败」
        eprintln!(
            "aginx-term: frame arrive {}b present={}",
            v["params"]["data"].as_str().map(|s| s.len()).unwrap_or(0),
            can_present
        );
        let sid = v["params"]["sessionId"].as_str().unwrap_or("").to_string();
        // 先 ack 后解码：ack 不出手引擎就不再发帧，解码的 ~70ms 不能串进引擎帧预算
        self.send(
            "Page.screencastFrameAck",
            serde_json::json!({"sessionId": sid}),
        );
        self.flush_out();
        if !can_present {
            return;
        }
        let b64 = v["params"]["data"].as_str().unwrap_or("");
        if b64.is_empty() {
            eprintln!("aginx-term: frame EMPTY data");
            return;
        }
        match STANDARD.decode(b64) {
            Ok(jpg) => {
                match aginx_img::decode_scaled(&jpg, self.pw, self.ph) {
                    Some(bm) => {
                        self.fresh = Some(bm);
                        self.frame_ok = true;
                        eprintln!(
                            "aginx-term: frame ok {}b",
                            jpg.len()
                        );
                    }
                    None => eprintln!("aginx-term: frame decode_scaled NONE ({}b)", jpg.len()),
                }
            }
            Err(e) => eprintln!("aginx-term: frame b64: {e}"),
        }
    }

    /// 帧饥饿看门狗：静页一生只发首帧，被让路（过渡轮恰逢 Eye/灭屏）或
    /// ack 卡死后不会再有 damage 换帧。可上屏且 REARM_AFTER 内一帧未解时
    /// 重发 startScreencast——引擎侧重置 damage 门并内联立出新帧。已解出
    /// 过一帧后静默即正常态（缓存帧持续在屏），不再烧引擎。
    fn maybe_rearm(&mut self, now: Instant, can_present: bool) {
        if !can_present || self.frame_ok || now.duration_since(self.last_frame) < REARM_AFTER {
            return;
        }
        self.last_frame = now;
        self.arm_screencast();
    }

    /// 武装 screencast（Setup 尾与看门狗重臂共用一份参数）。返回命令 id。
    fn arm_screencast(&mut self) -> u64 {
        let id = self.send(
            "Page.startScreencast",
            // q60 定档（S2 预检收据）：q60→q80 帧字节 347KB→489KB，
            // 36px 正文字缘眼验无差；帧更小 = term 解码也更便宜。
            serde_json::json!({
                "format": "jpeg", "quality": 60,
                "maxWidth": self.pw, "maxHeight": self.ph,
                "everyNthFrame": 1,
            }),
        );
        self.flush_out();
        id
    }

    fn flush_out(&mut self) -> bool {
        loop {
            if self.out.is_empty() {
                return true;
            }
            let Some(s) = self.sock.as_mut() else { return false };
            match s.write(&self.out) {
                Ok(0) => return true,
                Ok(n) => {
                    self.out.drain(..n);
                }
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                    return true;
                }
                Err(_) => return false,
            }
        }
    }

    /// 短阻塞冲刷。返回是否真的对 socket 冲刷了（sock=None 时 out 原样保留）。
    fn flush_out_blocking(&mut self) -> bool {
        let Some(s) = self.sock.as_mut() else { return false };
        let _ = s.set_nonblocking(false);
        let _ = s.set_write_timeout(Some(Duration::from_millis(200)));
        while !self.out.is_empty() {
            match s.write(&self.out) {
                Ok(0) | Err(_) => return true,
                Ok(n) => {
                    self.out.drain(..n);
                }
            }
        }
        true
    }
}

/// 非阻塞读尽 socket → 追加到 inp。false = 连接已死。
fn read_sock(sock: &mut Option<TcpStream>, inp: &mut Vec<u8>) -> bool {
    let mut tmp = [0u8; 16384];
    loop {
        let Some(s) = sock.as_mut() else { return false };
        match s.read(&mut tmp) {
            Ok(0) => return false,
            Ok(n) => {
                inp.extend_from_slice(&tmp[..n]);
                if inp.len() > IN_BUF_CAP {
                    return false;
                }
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {
                return true
            }
            Err(_) => return false,
        }
    }
}

fn data_url(html: &str) -> String {
    format!(
        "data:text/html;charset=utf-8;base64,{}",
        STANDARD.encode(html.as_bytes())
    )
}

fn mask_key(seq: u64) -> [u8; 4] {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    ((seq.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nanos) as u32).to_le_bytes()
}

fn client_frame(out: &mut Vec<u8>, opcode: u8, payload: &[u8], key: [u8; 4]) {
    out.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= 0xFFFF {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&key);
    for (n, b) in payload.iter().enumerate() {
        out.push(b ^ key[n % 4]);
    }
}

/// 从缓冲摘完整帧；跨包分片在 frag 累积。控制帧（close/ping/pong）不参与
/// 分片、直接上交。
fn take_frames(
    inp: &mut Vec<u8>,
    frag: &mut Vec<u8>,
    frag_op: &mut Option<u8>,
) -> Vec<(u8, Vec<u8>)> {
    let mut done = Vec::new();
    while let Some((opcode, fin, payload, used)) = try_parse_frame(inp) {
        inp.drain(..used);
        match opcode {
            0x8 | 0x9 | 0xA => done.push((opcode, payload)),
            _ => {
                if frag_op.is_none() {
                    *frag_op = Some(opcode);
                }
                frag.extend_from_slice(&payload);
                if fin {
                    let op = frag_op.take().unwrap_or(0x1);
                    done.push((op, std::mem::take(frag)));
                }
            }
        }
    }
    done
}

/// 试图从 buf 头部解出一个 WS 帧；(opcode, fin, payload, 消耗字节数)。
fn try_parse_frame(buf: &[u8]) -> Option<(u8, bool, Vec<u8>, usize)> {
    if buf.len() < 2 {
        return None;
    }
    let opcode = buf[0] & 0x0F;
    let fin = buf[0] & 0x80 != 0;
    let masked = buf[1] & 0x80 != 0;
    let l7 = (buf[1] & 0x7F) as usize;
    let mut off = 2;
    let len = match l7 {
        126 => {
            if buf.len() < off + 2 {
                return None;
            }
            let l = u16::from_be_bytes([buf[2], buf[3]]) as usize;
            off += 2;
            l
        }
        127 => {
            if buf.len() < off + 8 {
                return None;
            }
            let mut a = [0u8; 8];
            a.copy_from_slice(&buf[off..off + 8]);
            off += 8;
            u64::from_be_bytes(a) as usize
        }
        _ => l7,
    };
    let key = if masked {
        if buf.len() < off + 4 {
            return None;
        }
        let k = [buf[off], buf[off + 1], buf[off + 2], buf[off + 3]];
        off += 4;
        Some(k)
    } else {
        None
    };
    if buf.len() < off + len {
        return None;
    }
    let mut payload = buf[off..off + len].to_vec();
    if let Some(k) = key {
        for (n, b) in payload.iter_mut().enumerate() {
            *b ^= k[n % 4];
        }
    }
    Some((opcode, fin, payload, off + len))
}

/// 引擎 /json/version 拉取（Connection: close 读到 EOF），返回 body。
fn http_get(path: &str) -> Result<String, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], ENGINE_PORT));
    let mut s = TcpStream::connect_timeout(&addr, DIAL_BUDGET)
        .map_err(|e| format!("engine dial: {e}"))?;
    let _ = s.set_read_timeout(Some(DIAL_BUDGET));
    let _ = s.set_write_timeout(Some(DIAL_BUDGET));
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{ENGINE_PORT}\r\nConnection: close\r\n\r\n"
    );
    s.write_all(req.as_bytes()).map_err(|e| format!("http write: {e}"))?;
    let mut raw = Vec::new();
    let mut one = [0u8; 4096];
    loop {
        match s.read(&mut one) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&one[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("http read: {e}")),
        }
        if raw.len() > 1024 * 1024 {
            return Err("http body too big".into());
        }
    }
    let txt = String::from_utf8_lossy(&raw);
    txt.split("\r\n\r\n")
        .nth(1)
        .map(String::from)
        .ok_or_else(|| "http no body".into())
}

/// /json/version body → (host, port, ws path)
fn ws_endpoint(version_body: &str) -> Result<(String, u16, String), String> {
    let v: serde_json::Value =
        serde_json::from_str(version_body).map_err(|e| format!("version json: {e}"))?;
    let url = v["webSocketDebuggerUrl"]
        .as_str()
        .ok_or("no webSocketDebuggerUrl")?;
    let rest = url.strip_prefix("ws://").ok_or("ws url not ws://")?;
    let slash = rest.find('/').ok_or("ws url no path")?;
    let (hp, path) = rest.split_at(slash);
    let (host, port) = hp.split_once(':').ok_or("ws url no port")?;
    let port: u16 = port.parse().map_err(|_| "ws port parse")?;
    Ok((host.to_string(), port, path.to_string()))
}

/// WS 握手（阻塞有界），成功返回非阻塞流。
fn ws_dial(host: &str, port: u16, path: &str, seq: u64) -> Result<TcpStream, String> {
    let addr: SocketAddr = format!("{host}:{port}")
        .parse()
        .map_err(|_| "ws host parse")?;
    let mut s =
        TcpStream::connect_timeout(&addr, DIAL_BUDGET).map_err(|e| format!("ws dial: {e}"))?;
    let _ = s.set_read_timeout(Some(DIAL_BUDGET));
    let _ = s.set_write_timeout(Some(DIAL_BUDGET));
    let nonce = format!(
        "{:016x}{:016x}",
        seq,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
    );
    let key = STANDARD.encode(nonce.as_bytes());
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    s.write_all(req.as_bytes()).map_err(|e| format!("ws write: {e}"))?;
    let mut head = Vec::new();
    let mut one = [0u8; 1];
    loop {
        let n = s.read(&mut one).map_err(|e| format!("ws handshake: {e}"))?;
        if n == 0 {
            return Err("ws handshake eof".into());
        }
        head.push(one[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 8192 {
            return Err("ws handshake too big".into());
        }
    }
    let head = String::from_utf8_lossy(&head);
    if !head.starts_with("HTTP/1.1 101") && !head.starts_with("HTTP/1.0 101") {
        let first = head.lines().next().unwrap_or("").to_string();
        return Err(format!("ws handshake: {first}"));
    }
    s.set_nonblocking(true).map_err(|e| format!("nonblock: {e}"))?;
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// host 测试统一构造：fixture 面板（真实红皮尺寸，纯数据不碰 /etc——
    /// 与 voice render.rs 的 md() 同款 D14 切法）。
    fn browser(html: &str) -> Browser {
        Browser::start_panel(html, 1080, 2340) // D14-exempt: fixture panel geometry
    }

    #[test]
    fn client_frame_small_masked_golden() {
        let mut out = Vec::new();
        client_frame(&mut out, 0x1, b"Hi", [1, 2, 3, 4]);
        assert_eq!(
            out,
            vec![0x81, 0x82, 1, 2, 3, 4, b'H' ^ 1, b'i' ^ 2]
        );
    }

    #[test]
    fn client_frame_extended_lengths() {
        let payload = vec![b'x'; 200];
        let mut out = Vec::new();
        client_frame(&mut out, 0x1, &payload, [0; 4]);
        assert_eq!(out[1], 0x80 | 126);
        assert_eq!(&out[2..4], &[0, 200]);
        assert_eq!(out.len(), 2 + 2 + 4 + 200);

        let payload = vec![b'y'; 70_000];
        let mut out = Vec::new();
        client_frame(&mut out, 0x1, &payload, [0; 4]);
        assert_eq!(out[1], 0x80 | 127);
        assert_eq!(&out[2..10], &(70_000u64).to_be_bytes());
    }

    #[test]
    fn parse_server_unmasked_roundtrip() {
        let mut raw = vec![0x81, 0x05];
        raw.extend_from_slice(b"hello");
        let (op, fin, payload, used) = try_parse_frame(&raw).unwrap();
        assert_eq!((op, fin, used), (0x1, true, 7));
        assert_eq!(payload, b"hello");
        assert!(try_parse_frame(&raw[..6]).is_none(), "半包不解析");
    }

    #[test]
    fn parse_masked_16bit() {
        let key = [9u8, 8, 7, 6];
        let payload: Vec<u8> = (0..300u32).map(|x| x as u8).collect();
        let mut masked = Vec::new();
        client_frame(&mut masked, 0x1, &payload, key);
        let (op, fin, got, used) = try_parse_frame(&masked).unwrap();
        assert_eq!((op, fin, used), (0x1, true, masked.len()));
        assert_eq!(got, payload);
    }

    #[test]
    fn parse_server_64bit_length_frame() {
        // v4⑥ 现场雷回归：引擎 screencast 帧（~99KB base64）> 65535 → WS 127
        // 扩展长度。旧代码读 buf[4..12]（错位 2 字节）→ 长度成天文数字 →
        // 永远「等更多字节」→ 帧粘死在缓冲头，后续 ack/心跳全部堵死；
        // 看门狗每 1.5s 重臂再灌一帧 → inp 涨到 8MB 上限 → ws read/eof
        // （设备上零帧 + 2-3 分钟断连的单一根因）。
        let payload: Vec<u8> = (0..70_000u32).map(|x| (x % 251) as u8).collect();
        let mut raw = Vec::new();
        server_text(&mut raw, &payload);
        let (op, fin, got, used) = try_parse_frame(&raw).unwrap();
        assert_eq!((op, fin), (0x1, true));
        assert_eq!(used, raw.len());
        assert_eq!(got, payload);
        // 半包：差 1 字节也必须等待，不能错位解析
        assert!(try_parse_frame(&raw[..raw.len() - 1]).is_none());
    }

    #[test]
    fn fragmentation_accumulates() {
        let mut inp = Vec::new();
        let mut frag = Vec::new();
        let mut frag_op = None;
        // 首片 fin=0 "hel"，续片 fin=0 "lo w"，末片 fin=1 "orld"
        inp.extend_from_slice(&[0x01, 0x03, b'h', b'e', b'l']);
        inp.extend_from_slice(&[0x00, 0x04, b'l', b'o', b' ', b'w']);
        inp.extend_from_slice(&[0x80, 0x04, b'o', b'r', b'l', b'd']);
        let done = take_frames(&mut inp, &mut frag, &mut frag_op);
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].0, 0x1);
        assert_eq!(done[0].1, b"hello world");
        assert!(inp.is_empty() && frag.is_empty());
    }

    #[test]
    fn ping_between_fragments_delivered() {
        let mut inp = Vec::new();
        let mut frag = Vec::new();
        let mut frag_op = None;
        inp.extend_from_slice(&[0x01, 0x01, b'a']);
        inp.extend_from_slice(&[0x89, 0x00]); // ping 空 payload
        inp.extend_from_slice(&[0x80, 0x01, b'b']);
        let done = take_frames(&mut inp, &mut frag, &mut frag_op);
        assert_eq!(done.len(), 2);
        assert_eq!(done[0], (0x9, Vec::<u8>::new()));
        assert_eq!(done[1], (0x1, b"ab".to_vec()));
    }

    #[test]
    fn ws_endpoint_parses_version_body() {
        let body = r#"{"Browser":"Chrome/122.0","Protocol":"1.3",
            "webSocketDebuggerUrl":"ws://127.0.0.1:8089/devtools/browser/abc-123"}"#;
        let (host, port, path) = ws_endpoint(body).unwrap();
        assert_eq!((host.as_str(), port, path.as_str()), ("127.0.0.1", 8089, "/devtools/browser/abc-123"));
    }

    #[test]
    fn data_url_shape() {
        let u = data_url("<html></html>");
        assert!(u.starts_with("data:text/html;charset=utf-8;base64,"));
        assert!(STANDARD
            .decode(&u["data:text/html;charset=utf-8;base64,".len()..])
            .map(|b| b == b"<html></html>")
            .unwrap_or(false));
    }

    #[test]
    fn setup_walk_in_memory() {
        // 无引擎的内存走查：直接喂响应，验证 Setup 序与 Live 迁移
        let mut b = browser("<html>x</html>");
        b.state = State::Setup;
        b.setup = Some(SetupStage::CreateTarget);
        let now = Instant::now();

        b.wait = None;
        b.setup_tick(now);
        let (id, _) = b.wait.unwrap();
        assert!(take_out_method(&mut b.out).contains("Target.createTarget"));

        b.handle_msg(&format!(r#"{{"id":{id},"result":{{"targetId":"t1"}}}}"#), now, false);
        assert_eq!(b.target_id.as_deref(), Some("t1"));
        b.setup_tick(now);
        let (id2, _) = b.wait.unwrap();
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Target.attachToTarget") && sent.contains("\"t1\""));

        b.handle_msg(&format!(r#"{{"id":{id2},"result":{{"sessionId":"s1"}}}}"#), now, false);
        assert_eq!(b.session_id.as_deref(), Some("s1"));
        // session 域：后续消息带 sessionId
        b.setup_tick(now);
        let (id3, _) = b.wait.unwrap();
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Emulation.setDeviceMetricsOverride") && sent.contains("\"sessionId\":\"s1\""));
        assert!(sent.contains("1080") && sent.contains("2340") && sent.contains("\"mobile\":true")); // D14-exempt: fixture panel asserted on wire

        b.handle_msg(&format!(r#"{{"id":{id3},"result":{{}}}}"#), now, false);
        b.setup_tick(now);
        let (id4, _) = b.wait.unwrap();
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Page.navigate") && sent.contains("data:text/html;charset=utf-8;base64,"));
        assert!(sent.contains("\"sessionId\":\"s1\""));

        b.handle_msg(&format!(r#"{{"id":{id4},"result":{{}}}}"#), now, false);
        b.setup_tick(now);
        let (id5, _) = b.wait.unwrap();
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Page.getLayoutMetrics"));

        b.handle_msg(
            &format!(r#"{{"id":{id5},"result":{{"cssContentSize":{{"height":9000}}}}}}"#),
            now,
            false,
        );
        assert_eq!(b.max_off, 9000 - 2340); // D14-exempt: fixture panel height
        b.setup_tick(now);
        let (id6, _) = b.wait.unwrap();
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Page.startScreencast") && sent.contains("\"format\":\"jpeg\""));

        b.handle_msg(&format!(r#"{{"id":{id6},"result":{{}}}}"#), now, false);
        assert_eq!(b.state, State::Live);
        assert!(b.setup.is_none() && b.wait.is_none());
    }

    #[test]
    fn screencast_frame_acks_then_decodes() {
        let mut b = browser("x");
        b.state = State::Live;
        b.session_id = Some("s1".into());
        let now = Instant::now();
        let b64 = STANDARD.encode(minimal_png());
        let msg = format!(
            r#"{{"method":"Page.screencastFrame","params":{{"sessionId":"s1","data":"{b64}","metadata":{{}}}}}}"#
        );
        // can_present=false：ack 照发、解码让路（Eye/灭屏语义）
        b.handle_msg(&msg, now, false);
        assert!(take_out_method(&mut b.out).contains("Page.screencastFrameAck"));
        assert!(b.fresh.is_none());
        // can_present=true：ack + 解码出 Bitmap
        b.handle_msg(&msg, now, true);
        let _ = take_out_method(&mut b.out);
        assert!(b.fresh.is_some(), "1x1 PNG 应解码出 Bitmap");
    }

    #[test]
    fn scroll_clamps_and_sends() {
        let mut b = browser("x");
        b.state = State::Live;
        b.session_id = Some("s1".into());
        b.max_off = 1000;
        b.scroll_by(300); // 0 - 300 → clamp 0
        assert_eq!(b.scroll, 0);
        b.scroll_by(-5000); // 0 + 5000 → clamp 1000
        assert_eq!(b.scroll, 1000);
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("window.scrollTo(0,1000)"));
        // 非 Live 拒绝
        b.state = State::Failed;
        b.scroll_by(1);
        assert!(b.out.is_empty());
    }

    #[test]
    fn teardown_sends_close_browser_scoped() {
        let mut b = browser("x");
        b.state = State::Live;
        b.target_id = Some("t1".into());
        b.session_id = Some("s1".into());
        b.teardown();
        // sock=None：flush 保留 out，帧内容可回读断言
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Target.closeTarget") && sent.contains("\"t1\""));
        assert!(!sent.contains("\"sessionId\""), "closeTarget 走 browser 域");
        assert_eq!(b.state, State::Failed);
        assert!(b.sock.is_none());
    }

    #[test]
    fn cdp_error_fails() {
        let mut b = browser("x");
        b.state = State::Setup;
        b.setup = Some(SetupStage::Attach);
        b.wait = Some((7, Instant::now()));
        b.handle_msg(r#"{"id":7,"error":{"message":"boom"}}"#, Instant::now(), false);
        assert_eq!(b.state, State::Failed);
    }

    #[test]
    fn transition_frame_decodes_same_pass() {
        // v4⑥ 现场雷回归：引擎把首帧紧跟 startScreencast 响应同批冲出，
        // 同一轮 drain 里状态已翻 Live；静页一生只有这帧。这轮必须以真
        // can_present 解码它——否则帧被 ack 后永久丢失（零帧、永远光标面）。
        let mut b = browser("<html>x</html>");
        b.state = State::Setup;
        b.setup = Some(SetupStage::Screencast);
        b.session_id = Some("s1".into());
        b.target_id = Some("t1".into());
        b.wait = Some((9, Instant::now()));
        let now = Instant::now();
        let b64 = STANDARD.encode(minimal_png());
        let frame = format!(
            r#"{{"method":"Page.screencastFrame","params":{{"sessionId":1,"data":"{b64}"}}}}"#
        );
        let mut inp = Vec::new();
        server_text(&mut inp, br#"{"id":9,"result":{}}"#);
        server_text(&mut inp, frame.as_bytes());
        b.inp = inp;
        b.drain(now, true);
        assert_eq!(b.state, State::Live);
        assert!(b.fresh.is_some(), "过渡帧必须在同轮解码");
        assert!(b.frame_ok);
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Page.screencastFrameAck"), "ack 仍先于解码");
    }

    #[test]
    fn starved_live_rearms_until_first_frame() {
        let mut b = browser("x");
        b.state = State::Live;
        b.session_id = Some("s1".into());
        let now = Instant::now();
        // 帧新鲜：不重臂
        b.last_frame = now;
        b.maybe_rearm(now, true);
        assert!(b.out.is_empty());
        // 饿了但不可上屏（Eye/灭屏）：不烧引擎
        b.last_frame = now - REARM_AFTER - Duration::from_millis(500);
        b.maybe_rearm(now, false);
        assert!(b.out.is_empty());
        // 饿了且可上屏：重臂，节流基准重置
        b.maybe_rearm(now, true);
        let sent = take_out_method(&mut b.out);
        assert!(sent.contains("Page.startScreencast") && sent.contains("\"sessionId\":\"s1\""));
        assert_eq!(b.last_frame, now);
        // 已解出过一帧：静默是正常态（缓存帧在屏），永不重臂
        b.frame_ok = true;
        b.last_frame = now - Duration::from_secs(60);
        b.maybe_rearm(now, true);
        assert!(b.out.is_empty());
    }

    // --- 测试脚手架 ---
    // 内存测试不接真 socket：sock=None 时 flush_out 保留 out 缓冲，
    // take_out_method 用 try_parse_frame 解帧（client 帧 mask key 内嵌可自解）。

    fn take_out_method(out: &mut Vec<u8>) -> String {
        let mut txt = String::new();
        while let Some((op, _fin, payload, used)) = try_parse_frame(out) {
            out.drain(..used);
            if op == 0x1 {
                txt.push_str(&String::from_utf8_lossy(&payload));
            }
        }
        txt
    }

    /// 服务器→客户端方向的 text 帧（不 mask，126/127 扩展长度）。
    fn server_text(out: &mut Vec<u8>, payload: &[u8]) {
        out.push(0x81);
        let len = payload.len();
        if len < 126 {
            out.push(len as u8);
        } else if len <= 0xFFFF {
            out.push(126);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        } else {
            out.push(127);
            out.extend_from_slice(&(len as u64).to_be_bytes());
        }
        out.extend_from_slice(payload);
    }

    fn push_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(tag);
        out.extend_from_slice(data);
        let mut crc_input = Vec::new();
        crc_input.extend_from_slice(tag);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    /// 1x1 红点合法 PNG（手攒：zlib stored block + 手算 CRC/adler）
    fn minimal_png() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // depth 8, RGB, 无压缩/滤波/交错
        push_chunk(&mut out, b"IHDR", &ihdr);
        let raw: [u8; 4] = [0, 0xFF, 0x00, 0x00]; // filter 0 + 红
        let mut idat = vec![0x78, 0x01];
        idat.push(0x01); // stored 块 BFINAL=1 BTYPE=00
        idat.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        idat.extend_from_slice(&(!((raw.len() as u16))).to_le_bytes());
        idat.extend_from_slice(&raw);
        idat.extend_from_slice(&adler32(&raw).to_be_bytes());
        push_chunk(&mut out, b"IDAT", &idat);
        push_chunk(&mut out, b"IEND", &[]);
        out
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut table = [0u32; 256];
        for n in 0..256u32 {
            let mut c = n;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            table[n as usize] = c;
        }
        let mut c = 0xFFFF_FFFFu32;
        for &b in data {
            c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
        c ^ 0xFFFF_FFFF
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }
}
