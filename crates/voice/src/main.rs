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
//!   aginx-voice --script              stdin 每行一条 Heard，同一 Vm 跨步（多步流）
//!   aginx-voice --face                打印当前屏面 JSON
//!
//! 没有嘴耳同开的回环自检：M18 的硬件收据写明 MM1 边放边采会把放音叠
//! 进采集（数字回环是失真副本，880Hz 可验、语音不可认，2026-09-04 实测
//! ASR 出"うん、うん"）——产品路径本来也是顺序的：PTT 采完才 TTS。

mod audio;
mod face;
mod protocol;
mod ptt;
mod render;

use protocol::{Act, Ev, NetState, Out, PowerAction, Vm};
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
/// 面法B：AGINX_POWER_KEY 从 /etc/aginx/env 经 unit env_file 进程环境，
/// norm 后注入（与 Heard 同一归一化——口令怎么说的就怎么比）；空值/缺键
/// = 未设置 = fail-closed。值只进 Vm，永不回显/入日志（日志只记 set/unset）。
fn make_vm() -> Vm {
    let key = std::env::var("AGINX_POWER_KEY")
        .ok()
        .map(|v| protocol::norm(&v))
        .filter(|v| !v.is_empty());
    eprintln!(
        "aginx-voice: power key {}",
        if key.is_some() { "set" } else { "unset" }
    );
    let vm = if front_bin().is_some() {
        Vm::with_front()
    } else {
        Vm::new()
    };
    vm.with_power_key(key)
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
            // v4③：transcript 与 PTT 同法上脸（term 光标面打出）；词表应答
            // 随 run_outs 的 Say/Speak 换行。旧结果面随 face::write 的
            // result=false 下降沿由 term 自拆（v4⑥ 同步失效线）。
            face::set_line(Some(&text));
            face::write(false);
            let outs = vm.step(Ev::Heard(text));
            run_outs(&mut vm, outs, None, &mut None);
        }
        Some("--script") => {
            // 收据阶梯：stdin 每行一条 Heard，同一个 Vm 跨步保持（--inject
            // 一次一进程，扫码→确认这种多步流跑不完整）。run_outs 在步间阻塞
            // ——相机/TTS 落完才读下一行，喂两行也能按序走完。
            let brain = audio::Brain::from_env();
            let mut vm = make_vm();
            face::write(false);
            let mut line = String::new();
            let mut step = 0usize;
            loop {
                line.clear();
                match std::io::stdin().read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let t = line.trim_end_matches('\n');
                        if t.is_empty() {
                            continue;
                        }
                        // 只记步数不记原文——stdin 可能含口令，日志零回显。
                        step += 1;
                        eprintln!("aginx-voice: script step {step}");
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

// ---- #282 开机等网（网络是最后一步）--------------------------------------
// bootcard 已改盯 phase-1 `done` 退场（光标不等网），等网的故事搬到这里：
// 光标面上「正在联网…」打字行 → boot.state `internet ok` → 问候上脸。
// 问候 = status_text() 状态行（09-10 定稿，剧场话术退役）：开机第一句话
// 就是机器的真实状态，与查询「状态」同一句话。
// 只显示、不出声���不自己连网（net-bringup phase 2 / net-watch 是写者）。
// 跑在主循环 200ms 节拍里（内部 5s 轮询门），无线程无阻塞。
const BOOT_STATE: &str = "/run/boot.state";
/// 轮询间隔：boot.state phase 2 每步落行，5s 粒度足够。
const BOOT_NET_POLL: Duration = Duration::from_secs(5);
/// 等网窗口：超过即静默退役（红警面接着讲无网的故事，等待行留着不撤）。
const BOOT_NET_WATCH: Duration = Duration::from_secs(300);
/// 只在开机窗内布防：中午 svc 重启不问候——问候是开机的事，不是重启的事。
const BOOT_NET_UPTIME_GATE_SECS: f64 = 180.0;
/// 未配对机不布防（无 wifi.conf）：它的路是配对面，不是等网。
const WIFI_CONF: &str = "/etc/wifi.conf";
const BOOT_NET_WAITING: &str = "正在联网…";

struct BootNet {
    /// 我们放上的等待行（所有权判据：还等于它才是我们的台）。
    mine: Option<&'static str>,
    armed_at: Instant,
    last_poll: Instant,
}

fn boot_state_has_internet() -> bool {
    // 行形如 `internet ok www.baidu.com`；run/fail/缺行都算未通。
    std::fs::read_to_string(BOOT_STATE)
        .map(|s| s.lines().any(|l| l.trim_start().starts_with("internet ok")))
        .unwrap_or(false)
}

fn uptime_secs() -> f64 {
    // 读不到（非 Linux host 测试）按超窗处理——不布防。
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|t| t.parse().ok()))
        .unwrap_or(f64::MAX)
}

