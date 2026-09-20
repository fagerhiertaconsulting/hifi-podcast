use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Interleaved f32 ring. try-lock in the audio callback; silence + underrun on contention or empty.
pub struct FrameRing {
    inner: Mutex<VecDeque<f32>>,
    channels: usize,
    underruns: AtomicU64,
}

impl FrameRing {
    pub fn new(channels: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(48_000 * channels.max(1))),
            channels: channels.max(1),
            underruns: AtomicU64::new(0),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn underrun_count(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    pub fn note_underrun(&self) {
        self.underruns.fetch_add(1, Ordering::Relaxed);
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
    /// Mutex contention **or** a short/empty ring is an underrun (counted); silence is never silent without a metric.
    pub fn pop_interleaved(&self, out: &mut [f32]) -> usize {
        let Some(mut g) = self.inner.try_lock().ok() else {
            out.fill(0.0);
            self.note_underrun();
            return 0;
        };
        let n = out.len().min(g.len());
        for slot in out.iter_mut().take(n) {
            *slot = g.pop_front().unwrap_or(0.0);
        }
        for slot in out.iter_mut().skip(n) {
            *slot = 0.0;
        }
        if n < out.len() {
            self.note_underrun();
        }
        n / self.channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contention_counts_underrun_and_writes_silence() {
        let ring = FrameRing::new(2);
        ring.push_interleaved(&[0.5, -0.5, 0.25, -0.25]);
        let _hold = ring.inner.lock().unwrap();
        let mut out = [1.0f32, 1.0, 1.0, 1.0];
        assert_eq!(ring.pop_interleaved(&mut out), 0);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(ring.underrun_count(), 1);
        ring.note_underrun();
        assert_eq!(ring.underrun_count(), 2);
    }

    #[test]
    fn pop_without_contention_does_not_count_underrun() {
        let ring = FrameRing::new(2);
        ring.push_interleaved(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = [0.0f32; 4];
        assert_eq!(ring.pop_interleaved(&mut out), 2);
        assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(ring.underrun_count(), 0);
    }

    #[test]
    fn empty_ring_writes_silence_and_counts_underrun() {
        let ring = FrameRing::new(2);
        let mut out = [1.0f32, 1.0, 1.0, 1.0];
        assert_eq!(ring.pop_interleaved(&mut out), 0);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(ring.underrun_count(), 1);
    }

    #[test]
    fn short_ring_pads_silence_and_counts_underrun() {
        let ring = FrameRing::new(2);
        ring.push_interleaved(&[0.5, -0.5]);
        let mut out = [9.0f32; 4];
        assert_eq!(ring.pop_interleaved(&mut out), 1);
        assert_eq!(out, [0.5, -0.5, 0.0, 0.0]);
        assert_eq!(ring.underrun_count(), 1);
    }
}
