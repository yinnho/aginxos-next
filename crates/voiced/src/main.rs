//! voiced — 语音对话守护（M42a，产品定义 2026-09-04：手机即智能体）。
//!
//! 产品面唯一的输入是语音（PTT=按住音量下键）和眼（M42b），输出是脸
//! （/run/voice/face → aterm 渲染）和嘴（TTS）。**拉式语音**（2026-09-04
//! 用户收据：识别和回应都是毫秒级，唯独嘴是慢车道）——回应默认只上脸，
//! 用户点名（「你说给我听」「念一下」「再念一遍」）才出声。协议是封闭
//! 词表的确定性状态机（protocol.rs），没有 LLM——WiFi 必须在 LLM 可用
//! 之前连得上。
//!
//! **前台模式（N2②）**：env `VOICED_FRONT=<aginx 路由器路径>` 时自由文本
//! （封闭词表 miss）改投新前台——`aginx agent send`（母体/化身光标，
//! AGINX_SOCK 决定找哪台 server）。封闭词表仍本地优先（离线地板），
//! 前台不可达落回地板话。不设此 env = 老行为分毫不动。
//!
//! 调试面（收据阶梯，从嘴/耳单器官到全环）：
//!   voiced --say "文本"          只测嘴（TTS→扬声器）
//!   voiced --hear <wav文件>      只测耳（WAV→ASR→打印文本）
//!   voiced --inject "文本"       喂状态机走全流程（不出声，Act 真执行）
//!   voiced --face                打印当前屏面 JSON
//!
//! 没有嘴耳同开的回环自检：M18 的硬件收据写明 MM1 边放边采会把放音叠
//! 进采集（数字回环是失真副本，880Hz 可验、语音不可认，2026-09-04 实测
//! ASR 出"うん、うん"）——产品路径本来也是顺序的：PTT 采完才 TTS。

mod audio;
mod face;
mod protocol;
mod ptt;

use protocol::{Act, Ev, Out, Vm};
use std::process::Command;
use std::time::{Duration, Instant};

const TIMEOUT_SECS: u64 = 45; // 提示后无语音的退出时限
const JOIN_BUDGET_SECS: u32 = 90;
/// 母体一轮（真 brain，含工具往返）的等待预算——超了杀掉落地板话。
const FRONT_BUDGET_SECS: u32 = 90;

/// 前台模式开关：VOICED_FRONT=新前台路由器路径（aginx）→ 开。
/// 只认这个 env，不猜 PATH——试跑期路由器在隔离树里，路径是显式合同。
fn front_bin() -> Option<String> {
    match std::env::var("VOICED_FRONT") {
        Ok(v) if !v.trim().is_empty() => Some(v),
        _ => None,
    }
}

/// Vm 构造口：前台模式开 with_front，否则老 Vm（行为分毫不动）。
fn make_vm() -> Vm {
    if front_bin().is_some() {
        Vm::with_front()
    } else {
        Vm::new()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--say") => {
            let text = args.get(2).expect("usage: voiced --say <text>");
            let brain = audio::Brain::from_env();
            say(text, brain.as_ref());
        }
        Some("--hear") => {
            let path = args.get(2).expect("usage: voiced --hear <wav>");
            let brain = audio::Brain::from_env();
            let wav = std::fs::read(path).expect("read wav");
            let text = hear(&wav, brain.as_ref()).expect("asr failed");
            println!("{text}");
        }
        Some("--inject") => {
            let text = args.get(2).expect("usage: voiced --inject <text>").clone();
            let mut vm = make_vm();
            face::write(&vm, false, false);
            let outs = vm.step(Ev::Heard(text));
            run_outs(&mut vm, outs, None);
        }
        Some("--script") => {
            // 收据阶梯：stdin 每行一条 Heard，同一个 Vm 跨步保持（--inject
            // 一次一进程，扫码→确认这种多步流跑不完整）。run_outs 在步间阻塞
            // ——相机/TTS 落完才读下一行，喂两行也能按序走完。
            let brain = audio::Brain::from_env();
            let mut vm = make_vm();
            face::write(&vm, false, false);
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let t = line.trim_end_matches('\n');
                        if t.is_empty() {
                            continue;
                        }
                        eprintln!("voiced: script {t:?}");
                        let outs = vm.step(Ev::Heard(t.to_string()));
                        run_outs(&mut vm, outs, brain.as_ref());
                    }
                }
            }
        }
        Some("--face") => match face::read() {
            Some(s) => println!("{s}"),
            None => println!("(no face)"),
        },
        _ => daemon(),
    }
}

