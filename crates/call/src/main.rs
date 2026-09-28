//! SIP interop target — minimal callee/caller for testing against real
//! third-party softphones (Linphone, pjsua). Spike for刀1 (M48①).
//!
//! - `aginx-call [answer] [port=5060] [max_secs=60]` — listen, auto-answer (tone mode)
//! - `aginx-call dial <sip:uri> [max_secs=60] [port=5060]` — place outbound call (tone mode)
//! - `aginx-call talk [sip:uri] [port=5060]` — real mic/speaker call: with a URI
//!   it dials, without it waits and answers. Ctrl+C hangs up.
//! - tone modes send a continuous 440 Hz PCMU/Opus tone (paced: one 20 ms frame
//!   per tick) and exit 0 after >= 2 s of received non-silent audio

// talk backend: cpal on macOS, raw ALSA ioctls on the phone (刀2 — L0 has
// no libasound, so cpal's ALSA host can't link there).
mod dsp;
#[cfg(target_os = "macos")]
mod talk;
#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "linux")]
#[path = "talk_dev.rs"]
mod talk;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rvoip_sip::{Config, Endpoint, EndpointAudioFrame, EndpointProfile};

const SAMPLE_RATE: u32 = 8_000;
const FRAME_MS: u64 = 20;
const TONE_AMPLITUDE: f32 = 0.30;
const MIN_RECEIVED_SECS: f32 = 2.0;

#[derive(PartialEq)]
enum Mode {
    Tone { uri: Option<String>, max_secs: u64 },
    Talk { uri: Option<String> },
}

