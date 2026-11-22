use crate::platform::PcmFrame;

/// Target PCM sample rate required by `gemini-3.5-transcribe-live` (ADR-0002).
pub const TARGET_SAMPLE_RATE_HZ: u32 = 16_000;

/// Target channel count required by `gemini-3.5-transcribe-live` (mono).
pub const TARGET_CHANNELS: u16 = 1;

/// Default streaming chunk size: 20ms at 16kHz mono (320 samples = 640 PCM bytes).
pub const DEFAULT_CHUNK_SAMPLES: usize = 320;

/// Convert normalized `f32` samples in `[-1.0, 1.0]` to signed 16-bit PCM.
pub fn f32_samples_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let clamped = if s.is_finite() {
                s.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            (clamped * f32::from(i16::MAX)).round() as i16
        })
        .collect()
}

/// Convert unsigned 16-bit PCM samples (`[0, 65535]`, midpoint `32768`) to signed 16-bit PCM.
pub fn u16_samples_to_i16(samples: &[u16]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| (i32::from(s) - 32_768) as i16)
        .collect()
}

/// Downmix interleaved multi-channel 16-bit PCM samples to mono.
pub fn downmix_to_mono(samples: &[i16], channels: u16) -> Vec<i16> {
    if samples.is_empty() || channels == 0 {
        return Vec::new();
    }
    if channels == 1 {
        return samples.to_vec();
    }
    let ch = usize::from(channels);
    samples
        .chunks(ch)
        .map(|chunk| {
            let sum: i64 = chunk.iter().map(|&s| i64::from(s)).sum();
            (sum / chunk.len() as i64).clamp(i64::from(i16::MIN), i64::from(i16::MAX)) as i16
        })
        .collect()
}

/// Stateful streaming downmixer, linear resampler, and chunker targeting 16kHz mono 16-bit PCM.
///
/// Preserves fractional sample phase across consecutive hardware frames so arbitrary sample rates
/// (e.g., 44.1kHz, 48kHz, 96kHz) resample without boundary drift or clicks.
#[derive(Debug, Clone)]
pub struct AudioResampler {
    chunk_size_samples: usize,
    pending_out: Vec<i16>,
    last_sample: Option<i16>,
    src_pos: f64,
    last_rate_hz: Option<u32>,
}

impl AudioResampler {
    pub fn new(chunk_size_samples: usize) -> Self {
        Self {
            chunk_size_samples: chunk_size_samples.max(1),
            pending_out: Vec::new(),
            last_sample: None,
            src_pos: 0.0,
            last_rate_hz: None,
        }
    }

    /// Push a hardware `PcmFrame` and return any complete 16kHz mono PCM chunks.
    pub fn push_frame(&mut self, frame: &PcmFrame) -> Vec<Vec<i16>> {
        if frame.samples.is_empty() || frame.channels == 0 || frame.sample_rate_hz == 0 {
            return Vec::new();
        }

        let mono = downmix_to_mono(&frame.samples, frame.channels);
        if mono.is_empty() {
            return Vec::new();
        }

        if self.last_rate_hz != Some(frame.sample_rate_hz) {
            self.last_rate_hz = Some(frame.sample_rate_hz);
            self.src_pos = 0.0;
            self.last_sample = None;
        }

        if frame.sample_rate_hz == TARGET_SAMPLE_RATE_HZ {
            self.pending_out.extend_from_slice(&mono);
            self.last_sample = mono.last().copied();
            return self.drain_full_chunks();
        }

        let ratio = f64::from(frame.sample_rate_hz) / f64::from(TARGET_SAMPLE_RATE_HZ);
        let mut combined = Vec::with_capacity(mono.len() + 1);
        if let Some(prev) = self.last_sample {
            combined.push(prev);
        }
        combined.extend_from_slice(&mono);

        let mut pos = self.src_pos;
        let limit = (combined.len() - 1) as f64;
        while pos < limit {
            let idx = pos.floor() as usize;
            let next_idx = (idx + 1).min(combined.len() - 1);
            let frac = pos - (idx as f64);
            let sample = (f64::from(combined[idx]) * (1.0 - frac)
                + f64::from(combined[next_idx]) * frac)
                .round()
                .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
            self.pending_out.push(sample);
            pos += ratio;
        }

        self.src_pos = pos - (combined.len() - 1) as f64;
        self.last_sample = mono.last().copied();

        self.drain_full_chunks()
    }

    /// Push a hardware `PcmFrame` and immediately return 16kHz mono PCM chunks of at most
    /// `chunk_size_samples`, without holding a partial tail across frames.
    pub fn push_frame_immediate(&mut self, frame: &PcmFrame) -> Vec<Vec<i16>> {
        let mut chunks = self.push_frame(frame);
        if !self.pending_out.is_empty() {
            chunks.push(std::mem::take(&mut self.pending_out));
        }
        chunks
    }

    /// Flush any remaining buffered 16kHz mono samples as a final chunk and reset resampler state.
    pub fn flush(&mut self) -> Vec<Vec<i16>> {
        self.last_sample = None;
        self.src_pos = 0.0;
        self.last_rate_hz = None;
        if self.pending_out.is_empty() {
            Vec::new()
        } else {
            vec![std::mem::take(&mut self.pending_out)]
        }
    }

    fn drain_full_chunks(&mut self) -> Vec<Vec<i16>> {
        let mut chunks = Vec::new();
        while self.pending_out.len() >= self.chunk_size_samples {
            let chunk: Vec<i16> = self.pending_out.drain(..self.chunk_size_samples).collect();
            chunks.push(chunk);
        }
        chunks
    }
}

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
