use crate::platform::PcmFrame;

/// Target PCM sample rate required by `gemini-3.5-transcribe-live` (ADR-0002).
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// Target channel count required by `gemini-3.5-transcribe-live` (mono).
pub const TARGET_CHANNELS: u16 = 1;

/// Convert mono 16-bit signed PCM samples into little-endian wire bytes.
pub fn pcm_i16_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Compute normalized RMS audio level strictly clamped to `[0.0, 1.0]`.
pub fn compute_normalized_rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples
        .iter()
        .map(|&s| {
            let norm = f64::from(s) / f64::from(i16::MAX);
            norm * norm
        })
        .sum();
    let rms = (sum_sq / samples.len() as f64).sqrt() as f32;
    rms.clamp(0.0, 1.0)
}

/// Baseline downmix and linear resampling seam to produce 16kHz mono PCM.
///
/// Issue #4 extends this pipeline with full hardware capture and high-quality resampling.
pub fn normalize_frame_to_16khz_mono(frame: &PcmFrame) -> Vec<i16> {
    if frame.samples.is_empty() || frame.channels == 0 || frame.sample_rate_hz == 0 {
        return Vec::new();
    }

    let mono: Vec<i16> = if frame.channels == 1 {
        frame.samples.clone()
    } else {
        let ch = usize::from(frame.channels);
        frame
            .samples
            .chunks(ch)
            .map(|chunk| {
                let sum: i64 = chunk.iter().map(|&s| i64::from(s)).sum();
                (sum / chunk.len() as i64).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
            })
            .collect()
    };

    if frame.sample_rate_hz == TARGET_SAMPLE_RATE_HZ {
        return mono;
    }

    let ratio = f64::from(frame.sample_rate_hz) / f64::from(TARGET_SAMPLE_RATE_HZ);
    let out_len = ((mono.len() as f64) / ratio).round().max(1.0) as usize;
    let mut resampled = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let src_pos = (i as f64) * ratio;
        let idx = (src_pos.floor() as usize).min(mono.len() - 1);
        let next_idx = (idx + 1).min(mono.len() - 1);
        let frac = src_pos - (idx as f64);
        let sample = (f64::from(mono[idx]) * (1.0 - frac) + f64::from(mono[next_idx]) * frac)
            .round()
            .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
        resampled.push(sample);
    }
    resampled
}
