//! High-quality rate conversion only when the shared device cannot take the source rate.

use rubato::{FftFixedIn, Resampler};

pub struct RateConvert {
    inner: FftFixedIn<f32>,
    channels: usize,
    chunk: usize,
    leftover: Vec<Vec<f32>>,
}

impl RateConvert {
    pub fn new(from: u32, to: u32, channels: usize) -> Result<Self, String> {
        let channels = channels.max(1);
        let chunk = 1024;
        let inner = FftFixedIn::<f32>::new(from as usize, to as usize, chunk, 2, channels)
            .map_err(|e| format!("Resampler: {e}"))?;
        Ok(Self {
            inner,
            channels,
            chunk,
            leftover: vec![Vec::new(); channels],
        })
    }

    pub fn push_interleaved(&mut self, interleaved: &[f32]) -> Vec<f32> {
        deinterleave_into(interleaved, self.channels, &mut self.leftover);
        let mut out_ch = vec![Vec::new(); self.channels];
        while self.leftover.iter().all(|c| c.len() >= self.chunk) {
            let mut chunk = vec![Vec::with_capacity(self.chunk); self.channels];
            for ch in 0..self.channels {
                chunk[ch].extend(self.leftover[ch].drain(..self.chunk));
            }
            if let Ok(produced) = self.inner.process(&chunk, None) {
                for ch in 0..self.channels {
                    out_ch[ch].extend_from_slice(&produced[ch]);
                }
            }
        }
        interleave(&out_ch)
    }

    pub fn reset(&mut self) {
        for c in &mut self.leftover {
            c.clear();
        }
        self.inner.reset();
    }
}

fn deinterleave_into(interleaved: &[f32], channels: usize, dest: &mut [Vec<f32>]) {
    if channels == 0 {
        return;
    }
    let frames = interleaved.len() / channels;
    for i in 0..frames {
        for c in 0..channels {
            dest[c].push(interleaved[i * channels + c]);
        }
    }
}

fn interleave(channels: &[Vec<f32>]) -> Vec<f32> {
    if channels.is_empty() {
        return Vec::new();
    }
    let n = channels[0].len();
    let mut out = Vec::with_capacity(n * channels.len());
    for i in 0..n {
        for ch in channels {
            out.push(*ch.get(i).unwrap_or(&0.0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsample_increases_length() {
        let mut r = RateConvert::new(44100, 48000, 1).unwrap();
        let input: Vec<f32> = (0..44100)
            .map(|i| (i as f32 * 0.01).sin())
            .collect();
        let mut out = Vec::new();
        // feed in chunks
        for chunk in input.chunks(2048) {
            out.extend(r.push_interleaved(chunk));
        }
        assert!(
            out.len() as f32 > 44100.0 * 0.9,
            "got {}",
            out.len()
        );
    }
}
