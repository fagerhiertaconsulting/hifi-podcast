use std::collections::VecDeque;
use std::sync::Mutex;

/// Interleaved f32 ring. try-lock in the audio callback; silence on contention.
pub struct FrameRing {
    inner: Mutex<VecDeque<f32>>,
    channels: usize,
}

impl FrameRing {
    pub fn new(channels: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(48_000 * channels.max(1))),
            channels: channels.max(1),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.clear();
        }
    }

    pub fn len_frames(&self) -> usize {
        self.inner
            .lock()
            .map(|g| g.len() / self.channels)
            .unwrap_or(0)
    }

    pub fn push_interleaved(&self, samples: &[f32]) {
        if let Ok(mut g) = self.inner.lock() {
            g.extend(samples.iter().copied());
        }
    }

    /// Fill `out` with frames. Returns frames written (not samples). Remainder is silence.
    pub fn pop_interleaved(&self, out: &mut [f32]) -> usize {
        let Some(mut g) = self.inner.try_lock().ok() else {
            out.fill(0.0);
            return 0;
        };
        let n = out.len().min(g.len());
        for slot in out.iter_mut().take(n) {
            *slot = g.pop_front().unwrap_or(0.0);
        }
        for slot in out.iter_mut().skip(n) {
            *slot = 0.0;
        }
        n / self.channels
    }
}
