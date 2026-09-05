//! 音频件（M42a）：采集（snd-cap 子进程）、WAV 封装、brain ASR/TTS HTTP、
//! 放音（snd-play 子进程）。
//!
//! brain 音频形状（2026-09-04 Mac 侧探明并实测，非文档推断）：
//! - ASR：POST /v1/chat/completions {"model":"audio", content 块
//!   {"type":"input_audio","input_audio":{"data":<b64 wav>,"format":"wav"}}}
//!   → choices[0].message.content。中文实测近完美（含"大写A小写B数字3"）。
//!   注意 type 必须是 "input_audio"；model 必须 "audio"（"asr" 会掉进严格
//!   chat 反序列化拒掉 input_audio 块）。
//! - TTS：POST {"model":"tts","messages":[user text],"audio_format":"wav",
//!   "sample_rate":48000} → {"output":{"audio":"/audio/<id>.mp3"}}（URL 后缀
//!   是假的，内容实为 WAV）→ GET 下载 → RIFF S16LE mono 48k，正是 snd-play
//!   吃的格式。voice 缺省 longxiaochun_v2（"Cherry" 会 418）。

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

pub const PCM_CAP: &str = "/dev/snd/pcmC0D0c";
pub const PCM_PLAY: &str = "/dev/snd/pcmC0D0p";
pub const SND_CAP: &str = "/bin/snd-cap";
pub const SND_PLAY: &str = "/bin/snd-play";
pub const RATE: u32 = 48_000; // M18 听 recipe 的已证形状（MM1 mono 48k）
pub const CHANS: u32 = 1;
pub const CAP_MAX_SECS: u32 = 30;

// ---- 音量（M42e 产品面：短按音量±键调，长按音量下=PTT）----
// VOL 75 的观察收据：机身震 + 4.5-6k 破音——功放过推。改 60 起步，用户
// 键控微调。优先级：/var/lib/voiced/vol（键调持久，state tar 内存活）>
// AG_VOICE_VOL env > 缺省 60。AtomicU8=0 表示未初始化（真值经 clamp_vol 恒 ≥20）。
static VOL: AtomicU8 = AtomicU8::new(0);
const VOL_FILE: &str = "/var/lib/voiced/vol";
/// 地板 20：2026-09-03 设备收据——连续短按音量下到 0 后整机静默，连「音量0」
/// 播报都被自己的 0 音量吞掉。纯语音产品里 vol=0 等于设备失联，0-19 一律抬 20。
const VOL_MIN: u8 = 20;

fn clamp_vol(v: i32) -> u8 {
    v.clamp(VOL_MIN as i32, 100) as u8
}

pub fn vol() -> u8 {
    let v = VOL.load(Ordering::Relaxed);
    if v != 0 {
        return v;
    }
    let v = std::fs::read_to_string(VOL_FILE)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .or_else(|| {
            std::env::var("AG_VOICE_VOL")
                .ok()
                .and_then(|s| s.trim().parse::<i32>().ok())
        })
        .map(clamp_vol)
        .unwrap_or(60);
    VOL.store(v, Ordering::Relaxed);
    v
}

/// ±delta 钳 20-100，写 VOL_FILE 持久，返回新值。
pub fn adjust_vol(delta: i32) -> u8 {
    let v = clamp_vol(vol() as i32 + delta);
    VOL.store(v, Ordering::Relaxed);
    let _ = std::fs::create_dir_all("/var/lib/voiced");
    let _ = std::fs::write(VOL_FILE, v.to_string());
    v
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(120)))
        .build();
    ureq::Agent::new_with_config(config)
}

pub struct Brain {
    base: String,
    key: String,
    agent: ureq::Agent,
}

/// ASR 候选词提示——词表契约（见 asr() 注释：当前 brain audio 模型无视它）。
const ASR_HINT: &str = "你听到的是一台中文设备的语音指令录音。请把听到的话原样转写成简体中文。\
    可能出现的指令词：无线、网络、连网、上网、状态、时间、几点、电池、取消、算了、\
    第一个、第二个、第三个、第四个、第五个、对、不对、是的、否、密码、大写、小写、\
    数字零到数字九、完了、删掉、退格、你在吗、帮助。";

impl Brain {
    /// key 从环境 AGINXBRAIN_API_KEY 读（agsvc 单元 env_file 注入）。
    pub fn from_env() -> Option<Brain> {
        let key = std::env::var("AGINXBRAIN_API_KEY").ok()?;
        let base = std::env::var("AGINXBRAIN_URL")
            .unwrap_or_else(|_| "https://brain.aginx.net".to_string());
        Some(Brain {
            base,
            key,
            agent: agent(),
        })
    }

