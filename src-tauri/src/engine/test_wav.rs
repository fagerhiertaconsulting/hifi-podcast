//! PCM16 WAV writer for engine tests (no extra encoder crate).

#![cfg(test)]

use std::path::Path;

pub fn write_sine_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    hz: f32,
    seconds: f32,
    amp: f32,
) {
    let frames = (sample_rate as f32 * seconds) as usize;
    let mut pcm = Vec::with_capacity(frames * channels as usize);
    for n in 0..frames {
        let s = (2.0 * std::f32::consts::PI * hz * n as f32 / sample_rate as f32).sin() * amp;
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        for _ in 0..channels {
            pcm.push(v);
        }
    }
    write_pcm16_wav(path, sample_rate, channels, &pcm);
}

pub fn write_silence_then_tone(
    path: &Path,
    sample_rate: u32,
    silence_secs: f32,
    tone_secs: f32,
) {
    let mute_frames = (sample_rate as f32 * silence_secs) as usize;
    let tone_frames = (sample_rate as f32 * tone_secs) as usize;
    let mut pcm = Vec::with_capacity(mute_frames + tone_frames);
    pcm.extend(std::iter::repeat(0i16).take(mute_frames));
    for n in 0..tone_frames {
        let s = (2.0 * std::f32::consts::PI * 440.0 * n as f32 / sample_rate as f32).sin() * 0.5;
        pcm.push((s * 32767.0) as i16);
    }
    write_pcm16_wav(path, sample_rate, 1, &pcm);
}

pub fn write_pcm16_wav(path: &Path, sample_rate: u32, channels: u16, pcm: &[i16]) {
    let data_bytes = pcm.len() * 2;
    let mut out = Vec::with_capacity(44 + data_bytes);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32 + data_bytes as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * 2;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = channels * 2;
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}
