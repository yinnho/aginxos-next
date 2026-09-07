//! aginx-voice — 语音对话守护（M42a，产品定义 2026-09-05：手机即智能体）。
//!
//! 产品面唯一的输入是语音（PTT=按住音量下键）和眼（M42b/M42g），输出是脸
//! （/run/aginx-voice/face → aterm 渲染）和嘴（TTS）。**拉式语音**（2026-09-04
//! 用户收据：识别和回应都是毫秒级，唯独嘴是慢车道）——回应默认只上脸，
//! 用户点名（「你说给我听」「念一下」「再念一遍」）才出声。协议是**命令
//! 优先**的封闭词表状态机（protocol.rs，2026-09-06 重设计：无驻留态，机器
//! 缺什么自己补什么——连网先查现状再试记忆，最后才睁眼等码），没有 LLM
//! ——WiFi 必须在 LLM 可用之前连得上。
//!
//! **一眼自举（M42c）**：AGINXPAIR1 配对码是 WiFi+身份的超集，眼取景命中
//! 即 Act::PairApply——连网 + 身份三件进 /etc/aginx/env + 快速校时 + 拉起
//! 母体两单元，不回读不确认。秘密只在 env 文件里，永不上脸/日志。
//!
//! **前台模式（N2②）**：env `VOICED_FRONT=<aginx 路由器路径>` 时自由文本
//! （封闭词表 miss）改投新前台——`aginx agent send`（母体/化身光标，
//! AGINX_SOCK 决定找哪台 server）。封闭词表仍本地优先（离线地板），
//! 前台不可达落回地板话。不设此 env = 老行为分毫不动。
//!
//! 调试面（收据阶梯，从嘴/耳单器官到全环）：
//!   aginx-voice --say "文本"          只测嘴（TTS→扬声器）
//!   aginx-voice --hear <wav文件>      只测耳（WAV→ASR→打印文本）
//!   aginx-voice --inject "文本"       喂状态机走全流程（不出声，Act 真执行）
//!   aginx-voice --face                打印当前屏面 JSON
//!
//! 没有嘴耳同开的回环自检：M18 的硬件收据写明 MM1 边放边采会把放音叠
//! 进采集（数字回环是失真副本，880Hz 可验、语音不可认，2026-09-04 实测
//! ASR 出"うん、うん"）——产品路径本来也是顺序的：PTT 采完才 TTS。

mod audio;
mod face;
mod protocol;
mod ptt;

use protocol::{Act, Ev, NetState, Out, Vm};
use std::process::Command;
use std::time::{Duration, Instant};

const JOIN_BUDGET_SECS: u32 = 90;
/// 母体一轮（真 brain，含工具往返）的等待预算——超了杀掉落地板话。
const FRONT_BUDGET_SECS: u32 = 90;
/// 眼取景总时长上限：超时闭眼（人对准之前机器不催，但也不能永远开着镜头）。
const EYE_VIEW_SECS: u64 = 30;
/// M47⑤ 取景子进程重生预算：rc≠0（含 rc=3 连续 fence 超时）/ 卡帧 / 崩溃
/// 都杀掉重生，连败这么多次就闭眼报失败。
const EYE_RETRIES: u8 = 3;
/// 取景帧卡死判据：mtime 这么久不更新（首帧未落 = 子进程启动后这久还没
/// 文件）就杀掉重生。
const EYE_STUCK_SECS: u64 = 5;

