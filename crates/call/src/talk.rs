//! Real mic/speaker bridge for interactive calls (Mac client, M48①).
//!
//! Adapted from rvoip's `sip_client` example audio bridge and the
//! `rvoip-audio-device` crate, with one addition those don't have: the send
//! format follows the *negotiated* codec — learned from the first received
//! frame (Opus 48 kHz/2ch vs PCMU 8 kHz/1ch) — instead of hardcoded 8 kHz.
//!
//! Laws kept from the reference:
//! - one 20 ms frame per interval tick (MissedTickBehavior::Delay), never a
//!   per-frame sleep — sleep drifts to ~22 ms and starves the far end
//! - silence on underrun, accumulation buffer capped (~0.5 s) so latency
//!   can't grow when the capture clock runs ahead of the send clock
//! - playback via a bounded jitter-ish buffer (~300 ms cap)

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

use rvoip_sip::{EndpointAudioFrame, EndpointAudioReceiver, EndpointAudioSender};

use crate::dsp::{self, Levels, FRAME_MS, MAX_MIC_ACC, MAX_PLAYBACK};

pub struct RunningTalk {
    _input_stream: cpal::Stream,
    _output_stream: cpal::Stream,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    /// 末帧到达时刻（#61 看门狗探针）：speaker loop 每收一帧盖戳。
    rx_hb: tokio::sync::watch::Receiver<std::time::Instant>,
}

impl RunningTalk {
    pub fn rx_heartbeat(&self) -> tokio::sync::watch::Receiver<std::time::Instant> {
        self.rx_hb.clone()
    }
}

impl Drop for RunningTalk {
    fn drop(&mut self) {
        for task in self.tasks.drain(..) {
            task.abort();
        }
    }
}

/// Wire the default mic/speaker to an audio sender+receiver pair.
/// `default_format` is used for sending until the first received frame
/// reveals the negotiated (rate, channels).
pub async fn start(
    sender: EndpointAudioSender,
    mut receiver: EndpointAudioReceiver,
    default_format: (u32, u8),
) -> anyhow::Result<RunningTalk> {
    let host = cpal::default_host();
    let input = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default input device (mic permission denied?)"))?;
    let output = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default output device"))?;
    let in_cfg = input.default_input_config()?;
    let out_cfg = output.default_output_config()?;
    let in_rate = in_cfg.sample_rate().0;
    let out_rate = out_cfg.sample_rate().0;
    let in_ch = in_cfg.channels() as usize;
    let out_ch = out_cfg.channels() as usize;
    println!(
        "[talk] devices: in {}Hz/{in_ch}ch, out {}Hz/{out_ch}ch",
        in_cfg.sample_rate().0,
        out_cfg.sample_rate().0
    );

    let (mic_tx, mut mic_rx) = mpsc::unbounded_channel::<Vec<f32>>();
    let playback = Arc::new(Mutex::new(VecDeque::<f32>::with_capacity(out_rate as usize)));
    let levels = Arc::new(Mutex::new(Levels::default()));

    let input_stream = build_input_stream(&input, &in_cfg.into(), in_ch, mic_tx)?;
    let output_stream = build_output_stream(&output, &out_cfg.into(), out_ch, playback.clone())?;
    input_stream.play()?;
    output_stream.play()?;

    // #61 看门狗心跳：每帧一戳，main 的 rx_stall 臂盯死线。
    let (hb_tx, hb_rx) = tokio::sync::watch::channel(std::time::Instant::now());

    // Speaker side: decode frames → mono f32 → resample to device rate → buffer.
    let levels_rx = levels.clone();
    let speaker_task = tokio::spawn(async move {
        while let Some(frame) = receiver.recv().await {
            hb_tx.send_replace(std::time::Instant::now());
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
            let resampled = dsp::resample_linear(&mono, frame.sample_rate, out_rate);
            if let Ok(mut buffer) = playback.lock() {
                buffer.extend(resampled);
                let max_len = out_rate as usize * MAX_PLAYBACK.as_millis() as usize / 1000;
                while buffer.len() > max_len {
                    buffer.pop_front();
                }
            }
            if let Ok(mut l) = levels_rx.lock() {
                l.rx_rms = rms;
            }
        }
    });

    // Mic side: accumulate captured mono at device rate; every 20 ms emit one
    // frame in the negotiated format (grace: dial with the default until the
    // peer's first frame reveals what was actually negotiated).
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
                        let mut resampled = dsp::resample_linear(&acc[..need_in], in_rate, rate);
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

    Ok(RunningTalk {
        _input_stream: input_stream,
        _output_stream: output_stream,
        tasks: vec![speaker_task, mic_task, vu_task],
        rx_hb: hb_rx,
    })
}

fn build_input_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    tx: mpsc::UnboundedSender<Vec<f32>>,
) -> anyhow::Result<cpal::Stream> {
    let err_fn = |err| eprintln!("[talk] input stream error: {err}");
    let sample_format = device.default_input_config()?.sample_format();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config,
            move |data: &[f32], _| {
                let _ = tx.send(dsp::mix_to_mono(data, channels));
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            config,
            move |data: &[i16], _| {
                let converted = data
                    .iter()
                    .map(|s| *s as f32 / i16::MAX as f32)
                    .collect::<Vec<_>>();
                let _ = tx.send(dsp::mix_to_mono(&converted, channels));
            },
            err_fn,
            None,
        )?,
        other => anyhow::bail!("unsupported input sample format {other:?}"),
    };
    Ok(stream)
}

fn build_output_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    playback: Arc<Mutex<VecDeque<f32>>>,
) -> anyhow::Result<cpal::Stream> {
    let err_fn = |err| eprintln!("[talk] output stream error: {err}");
    let sample_format = device.default_output_config()?.sample_format();
    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _| fill_output(data, channels, &playback, |s| s),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _| fill_output(data, channels, &playback, dsp::float_to_i16),
            err_fn,
            None,
        )?,
        other => anyhow::bail!("unsupported output sample format {other:?}"),
    };
    Ok(stream)
}

fn fill_output<T: Copy>(
    data: &mut [T],
    channels: usize,
    playback: &Arc<Mutex<VecDeque<f32>>>,
    convert: impl Fn(f32) -> T,
) {
    let zero = convert(0.0);
    if let Ok(mut buffer) = playback.lock() {
        for frame in data.chunks_mut(channels.max(1)) {
            let converted = convert(buffer.pop_front().unwrap_or(0.0));
            for out in frame {
                *out = converted;
            }
        }
    } else {
        for out in data {
            *out = zero;
        }
    }
}
