//! Pitch-preserving time stretch (WSOLA). Bypassed at speed == 1.0.

/// Hann-windowed WSOLA for speech/podcast speed without shifting pitch.
pub struct TimeStretch {
    speed: f64,
    channels: usize,
    window: usize,
    hop_out: usize,
    input: Vec<f32>,
    ola: Vec<f32>,
    ola_filled: usize,
}

impl TimeStretch {
    pub fn new(channels: usize) -> Self {
        let hop_out = 512;
        let window = 1024;
        Self {
            speed: 1.0,
            channels: channels.max(1),
            window,
            hop_out,
            input: Vec::new(),
            ola: vec![0.0; window * channels.max(1)],
            ola_filled: 0,
        }
    }

    pub fn set_speed(&mut self, speed: f64) {
        let speed = speed.clamp(0.5, 4.0);
        if (speed - self.speed).abs() > 0.001 {
            self.speed = speed;
        }
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn reset(&mut self) {
        self.input.clear();
        self.ola.fill(0.0);
        self.ola_filled = 0;
    }

    pub fn push(&mut self, interleaved: &[f32]) -> Vec<f32> {
        if (self.speed - 1.0).abs() < 0.001 {
            if self.input.is_empty() && self.ola_filled == 0 {
                return interleaved.to_vec();
            }
            self.input.extend_from_slice(interleaved);
            let mut out = Vec::new();
            out.append(&mut self.input);
            self.reset();
            return out;
        }
        self.input.extend_from_slice(interleaved);
        self.process()
    }

    fn hop_in(&self) -> usize {
        ((self.hop_out as f64) * self.speed).round().max(1.0) as usize
    }

    fn process(&mut self) -> Vec<f32> {
        let ch = self.channels;
        let win = self.window;
        let hop_out = self.hop_out;
        let hop_in = self.hop_in();
        let mut out = Vec::new();
        let hann = hann_table(win);

        loop {
            let need = (win + 64) * ch;
            if self.input.len() < need {
                break;
            }
            let search = 64usize;
            let base = 0;
            let offset = best_offset(&self.input, &self.ola, ch, win, search);
            let start = (base + offset) * ch;

            if self.ola_filled == 0 {
                for i in 0..win {
                    let w = hann[i];
                    for c in 0..ch {
                        self.ola[i * ch + c] = self.input[start + i * ch + c] * w;
                    }
                }
                self.ola_filled = win;
            } else {
                for i in 0..win {
                    let w = hann[i];
                    for c in 0..ch {
                        let idx = i * ch + c;
                        if idx < self.ola.len() {
                            self.ola[idx] += self.input[start + i * ch + c] * w;
                        }
                    }
                }
            }

            let emit = hop_out.min(self.ola_filled);
            out.extend_from_slice(&self.ola[..emit * ch]);
            self.ola.copy_within(emit * ch.., 0);
            let keep = self.ola.len() - emit * ch;
            for s in &mut self.ola[keep..] {
                *s = 0.0;
            }
            self.ola_filled = self.ola_filled.saturating_sub(emit);

            let consume = hop_in.min(self.input.len() / ch);
            self.input.drain(..consume * ch);
        }
        out
    }
}

fn hann_table(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = std::f64::consts::PI * 2.0 * i as f64 / n as f64;
            0.5 * (1.0 - x.cos()) as f32
        })
        .collect()
}

fn best_offset(input: &[f32], ola: &[f32], ch: usize, win: usize, search: usize) -> usize {
    if ola.iter().all(|s| *s == 0.0) {
        return 0;
    }
    let mut best = 0usize;
    let mut best_corr = f64::MIN;
    let overlap = win / 2;
    for off in 0..=search {
        let mut corr = 0.0f64;
        let mut n = 0.0f64;
        for i in 0..overlap {
            for c in 0..ch {
                let a = ola[i * ch + c] as f64;
                let b = input[(off + i) * ch + c] as f64;
                corr += a * b;
                n += 1.0;
            }
        }
        let score = corr / n.max(1.0);
        if score > best_corr {
            best_corr = score;
            best = off;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unity_is_passthrough() {
        let mut s = TimeStretch::new(2);
        s.set_speed(1.0);
        let input: Vec<f32> = (0..256).map(|i| (i as f32) / 256.0).collect();
        assert_eq!(s.push(&input), input);
    }

    #[test]
    fn faster_emits_fewer_samples() {
        let mut s = TimeStretch::new(1);
        s.set_speed(2.0);
        let sr = 44100;
        let input: Vec<f32> = (0..sr)
            .map(|i| (i as f32 * 440.0 * 2.0 * std::f32::consts::PI / sr as f32).sin())
            .collect();
        let out = s.push(&input);
        assert!(
            out.len() as f32 > input.len() as f32 * 0.35
                && (out.len() as f32) < input.len() as f32 * 0.7,
            "in={} out={}",
            input.len(),
            out.len()
        );
    }
}
