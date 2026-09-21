use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Interleaved byte ring for packed DSD (1 byte = 8 DSD bits per channel).
/// Same underrun policy as [`super::ring::FrameRing`]: contention or short fill counts.
pub struct ByteRing {
    inner: Mutex<VecDeque<u8>>,
    channels: usize,
    underruns: AtomicU64,
}

impl ByteRing {
    pub fn new(channels: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(256_000 * channels.max(1))),
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

    pub fn push_interleaved(&self, bytes: &[u8]) {
        if let Ok(mut g) = self.inner.lock() {
            g.extend(bytes.iter().copied());
        }
    }

    /// Fill `out`. Remainder is DSD mute (`0x69`), never raw zero (DC).
    pub fn pop_interleaved(&self, out: &mut [u8], mute: u8) -> usize {
        let Some(mut g) = self.inner.try_lock().ok() else {
            out.fill(mute);
            self.note_underrun();
            return 0;
        };
        let n = out.len().min(g.len());
        for slot in out.iter_mut().take(n) {
            *slot = g.pop_front().unwrap_or(mute);
        }
        for slot in out.iter_mut().skip(n) {
            *slot = mute;
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
    fn empty_counts_underrun_and_mutes() {
        let ring = ByteRing::new(2);
        let mut out = [0u8; 4];
        assert_eq!(ring.pop_interleaved(&mut out, 0x69), 0);
        assert!(out.iter().all(|b| *b == 0x69));
        assert_eq!(ring.underrun_count(), 1);
    }

    #[test]
    fn pop_full_does_not_underrun() {
        let ring = ByteRing::new(2);
        ring.push_interleaved(&[1, 2, 3, 4]);
        let mut out = [0u8; 4];
        assert_eq!(ring.pop_interleaved(&mut out, 0x69), 2);
        assert_eq!(out, [1, 2, 3, 4]);
        assert_eq!(ring.underrun_count(), 0);
    }
}