fn daemon() {
    let brain = audio::Brain::from_env();
    let mut vm = make_vm();
    let mut ptt = ptt::Ptt::open();
    if ptt.is_none() {
        eprintln!("voiced: no {} — PTT dead, face only", ptt::PTT_DEV);
    }
    face::write(&vm, false, false);
    eprintln!(
        "voiced: up (local={}, brain={}, ptt={})",
        audio::local_voice_ready(),
        brain.is_some(),
        ptt.as_ref()
            .map(|p| p.devs())
            .unwrap_or_else(|| "none".into())
    );
    // M42e: 预载常驻嘴耳模型（spawn 即返回，���载在子进程里）——第一次
    // 说话不再等 ~4s/侧 的装载。
    if audio::local_voice_ready() {
        audio::warm_local_voice();
    }

    let mut capturing: Option<std::process::Child> = None;
    let mut deadline: Option<Instant> = None;
    // 音量下键按下时刻：短按(<300ms)=音量−10、长按=PTT（M42e 产品面）
    let mut ptt_down: Option<Instant> = None;

    loop {
        // ---- PTT ----
        if let Some(p) = ptt.as_mut() {
            for ev in p.wait(200) {
                match ev {
                    ptt::PttEv::Down => {
                        ptt_down = Some(Instant::now());
                        if capturing.is_none() {
                            match audio::capture_start() {
                                Ok(c) => {
                                    capturing = Some(c);
                                    deadline = None; // 采集中不计时
                                    face::write(&vm, true, false);
                                }
                                Err(e) => eprintln!("voiced: cap start {e}"),
                            }
                        }
                    }
                    ptt::PttEv::Up => {
                        let short_tap = ptt_down
                            .take()
                            .is_some_and(|d| d.elapsed() < Duration::from_millis(300));
                        if short_tap {
                            // 短按=音量−：采集立即弃（无 600ms 词尾冲刷）
                            if let Some(mut c) = capturing.take() {
                                let _ = c.kill();
                                let _ = c.wait();
                            }
                            face::write(&vm, false, false);
                            let v = audio::adjust_vol(-10);
                            eprintln!("voiced: vol {v}");
                            say(&format!("音量{v}"), brain.as_ref());
                            continue;
                        }
                        if let Some(mut c) = capturing.take() {
                            // 词尾冲刷：立即 kill 会截掉最后几百毫秒（snd-cap
                            // 缓冲 + 松手瞬间）。2026-09-04 收据：「连接无线
                            // 网络」只剩 0.96s，「络」被截，ASR 三连空串。
                            std::thread::sleep(Duration::from_millis(600));
                            let _ = c.kill();
                            let _ = c.wait();
                            if let Some(wav) = audio::capture_take() {
                                face::write(&vm, false, true);
                                match hear(&wav, brain.as_ref()) {
                                    Ok(text) => {
                                        eprintln!("voiced: heard {text:?}");
                                        let outs = vm.step(Ev::Heard(text));
                                        run_outs(&mut vm, outs, brain.as_ref());
                                    }
                                    Err(e) => {
                                        eprintln!("voiced: asr {e}");
                                        let outs = vm.step(Ev::Heard("没听懂".into()));
                                        // asr 失败提示本身也要能说——但 asr
                                        // 挂了多半网络不通，TTS 也挂；只刷屏
                                        for o in outs {
                                            if o == Out::Show {
                                                face::write(&vm, false, false);
                                            }
                                        }
                                    }
                                }
                            } else {
                                // 误触（<0.1s）
                                face::write(&vm, false, false);
                            }
                            face::write(&vm, false, false);
                        }
                    }
                    ptt::PttEv::VolUp => {
                        let v = audio::adjust_vol(10);
                        eprintln!("voiced: vol {v}");
                        say(&format!("音量{v}"), brain.as_ref());
                    }
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(200));
        }

        // ---- 超时 ----
        if capturing.is_none() && !matches!(vm.state_name(), "idle") {
            let dl =
                *deadline.get_or_insert_with(|| Instant::now() + Duration::from_secs(TIMEOUT_SECS));
            if Instant::now() >= dl {
                deadline = None;
                let outs = vm.step(Ev::Timeout);
                run_outs(&mut vm, outs, brain.as_ref());
            }
        } else {
            deadline = None;
        }
    }
}

/// 嘴：本地 ag-tts 优先（M42d，离线即产品），失败/缺件落 brain TTS。
fn say(text: &str, brain: Option<&audio::Brain>) {
    if audio::local_voice_ready() {
        match audio::local_speak(text) {
            Ok(()) => return,
            Err(e) => eprintln!("voiced: local tts {e}"),
        }
    }
    if let Some(b) = brain {
        if let Err(e) = b.speak(text) {
            eprintln!("voiced: tts {e}");
        }
    } else {
        eprintln!("voiced: (mute) {text}");
    }
}

/// 耳：本地 ag-asr 优先，失败/缺件落 brain ASR（brain 对本机采集链幻听，
/// 见 audio.rs 法医收据——本地在位时实际不会走到云）。
fn hear(wav: &[u8], brain: Option<&audio::Brain>) -> Result<String, String> {
    if audio::local_voice_ready() {
        match audio::local_asr(wav) {
            Ok(t) => return Ok(t),
            Err(e) => eprintln!("voiced: local asr {e}"),
        }
    }
    match brain {
        Some(b) => b.asr(wav),
        None => Err("no asr backend".into()),
    }
}

/// 落地状态机输出。拉式语音：Say 只上脸（行已在 vm.lines 里，末尾统一
/// face::write），Speak 才走 TTS；Act → 执行并把结果喂回状态机。
fn run_outs(vm: &mut Vm, outs: Vec<Out>, brain: Option<&audio::Brain>) {
    let mut followups: Vec<Ev> = Vec::new();
    for o in outs {
        match o {
            Out::Say(_) => {}
            Out::Speak(s) => {
                face::write(vm, false, true);
                say(&s, brain);
            }
            Out::Show => {}
            Out::Act(a) => match a {
                Act::Scan => {
                    face::write(vm, false, true);
                    match scan_ssids() {
                        Ok(list) => followups.push(Ev::ScanDone(list)),
                        Err(e) => {
                            eprintln!("voiced: scan {e}");
                            followups.push(Ev::ScanDone(Vec::new()));
                        }
                    }
                }
                Act::Join { ssid, psk } => {
                    face::write(vm, false, true);
                    followups.push(Ev::JoinDone(join_wifi(&ssid, &psk)));
                }
                Act::QrScan => {
                    face::write(vm, false, true);
                    let r = scan_qr();
                    if let Err(e) = &r {
                        eprintln!("voiced: qr {e}");
                    }
                    followups.push(Ev::QrDone(r));
                }
                Act::Ocr => {
                    face::write(vm, false, true);
                    let r = read_text();
                    if let Err(e) = &r {
                        eprintln!("voiced: ocr {e}");
                    }
                    followups.push(Ev::OcrDone(r));
                }
                Act::Status => {
                    let o = vm.inject_say(&status_text());
                    if let Out::Say(s) = o {
                        say(&s, brain);
                    }
                }
                Act::Chat(text) => {
                    face::write(vm, false, true);
                    let reply = match chat_front(&text) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("voiced: front {e}");
                            "现在连不上母体。固定说法还在：连接无线网络，或扫码，或念一下。"
                                .to_string()
                        }
                    };
                    // 拉式：回复上脸不出声，点名（你说给我听）才 Speak
                    let _ = vm.inject_say(&reply);
                }
            },
        }
    }
    face::write(vm, false, false);
    for ev in followups {
        let outs = vm.step(ev);
        run_outs(vm, outs, brain);
    }
}