    /// WAV 字节 → 文本
    ///
    /// 2026-09-04 设备法医收据：brain 的 audio 模型无视 system 消息（有无
    /// ASR_HINT 结果逐字相同），且对本机采集链音频一律幻听或返回空——同一
    /// 句「连接无线网络」文件直喂全对（电平 12%/高通 200Hz 都过），经扬声
    /// 器隔空进麦克风后满刻度放大也废（→「很遗憾的呃。」）。云 ASR 与未
    /// 校准的 rt5514 裸 DMIC 链不兼容；真修法是 M42d 本地 ASR 换后端。
    /// ASR_HINT 保留当词表契约，换尊重 system 的后端时直接生效。
    pub fn asr(&self, wav: &[u8]) -> Result<String, String> {
        let b64 = b64_encode(wav);
        let body = serde_json::json!({
            "model": "audio",
            "messages": [
                { "role": "system", "content": ASR_HINT },
                {
                    "role": "user",
                    "content": [{
                        "type": "input_audio",
                        "input_audio": { "data": b64, "format": "wav" }
                    }]
                }
            ]
        });
        let resp = self
            .agent
            .post(format!("{}/v1/chat/completions", self.base))
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .send(&body.to_string())
            .map_err(|e| format!("asr post: {e}"))?;
        let status = resp.status().as_u16();
        let txt = resp
            .into_body()
            .read_to_string()
            .map_err(|e| format!("asr body: {e}"))?;
        if status != 200 {
            return Err(format!("asr http {status}: {}", &txt[..txt.len().min(200)]));
        }
        let v: serde_json::Value =
            serde_json::from_str(&txt).map_err(|e| format!("asr json: {e}"))?;
        v.pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .map(String::from)
            .ok_or_else(|| format!("asr no content: {}", &txt[..txt.len().min(200)]))
    }

    /// 文本 → WAV 字节（S16LE mono 48k）
    pub fn tts(&self, text: &str) -> Result<Vec<u8>, String> {
        let body = serde_json::json!({
            "model": "tts",
            "messages": [{ "role": "user", "content": text }],
            "audio_format": "wav",
            "sample_rate": RATE
        });
        let resp = self
            .agent
            .post(format!("{}/v1/chat/completions", self.base))
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .send(&body.to_string())
            .map_err(|e| format!("tts post: {e}"))?;
        let status = resp.status().as_u16();
        let txt = resp
            .into_body()
            .read_to_string()
            .map_err(|e| format!("tts body: {e}"))?;
        if status != 200 {
            return Err(format!("tts http {status}: {}", &txt[..txt.len().min(200)]));
        }
        let v: serde_json::Value =
            serde_json::from_str(&txt).map_err(|e| format!("tts json: {e}"))?;
        let path = v
            .pointer("/output/audio")
            .and_then(|a| a.as_str())
            .ok_or_else(|| format!("tts no audio url: {}", &txt[..txt.len().min(200)]))?;
        // GET 音频（URL 是 brain 的相对路径）
        let resp = self
            .agent
            .get(&format!("{}{}", self.base, path))
            .header("Authorization", &format!("Bearer {}", self.key))
            .call()
            .map_err(|e| format!("tts fetch: {e}"))?;
        if resp.status().as_u16() != 200 {
            return Err(format!("tts fetch http {}", resp.status()));
        }
        let mut buf = Vec::new();
        resp.into_body()
            .into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| format!("tts read: {e}"))?;
        if buf.len() < 44 || &buf[0..4] != b"RIFF" {
            return Err("tts not wav".into());
        }
        Ok(buf)
    }

    /// 说一句话：TTS → 拆 WAV 头 → 复制成 L=R 立体声 → snd-play 阻塞放完。
    /// 音长上限 = 样本数/Rate + 5s 余量，防止挂死。
    ///
    /// 立体声不是可选的：QUIN_TDM_RX_0 后端是双通道，mono FE 在这张卡上
    /// 会话健康但无声（2026-09-04 收据：mono rms 26 / dft880 0.2，stereo
    /// rms 3650 / dft880 2396）。M18 的原收据也是 stereo。
    pub fn speak(&self, text: &str) -> Result<(), String> {
        let wav = self.tts(text)?;
        let (off, len) = wav_data_span(&wav)?;
        let raw = &wav[off..off + len];
        let samples = len / 2; // S16 mono in
        let mut stereo = Vec::with_capacity(len * 2);
        for s in raw.chunks_exact(2) {
            stereo.extend_from_slice(s);
            stereo.extend_from_slice(s); // L = R
        }
        let tmp = "/tmp/voiced-tts.raw";
        fs::write(tmp, &stereo).map_err(|e| format!("tts tmp: {e}"))?;
        play_stereo_blocking(samples)
    }
}

/// 放 /tmp/voiced-tts.raw（48k L=R stereo），阻塞到放完。
fn play_stereo_blocking(samples: usize) -> Result<(), String> {
    let budget = (samples / RATE as usize + 5) as u32;
    match play_stereo_spawn()? {
        None => Ok(()), // 短音频在宽限窗内已放完
        Some(mut child) => wait_limited(&mut child, budget),
    }
}