/// daemon 启动时布防。三道门：开机窗（uptime ≤180s）、已配对（wifi.conf
/// 在）、网未通（已通就直接问候，不占台）。voice 由 svc 拉起 ~35s uptime，
/// 正常开机两道门都过；face::write 在此窗内安全——站立结果页与等待行
/// 互斥（任何 Chat 回合先 set_line(transcript) 破所有权）。
fn boot_net_arm() -> BootNet {
    let idle = || BootNet {
        mine: None,
        armed_at: Instant::now(),
        last_poll: Instant::now(),
    };
    if uptime_secs() > BOOT_NET_UPTIME_GATE_SECS || !std::path::Path::new(WIFI_CONF).exists() {
        return idle();
    }
    if boot_state_has_internet() {
        let greet = status_text();
        face::set_line(Some(&greet));
        face::write(false);
        eprintln!("aginx-voice: boot net up before voice — greeted");
        return idle();
    }
    face::set_line(Some(BOOT_NET_WAITING));
    face::write(false);
    eprintln!("aginx-voice: boot net watching");
    BootNet {
        mine: Some(BOOT_NET_WAITING),
        armed_at: Instant::now(),
        last_poll: Instant::now(),
    }
}

/// 主循环每拍调用。三路静默退出：行易主（用户说话/一次性进程改了面）→
/// 退役；窗口尽（300s）→ 退役；internet ok → 问候上脸后退役。
fn boot_net_tick(bn: &mut BootNet) {
    let Some(mine) = bn.mine else { return };
    if face::current_line().as_deref() != Some(mine) {
        bn.mine = None;
        eprintln!("aginx-voice: boot net line taken — retire");
        return;
    }
    if bn.armed_at.elapsed() >= BOOT_NET_WATCH {
        bn.mine = None;
        eprintln!("aginx-voice: boot net window over — no greet");
        return;
    }
    if bn.last_poll.elapsed() < BOOT_NET_POLL {
        return;
    }
    bn.last_poll = Instant::now();
    if boot_state_has_internet() {
        // 跨进程护栏：--inject 一次性进程写的是面文件、改不到本进程静态量
        // ——问候覆盖前再读一次面，等待行不在了就放弃（不clobber别人的台）。
        if !face::read().is_some_and(|f| f.contains(mine)) {
            bn.mine = None;
            eprintln!("aginx-voice: boot net line taken — retire");
            return;
        }
        let greet = status_text();
        face::set_line(Some(&greet));
        face::write(false);
        bn.mine = None;
        eprintln!("aginx-voice: boot net up — greeted");
    }
}