// ---------------- 执行件 ----------------

/// 自由文本 → 母体/新前台（N2②）。spawn VOICED_FRONT 的路由器
/// （`aginx agent send`——不带名字=住当前光标），成功 stdout 就是回复
/// 文本；挂死有预算（wait_limited kill）。AGINX_SOCK 由环境继承。
fn chat_front(text: &str) -> Result<String, String> {
    let bin = front_bin().ok_or_else(|| "VOICED_FRONT not set".to_string())?;
    let mut child = Command::new(&bin)
        .args(["agent", "send", text])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn {bin}: {e}"))?;
    audio::wait_limited(&mut child, FRONT_BUDGET_SECS)
        .map_err(|e| format!("front {e}"))?;
    let out = child.wait().map_err(|e| e.to_string())?;
    let mut stdout = String::new();
    if let Some(mut r) = child.stdout.take() {
        use std::io::Read;
        let _ = r.read_to_string(&mut stdout);
    }
    let reply = stdout.trim().to_string();
    if !out.success() {
        return Err(format!("exit {}", out.code().unwrap_or(-1)));
    }
    if reply.is_empty() {
        return Err("empty reply".into());
    }
    Ok(reply)
}

/// nlscan wlan0 → 去重（保信号最强）、滤 hidden、按信号排序，cap 10（序数上限）。
fn scan_ssids() -> Result<Vec<String>, String> {
    let out = Command::new("/bin/nlscan")
        .arg("wlan0")
        .output()
        .map_err(|e| format!("spawn: {e}"))?;
    let txt = String::from_utf8_lossy(&out.stdout);
    // 行形状: "<mac>  ch=<n>  -68.00  dBm  <ssid 可能 \xNN 转义>" — dBm 是
    // 独立 token，SSID 从它之后开始（2026-09-04 设备定格；此前把 dBm 当
    // SSID 前缀，列表全是 "dBm xxx"）。
    let mut seen: Vec<(String, f32)> = Vec::new();
    for line in txt.lines() {
        let line = line.trim_end();
        let toks: Vec<&str> = line.split_whitespace().collect();
        // mac, ch=, dbm 数值, "dBm", ssid...（ssid 可含空格，join 回去）
        let dbm = match toks
            .get(2)
            .and_then(|d| d.trim_end_matches("dBm").parse::<f32>().ok())
        {
            Some(v) => v,
            None => continue,
        };
        let ssid_start = if toks.get(3) == Some(&"dBm") { 4 } else { 3 };
        let ssid_esc: String = toks
            .iter()
            .skip(ssid_start)
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        if ssid_esc.is_empty() || ssid_esc.contains("<hidden>") {
            continue;
        }
        let ssid = unescape_hex(&ssid_esc);
        // 邻居 AP 会有二进制 SSID（\x04\x00…）——念不出来也画不出来，滤掉
        if ssid.is_empty() || ssid.chars().any(|c| c.is_control()) {
            continue;
        }
        if let Some(slot) = seen.iter_mut().find(|(s, _)| *s == ssid) {
            if dbm > slot.1 {
                slot.1 = dbm;
            }
        } else {
            seen.push((ssid, dbm));
        }
    }
    seen.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    Ok(seen.into_iter().take(10).map(|(s, _)| s).collect())
}

