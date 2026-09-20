//! Opt-in skip-silence: jump the sample clock. Never rewrites the file.

#[derive(Debug, Clone)]
pub struct SkipSilence {
    enabled: bool,
    silent_ms: f32,
    threshold: f32,
    trigger_ms: f32,
    skip_ms: f32,
    hold_ms: f32,
}

impl Default for SkipSilence {
    fn default() -> Self {
        Self {
            enabled: false,
            silent_ms: 0.0,
            threshold: 0.012,
            trigger_ms: 450.0,
            skip_ms: 400.0,
            hold_ms: 200.0,
        }
    }
}

impl SkipSilence {
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.silent_ms = 0.0;
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn reset(&mut self) {
        self.silent_ms = 0.0;
    }

    /// RMS of an interleaved block.
    pub fn rms(samples: &[f32]) -> f32 {
        if samples.is_empty() {
            return 0.0;
        }
        let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
        (sum / samples.len() as f64).sqrt() as f32
    }

    /// If the block is silent long enough, return extra milliseconds to skip in the source.
    pub fn observe(&mut self, samples: &[f32], block_ms: f32) -> f32 {
        if !self.enabled || samples.is_empty() {
            return 0.0;
        }
        if Self::rms(samples) < self.threshold {
            self.silent_ms += block_ms;
            if self.silent_ms > self.trigger_ms {
                self.silent_ms = self.hold_ms;
                return self.skip_ms;
            }
        } else {
            self.silent_ms = 0.0;
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_does_not_skip() {
        let mut s = SkipSilence::default();
        let silence = vec![0.0f32; 2048];
        assert_eq!(s.observe(&silence, 50.0), 0.0);
    }

    #[test]
    fn opt_in_skips_after_run_of_silence() {
        let mut s = SkipSilence::default();
        s.set_enabled(true);
        let silence = vec![0.0f32; 512];
        let mut skipped = 0.0;
        for _ in 0..20 {
            skipped += s.observe(&silence, 50.0);
        }
        assert!(skipped >= 400.0, "{skipped}");
        let tone: Vec<f32> = (0..512).map(|i| ((i as f32) / 64.0).sin() * 0.2).collect();
        assert_eq!(s.observe(&tone, 20.0), 0.0);
    }

    #[test]
    fn rms_of_unity_is_one() {
        assert!((SkipSilence::rms(&[1.0, -1.0]) - 1.0).abs() < 1e-6);
    }
}