/// 起 snd-play 放 /tmp/voiced-tts.raw，不阻塞：open EBUSY（上一会话
/// teardown 的尾巴，M42e 设备收据）是毫秒级退出——原地小睡重试；GRACE
/// 后仍在跑即接管成功返回 child。分句下限 4 字 ≈0.5s 音频，宽限内
/// clean 退出=真放完了（返回 None）。
fn play_stereo_spawn() -> Result<Option<Child>, String> {
    const GRACE_MS: u64 = 400;
    let mut last_err = String::new();
    for _ in 0..6 {
        let vol_s = vol().to_string();
        let mut child = Command::new(SND_PLAY)
            .args([
                PCM_PLAY,
                "/tmp/voiced-tts.raw",
                &RATE.to_string(),
                "2",
                &vol_s,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("snd-play spawn: {e}"))?;
        let deadline = std::time::Instant::now() + Duration::from_millis(GRACE_MS);
        loop {
            match child.try_wait() {
                Ok(Some(st)) if st.success() => return Ok(None),
                Ok(Some(st)) => {
                    last_err = format!("exit {st}");
                    break; // EBUSY 等 → 小睡重试
                }
                Ok(None) if std::time::Instant::now() >= deadline => return Ok(Some(child)),
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => return Err(format!("wait: {e}")),
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(format!("snd-play: {last_err}"))
}

// ---------------- capture ----------------

/// 起一次最长 CAP_MAX_SECS 的采集；PTT 松手时 kill，采到多少算多少。
pub fn capture_start() -> std::io::Result<Child> {
    Command::new(SND_CAP)
        .args([
            PCM_CAP,
            &CAP_MAX_SECS.to_string(),
            "/tmp/voiced-cap.raw",
            &RATE.to_string(),
            &CHANS.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// 读采集产物并封 WAV。过短（<0.1s）返回 None（误触）。
pub fn capture_take() -> Option<Vec<u8>> {
    let raw = fs::read("/tmp/voiced-cap.raw").ok()?;
    let raw = &raw[..raw.len() - raw.len() % 2]; // 整样本截齐
    if raw.len() < (RATE as usize / 10) * 2 {
        return None;
    }
    Some(wav_wrap(raw, RATE, CHANS))
}

// ---------------- wav ----------------

pub fn wav_wrap(raw: &[u8], rate: u32, chans: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(44 + raw.len());
    let byte_rate = rate * chans * 2;
    let block_align = (chans * 2) as u16;
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&((36 + raw.len()) as u32).to_le_bytes());
    h.extend_from_slice(b"WAVE");
    h.extend_from_slice(b"fmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&1u16.to_le_bytes()); // PCM
    h.extend_from_slice(&(chans as u16).to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&byte_rate.to_le_bytes());
    h.extend_from_slice(&block_align.to_le_bytes());
    h.extend_from_slice(&16u16.to_le_bytes()); // bits
    h.extend_from_slice(b"data");
    h.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    h.extend_from_slice(raw);
    h
}

/// 遍历 RIFF 块找 data 的 (offset, len)。TTS 产物块序不保证 44 定长。
pub fn wav_data_span(wav: &[u8]) -> Result<(usize, usize), String> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("not riff".into());
    }
    let mut off = 12;
    while off + 8 <= wav.len() {
        let id = &wav[off..off + 4];
        let sz =
            u32::from_le_bytes([wav[off + 4], wav[off + 5], wav[off + 6], wav[off + 7]]) as usize;
        let body = off + 8;
        if body + sz > wav.len() {
            // brain 的 TTS 是流式合成：未知长度打成 0x7fffffff 哨兵
            // (2026-09-04 设备收据)。data 块实际跑到文件尾——只有 data
            // 可以这么收敛，fmt 被截是真错误。
            if id == b"data" {
                let len = wav.len() - body - (wav.len() - body) % 2;
                return Ok((body, len));
            }
            return Err("riff chunk overruns".into());
        }
        if id == b"data" {
            return Ok((body, sz));
        }
        off = body + sz + (sz % 2); // 块按 2 对齐
    }
    Err("no data chunk".into())
}

// ---------------- 本地后端（M42d：ag-asr/ag-tts bionic-static 子进程）----------------

pub const AG_ASR: &str = "/var/bin/ag-asr";
pub const AG_TTS: &str = "/var/bin/ag-tts";
pub const ASR_MODEL_DIR: &str = "/var/models/asr";
// vits(melo) 是产品嘴（ag-tts 默认 KIND 同此）：kokoro 的 zh 前端整词吞
// Latin——OCR 念读「AginxOS/TEL」无声的根因（2026-09-04 用户收据）。
pub const TTS_MODEL_DIR: &str = "/var/models/tts/vits-melo-tts-zh_en";

/// 本地嘴耳是否在位（binary + 模型目录）。真调用失败仍返回 Err 由调用方落云。
pub fn local_voice_ready() -> bool {
    std::path::Path::new(AG_TTS).exists()
        && std::path::Path::new(TTS_MODEL_DIR).exists()
        && std::path::Path::new(AG_ASR).exists()
        && std::path::Path::new(ASR_MODEL_DIR).exists()
}

/// 钉推理子进程到大核（cpu6/7 = A76 2.2-2.4GHz）。不钉则调度器会把两个
/// 推理线程摊上 A55——实测同一句 11.5s vs 8.7s（M42e）。pre_exec 里只能做
/// async-signal-safe 的调用；sched_setaffinity 是裸系统调用零 malloc，安全。
/// 钉不上（非本机拓扑/host 测试）就随它跑，不算错。
#[cfg(target_os = "linux")]
fn pin_big_cores(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_ZERO(&mut set);
            libc::CPU_SET(6, &mut set);
            libc::CPU_SET(7, &mut set);
            libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
            Ok(())
        });
    }
}
#[cfg(not(target_os = "linux"))]
fn pin_big_cores(_cmd: &mut Command) {}

// ---------------- 常驻本地后端（M42e：摊模型加载）----------------
// 一次性调用每次重付装载：ag-tts ~3.8s、ag-asr ~2s（cpufreq 拉满后实测，
// HARDWARE.md M42e）——短句延迟的大头是装载不是推理。--serve 进程在
// daemon 生命周期里只加载一次：stdin 一行一个请求，stdout 一行
// "OK ..." / "ERR ..."。任何失败清空常驻、当次落回一次性老路径。
// --say 一次性入口也走这里：起服务→一句→进程退出（stdin EOF 子进程自退），
// 代价与老路径相同，代码单路径。

/// 固定 wav 落点：daemon 与 --say 各有自己的 server 实例，写同一路径。
/// 并发跑两个入口理论上互踩这个文件——调试面小概率可忍，产品面只有 daemon。
const TTS_WAV: &str = "/tmp/voiced-tts.wav";
const HEAR_WAV: &str = "/tmp/voiced-hear.wav";

struct VoiceServer {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
}

impl VoiceServer {
    fn spawn(bin: &str, args: &[&str]) -> Result<VoiceServer, String> {
        let mut cmd = Command::new(bin);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null()); // sherpa 初始化告警没人读会塞满管道
        pin_big_cores(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| format!("spawn {bin}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        Ok(VoiceServer {
            child,
            stdin,
            reader,
        })
    }

    /// 一行请求一行应答；poll 等应答行可读（挂死的子进程不能拖死 daemon
    /// 主环）。首请求若赶上 spawn 后的模型装载，等的就是装载+推理，预算
    /// 给足。
    fn roundtrip(&mut self, req: &str, timeout: Duration) -> Result<String, String> {
        use std::os::fd::AsRawFd;
        writeln!(self.stdin, "{req}").map_err(|e| format!("write: {e}"))?;
        self.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        let mut pfd = libc::pollfd {
            fd: self.reader.get_ref().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let rc = unsafe { libc::poll(&mut pfd, 1, timeout.as_millis() as i32) };
        if rc < 0 {
            return Err(format!("poll: {}", std::io::Error::last_os_error()));
        }
        if rc == 0 {
            return Err("timeout".into());
        }
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            return Err("eof".into()); // server 死了
        }
        Ok(line.trim_end().to_string())
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner()) // 中毒不拖死 daemon
}

fn tts_server() -> &'static Mutex<Option<VoiceServer>> {
    static S: OnceLock<Mutex<Option<VoiceServer>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn asr_server() -> &'static Mutex<Option<VoiceServer>> {
    static S: OnceLock<Mutex<Option<VoiceServer>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// 开机预载（M42e）：daemon 起来就 spawn 两个 --serve，模型在开机尾巴上
/// 后台加载完，第一次说话即热路径。spawn 即返回，装载在子进程里，不挡环。
/// 起不来（bin 不在位）就留 None——真调用时还会再试。
pub fn warm_local_voice() {
    let mut slot = lock(tts_server());
    if slot.is_none() {
        *slot = VoiceServer::spawn(AG_TTS, &["--serve", TTS_WAV]).ok();
    }
    drop(slot);
    let mut slot = lock(asr_server());
    if slot.is_none() {
        *slot = VoiceServer::spawn(AG_ASR, &["--serve"]).ok();
    }
}

/// 常驻合成一句到 TTS_WAV。Err = 常驻不可用（调用方落一次性路径）。
fn resident_tts(text: &str) -> Result<(), String> {
    let mut slot = lock(tts_server());
    if slot.is_none() {
        *slot = Some(VoiceServer::spawn(AG_TTS, &["--serve", TTS_WAV])?);
    }
    let srv = slot.as_mut().unwrap();
    match srv.roundtrip(text, Duration::from_secs(120)) {
        Ok(line) if line.starts_with("OK ") => Ok(()),
        Ok(line) => Err(format!("ag-tts: {line}")), // server 活着，这句真失败
        Err(e) => {
            srv.kill();
            *slot = None; // 死/挂——清掉，下一次调用重 spawn
            Err(e)
        }
    }
}

/// 常驻识别 HEAR_WAV。Err = 常驻不可用（调用方落一次性路径）。
fn resident_asr() -> Result<String, String> {
    let mut slot = lock(asr_server());
    if slot.is_none() {
        *slot = Some(VoiceServer::spawn(AG_ASR, &["--serve"])?);
    }
    let srv = slot.as_mut().unwrap();
    match srv.roundtrip(HEAR_WAV, Duration::from_secs(60)) {
        Ok(line) if line.starts_with("OK ") => Ok(line[3..].trim().to_string()),
        Ok(line) => Err(format!("ag-asr: {line}")),
        Err(e) => {
            srv.kill();
            *slot = None;
            Err(e)
        }
    }
}

/// WAV 字节 → 文本（sense-voice 子进程；常驻优先，失败落一次性）。
pub fn local_asr(wav: &[u8]) -> Result<String, String> {
    fs::write(HEAR_WAV, wav).map_err(|e| format!("hear tmp: {e}"))?;
    let text = match resident_asr() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("voiced: asr serve {e} — one-shot");
            let mut cmd = Command::new(AG_ASR);
            cmd.arg(HEAR_WAV);
            pin_big_cores(&mut cmd);
            let out = cmd.output().map_err(|e| format!("ag-asr spawn: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "ag-asr {}: {}",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
    };
    if text.is_empty() {
        return Err("ag-asr empty".into());
    }
    Ok(text)
}

/// 文本 → 扬声器：分句流水（M42e 续：整段合成完才放是长句延迟的大头）。
/// 按句读切句，第一句合成完立即起放音，后续句在放音中由常驻 server
/// 并行合成——server 是独立进程，snd-play 放音不占它。TTS wav → FIR 升采
/// 样 48k → L=R 立体声 → snd-play。常驻不可用/中途挂 → 整段一次性兜底。
pub fn local_speak(text: &str) -> Result<(), String> {
    let spoken = expand_digit_chains(text);
    if speak_streamed(&split_clauses(&spoken)).is_ok() {
        return Ok(());
    }
    eprintln!("voiced: tts serve failed — one-shot");
    let mut cmd = Command::new(AG_TTS);
    cmd.args([&spoken, TTS_WAV]);
    pin_big_cores(&mut cmd);
    let out = cmd.output().map_err(|e| format!("ag-tts spawn: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ag-tts {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let wav = fs::read(TTS_WAV).map_err(|e| format!("tts read: {e}"))?;
    let samples = stage_wav(&wav)?;
    play_stereo_blocking(samples)
}

/// TTS 前文本整形：长数字链（电话/IP/日期等，链内总位数 ≥5）展开成空格
/// 分隔的逐位数字。melo 的 number.fst 把 11 位连写当整数读——ASR 回环收据
/// 「13800138000」→「138亿0138000」，连字符/空格分段同病（8000→「八千」）；
/// 空格分隔的 ASCII 数字实测逐位念（回环纯数位、无亿万，2026-09-04）。
/// 短链（≤4 位：数量/年份/小数）保持原样交给前端按数读。
fn expand_digit_chains(text: &str) -> String {
    const SEP: &str = "-. "; // 链内组间分隔：连字符/点/空格
    const MIN_DIGITS: usize = 5;
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // 吞整条链：数字组 (分隔 run + 数字组)*。分隔 run 只有后随数字才算
        // 链内（「第 3 章」的空格不会把 3 和别的数粘起来）。
        let mut j = i;
        let mut groups: Vec<(usize, usize)> = Vec::new();
        loop {
            let gs = j;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            groups.push((gs, j));
            let ss = j;
            while j < chars.len() && SEP.contains(chars[j]) {
                j += 1;
            }
            if !(j < chars.len() && chars[j].is_ascii_digit()) {
                j = ss; // 分隔不属于链，吐回去
                break;
            }
        }
        let total: usize = groups.iter().map(|(a, b)| b - a).sum();
        if total >= MIN_DIGITS {
            // 整链逐位单空格分隔（组界不再多补）
            for &(a, b) in &groups {
                for &c in &chars[a..b] {
                    out.push(c);
                    out.push(' ');
                }
            }
            out.pop(); // 尾随空格
        } else {
            // 短链原样（含组间分隔）—— chars[i..j] 正是整条链的原文跨度
            out.extend(chars[i..j].iter());
        }
        i = j;
    }
    out
}

/// 分句：只在句读（。！？；及 ASCII 对应）切，句 ≥4 字（太短并前句——
/// 放音 <0.5s 会踩接棒宽限期），无标点长段每 60 字硬切，且永不落在数字
/// 链中间（切断 = 两段各自又被前端按整数读）。逗号不是切点——M42e 时为
/// kokoro 慢合成压首响拆得细，melo 常驻后拆太散断语气（用户收据
/// 2026-09-04「不用拆句拆的太散」）。
fn split_clauses(text: &str) -> Vec<String> {
    const MIN: usize = 4;
    const MAX: usize = 60;
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut n = 0;
    for (idx, &ch) in chars.iter().enumerate() {
        cur.push(ch);
        n += 1;
        let stop = matches!(ch, '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';');
        let next_digit = chars.get(idx + 1).is_some_and(|c| c.is_ascii_digit());
        if (stop && n >= MIN || n >= MAX) && !next_digit {
            out.push(std::mem::take(&mut cur));
            n = 0;
        }
    }
    if !cur.is_empty() {
        if cur.chars().count() < MIN {
            match out.last_mut() {
                Some(l) => l.push_str(&cur),
                None => out.push(cur),
            }
        } else {
            out.push(cur);
        }
    }
    if out.is_empty() {
        out.push(text.to_string());
    }
    out
}

/// 逐句流水：合成一句 → 转 raw 落盘 → 等上一句放完 → 接棒放本句。
fn speak_streamed(clauses: &[String]) -> Result<(), String> {
    let mut playing: Option<(Child, u32)> = None;
    for c in clauses {
        resident_tts(c)?;
        let wav = fs::read(TTS_WAV).map_err(|e| format!("tts read: {e}"))?;
        let samples = stage_wav(&wav)?;
        if let Some((mut prev, budget)) = playing.take() {
            wait_limited(&mut prev, budget).map_err(|e| format!("prev snd-play: {e}"))?;
        }
        if let Some(child) = play_stereo_spawn()? {
            playing = Some((child, (samples / RATE as usize + 5) as u32));
        }
    }
    if let Some((mut last, budget)) = playing.take() {
        wait_limited(&mut last, budget).map_err(|e| format!("snd-play: {e}"))?;
    }
    Ok(())
}

/// wav（TTS 产物）→ FIR 升采样 48k → 7.5k 低通 → L=R 立体声写
/// /tmp/voiced-tts.raw，返回样本数（放音等待预算用）。
fn stage_wav(wav: &[u8]) -> Result<usize, String> {
    let (off, len) = wav_data_span(wav)?;
    let rate = wav_rate(wav)?;
    let up = if rate != RATE {
        resample(&wav[off..off + len], rate, RATE)?
    } else {
        wav[off..off + len].to_vec()
    };
    let up = lowpass(&up, RATE, 7_500.0);
    let samples = up.len() / 2;
    let mut stereo = Vec::with_capacity(up.len() * 2);
    for s in up.chunks_exact(2) {
        stereo.extend_from_slice(s);
        stereo.extend_from_slice(s); // L = R
    }
    fs::write("/tmp/voiced-tts.raw", &stereo).map_err(|e| format!("tts tmp: {e}"))?;
    Ok(samples)
}

/// 从 RIFF 头取采样率（fmt 块 body+4）。
pub fn wav_rate(wav: &[u8]) -> Result<u32, String> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("not riff".into());
    }
    let mut off = 12;
    while off + 8 <= wav.len() {
        let id = &wav[off..off + 4];
        let sz =
            u32::from_le_bytes([wav[off + 4], wav[off + 5], wav[off + 6], wav[off + 7]]) as usize;
        let body = off + 8;
        if id == b"fmt " {
            if body + 8 > wav.len() {
                return Err("fmt truncated".into());
            }
            return Ok(u32::from_le_bytes([
                wav[body + 4],
                wav[body + 5],
                wav[body + 6],
                wav[body + 7],
            ]));
        }
        if body + sz > wav.len() {
            return Err("riff chunk overruns".into());
        }
        off = body + sz + (sz % 2);
    }
    Err("no fmt chunk".into())
}

/// S16 mono 线性插值重采样。
/// 窗 sinc 多相 FIR 重采样。M42e 设备收据：线性插值在 44.1k→48k 把
/// 8-11k 齿音镜像折回 12-15k，用户耳朵听出破音；FIR 版同句 A/B 干净
/// （kokoro 24k 时镜像落点不同没暴露）。相数 p=升采样因子、每相 L 抽头、
/// blackman 窗、截止=低侧奈奎斯特——语音带 ≤11.5k，镜像全落阻带。
pub fn resample(raw: &[u8], from: u32, to: u32) -> Result<Vec<u8>, String> {
    if from == to {
        return Ok(raw.to_vec());
    }
    if from == 0 {
        return Err("rate 0".into());
    }
    let s16 = |i: usize| i16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]) as f32;
    let n = raw.len() / 2;
    if n == 0 {
        return Err("empty pcm".into());
    }
    let g = gcd(from, to);
    let p = (to / g) as usize; // 升采样因子 = 相数
    let q = (from / g) as usize; // 降采样步长（升采样域）
    const L: usize = 16; // 每相抽头
                         // 原型低通 h[j], j<p*L：第一零点在 ±D（D=max(p,q)）的 sinc——通带边
                         // 恰在低侧奈奎斯特；blackman 窗压旁瓣。x∈[-N/2,N/2] 中心对称。
    let n_taps = p * L;
    // 整样本中心（N 偶数 → N/2）：窗对称使各相 DC 和≈1（半样本中心实测
    // 逐相纹波 ±10%，DC 都会泄漏）。
    let center = (n_taps / 2) as f64;
    let d = p.max(q) as f64;
    let mut proto = vec![0f32; n_taps];
    for (j, h) in proto.iter_mut().enumerate() {
        let x = j as f64 - center;
        let s = if x == 0.0 {
            1.0 / d
        } else {
            (std::f64::consts::PI * x / d).sin() / (std::f64::consts::PI * x)
        };
        // blackman：w[0]=w[N-1]=0、中心=1（首版符号写反成倒窗，逐相 DC
        // 纹波 ±10% 的真凶，python 逐行对拍收据）
        let w = blackman(j, n_taps);
        *h = (s * w) as f32;
    }
    // 全局增益补偿：零填塞占空 1/p → 通带增益要 ×p。注意不能逐相归一——
    // 各相和≈1 但带窗纹波，强行拉平会把远心相放大成强杂散（数值对拍收据）。
    let sum: f32 = proto.iter().sum();
    let scale = if sum != 0.0 { p as f32 / sum } else { 1.0 };
    let mut phases = vec![vec![0f32; L]; p];
    for ph in 0..p {
        for k in 0..L {
            phases[ph][k] = proto[ph + k * p] * scale;
        }
    }
    let out_n = n * to as usize / from as usize;
    let mut out = Vec::with_capacity(out_n * 2);
    for i_out in 0..out_n {
        // 升采样域位置 m = i_out*q + center（补偿群延迟）→ 相 ph、输入锚 i
        let m = i_out * q + center as usize;
        let ph = m % p;
        let i = m / p;
        let taps = &phases[ph];
        let mut acc = 0f32;
        for (k, t) in taps.iter().enumerate() {
            let idx = i as isize - k as isize;
            let idx = if idx < 0 {
                0
            } else if idx as usize >= n {
                n - 1
            } else {
                idx as usize
            };
            acc += t * s16(idx);
        }
        let v = acc.clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    Ok(out)
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// blackman：w[0]=w[N-1]=0、中心=1。首版 resample 符号写反成倒窗（逐相 DC
/// 纹波 ±10% 的真凶，python 逐行对拍收据），抽出来两处共用别再写错。
fn blackman(j: usize, n_taps: usize) -> f64 {
    let p = std::f64::consts::PI;
    0.42 - 0.5 * (2.0 * p * j as f64 / (n_taps - 1) as f64).cos()
        + 0.08 * (4.0 * p * j as f64 / (n_taps - 1) as f64).cos()
}

/// 48k 域窗 sinc 低通。M42e 设备收据：melo（44.1k 全带宽）经功放破音，
/// 7.5k 低通版用户判干净——VITS 高频毛刺是真凶；重采样（线性/FIR 同破）、
/// 播放器（tinyalsa tinyplay 对照同破）、麦克风链（Mac 上干净）全排除。
/// 行业同构：WebRTC/LiveKit 语音走 OPUS 语音模式本就 8-12k 带宽，没人
/// 裸放全带宽 VITS。193 抽头（过渡带 ~1k）、DC 全局归一（resample 同理，
/// 不逐窗归一）、边缘钳位。
fn lowpass(raw: &[u8], rate: u32, cutoff: f64) -> Vec<u8> {
    let n = raw.len() / 2;
    if n == 0 || cutoff >= rate as f64 / 2.0 {
        return raw.to_vec();
    }
    const N_TAPS: usize = 193;
    let center = (N_TAPS / 2) as f64;
    let fc = cutoff / rate as f64; // 周期/样本
    let mut h = vec![0f32; N_TAPS];
    for (j, t) in h.iter_mut().enumerate() {
        let x = j as f64 - center;
        let s = if x == 0.0 {
            2.0 * fc
        } else {
            (2.0 * std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
        };
        *t = (s * blackman(j, N_TAPS)) as f32;
    }
    let sum: f32 = h.iter().sum();
    let h: Vec<f32> = h.iter().map(|t| t / sum).collect();
    let s16 = |i: usize| i16::from_le_bytes([raw[i * 2], raw[i * 2 + 1]]) as f32;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let mut acc = 0f32;
        for (k, t) in h.iter().enumerate() {
            let idx = i as isize - center as isize + k as isize;
            let idx = if idx < 0 {
                0
            } else if idx as usize >= n {
                n - 1
            } else {
                idx as usize
            };
            acc += t * s16(idx);
        }
        let v = acc.clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

// ---------------- base64（标准字母表，含 padding；手写避免加依赖） ----------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

// ---------------- 子进程等待 ----------------

/// 轮询等子进程退出，超秒数杀之（返回错误）。wifi-join/snd-play 都可能挂。
pub fn wait_limited(child: &mut Child, secs: u32) -> Result<(), String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs as u64);
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                return if st.success() {
                    Ok(())
                } else {
                    Err(format!("exit {st}"))
                };
            }
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("timeout".into());
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("wait: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b64_known_vectors() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn wav_wrap_and_span_roundtrip() {
        let raw = vec![0u8; 1000];
        let wav = wav_wrap(&raw, RATE, 1);
        let (off, len) = wav_data_span(&wav).unwrap();
        assert_eq!(&wav[off..off + len], &raw[..]);
        // fmt 块非 data，跳过后命中 data
        assert_eq!(off, 44);
    }

    #[test]
    fn wav_span_handles_extra_chunks() {
        // RIFF + 一个 junk 块（奇数长度，吃 padding）+ data
        let mut w = Vec::new();
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&0u32.to_le_bytes());
        w.extend_from_slice(b"WAVE");
        w.extend_from_slice(b"junk");
        w.extend_from_slice(&3u32.to_le_bytes());
        w.extend_from_slice(b"abc");
        w.extend_from_slice(&[0]); // padding
        w.extend_from_slice(b"data");
        w.extend_from_slice(&2u32.to_le_bytes());
        w.extend_from_slice(b"xy");
        let (off, len) = wav_data_span(&w).unwrap();
        assert_eq!(&w[off..off + len], b"xy");
    }

    #[test]
    fn wav_rate_reads_fmt() {
        let raw = vec![0u8; 100];
        assert_eq!(wav_rate(&wav_wrap(&raw, 24_000, 1)).unwrap(), 24_000);
        assert_eq!(wav_rate(&wav_wrap(&raw, RATE, 1)).unwrap(), RATE);
        assert!(wav_rate(b"not a wav").is_err());
    }

    #[test]
    fn resample_identity_and_upsample() {
        let raw = vec![1u8, 2, 3, 4, 5, 6, 7, 8]; // 4 样本
        assert_eq!(resample(&raw, 48_000, 48_000).unwrap(), raw);
        // 24k→48k：时长不变 → 样本数翻倍
        let up = resample(&raw, 24_000, 48_000).unwrap();
        assert_eq!(up.len(), raw.len() * 2);
        // 单样本输入不越界
        assert_eq!(resample(&[9, 9], 24_000, 48_000).unwrap().len(), 4);
        assert!(resample(&raw, 0, 48_000).is_err());
    }

    #[test]
    fn resample_fir_dc_invariant() {
        // DC 过任意比率仍是同值 DC（每相归一到单位增益）——线性插值时代
        // 的首样本断言不适用于 FIR，DC 不变性是重采样器的底线性质。
        let n = 400;
        let raw: Vec<u8> = (0..n).flat_map(|_| 1000i16.to_le_bytes()).collect();
        for (from, to) in [(44_100u32, 48_000u32), (24_000, 48_000), (48_000, 16_000)] {
            let out = resample(&raw, from, to).unwrap();
            let s: Vec<i16> = out
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            assert_eq!(s.len(), n * to as usize / from as usize);
            // 端部是滤波器暂态（历史钳位），只断言稳态段
            let edge = 64;
            let mid = &s[edge..s.len() - edge];
            assert!(
                mid.iter().all(|&v| (v - 1000).abs() <= 2),
                "{from}->{to} dc leaked: {:?}…{:?}",
                &s[..8.min(s.len())],
                &s[s.len() - 8.min(s.len())..]
            );
        }
    }

    #[test]
    fn lowpass_kills_hf_passes_speech_band() {
        // 12k 正弦（M42e 破音判别：melo 高频毛刺 >7.5k）应被压到噪声级；
        // 440Hz 语音带原样通过。稳态段断言，端部 96 样本是滤波暂态。
        let rate = 48_000u32;
        let sine = |f: f64, n: usize| -> Vec<u8> {
            (0..n)
                .map(|i| {
                    let v = (8000.0
                        * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin())
                        as i16;
                    v.to_le_bytes().to_vec()
                })
                .flatten()
                .collect::<Vec<u8>>()
        };
        let out = lowpass(&sine(12_000.0, 1000), rate, 7_500.0);
        let s: Vec<i16> = out
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let hf_peak = s[200..800].iter().map(|v| v.abs()).max().unwrap();
        assert!(hf_peak < 80, "12k leaked through: {hf_peak}");
        let out = lowpass(&sine(440.0, 1000), rate, 7_500.0);
        let s: Vec<i16> = out
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let sp_peak = s[200..800].iter().map(|v| v.abs()).max().unwrap();
        assert!((sp_peak - 8000).abs() < 200, "440 attenuated: {sp_peak}");
        // 长度不变（原地替换滤波，不动帧结构）
        assert_eq!(out.len(), 2000);
    }

    #[test]
    fn vol_clamped_to_audible_floor() {
        // 地板 20：键调/文件都不能把纯语音产品调成哑巴（2026-09-03 收据）
        assert_eq!(clamp_vol(0), 20);
        assert_eq!(clamp_vol(-30), 20);
        assert_eq!(clamp_vol(19), 20);
        assert_eq!(clamp_vol(20), 20);
        assert_eq!(clamp_vol(60), 60);
        assert_eq!(clamp_vol(100), 100);
        assert_eq!(clamp_vol(150), 100);
    }

    #[test]
    fn split_clauses_punctuation_and_limits() {
        // 句读切句；尾句 3 字 <4 并前句
        assert_eq!(
            split_clauses("已连接无线网络。现在可以开始，对话。"),
            vec!["已连接无线网络。", "现在可以开始，对话。"]
        );
        // 逗号不是切点（melo 常驻后拆散断语气，2026-09-04）——整句一口气
        let cs = split_clauses("无线网络已经连接成功，现在可以。");
        assert_eq!(cs, vec!["无线网络已经连接成功，现在可以。"]);
        // 无标点长段 60 字硬切，内容不丢
        let long = "字".repeat(130);
        let cs = split_clauses(&long);
        assert!(cs.iter().all(|c| c.chars().count() <= 60));
        assert_eq!(cs.concat(), long);
        // 硬切不落数字链中间：链后补刀，链完整进前句或后句
        let t = format!("{}电话{}", "字".repeat(58), "13800138000");
        let cs = split_clauses(&t);
        assert!(cs.iter().all(|c| !c.ends_with(|c: char| c.is_ascii_digit())
            || c.ends_with("13800138000")));
        assert_eq!(cs.concat(), t);
        // 小数点后跟数字不是切点（「3.14」不拦腰断）
        assert_eq!(split_clauses("圆周率是3.14约等于。"), vec!["圆周率是3.14约等于。"]);
        // 空文本给单句（调用方兜底）
        assert_eq!(split_clauses(""), vec![""]);
    }

    #[test]
    fn expand_digit_chains_phone_ip_and_short() {
        // 电话（连字符分段）：总 11 位 ≥5 → 逐位
        assert_eq!(
            expand_digit_chains("TEL 138-0013-8000"),
            "TEL 1 3 8 0 0 1 3 8 0 0 0"
        );
        // 连写
        assert_eq!(
            expand_digit_chains("电话13800138000。"),
            "电话1 3 8 0 0 1 3 8 0 0 0。"
        );
        // IP（点分段，状态查询的话术）
        assert_eq!(
            expand_digit_chains("连上了，地址192.168.0.166。"),
            "连上了，地址1 9 2 1 6 8 0 1 6 6。"
        );
        // 空格分段同样成链
        assert_eq!(
            expand_digit_chains("138 0013 8000"),
            "1 3 8 0 0 1 3 8 0 0 0"
        );
        // 短链不动：数量/年份/小数/版本号交给前端按数读
        assert_eq!(expand_digit_chains("找到3个网络"), "找到3个网络");
        assert_eq!(expand_digit_chains("2026年"), "2026年");
        assert_eq!(expand_digit_chains("3.14"), "3.14");
        assert_eq!(expand_digit_chains("v1.2.3"), "v1.2.3");
        // 「第 3 章」：空格后随数字才算链内，两组不相粘
        assert_eq!(expand_digit_chains("第3章 第5节"), "第3章 第5节");
        // 链前分隔不吞：空格留在原位
        assert_eq!(expand_digit_chains("验证码 654321"), "验证码 6 5 4 3 2 1");
    }
}