/// busybox 输出把非 ASCII 打成 \xe5\x87 字样；解回 UTF-8。
fn unescape_hex(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if i + 3 < bytes.len() + 1 && bytes[i] == b'\\' && bytes[i + 1] == b'x' {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 2]), hex(bytes[i + 3])) {
                out.push((h * 16 + l) as u8);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 拍照解 QR（M42b 眼分支）。尝试阶梯：默认曝光 ×3 → 慢模式+增益兜底。
///
/// 2026-09-04 设备收据定形：冷启动后头几次 cam-shot 调用整段是废片
/// （IOMMU/流会话热身——sweep 10 连拍第 3 发才中），同一轮内 --frames 3
/// 只是帧内曝光收敛，救不了会话级废片，所以要**多次调用**而不是多帧；
/// 慢门+gain8 档三连败（暗/糊），只配末位。每轮独立留档（voiced-qrN.jpg），
/// 收据可逐轮复盘。cam-shot 挂死有预算（wait_limited kill）。
const QR_BUDGET_SECS: u32 = 15;

fn scan_qr() -> Result<Vec<String>, String> {
    let mut last_err = String::new();
    // (轮次从 1 计) — 第 4 轮才是慢门兜底
    for round in 1..=4u32 {
        let t0 = Instant::now();
        let qr_jpg = format!("/tmp/voiced-qr{round}.jpg");
        let mut cmd = Command::new("/bin/cam-shot");
        cmd.args(["--stream", "--rear", "--frames", "3", "--jpeg-gray"])
            .arg("--jpeg-out")
            .arg(&qr_jpg)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if round == 4 {
            // 末位兜底：imx363 慢模式 #2610（fll 2488 更长积分）+ 模拟增益
            // ——黑底白码贴纸、夜间的最后一搏
            cmd.args(["--slowrear", "--gain", "8"]);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cam-shot spawn: {e}"))?;
        if let Err(e) = audio::wait_limited(&mut child, QR_BUDGET_SECS) {
            last_err = format!("cam-shot {e}");
            continue; // 挂死被 kill——按失败重试
        }
        if !child.wait().map(|s| s.success()).unwrap_or(false) {
            // 冷加载 IOMMU 间歇性失败是已知收据（M19c/M19b），直接重试
            last_err = "cam-shot rc!=0".into();
            continue;
        }
        // 解码（agqr 进程，payload 一行一个）。output() 不带超时——解码
        // 是 <300ms 量级的纯计算，等待预算都在拍照那侧
        let dec = Command::new("/usr/bin/agqr").arg(&qr_jpg).output();
        match dec {
            Ok(out) if out.status.success() => {
                let payloads = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if !payloads.is_empty() {
                    eprintln!(
                        "voiced: qr round {round}, {:.1}s",
                        t0.elapsed().as_secs_f32()
                    );
                    return Ok(payloads);
                }
                last_err = "没找到二维码".into();
            }
            Ok(_) => last_err = "agqr rc!=0".into(), // exit 1 = 没码，也重试
            Err(e) => last_err = format!("agqr spawn: {e}"),
        }
    }
    Err(last_err)
}

/// 拍照念字（M45 眼分支）。同 QR 的冷启动废片收据：默认曝光两轮，末轮
/// gain 提亮（暗房实测定形：默认曝光 det 颗粒无收，gain16+dgain2 出 4 框）。
/// ag-ocr 自带 auto 旋转——竖握手机拍横排文字是产品常态（传感器横向安装）。
/// 识别 ~3-6s（auto 两轮 det + rec），预算在拍照和识别两侧都给足。
const OCR_BUDGET_SECS: u32 = 20;

fn read_text() -> Result<Vec<String>, String> {
    use std::io::Read;
    let mut last_err = String::new();
    for round in 1..=3u32 {
        let jpg = format!("/tmp/voiced-ocr{round}.jpg");
        let mut cmd = Command::new("/bin/cam-shot");
        cmd.args(["--stream", "--rear", "--frames", "3", "--jpeg"])
            .arg("--jpeg-out")
            .arg(&jpg)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if round == 3 {
            // 末位兜底：满增益提亮（M45 暗房收据，det 0 框→4 框的档位）
            cmd.args(["--gain", "16", "--dgain", "2"]);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cam-shot spawn: {e}"))?;
        if let Err(e) = audio::wait_limited(&mut child, OCR_BUDGET_SECS) {
            last_err = format!("cam-shot {e}");
            continue; // 挂死被 kill——按失败重试
        }
        if !child.wait().map(|s| s.success()).unwrap_or(false) {
            last_err = "cam-shot rc!=0".into();
            continue;
        }
        // ag-ocr：stdout 每行 "text\tconf"，exit 0=有字 / 1=没字 / 2=错误。
        // 识别要秒级（agqr 的 <300ms 先例不适用），piped + wait_limited 给预算。
        let mut child = match Command::new("/var/bin/ag-ocr")
            .arg(&jpg)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return Err(format!("ag-ocr spawn: {e}")), // 装机缺失不重试
        };
        let mut buf = String::new();
        if let Some(mut so) = child.stdout.take() {
            let _ = so.read_to_string(&mut buf); // 输出 <64KB 管道缓冲，不会死锁
        }
        if let Err(e) = audio::wait_limited(&mut child, OCR_BUDGET_SECS) {
            last_err = format!("ag-ocr {e}");
            continue;
        }
        match child.wait().map(|s| s.code()).unwrap_or(None) {
            Some(0) => {
                let lines: Vec<String> = buf
                    .lines()
                    .map(|l| l.split('\t').next().unwrap_or("").to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                if !lines.is_empty() {
                    eprintln!("voiced: ocr round {round}, {} 行", lines.len());
                    return Ok(lines);
                }
                last_err = "没识别到文字".into();
            }
            Some(1) => last_err = "没识别到文字".into(),
            _ => last_err = "ag-ocr rc=2".into(),
        }
    }
    Err(last_err)
}

/// wifi-join wlan0 ssid psk，然后读 wlan0 的 IPv4。
fn join_wifi(ssid: &str, psk: &str) -> Result<String, String> {
    let mut child = Command::new("/bin/wifi-join")
        .args(["wlan0", ssid, psk])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    audio::wait_limited(&mut child, JOIN_BUDGET_SECS).map_err(|e| format!("wifi-join {e}"))?;
    // dhcp 在 wifi-join 里；地址落不落直接看
    for _ in 0..10 {
        if let Ok(out) = Command::new("ip")
            .args(["-4", "addr", "show", "wlan0"])
            .output()
        {
            let txt = String::from_utf8_lossy(&out.stdout);
            for line in txt.lines() {
                let line = line.trim();
                if let Some(rest) = line.strip_prefix("inet ") {
                    if let Some(ip) = rest.split_whitespace().next() {
                        if ip != "127.0.0.1" {
                            return Ok(ip.to_string());
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("没拿到地址".into())
}

/// 状态一句话：时间 + 电池 + 网络。
fn status_text() -> String {
    let time = Command::new("date")
        .arg("+%H点%M分")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| {
            // 去前导零（"06点05分"→"6点5分"）——TTS 会把 0 也念出来
            s.trim().trim_start_matches('0').replace("点0", "点")
        })
        .unwrap_or_default();
    let bat = std::fs::read_to_string("/sys/class/power_supply/battery/capacity")
        .ok()
        .and_then(|s| s.trim().parse::<u8>().ok())
        .unwrap_or(0);
    // 只报连没连——IP 逐位念出来又长又难听（数字展开还多 10s 合成+播放）
    let net = Command::new("ip")
        .args(["-4", "addr", "show", "wlan0"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|t| t.lines().any(|l| l.trim().starts_with("inet ")))
        .unwrap_or(false);
    let net = if net { "网已连" } else { "没联网" };
    format!("{time}，电池{bat}%，{net}。")
}