fn daemon() {
    let brain = audio::Brain::from_env();
    let mut vm = make_vm();
    let mut ptt = ptt::Ptt::open();
    if ptt.is_none() {
        eprintln!(
            "aginx-voice: no {} — PTT dead, face only",
            hwd::load_or_exit().input.ptt.device
        );
    }
    face::write(false);
    eprintln!(
        "aginx-voice: up (local={}, brain={}, ptt={})",
        audio::local_voice_ready(),
        brain.is_some(),
        ptt.as_ref()
            .map(|p| p.devs())
            .unwrap_or_else(|| "none".into())
    );
    // M42e: 预载常驻嘴耳模型（spawn 即返回，加载在子进程里）——第一次
    // 说话不再等 ~4s/侧 的装载。
    if audio::local_voice_ready() {
        audio::warm_local_voice();
    }

    // #282 开机等网：光标面上「正在联网…」→ internet ok → 问候。
    let mut boot_net = boot_net_arm();

    let mut capturing: Option<std::process::Child> = None;
    // 音量下键按下时刻：短按(<300ms)=音量−10、长按=PTT（M42e 产品面）
    let mut ptt_down: Option<Instant> = None;
    // 眼取景（M42g→M47⑤）：Some = 取景中（常驻子进程 + 命中轮询）。音量+
    // 开/再按关，音量下关。帧由子进程逐张原子发布，这里只轮询 mtime 解码
    // ——命中 WIFI: 码直接连（扫码即指令，拉式——机器开着取景等的就是
    // 这个格式）。
    let mut eye: Option<EyeView> = None;

    // 面法B：口令等待的心跳基准（真实流逝喂进状态机，超时由协议判）
    let mut last_tick = Instant::now();

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
                            face::write(false);
                            continue;
                        }
                        ptt_down = Some(Instant::now());
                        if capturing.is_none() {
                            match audio::capture_start() {
                                Ok(c) => {
                                    capturing = Some(c);
                                    face::write(false);
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
                            face::write(false);
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
                                face::write(false);
                                match hear(&wav, brain.as_ref()) {
                                    Ok(text) => {
                                        eprintln!("aginx-voice: heard {text:?}");
                                        // v4③：transcript 原子上脸——term
                                        // 无论此刻在结果面还是光标面，这一
                                        // 写把它拉回 prompt 面开打；应答由
                                        // run_outs 的 Say/Speak 换行。
                                        face::set_line(Some(&text));
                                        face::write(false);
                                        let outs = vm.step(Ev::Heard(text));
                                        run_outs(&mut vm, outs, brain.as_ref(), &mut eye);
                                    }
                                    Err(e) => {
                                        eprintln!("aginx-voice: asr {e}");
                                        // asr 失败提示本身也要能说——但 asr
                                        // 挂了多半网络不通，TTS 也挂；只刷屏
                                        let _ = vm.step(Ev::Heard("没听懂".into()));
                                        face::write(false);
                                    }
                                }
                            } else {
                                // 误触（<0.1s）
                                face::write(false);
                            }
                            // 尾部不得再写 face：run_outs 尾部 flush_pending
                            // 刚翻 result:true，此处写会亚 60ms 清旗，term
                            // 轮询永远看不见上升沿（v4⑥ 真人 PTT 首雷）。
                            // 新动作失效由 PTT Down(231)/闭眼(223) 开头写承担。
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
                        face::write(eye.is_some());
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
                                // （100-300ms/次）吃满整个 loop 还耗编码
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
            face::write(false);
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

        // ---- #282 开机等网（5s 节拍非阻塞；行易主/窗口尽即静默退役） ----
        boot_net_tick(&mut boot_net);

        // ---- 口令等待心跳（面法B）：真实流逝喂进状态机 ----
        // run_outs 可能阻塞数秒（TTS/brain 往返）——喂真实 dt，等待按
        // 人间的钟作废。空等 = 空返回，无输出零成本。
        let now = Instant::now();
        let outs = vm.step(Ev::Tick(
            now.duration_since(last_tick).as_millis() as u32
        ));
        last_tick = now;
        if !outs.is_empty() {
            run_outs(&mut vm, outs, brain.as_ref(), &mut eye);
        }
    }
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
            Out::Say(s) => {
                // v4：话术=光标面打字文本（屏幕对话）+ stderr 日志（真源）。
                // 脸不再序列化 lines 历史，终值即当前一行。
                eprintln!("aginx-voice: say {s}");
                face::set_line(Some(&s));
            }
            Out::Speak(s) => {
                eprintln!("aginx-voice: speak {s}");
                face::set_line(Some(&s));
                face::write(eye.is_some());
                say(&s, brain);
            }
            Out::Act(a) => match a {
                Act::NetConnect => {
                    // 机器干活：先看现状，再试记忆里的网，都不行才睁眼要码
                    face::write(eye.is_some());
                    followups.push(Ev::NetState(net_check()));
                }
                Act::Join { ssid, psk } => {
                    face::write(eye.is_some());
                    followups.push(Ev::JoinDone(join_wifi(&ssid, &psk)));
                }
                Act::PairApply { bundle } => {
                    face::write(eye.is_some());
                    let r = pair_apply(&bundle);
                    if r.is_ok() {
                        // v4: 剧场已退役——镜头先收（眼视图还占着面板），
                        // PairDone 就地喂（收尾句照常出声，Say 落 face）。
                        eye_shut(vm, eye);
                        let outs = vm.step(Ev::PairDone(r));
                        run_outs(vm, outs, brain, eye);
                    } else {
                        followups.push(Ev::PairDone(r));
                    }
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
                    face::write(false);
                    let r = scan_qr();
                    if let Err(e) = &r {
                        eprintln!("aginx-voice: qr {e}");
                    }
                    followups.push(Ev::QrDone(r));
                }
                Act::Ocr => {
                    face::write(eye.is_some());
                    let r = read_text();
                    if let Err(e) = &r {
                        eprintln!("aginx-voice: ocr {e}");
                    }
                    followups.push(Ev::OcrDone(r));
                }
                Act::Status => {
                    // V5（09-10）：状态与其他 Say 同律——只上脸+日志，不出声；
                    // 要听跟「你说给我听」。这里原是全程序唯一一个 Say 还走
                    // TTS 的漏网口。
                    let o = vm.inject_say(&status_text());
                    if let Out::Say(s) = o {
                        eprintln!("aginx-voice: say {s}");
                        face::set_line(Some(&s));
                    }
                }
                Act::PowerExec { action } => {
                    // 面法B：确认话术已由 Speak 行播放（local_speak 阻塞——
                    // 放音走完才到这）。再留一拍让尾音落稳，然后交
                    // aginx-reboot（自己 sync）。daemon 不退出——随整机一起走。
                    // 日志只记动作，永不记口令。
                    std::thread::sleep(Duration::from_millis(1500));
                    let arg = match action {
                        PowerAction::Poweroff => "poweroff",
                        PowerAction::Reboot => "reboot",
                    };
                    eprintln!("aginx-voice: power exec {arg}");
                    let _ = Command::new("/usr/bin/aginx-reboot").arg(arg).spawn();
                }
                Act::Chat(text) => {
                    face::write(eye.is_some());
                    let hit = roster_hit(&text);
                    if let Some(n) = &hit {
                        eprintln!("aginx-voice: roster {n}");
                    }
                    let reply = match chat_front(&text, hit.as_deref()) {
                        Ok(r) => r,
                        Err(e) => {
                            eprintln!("aginx-voice: front {e}");
                            "现在连不上母体。固定说法还在：连接无线网络，或扫码，或念一下。"
                                .to_string()
                        }
                    };
                    // 拉式：回复上脸不出声，点名（你说给我听）才 Speak。
                    // #283 问句常驻：行=「问句\n回复」——问句整段打完，回复
                    // 换行续打（term 前缀续打识别）；下一次用户说话
                    // set_line(transcript) 整行替换，即自然清场。
                    eprintln!("aginx-voice: say {reply}");
                    face::set_line(Some(&format!("{text}\n{reply}")));
                    // v4⑥：文本已上脸，同步写结果页（term 独立 attach 上屏）；
                    // 翻旗归 run_outs 尾部 flush_pending——这里早翻会被本回合
                    // 后续 face::write 清掉。写失败则文本就是结果（降级一等）。
                    // 结果页同律带问句块（#283：问句在答句上方）。
                    render::stage_reply(&text, &reply);
                }
            },
        }
    }
    // 例行刷脸只在无站立结果时做：tick 超时/纯 say 再入 run_outs 不得踩
    // 站立页（结果页不超时=产品线；v4⑥ 真人第二雷：live 几秒后必 teardown
    // 即此尾写所为）。失效只归新用户动作（PTT down/闭眼等显式 face::write）。
    if !face::result_standing() {
        face::write(eye.is_some());
    }
    for ev in followups {
        let outs = vm.step(ev);
        run_outs(vm, outs, brain, eye);
    }
    // v4⑥ 翻旗点（全程序唯一）：本回合若有 Chat 暂存了结果页，这里统一
    // 翻 result:true——在出口 face::write 清旗之后、followups 跑完之后。
    render::flush_pending();
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
/// 花名册点名（v4③ v0，D11）：化身=workspaces 文件夹，目录即注册——
/// voice 读与 server 同一几何（AGINX_HOME 或 ~/.aginx 下的 workspaces/），
/// transcript 含化身名子串 = 显式点名（server resolve_send 显式臂），不
/// 命中不点名落母体（D10 住）。字典序取首个命中；ASR 转写不保证名字还
/// 原，v0 宁落母体不误投。
fn roster_hit_in(root: &std::path::Path, text: &str) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names.into_iter().find(|n| text.contains(n.as_str()))
}