/// M47⑤ 眼取景常驻：一个 --forever cam-shot 子进程 + mtime 轮询。子进程
/// 自己原子发布 eye.jpg（tmp+rename），voice 不再逐帧起停相机——双会话
/// 撞 sensor 必翻车，相机就这一个持有者。
struct EyeView {
    child: std::process::Child,
    since: Instant,
    /// 最近一次看到的 eye.jpg mtime（None=还没见过帧）
    mtime: Option<std::time::SystemTime>,
    /// 上次 mtime 变化（或 spawn）时刻——卡帧自愈的基准
    mtime_seen: Instant,
    /// 上次发起 aginx-qr 解码时刻——解码限频（一次 100-300ms，逐帧跑把
    /// loop 吃满还抢 cam-shot 编码 CPU；2Hz 对人对准足够）
    last_qr: Instant,
    retries: u8,
}

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
            let text = args.get(2).expect("usage: aginx-voice --say <text>");
            let brain = audio::Brain::from_env();
            say(text, brain.as_ref());
        }
        Some("--hear") => {
            let path = args.get(2).expect("usage: aginx-voice --hear <wav>");
            let brain = audio::Brain::from_env();
            let wav = std::fs::read(path).expect("read wav");
            let text = hear(&wav, brain.as_ref()).expect("asr failed");
            println!("{text}");
        }
        Some("--inject") => {
            let text = args.get(2).expect("usage: aginx-voice --inject <text>").clone();
            let mut vm = make_vm();
            face::write(&vm, false, false, false);
            let outs = vm.step(Ev::Heard(text));
            run_outs(&mut vm, outs, None, &mut None);
        }
        Some("--script") => {
            // 收据阶梯：stdin 每行一条 Heard，同一个 Vm 跨步保持（--inject
            // 一次一进程，扫码→确认这种多步流跑不完整）。run_outs 在步间阻塞
            // ——相机/TTS 落完才读下一行，喂两行也能按序走完。
            let brain = audio::Brain::from_env();
            let mut vm = make_vm();
            face::write(&vm, false, false, false);
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
                        eprintln!("aginx-voice: script {t:?}");
                        let outs = vm.step(Ev::Heard(t.to_string()));
                        run_outs(&mut vm, outs, brain.as_ref(), &mut None);
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

/// 眼取景一轮的退出决定（M47⑤）：命中 or 闭眼（带给人一句话）。
enum EyeExit {
    Hit(Vec<String>),
    GiveUp(&'static str),
}

/// 杀掉旧子进程并在原 EyeView 里重生（retries+1、mtime 清零）。返回 Err
/// = spawn 失败——下一轮 try_wait 会再走重生/放弃路径，不用在这里叠状态。
fn eye_respawn(ev: &mut EyeView) -> Result<(), String> {
    eye_stop(&mut ev.child);
    ev.retries += 1;
    ev.mtime = None;
    ev.mtime_seen = Instant::now();
    ev.last_qr = Instant::now();
    ev.child = eye_spawn()?;
    Ok(())
}

fn daemon() {
    let brain = audio::Brain::from_env();
    let mut vm = make_vm();
    let mut ptt = ptt::Ptt::open();
    if ptt.is_none() {
        eprintln!("aginx-voice: no {} — PTT dead, face only", ptt::PTT_DEV);
    }
    face::write(&vm, false, false, false);
    eprintln!(
        "aginx-voice: up (local={}, brain={}, ptt={})",
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
    // 音量下键按下时刻：短按(<300ms)=音量−10、长按=PTT（M42e 产品面）
    let mut ptt_down: Option<Instant> = None;
    // 眼取景（M42g→M47⑤）：Some = 取景中（常驻子进程 + 命中轮询）。音量+
    // 开/再按关，音量下关。帧由子进程逐张原子发布，这里只轮询 mtime 解码
    // ——命中 WIFI: 码直接连（扫码即指令，拉式——机器开着取景等的就是
    // 这个格式）。
    let mut eye: Option<EyeView> = None;

    boot_sequence(&mut vm, brain.as_ref(), &mut eye);

    loop {
        // ---- PTT ----
        if let Some(p) = ptt.as_mut() {
            for ev in p.wait(200) {
                match ev {
                    ptt::PttEv::Down => {
                        // 取景中音量下 = 闭眼（吞掉本次按压周期：不开采集，
                        // 后续 Up 因 ptt_down 为空自然空走）
                        let had_eye = eye.is_some();
                        eye_shut(&mut vm, &mut eye);
                        if had_eye {
                            face::write(&vm, false, false, false);
                            continue;
                        }
                        ptt_down = Some(Instant::now());
                        if capturing.is_none() {
                            match audio::capture_start() {
                                Ok(c) => {
                                    capturing = Some(c);
                                    face::write(&vm, true, false, false);
                                }
                                Err(e) => eprintln!("aginx-voice: cap start {e}"),
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
                            face::write(&vm, false, false, false);
                            let v = audio::adjust_vol(-10);
                            eprintln!("aginx-voice: vol {v}");
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
                                face::write(&vm, false, true, false);
                                match hear(&wav, brain.as_ref()) {
                                    Ok(text) => {
                                        eprintln!("aginx-voice: heard {text:?}");
                                        let outs = vm.step(Ev::Heard(text));
                                        run_outs(&mut vm, outs, brain.as_ref(), &mut eye);
                                    }
                                    Err(e) => {
                                        eprintln!("aginx-voice: asr {e}");
                                        let outs = vm.step(Ev::Heard("没听懂".into()));
                                        // asr 失败提示本身也要能说——但 asr
                                        // 挂了多半网络不通，TTS 也挂；只刷屏
                                        for o in outs {
                                            if o == Out::Show {
                                                face::write(&vm, false, false, false);
                                            }
                                        }
                                    }
                                }
                            } else {
                                // 误触（<0.1s）
                                face::write(&vm, false, false, false);
                            }
                            face::write(&vm, false, false, false);
                        }
                    }
                    ptt::PttEv::VolUp => {
                        // M42g：音量+ = 眼开关（音量+10 的老义退役；一屏一键
                        // 一义，加音量走语音）。协议的 Act::Eye/EyeClose 走
                        // 同两个助手——键是手动挡，机器会自己睁眼/闭眼。
                        if eye.is_some() {
                            eye_shut(&mut vm, &mut eye);
                        } else {
                            eye_start(&mut vm, &mut eye);
                        }
                        face::write(&vm, false, false, eye.is_some());
                    }
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(200));
        }

        // ---- 眼取景（M47⑤：常驻子进程 + 命中轮询）----
        let mut eye_exit: Option<EyeExit> = None;
        if let Some(ev) = eye.as_mut() {
            if capturing.is_some() {
                // 罕见赛跑：PTT 采集优先，帧轮询这轮让路（子进程继续跑）
            } else if ev.since.elapsed() >= Duration::from_secs(EYE_VIEW_SECS) {
                eye_exit = Some(EyeExit::GiveUp("没拍到码，再按音量上重试。"));
            } else {
                // 子进程死掉（rc=3 连续 fence 超时 / 崩溃）→ 重生 ≤EYE_RETRIES
                match ev.child.try_wait() {
                    Ok(Some(st)) => {
                        eprintln!("aginx-voice: eye cam-shot exit {}", st.code().unwrap_or(-1));
                        if ev.retries >= EYE_RETRIES {
                            eye_exit = Some(EyeExit::GiveUp("相机反复掉线，取景关闭。"));
                        } else if eye_respawn(ev).is_err() {
                            eye_exit = Some(EyeExit::GiveUp("相机没起来，取景关闭。"));
                        }
                    }
                    Err(e) => {
                        eprintln!("aginx-voice: eye wait {e}");
                        eye_exit = Some(EyeExit::GiveUp("相机掉线，取景关闭。"));
                    }
                    Ok(None) => {
                        // 帧轮询：mtime 变 → 解码；停滞 → 卡帧自愈重生
                        let mtime = std::fs::metadata(face::EYE_JPG)
                            .and_then(|m| m.modified())
                            .ok();
                        match mtime {
                            Some(t) if Some(t) != ev.mtime => {
                                ev.mtime = Some(t);
                                ev.mtime_seen = Instant::now();
                                // 解码限频：帧 ~8fps 全要解的话 aginx-qr
                                // （100-300ms/次）吃满整个 loop 还���编码
                                // CPU——2Hz 足够对准
                                if ev.last_qr.elapsed() >= Duration::from_millis(400) {
                                    ev.last_qr = Instant::now();
                                    if let Some(payloads) = eye_decode_qr() {
                                        eye_exit = Some(EyeExit::Hit(payloads));
                                    }
                                }
                            }
                            _ => {
                                // mtime 没动（或首帧未落）超时 → 杀重生。首帧
                                // 预算放宽一倍（子进程冷启动 + 前 3 帧只统计）
                                let stuck = if ev.mtime.is_none() {
                                    2 * EYE_STUCK_SECS
                                } else {
                                    EYE_STUCK_SECS
                                };
                                if ev.mtime_seen.elapsed() >= Duration::from_secs(stuck) {
                                    if ev.retries >= EYE_RETRIES {
                                        eye_exit = Some(EyeExit::GiveUp("取景卡住了，取景关闭。"));
                                    } else {
                                        eprintln!("aginx-voice: eye stuck frame, respawn");
                                        let _ = eye_respawn(ev);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Some(exit) = eye_exit {
            if let Some(mut ev) = eye.take() {
                eye_stop(&mut ev.child);
            }
            face::write(&vm, false, false, false);
            match exit {
                EyeExit::Hit(payloads) => {
                    // 命中即自动走：配对码（超集，PairApply）/ WIFI: 码直连 /
                    // 文本码念前 40 字——拉式，码到手就用
                    let outs = vm.step(Ev::QrDone(Ok(payloads)));
                    run_outs(&mut vm, outs, brain.as_ref(), &mut eye);
                }
                EyeExit::GiveUp(msg) => {
                    let _ = vm.inject_say(msg);
                }
            }
        }
    }
}

/// net-bringup 判词（/run/boot.state: "wifi ok|fail [ssid]"）。voice 起来
/// 时 bring-up 常还在路上——先等它的判词（有界 20s），没有再自己查
/// （net_check 会补一次 join，与已放弃的 bring-up 不再竞争）。
fn boot_net_state() -> NetState {
    for _ in 0..20 {
        if let Ok(s) = std::fs::read_to_string("/run/boot.state") {
            let mut decided = false;
            for line in s.lines() {
                if line.starts_with("wifi ok") {
                    return NetState::Up;
                }
                if line.starts_with("wifi fail") {
                    decided = true;
                    break;
                }
            }
            if decided {
                break;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    net_check()
}

/// 开机序列：接线员分流。已连：铃 → "Operator. Go ahead."。未连：英文
/// 警告 → 状态机接 NetState——静默脸行 + 自动睁眼走 M42c 现成链（协议
/// 行是 Out::Say 不出声，无语言冲突）。
fn boot_sequence(vm: &mut Vm, brain: Option<&audio::Brain>, eye: &mut Option<EyeView>) {
    let ns = boot_net_state();
    if matches!(ns, NetState::Up) {
        if let Err(e) = audio::play_ring() {
            eprintln!("aginx-voice: ring {e}");
        }
        let _ = vm.inject_say("Operator. Go ahead.");
        face::write(vm, false, false, false);
        say("Operator. Go ahead.", brain);
        return;
    }
    let _ = vm.inject_say("Warning: I've lost the hardline.");
    say("Warning: I've lost the hardline.", brain);
    let outs = vm.step(Ev::NetState(ns));
    run_outs(vm, outs, brain, eye);
}

/// 嘴：本地 aginx-tts 优先（M42d，离线即产品），失败/缺件落 brain TTS。
fn say(text: &str, brain: Option<&audio::Brain>) {
    if audio::local_voice_ready() {
        match audio::local_speak(text) {
            Ok(()) => return,
            Err(e) => eprintln!("aginx-voice: local tts {e}"),
        }
    }
    if let Some(b) = brain {
        if let Err(e) = b.speak(text) {
            eprintln!("aginx-voice: tts {e}");
        }
    } else {
        eprintln!("aginx-voice: (mute) {text}");
    }
}

/// 耳：本地 aginx-asr 优先，失败/缺件落 brain ASR（brain 对本机采集链幻听，
/// 见 audio.rs 法医收据——本地在位时实际不会走到云）。
fn hear(wav: &[u8], brain: Option<&audio::Brain>) -> Result<String, String> {
    if audio::local_voice_ready() {
        match audio::local_asr(wav) {
            Ok(t) => return Ok(t),
            Err(e) => eprintln!("aginx-voice: local asr {e}"),
        }
    }
    match brain {
        Some(b) => b.asr(wav),
        None => Err("no asr backend".into()),
    }
}

/// 落地状态机输出。拉式语音：Say 只上脸（行已在 vm.lines 里，末尾统一
/// face::write），Speak 才走 TTS；Act → 执行并把结果喂回状态机。eye：
/// 眼取景所有权借用——Act::Eye/EyeClose 由协议出生（缺网自动睁眼、取消
/// 自动闭眼），VolUp/音量下走同两个助手。
fn run_outs(
    vm: &mut Vm,
    outs: Vec<Out>,
    brain: Option<&audio::Brain>,
    eye: &mut Option<EyeView>,
) {
    let mut followups: Vec<Ev> = Vec::new();
    for o in outs {
        match o {
            Out::Say(_) => {}
            Out::Speak(s) => {
                face::write(vm, false, true, eye.is_some());
                say(&s, brain);
            }
            Out::Show => {}
            Out::Act(a) => match a {
                Act::NetConnect => {
                    // 机器干活：先看现状，再试记忆里的网，都不行才睁眼要码
                    face::write(vm, false, true, eye.is_some());
                    followups.push(Ev::NetState(net_check()));
                }
                Act::Join { ssid, psk } => {
                    face::write(vm, false, true, eye.is_some());
                    followups.push(Ev::JoinDone(join_wifi(&ssid, &psk)));
                }
                Act::PairApply { bundle } => {
                    face::write(vm, false, true, eye.is_some());
                    followups.push(Ev::PairDone(pair_apply(&bundle)));
                }
                Act::Eye => eye_start(vm, eye),
                Act::EyeClose => eye_shut(vm, eye),
                Act::QrScan => {
                    if eye.is_some() {
                        // M47⑤ 互斥门：取景轮询已在逐帧解，等命中即可——
                        // 这里另起 cam-shot 会撞 sensor
                        let _ = vm.inject_say("取景开着，对准码就行。");
                        continue;
                    }
                    face::write(vm, false, true, false);
                    let r = scan_qr();
                    if let Err(e) = &r {
                        eprintln!("aginx-voice: qr {e}");
                    }
                    followups.push(Ev::QrDone(r));
                }
                Act::Ocr => {
                    face::write(vm, false, true, eye.is_some());
                    let r = read_text();
                    if let Err(e) = &r {
                        eprintln!("aginx-voice: ocr {e}");
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
                    face::write(vm, false, true, eye.is_some());
                    let reply = match chat_front(&text) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("aginx-voice: front {e}");
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
    face::write(vm, false, false, eye.is_some());
    for ev in followups {
        let outs = vm.step(ev);
        run_outs(vm, outs, brain, eye);
    }
}

/// 开眼（VolUp 与协议 Act::Eye 同一条路）。已开=守门话不双开——双会话
/// 撞 sensor 必翻车，相机就这一个持有者。
fn eye_start(vm: &mut Vm, eye: &mut Option<EyeView>) {
    if eye.is_some() {
        let _ = vm.inject_say("取景开着，对准码。");
        return;
    }
    match eye_spawn() {
        Ok(child) => {
            *eye = Some(EyeView {
                child,
                since: Instant::now(),
                mtime: None,
                mtime_seen: Instant::now(),
                last_qr: Instant::now(),
                retries: 0,
            });
            let _ = vm.inject_say("取景中，对准码。");
        }
        Err(e) => {
            eprintln!("aginx-voice: eye spawn {e}");
            let _ = vm.inject_say("相机没起来，再按一次重试。");
        }
    }
}

/// 闭眼（VolUp 再按、音量下、协议 Act::EyeClose=取消的落地点）。没开=空转。
fn eye_shut(vm: &mut Vm, eye: &mut Option<EyeView>) {
    if let Some(mut ev) = eye.take() {
        eye_stop(&mut ev.child);
        let _ = vm.inject_say("取景已关。");
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

/// Act::NetConnect 判定（命令优先 2026-09-06）：有 IP=Up；无 IP 先试
/// /etc/wifi.conf 记忆（net-bringup 同形 KEY=VALUE），连上=Up、连不上=
/// ConfFail；没记录=NoConf。ConfFail/NoConf 由协议接 Act::Eye——机器自
/// 己把下一步走完，人只管对准码。
fn net_check() -> NetState {
    if wlan0_ip().is_some() {
        return NetState::Up;
    }
    match read_wifi_conf() {
        Some((ssid, psk)) => match join_wifi(&ssid, &psk) {
            Ok(_) => NetState::Up,
            Err(e) => {
                eprintln!("aginx-voice: conf join {ssid}: {e}");
                NetState::ConfFail
            }
        },
        None => NetState::NoConf,
    }
}

/// wlan0 的 IPv4（回环除外）。None = 没网。
fn wlan0_ip() -> Option<String> {
    let out = Command::new("ip")
        .args(["-4", "addr", "show", "wlan0"])
        .output()
        .ok()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        if let Some(rest) = line.trim().strip_prefix("inet ") {
            if let Some(ip) = rest.split_whitespace().next() {
                if ip != "127.0.0.1" {
                    return Some(ip.to_string());
                }
            }
        }
    }
    None
}

/// /etc/wifi.conf 读取（net-bringup 同形：ssid=/psk=，容忍 CR）。None =
/// 没有身份记录。
fn read_wifi_conf() -> Option<(String, String)> {
    let txt = std::fs::read_to_string("/etc/wifi.conf").ok()?;
    let mut ssid = None;
    let mut psk = String::new();
    for line in txt.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(v) = line.strip_prefix("ssid=") {
            ssid = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("psk=") {
            psk = v.to_string();
        }
    }
    Some((ssid?, psk))
}

/// Act::PairApply（M42c 一眼自举的机器侧全流程）：连网 → 身份三件进
/// /etc/aginx/env → 快速校时 → 拉起母体两单元。结果全部进报告话。秘密
/// 只进 env 文件（0600），永不出现在日志/脸/报告。svc 每次 spawn 重读
/// env_file，restart 即生效；failed（熔断）单元 restart 同样能救（M42e
/// 收据）。**不重启 aginx-voice**——重启=自杀，本地语音离线路径不依赖
/// env，下次 boot 自然带上。
fn pair_apply(bundle: &aginx_qr::PairBundle) -> Result<String, String> {
    join_wifi(&bundle.ssid, &bundle.psk)?;
    write_env_keys(&[
        ("AGINXBRAIN_API_KEY", &bundle.brain_key),
        ("AGINX_GATEWAY_ID", &bundle.gateway_id),
        ("AGINX_RELAY_SECRET", &bundle.relay_secret),
    ])?;
    let clock_ok = quick_clock();
    let up = svc_ready_after_restart("aginx-gateway") && svc_ready_after_restart("aginx-server");
    let mut msg = String::from("网已连");
    if !clock_ok {
        msg.push_str("，时钟没同步");
    }
    msg.push_str(if up { "，母体在线" } else { "，母体没起来" });
    Ok(msg)
}

/// 身份键并入 /etc/aginx/env（KEY=VALUE、# 注释——svc spawn 重读的同一
/// 形状）。保留既有行（HOME 等），同名键原地替换，缺的尾部追加。0600
/// tmp+rename（persist_wifi 同法）。
fn write_env_keys(kvs: &[(&str, &str)]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let existing = std::fs::read_to_string("/etc/aginx/env").unwrap_or_default();
    let has_key = |k: &str| {
        existing
            .lines()
            .any(|l| l.split_once('=').map(|(ek, _)| ek == k).unwrap_or(false))
    };
    let mut out = String::new();
    for line in existing.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        match line.split_once('=') {
            Some((k, _)) if kvs.iter().any(|(nk, _)| *nk == k) => {
                let v = &kvs.iter().find(|(nk, _)| *nk == k).unwrap().1;
                out.push_str(&format!("{k}={v}\n"));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for (k, v) in kvs {
        if !has_key(k) {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    let tmp = "/etc/aginx/env.tmp";
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp)
        .and_then(|mut f| f.write_all(out.as_bytes()))
        .map_err(|e| format!("env write: {e}"))?;
    std::fs::rename(tmp, "/etc/aginx/env").map_err(|e| format!("env rename: {e}"))
}

/// 快速校时（net-bringup:111-136 律的短版）：TLS 验证书要近似正确的钟，
/// 不然 gateway 连 relay 全被拒。两次×10s 交替双 NTP，`date +%Y≥2026`
/// 判定；失败不致命——进报告话，net-watch/下次 bringup 会补。
fn quick_clock() -> bool {
    for server in ["ntp.aliyun.com", "cn.pool.ntp.org"] {
        if let Ok(mut child) = Command::new("ntpd")
            .args(["-q", "-n", "-p", server])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            let _ = audio::wait_limited(&mut child, 10);
            let _ = child.wait();
        }
        let ok = Command::new("date")
            .arg("+%Y")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| s.trim().parse::<i32>().ok())
            .map(|y| y >= 2026)
            .unwrap_or(false);
        if ok {
            return true;
        }
    }
    false
}

/// restart 一个 unit 并回查到 ready（simple 型 spawn 即 ready；熔断
/// failed 单元 restart 照样救活）。restart 失败或 10s 内仍 failed = false。
fn svc_ready_after_restart(unit: &str) -> bool {
    let st = Command::new("/usr/bin/aginx-svc")
        .args(["restart", unit])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if !st.map(|s| s.success()).unwrap_or(false) {
        eprintln!("aginx-voice: svc restart {unit} failed");
        return false;
    }
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(o) = Command::new("/usr/bin/aginx-svc")
            .args(["status", unit])
            .output()
        {
            let txt = String::from_utf8_lossy(&o.stdout);
            if txt.lines().any(|l| l.trim() == "state   ready") {
                return true;
            }
            if txt.lines().any(|l| l.trim() == "state   failed") {
                return false;
            }
        }
    }
    false
}

/// 拍照解 QR（M42b 眼分支）。尝试阶梯：默认曝光 ×3 → 慢模式+增益兜底。
///
/// 2026-09-04 设备收据定形：冷启动后头几次 cam-shot 调用整段是废片
/// （IOMMU/流会话热身——sweep 10 连拍第 3 发才中），同一轮内 --frames 3
/// 只是帧内曝光收敛，救不了会话级废片，所以要**多次调用**而不是多帧；
/// 慢门+gain8 档三连败（暗/糊），只配末位。每轮独立留档（aginx-voice-qrN.jpg），
/// 收据可逐轮复盘。cam-shot 挂死有预算（wait_limited kill）。
const QR_BUDGET_SECS: u32 = 15;

fn scan_qr() -> Result<Vec<String>, String> {
    let mut last_err = String::new();
    // (轮次从 1 计) — 第 4 轮才是慢门兜底
    for round in 1..=4u32 {
        let t0 = Instant::now();
        let qr_jpg = format!("/tmp/aginx-voice-qr{round}.jpg");
        let mut cmd = Command::new("/usr/bin/aginx-cam-shot");
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
        // 解码（aginx-qr 进程，payload 一行一个）。output() 不带超时——解码
        // 是 <300ms 量级的纯计算，等待预算都在拍照那侧
        let dec = Command::new("/usr/bin/aginx-qr").arg(&qr_jpg).output();
        match dec {
            Ok(out) if out.status.success() => {
                let payloads = String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                if !payloads.is_empty() {
                    eprintln!(
                        "aginx-voice: qr round {round}, {:.1}s",
                        t0.elapsed().as_secs_f32()
                    );
                    return Ok(payloads);
                }
                last_err = "没找到二维码".into();
            }
            Ok(_) => last_err = "aginx-qr rc!=0".into(), // exit 1 = 没码，也重试
            Err(e) => last_err = format!("aginx-qr spawn: {e}"),
        }
    }
    Err(last_err)
}

/// M47⑤ 眼取景常驻子进程：--forever cam-shot，全屏竖帧原子发布（tmp+rename
/// 由 cam-shot 自己做）。**双产物**（M47⑤c）：--raw-out eye.raw 每帧出
/// RGB565（term 直读免解码，显示 ~12-15fps）；--jpeg-every-ms 500 把编码
/// （0.125s/帧@720×1561，实测 2026-09-05——8fps 天花板的全部根因）摊薄成
/// 慢车道副产物，只剩 QR 在读 eye.jpg（2Hz 限频正好对上）。--aspect
/// 1080:2340 = 整屏（2026-09-05 用户收据「界面要做成全屏」），与 term 的
/// launch::VIEWFINDER_ASPECT 钉在一起（那边 host 测试守着）——布局属性由
/// 本粘合层显式注入，不共享 crate。AEC 状态由 cam-shot 落
/// /run/aginx-cam/aec.state，下次开眼首帧即正常亮度。
///
/// ⑤u 两个变化（A/B 2026-09-06）：取景参数加 `--nr 0:0:0:0`——⑤o 全分辨率
/// demosaic+面积均值已结构性砍掉颗粒（√1.29×），⑤m 整面空间 NR 在这之上
/// 纯付 17.2ms/帧（fps 26→10.9「可怜感」的主犯之一），取景关 NR、出片仍走
/// 全 look；子进程 stdout/stderr 不再进 null，落 /run/aginx-voice/cam.log
/// （每次开眼截断一份）——真实会话第一次可见 aec 走线与 vf: 链路心跳。
///
/// ⑤v-1（2026-09-06，P2 探针已证）：`--vf-window 4`——aec_step 的 pending 门
/// 是 window+3 帧/步（window 8 → 11 帧 ≈0.77s@14fps，场景切换 ~2 粗步+trim
/// 就是用户判的「曝光 3s」）。window 4 门降到 7 帧，收敛 ~1.9→~1.4s，fps
/// 无损（P2 实跑 498 帧正常）；ring 更浅只会让 kernel UPDATE 池余量更大。
fn eye_spawn() -> Result<std::process::Child, String> {
    let mut cmd = Command::new("/usr/bin/aginx-cam-shot");
    cmd.args([
            "--stream", "--rear", "--forever", "--aec", "--rot", "90",
            "--aspect", "1080:2340", "--preview", "720", "--jpeg",
            "--jpeg-every-ms", "500",
            "--nr", "0:0:0:0",
            "--vf-window", "4",
        ])
        .arg("--jpeg-out")
        .arg(face::EYE_JPG)
        .arg("--raw-out")
        .arg("/run/aginx-voice/eye.raw");
    // ⑤u: 一个日志文件，开眼截断（tmpfs 限一次会话）；stderr 挂 stdout 的
    // dup——共享偏移，2>&1 语义。开不了就退回 null：观察不能弄死眼。
    let log = std::fs::File::create("/run/aginx-voice/cam.log").ok();
    cmd.stdout(log.as_ref().and_then(|f| f.try_clone().ok()).map_or_else(
        std::process::Stdio::null,
        std::process::Stdio::from,
    ));
    cmd.stderr(log.map_or_else(std::process::Stdio::null, std::process::Stdio::from));
    cmd.spawn().map_err(|e| format!("cam-shot spawn: {e}"))
}

/// 优雅停机（TERM-then-wait，2s 预算）：cam-shot --forever 的退出路径是
/// SIGTERM → STREAMOFF teardown + aec.state 落盘；超时才 SIGKILL（std 的
/// Child::kill() 只有 SIGKILL，所以先 libc::kill 发 TERM）。
fn eye_stop(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// 取景帧解 QR（aginx-qr 独立进程，100-300ms 纯计算，last_qr 限频 2Hz）。
/// 全屏裁法（2016×930 vs 旧 1530×1136）横 FOV -18%/纵 +32%，预览缩放
/// 720/930 反而比旧 720/1136 大 1.22×——M42b 甜点模块在预览里更大，
/// 仍在 quirc + Bradley 已证域（实测定终）。None = 没码/解码器不在——
/// 取景继续等下一帧。
fn eye_decode_qr() -> Option<Vec<String>> {
    let out = Command::new("/usr/bin/aginx-qr")
        .arg(face::EYE_JPG)
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // exit 1 = 没码，取景继续
    }
    let payloads: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    (!payloads.is_empty()).then_some(payloads)
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
        let jpg = format!("/tmp/aginx-voice-ocr{round}.jpg");
        let mut cmd = Command::new("/usr/bin/aginx-cam-shot");
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
        // 识别要秒级（aginx-qr 的 <300ms 先例不适用），piped + wait_limited 给预算。
        let mut child = match Command::new("/var/bin/aginx-ocr")
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
                    eprintln!("aginx-voice: ocr round {round}, {} 行", lines.len());
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
    let mut child = Command::new("/usr/bin/aginx-net-join")
        .args(["wlan0", ssid, psk])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    audio::wait_limited(&mut child, JOIN_BUDGET_SECS).map_err(|e| format!("wifi-join {e}"))?;
    // dhcp 在 wifi-join 里；地址落不落直接看
    for _ in 0..10 {
        if let Some(ip) = wlan0_ip() {
            persist_wifi(ssid, psk);
            return Ok(ip);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Err("没拿到地址".into())
}

/// 连上网后落 /etc/wifi.conf（M42g③）：0600、`ssid=`/`psk=` KEY=VALUE——
/// 开机 net-bringup 与 net-watch 自愈都读它。语音序数/拼读、WIFI: 码、
/// 眼取景三条连网路从此持久，重启不丢网。**失败不写**：坏密钥不落盘
/// （wizard 撤回语义）；同网重连写同值幂等。
fn persist_wifi(ssid: &str, psk: &str) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let conf = format!("ssid={ssid}\npsk={psk}\n");
    let tmp = "/etc/wifi.conf.tmp";
    let ok = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp)
        .and_then(|mut f| f.write_all(conf.as_bytes()))
        .is_ok();
    if ok {
        let _ = std::fs::rename(tmp, "/etc/wifi.conf");
    } else {
        eprintln!("aginx-voice: persist wifi.conf failed");
    }
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
    let net = if wlan0_ip().is_some() { "网已连" } else { "没联网" };
    format!("{time}，电池{bat}%，{net}。")
}