#[derive(Default)]
struct RxStats {
    frames: u64,
    samples: usize, // interleaved sample count
    // Negotiated (sample_rate, channels), observed from the first decoded
    // frame — the sender adapts to it (codec_runtime::encode hard-rejects
    // frames whose rate/channels differ from the negotiated format).
    format: Option<(u32, u8)>,
    recent: Vec<f32>, // last ~5 s, for the periodic report
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    // answer [port=5060] [max_secs=60] | dial <uri> [max_secs=60] [port=5060]
    // | talk [uri] [port=5060]
    let (mode, port, max_secs) = match args.first().map(String::as_str) {
        Some("talk") => {
            let uri = args.get(1).filter(|s| s.starts_with("sip:")).cloned();
            let port = match uri {
                Some(_) => args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5060),
                None => args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5060),
            };
            (Mode::Talk { uri }, port, u64::MAX)
        }
        Some("dial") => {
            let uri = args
                .get(1)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("dial needs <sip:uri>"))?;
            let max_secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60);
            let port = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5060);
            (Mode::Tone { uri: Some(uri), max_secs }, port, max_secs)
        }
        mode => {
            let port: u16 = match mode {
                Some("answer") | None => args
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(5060),
                _ => args.first().and_then(|s| s.parse().ok()).unwrap_or(5060),
            };
            let max_secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(60);
            (Mode::Tone { uri: None, max_secs }, port, max_secs)
        }
    };

    let mut config = Config::local("aginx", port);
    // 刀3 LAN legs: Config::local hardcodes 127.0.0.1 into local_ip /
    // bind_addr / local_uri — a loopback-bound UDP socket can't even send to
    // a LAN peer, so both legs need a real interface IP.
    if let Ok(bind) = std::env::var("AGINX_CALL_BIND") {
        let ip: std::net::IpAddr = bind.parse().expect("AGINX_CALL_BIND: invalid ip");
        config.local_ip = ip;
        config.bind_addr = std::net::SocketAddr::new(ip, port);
        config.local_uri = format!("sip:aginx@{ip}:{port}");
    }
    config.media_port_start = 17_600;
    config.media_port_end = 17_699;
    // The unified Endpoint's SDP offer comes from Config::offered_codecs —
    // default [0,8,101] (PCMU/PCMA/telephone-event), Opus is NOT auto-added by
    // the opus feature. With opus-media, offer Opus (PT 111) first.
    #[cfg(feature = "opus-media")]
    {
        config.offered_codecs = vec![111, 0, 8, 101];
    }

    let mut endpoint = Endpoint::builder()
        .name("aginx")
        .profile(EndpointProfile::Custom(config))
        .build()
        .await?;

    let is_dialer = matches!(&mode, Mode::Tone { uri: Some(_), .. } | Mode::Talk { uri: Some(_) });
    let call = match &mode {
        Mode::Tone { uri: Some(uri), .. } | Mode::Talk { uri: Some(uri), .. } => {
            println!("[aginx-call] dialing {uri}");
            endpoint
                .call_and_wait(&uri, Some(Duration::from_secs(15)))
                .await?
        }
        Mode::Tone { uri: None, .. } | Mode::Talk { uri: None } => {
            println!("[aginx-call] waiting on sip:aginx@127.0.0.1:{port} (auto-answer)");
            let incoming = endpoint.wait_for_incoming().await?;
            println!("[aginx-call] incoming call from {}", incoming.from());
            incoming.answer().await?
        }
    };
    println!("[aginx-call] call up as {}", call.id());

    let audio = call.audio().await?;
    let (sender, mut receiver) = audio.split();

    if let Mode::Talk { .. } = mode {
        // default send format until the peer's first frame reveals the
        // negotiated one (opus build: 48k/2ch, else PCMU 8k/1ch)
        #[cfg(feature = "opus-media")]
        let default_format = (48_000u32, 2u8);
        #[cfg(not(feature = "opus-media"))]
        let default_format = (SAMPLE_RATE, 1u8);
        let running = talk::start(sender, receiver, default_format).await?;
        println!("[aginx-call] talk bridge up — Ctrl+C to hang up");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => println!("[aginx-call] Ctrl+C — hanging up"),
            _ = call.wait_for_end(None) => println!("[aginx-call] remote ended the call"),
        }
        drop(running);
        let _ = call.hangup_and_wait(Some(Duration::from_secs(3))).await;
        endpoint.shutdown().await?;
        return Ok(());
    }

    let stats = Arc::new(Mutex::new(RxStats::default()));
    let stats_rx = stats.clone();
    let receive_task = tokio::spawn(async move {
        while let Some(frame) = receiver.recv().await {
            if let Ok(mut s) = stats_rx.lock() {
                if s.format.is_none() {
                    s.format = Some((frame.sample_rate, frame.channels));
                    println!(
                        "[aginx-call] rx format negotiated: {}Hz/{}ch",
                        frame.sample_rate, frame.channels
                    );
                }
                s.frames += 1;
                s.samples += frame.samples.len();
                s.recent.extend(frame.samples.iter().map(|v| *v as f32 / i16::MAX as f32));
                let keep = frame.sample_rate as usize * frame.channels as usize * 5;
                if s.recent.len() > keep {
                    let overflow = s.recent.len() - keep;
                    s.recent.drain(..overflow);
                }
            }
        }
    });

    // Send tone until the call ends. Pacing law (from rvoip sip_client):
    // exactly one frame per interval tick, never per-frame sleep.
    // Format: hold off until the negotiated format is observed on the first
    // received frame — codec_runtime::encode hard-rejects rate/channel
    // mismatches AND the rvoip sender pump (handle.rs audio) breaks on the
    // first such error, killing the send path for the rest of the call.
    // The dialer breaks the both-waiting deadlock after a grace period by
    // sending in its per-build default format.
    let stats_tx = stats.clone();
    let send_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_MS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        #[cfg(feature = "opus-media")]
        let (default_rate, default_ch) = (48_000u32, 2u8);
        #[cfg(not(feature = "opus-media"))]
        let (default_rate, default_ch) = (SAMPLE_RATE, 1u8);
        let (mut rate, mut ch) = (default_rate, default_ch);
        let mut phase = 0.0f32;
        let mut timestamp = 0u32;
        let start = std::time::Instant::now();
        loop {
            ticker.tick().await;
            let observed = stats_tx.lock().ok().and_then(|s| s.format);
            match observed {
                Some((r, c)) => {
                    rate = r;
                    ch = c;
                }
                None if !is_dialer || start.elapsed() < Duration::from_millis(600) => {
                    continue; // wait for the peer's first frame to reveal the format
                }
                None => {} // dialer grace period over: try the default format
            }
            let per_ch = rate as usize * FRAME_MS as usize / 1000; // 20 ms of samples per channel
            let phase_step = std::f32::consts::TAU * 440.0 / rate as f32;
            let mut samples: Vec<i16> = Vec::with_capacity(per_ch * ch as usize);
            for _ in 0..per_ch {
                let v = (TONE_AMPLITUDE * phase.sin().clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                phase += phase_step;
                if phase >= std::f32::consts::TAU {
                    phase -= std::f32::consts::TAU;
                }
                for _ in 0..ch {
                    samples.push(v); // same tone on every channel
                }
            }
            let frame = EndpointAudioFrame::new(samples, rate, ch, timestamp);
            timestamp = timestamp.wrapping_add(per_ch as u32);
            if sender.send(frame).await.is_err() {
                return;
            }
        }
    });

    // Periodic stats + max-duration cap.
    let started = std::time::Instant::now();
    let end = loop {
        let ended = call
            .wait_for_end(Some(Duration::from_secs(1)))
            .await
            .map(|_| "bye")
            .ok();
        if let Some(how) = ended {
            break how;
        }
        if started.elapsed() >= Duration::from_secs(max_secs) {
            break "timeout";
        }
        if let Ok(s) = stats.lock() {
            let (rate, ch) = s.format.unwrap_or((
                #[cfg(feature = "opus-media")]
                48_000,
                #[cfg(not(feature = "opus-media"))]
                SAMPLE_RATE,
                1,
            ));
            let (rms, dominant) = rms_and_dominant(&s.recent, rate);
            println!(
                "[aginx-call] t={:>3}s frames={} samples={} fmt={}Hz/{}ch recent-rms={:.4} dominant={:.0}Hz",
                started.elapsed().as_secs(),
                s.frames,
                s.samples,
                rate,
                ch,
                rms,
                dominant
            );
        }
    };
    println!("[aginx-call] call ended: {end}");

    send_task.abort();
    receive_task.abort();
    let _ = send_task.await;
    let _ = receive_task.await;
    let _ = call.hangup_and_wait(Some(Duration::from_secs(3))).await;

    let (total_frames, total_samples, fmt) = stats
        .lock()
        .map(|s| (s.frames, s.samples, s.format))
        .unwrap_or((0, 0, None));
    println!("[aginx-call] total rx frames={} samples={} fmt={fmt:?}", total_frames, total_samples);
    let (rate, ch) = fmt.unwrap_or((SAMPLE_RATE, 1));
    let secs = total_samples as f32 / (rate * ch as u32) as f32;
    let ok = secs >= MIN_RECEIVED_SECS;
    if ok {
        println!("✅ interop media received ({secs:.1}s of audio @ {rate}Hz/{ch}ch)");
    } else {
        println!("❌ insufficient media received ({secs:.1}s < {MIN_RECEIVED_SECS}s)");
    }
    endpoint.shutdown().await?;
    if ok {
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn rms_and_dominant(samples: &[f32], rate: u32) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let power = samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32;
    let mut best_hz = 0.0;
    let mut best = 0.0f32;
    for hz in (100..=3_500).step_by(50) {
        let omega = std::f32::consts::TAU * hz as f32 / rate as f32;
        let coeff = 2.0 * omega.cos();
        let (mut q1, mut q2) = (0.0f32, 0.0f32);
        for s in samples {
            let q0 = coeff * q1 - q2 + s;
            q2 = q1;
            q1 = q0;
        }
        let p = (q1 * q1 + q2 * q2 - coeff * q1 * q2) / (samples.len() * samples.len()) as f32;
        if p > best {
            best = p;
            best_hz = hz as f32;
        }
    }
    (power.sqrt(), best_hz)
}