fn roster_hit(text: &str) -> Option<String> {
    let home = std::env::var("AGINX_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into()))
                .join(".aginx")
        });
    roster_hit_in(&home.join("workspaces"), text)
}

fn chat_front(text: &str, name: Option<&str>) -> Result<String, String> {
    let bin = front_bin().ok_or_else(|| "VOICED_FRONT not set".to_string())?;
    let mut args: Vec<&str> = vec!["agent", "send"];
    if let Some(n) = name {
        args.push(n);
    }
    args.push(text);
    let mut child = Command::new(&bin)
        .args(&args)
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

/// Act::PairApply（M42c 一眼自举的机器侧全流程）委外 `/usr/bin/aginx-pair
/// apply`（C3）：payload 一行进 stdin——argv 恒两词，psk/三键永不进
/// /proc/*/cmdline。join+IP 轮询+落 wifi.conf、env 三键合并、快速校时、
/// internet 探测、母体两单元 restart-ready、boot.state 网四行定点刷新全
/// 在那一侧；汇总行（stdout 首行）回来作报告话。秘密只进 env 文件
/// （0600），两侧日志都永不记值。预算 240s（join 90s + ntpd 20s + 两单元
/// 各 10s ready，余量给首启冷路）。**不重启 aginx-voice**——重启=自杀，
/// 本地语音离线路径不依赖 env，下次 boot 自然带上。
const PAIR_APPLY_BUDGET_SECS: u32 = 240;

fn pair_apply(bundle: &aginx_qr::PairBundle) -> Result<String, String> {
    use std::io::Write as _;
    let mut child = Command::new("/usr/bin/aginx-pair")
        .arg("apply")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("aginx-pair spawn: {e}"))?;
    {
        let mut si = child.stdin.take().ok_or("aginx-pair stdin")?;
        si.write_all(format!("{}\n", bundle.payload()).as_bytes())
            .map_err(|e| format!("aginx-pair stdin: {e}"))?;
        // 块结束 drop 写端 —— apply 读到 EOF 收行
    }
    audio::wait_limited(&mut child, PAIR_APPLY_BUDGET_SECS)
        .map_err(|e| format!("aginx-pair {e}"))?;
    // 汇总行短（≤一屏行），pipe 缓冲装得下；子已退，读到 EOF 即回
    let out = child
        .wait_with_output()
        .map_err(|e| format!("aginx-pair read: {e}"))?;
    let line = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if line.is_empty() {
        return Err("aginx-pair 没给汇总行".into());
    }
    Ok(line)
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
            // 末位兜底（机型参数 [quirks] qr_scan_args）：黑底白码贴纸、
            // 夜间的最后一搏（本机是慢门模式+模拟增益档）
            cmd.args(&hwd::load_or_exit().quirks.qr_scan_args);
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
/// （0.125s/帧@预览宽，实测 2026-09-05——8fps 天花板的全部根因）摊薄成
/// 慢车道副产物，只剩 QR 在读 eye.jpg（2Hz 限频正好对上）。--aspect 由
/// [panel] 拼出 = 整屏（2026-09-05 用户收据「界面要做成全屏」），term 侧
/// eye_box 同样出自 [panel]（launch.rs 测试读同一档案守着）——布局属性由
/// 本粘合层显式注入，不共享 crate。AEC 状态由 cam-shot 落
/// /run/aginx-cam/aec.state，下次开眼首帧即正常亮度。
///
/// argv 分层（D14）：粘合层固定加平台旗标（--stream --rear --forever
/// --aec --jpeg --jpeg-every-ms 500 + 两个输出落点 + --aspect）；机器尾
/// （rot/preview/vf-window/nr）整串来自 [quirks] eye_stream_args——换机型
/// 只改数据。
///
/// ⑤u 两个变化（A/B 2026-09-06）：取景关 NR、出片仍走全 look（全分辨率
/// demosaic+面积均值已结构性砍掉颗粒，整面空间 NR 在这之上纯付帧时）；
/// 子进程 stdout/stderr 不再进 null，落 /run/aginx-voice/cam.log（每次
/// 开眼截断一份）——真实会话第一次可见 aec 走线与 vf: 链路心跳。
///
/// ⑤v-1（2026-09-06，P2 探针已证）：曝光门减半参数（vf-window）也在
/// 机器尾里——aec_step 的 pending 门是 window+3 帧/步，收敛 ~1.9→~1.4s，
/// fps 无损（P2 实跑 498 帧正常）。
fn eye_spawn() -> Result<std::process::Child, String> {
    let p = hwd::load_or_exit();
    let aspect = format!("{}:{}", p.panel.width, p.panel.height);
    let mut cmd = Command::new("/usr/bin/aginx-cam-shot");
    cmd.args(["--stream", "--rear", "--forever", "--aec", "--jpeg"])
        .arg("--jpeg-every-ms")
        .arg("500")
        .args(&p.quirks.eye_stream_args)
        .arg("--aspect")
        .arg(&aspect)
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
            // 末位兜底：满增益提亮（[camera] dark_*——M45 暗房收据，
            // det 0 框→4 框的档位）
            let cam = &hwd::load_or_exit().camera;
            cmd.args([
                "--gain",
                &cam.dark_gain.to_string(),
                "--dgain",
                &cam.dark_dgain.to_string(),
            ]);
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
/// net-join 只装钥匙；租约靠 udhcpc（net-bringup/net-rejoin 同款分法，
/// 2026-09-09 蛋首配收据：关联成而 IP 永不来——voice 旧路是被 net-watch
/// 的 net-rejoin 兜住的）。地址已在（开机路径跑过 udhcpc）就跳过。
fn join_wifi(ssid: &str, psk: &str) -> Result<String, String> {
    let mut child = Command::new("/usr/bin/aginx-net-join")
        .args(["wlan0", ssid, psk])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn: {e}"))?;
    audio::wait_limited(&mut child, JOIN_BUDGET_SECS).map_err(|e| format!("wifi-join {e}"))?;
    // 租约腿（net-bringup 同款参数）；地址已在就跳过，幂等不重试。
    if wlan0_ip().is_none() {
        let mut dhcp = Command::new("/bin/udhcpc")
            .args(["-i", "wlan0", "-n", "-q", "-t", "10", "-T", "3"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("udhcpc spawn: {e}"))?;
        audio::wait_limited(&mut dhcp, 40).map_err(|e| format!("udhcpc {e}"))?;
    }
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
        .arg("+%H %M")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            // 按整数解析自然去前导零（TTS 会把 0 也念出来）。字符串修剪法
            // 在 00 点会连吞两位（"00点45分"→"点45分"，09-10 立案次日修）。
            let mut it = s.split_whitespace();
            let h: u32 = it.next()?.parse().ok()?;
            let m: u32 = it.next()?.parse().ok()?;
            Some(format!("{h}点{m}分"))
        })
        .unwrap_or_default();
    let bat = std::fs::read_to_string(format!(
        "{}/capacity",
        hwd::load_or_exit().paths.power_supply
    ))
    .ok()
    .and_then(|s| s.trim().parse::<u8>().ok())
    .unwrap_or(0);
    // 只报连没连——IP 逐位念出来又长又难听（数字展开还多 10s 合成+播放）
    let net = if wlan0_ip().is_some() { "网已连" } else { "没联网" };
    format!("{time}，电池{bat}%，{net}。")
}

#[cfg(test)]
mod roster_tests {
    use super::roster_hit_in;

    #[test]
    fn roster_hit_matches_substring_sorted_first_dirs_only() {
        let d = std::env::temp_dir().join(format!("aginx-roster-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("小喜")).unwrap();
        std::fs::create_dir_all(d.join("阿福")).unwrap();
        std::fs::write(d.join("zzz-not-avatar"), b"").unwrap();
        // 子串命中
        assert_eq!(roster_hit_in(&d, "帮小喜看看天气"), Some("小喜".into()));
        // 双命中 → 字典序首个（小 U+5C0F < 阿 U+963F）
        assert_eq!(roster_hit_in(&d, "小喜和阿福都在吗"), Some("小喜".into()));
        // 不命中 → None（落母体）
        assert_eq!(roster_hit_in(&d, "今天天气怎么样"), None);
        // 文件不是化身（目录即注册）
        assert_eq!(roster_hit_in(&d, "读一下 zzz-not-avatar"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
