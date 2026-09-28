//! Device (redfin/L0) audio bridge for interactive calls — M48 刀2. // D14-exempt: provenance comment, facts live in devices/redfin/device.toml
//!
//! Linux twin of talk.rs (Mac/cpal): cpal is not an option on L0 (alsa-sys
//! wants libasound, the image has none), so this bridges the same way the
//! platform already does — raw PCM ioctls (alsa.rs, the snd-cap/snd-play
//! contract) on two blocking std threads:
//!
//!   capture thread: READI period → mono f32 → mpsc   (replaces cpal input)
//!   playback thread: jitter deque → dup L=R → WRITEI  (replaces cpal output)
//!
//! The three async tasks (speaker decode/resample, mic 20 ms pacer, VU)
//! keep the Mac version's laws verbatim: one 20 ms frame per interval
//! tick, silence on underrun, capped accumulation/playback buffers,
//! send format learned from the first received frame.
//!
//! Machine facts (devices/redfin/device.toml [audio]): capture // D14-exempt: pointer to the legal source, not inline data
//! /dev/snd/pcmC0D0c 48000/1 (capture mono is the law, M42a), playback
//! /dev/snd/pcmC0D0p 48000/2 with L=R copy (QUIN_TDM quirk). Volume scales
//! samples before WRITEI (snd-play's integer path) — 60 default, not 75+
//! (M42e: 75 overdrives the amp into buzz).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use rvoip_sip::{EndpointAudioFrame, EndpointAudioReceiver, EndpointAudioSender};

use crate::alsa::Pcm;
use crate::dsp::{self, Levels, FRAME_MS, MAX_MIC_ACC, MAX_PLAYBACK};

const VOL: i32 = 60;

pub struct RunningTalk {
    stop: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    _playback_drain: bool,
}

impl Drop for RunningTalk {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for task in self.tasks.drain(..) {
            task.abort();
        }
        // playback thread drains + closes on exit; capture thread exits at
        // its next period boundary (blocking READI returns first).
    }
}

