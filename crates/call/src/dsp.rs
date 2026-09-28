//! Shared DSP helpers + pacing constants for the talk bridge (both
//! backends: Mac/cpal and Linux/ALSA-ioctl use the same laws).

use std::time::Duration;

pub const FRAME_MS: u64 = 20;
pub const MAX_MIC_ACC: usize = 24_000; // 0.5 s of 48 kHz — cap regardless of rate
pub const MAX_PLAYBACK: Duration = Duration::from_millis(300);

#[derive(Default)]
pub struct Levels {
    pub tx_rms: f32,
    pub rx_rms: f32,
    pub negotiated: Option<(u32, u8)>,
}

pub fn mix_to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / frame.len() as f32)
        .collect()
}

pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == to_rate {
        return input.to_vec();
    }
    let out_len = ((input.len() as u64 * to_rate as u64) / from_rate as u64).max(1) as usize;
    let ratio = from_rate as f32 / to_rate as f32;
    let mut output = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f32 * ratio;
        let idx = pos.floor() as usize;
        let frac = pos - idx as f32;
        let a = input.get(idx).copied().unwrap_or(0.0);
        let b = input.get(idx + 1).copied().unwrap_or(a);
        output.push(a + (b - a) * frac);
    }
    output
}

pub fn float_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}