/// Wire mic/speaker PCM devices to an audio sender+receiver pair.
/// `default_format` is used for sending until the first received frame
/// reveals the negotiated (rate, channels).
pub async fn start(
    sender: EndpointAudioSender,
    mut receiver: EndpointAudioReceiver,
    default_format: (u32, u8),
) -> anyhow::Result<RunningTalk> {
    let cap_dev = std::env::var("AGINX_CALL_PCM_CAP").unwrap_or_else(|_| "/dev/snd/pcmC0D0c".into());
    let play_dev = std::env::var("AGINX_CALL_PCM_PLAY").unwrap_or_else(|_| "/dev/snd/pcmC0D0p".into());
    let cap = Pcm::open(&cap_dev, 48_000, 1, true)?;
    let play = Pcm::open(&play_dev, 48_000, 2, false)?;
    let (in_rate, in_ch) = (cap.rate as usize, cap.channels as usize);
    let (out_rate, out_ch) = (play.rate as usize, play.channels as usize);

    let stop = Arc::new(AtomicBool::new(false));
    let (mic_tx, mut mic_rx) = mpsc::unbounded_channel::<Vec<f32>>();
    let playback = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(out_rate)));
    let levels = Arc::new(Mutex::new(Levels::default()));

    // Capture thread: one blocking READI per period → mono f32 chunk.
    let stop_cap = stop.clone();
    let cap_thread = std::thread::Builder::new()
        .name("pcm-cap".into())
        .spawn(move || {
            let mut buf = vec![0i16; cap.period * in_ch];
            loop {
                if stop_cap.load(Ordering::Relaxed) {
                    break;
                }
                match cap.xferi(&mut buf) {
                    Ok(n) if n > 0 => {
                        let mono: Vec<f32> = buf[..n * in_ch]
                            .iter()
                            .map(|s| *s as f32 / i16::MAX as f32)
                            .collect();
                        let mono = dsp::mix_to_mono(&mono, in_ch);
                        if mic_tx.send(mono).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("[talk] capture READI: {e}");
                        break;
                    }
                }
            }
        })?;

    // Playback thread: drain the jitter deque into WRITEI; underrun writes
    // silence so the stream keeps running (snd-play pacing).
    let stop_play = stop.clone();
    let playback_play = playback.clone();
    let play_thread = std::thread::Builder::new()
        .name("pcm-play".into())
        .spawn(move || {
            let mut buf = vec![0i16; play.period * out_ch];
            loop {
                if stop_play.load(Ordering::Relaxed) {
                    play.drain();
                    break;
                }
                let mut n_samples = 0;
                if let Ok(mut deque) = playback_play.lock() {
                    for slot in buf.chunks_mut(out_ch) {
                        let s = deque.pop_front().unwrap_or(0.0);
                        let v = (dsp::float_to_i16(s) as i32 * VOL / 100) as i16;
                        for out in slot.iter_mut() {
                            *out = v; // L=R copy (QUIN_TDM quirk)
                        }
                        n_samples += 1;
                    }
                }
                let _ = n_samples;
                if let Err(e) = play.xferi(&mut buf) {
                    eprintln!("[talk] playback WRITEI: {e}");
                    break;
                }
            }
        })?;

    // Speaker side: decode frames → mono f32 → resample to device rate → buffer.
    let levels_rx = levels.clone();
    let speaker_task = tokio::spawn(async move {
        while let Some(frame) = receiver.recv().await {
            {
                let Ok(mut l) = levels_rx.lock() else { return };
                if l.negotiated.is_none() {
                    l.negotiated = Some((frame.sample_rate, frame.channels));
                }
            }
            let mono: Vec<f32> = frame
                .samples
                .chunks(frame.channels as usize)
                .map(|ch| ch.iter().map(|s| *s as f32 / i16::MAX as f32).sum::<f32>() / ch.len() as f32)
                .collect();
            let rms = dsp::rms(&mono);
            let resampled = dsp::resample_linear(&mono, frame.sample_rate, out_rate as u32);
            if let Ok(mut buffer) = playback.lock() {
                buffer.extend(resampled);
                let max_len = out_rate * MAX_PLAYBACK.as_millis() as usize / 1000;
                while buffer.len() > max_len {
                    buffer.pop_front();
                }
            }
            if let Ok(mut l) = levels_rx.lock() {
                l.rx_rms = rms;
            }
        }
    });

    // Mic side: accumulate captured mono; every 20 ms emit one frame in the
    // negotiated format (grace: dial with the default until the peer's
    // first frame reveals what was actually negotiated).
    let levels_tx = levels.clone();
    let mic_task = tokio::spawn(async move {
        let mut acc: Vec<f32> = Vec::new();
        let mut timestamp: u32 = 0;
        let mut ticker = tokio::time::interval(Duration::from_millis(FRAME_MS));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = ticker.tick() => {
                    let (rate, ch) = levels_tx
                        .lock()
                        .ok()
                        .and_then(|l| l.negotiated)
                        .unwrap_or(default_format);
                    let per_ch = rate as usize * FRAME_MS as usize / 1000;
                    // input samples needed to produce per_ch at the send rate
                    // (+1 so linear interpolation has its right neighbor)
                    let need_in = ((per_ch as f64 * in_rate as f64 / rate as f64).ceil() as usize) + 1;
                    let mono: Vec<f32> = if acc.len() >= need_in {
                        let mut resampled = dsp::resample_linear(&acc[..need_in], in_rate as u32, rate);
                        resampled.truncate(per_ch);
                        // keep the last input sample as the next frame's neighbor
                        acc.drain(..need_in - 1);
                        resampled
                    } else {
                        vec![0.0; per_ch] // underrun: silence keeps the RTP cadence
                    };
                    if acc.len() > MAX_MIC_ACC {
                        let overflow = acc.len() - MAX_MIC_ACC;
                        acc.drain(..overflow);
                    }
                    let rms_now = dsp::rms(&mono);
                    let mut pcm = Vec::with_capacity(per_ch * ch as usize);
                    for sample in mono {
                        let v = dsp::float_to_i16(sample);
                        for _ in 0..ch {
                            pcm.push(v);
                        }
                    }
                    let frame = EndpointAudioFrame::new(pcm, rate, ch, timestamp);
                    timestamp = timestamp.wrapping_add(per_ch as u32);
                    if sender.send(frame).await.is_err() {
                        return;
                    }
                    if let Ok(mut l) = levels_tx.lock() {
                        l.tx_rms = rms_now;
                    }
                }
                maybe = mic_rx.recv() => {
                    let Some(samples) = maybe else { return };
                    acc.extend(samples);
                }
            }
        }
    });

    // Level ticker: a one-line VU so a human sees both directions are alive.
    let levels_vu = levels.clone();
    let vu_task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            if let Ok(l) = levels_vu.lock() {
                println!(
                    "[talk] tx-rms={:.4} rx-rms={:.4} fmt={:?}",
                    l.tx_rms, l.rx_rms, l.negotiated
                );
            }
        }
    });

    // PCM threads are detached: they observe `stop` and exit at the next
    // period boundary; the process exit is the hard stop for a spike.
    let _ = (cap_thread, play_thread);

    Ok(RunningTalk {
        stop,
        tasks: vec![speaker_task, mic_task, vu_task],
        _playback_drain: true,
    })
}
