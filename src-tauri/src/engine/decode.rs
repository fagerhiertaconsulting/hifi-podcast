//! Streaming Symphonia decoder. Output is interleaved f32, unity gain, no DSP.

use super::formats::{classify_magic, FormatKind};
use super::EngineError;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::{CodecType, DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::Time;

#[derive(Debug, Clone)]
pub struct AudioSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: Option<u64>,
    pub codec: String,
    /// Container/codec bits (0 if unknown). Intern kedja är ändå f32.
    pub bits_per_sample: u32,
}

pub struct Decoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn symphonia::core::codecs::Decoder>,
    track_id: u32,
    spec: AudioSpec,
    sample_buf: Option<SampleBuffer<f32>>,
    leftover: Vec<f32>,
    eof: bool,
}

impl Decoder {
    pub fn open(path: &Path) -> Result<Self, EngineError> {
        let mut header = [0u8; 16];
        {
            let mut f = File::open(path)?;
            let _ = f.read(&mut header);
        }
        match classify_magic(&header, Some(path)) {
            FormatKind::Dsd => {
                return Err(EngineError::msg(
                    "DSD går inte genom Symphonia. Använd DSD-vägen (.dsf/.dff → ASIO native/DoP).",
                ));
            }
            FormatKind::Unknown => {
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if ext == "wma" {
                    return Err(EngineError::msg(
                        "WMA avkodas inte av native-motorn. Behåll originalfilen; använd WebView2 eller en PCM/FLAC-kopia bara om du själv skapat den.",
                    ));
                }
            }
            FormatKind::Pcm => {}
        }

        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
            hint.with_extension(ext);
        }
        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                mss,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|err| EngineError::msg(format!("Kunde inte identifiera ljudfilen: {err}")))?;

        let format = probed.format;
        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| EngineError::msg("Filen saknar avkodbart ljudspår."))?
            .clone();

        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions { verify: false })
            .map_err(|err| EngineError::msg(format!("Ingen avkodare för spåret: {err}")))?;

        let sample_rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| EngineError::msg("Ljudspåret saknar samplingsfrekvens."))?;
        let channels = track
            .codec_params
            .channels
            .map(|c| c.count() as u16)
            .unwrap_or(2);
        if channels == 0 {
            return Err(EngineError::msg("Ljudspåret har noll kanaler."));
        }
        let duration_ms = track.codec_params.n_frames.map(|frames| {
            (frames as u128 * 1000 / sample_rate as u128) as u64
        });
        let codec = codec_label(track.codec_params.codec);
        let bits_per_sample = track
            .codec_params
            .bits_per_sample
            .or(track.codec_params.bits_per_coded_sample)
            .filter(|b| *b > 0)
            .or_else(|| container_bits_per_sample(path))
            .unwrap_or(0);

        Ok(Self {
            format,
            decoder,
            track_id: track.id,
            spec: AudioSpec {
                sample_rate,
                channels,
                duration_ms,
                codec,
                bits_per_sample,
            },
            sample_buf: None,
            leftover: Vec::new(),
            eof: false,
        })
    }

    pub fn spec(&self) -> &AudioSpec {
        &self.spec
    }

    pub fn is_eof(&self) -> bool {
        self.eof && self.leftover.is_empty()
    }

    /// Interleaved f32, unity. Empty vec means EOF.
    pub fn read_interleaved(&mut self, min_samples: usize) -> Result<Vec<f32>, EngineError> {
        let mut out = Vec::new();
        if !self.leftover.is_empty() {
            out.append(&mut self.leftover);
        }
        while out.len() < min_samples && !self.eof {
            match self.format.next_packet() {
                Ok(packet) => {
                    if packet.track_id() != self.track_id {
                        continue;
                    }
                    match self.decoder.decode(&packet) {
                        Ok(decoded) => {
                            let samples = copy_f32(decoded, &mut self.sample_buf);
                            out.extend_from_slice(samples);
                        }
                        Err(SymphoniaError::DecodeError(_)) => continue,
                        Err(SymphoniaError::IoError(err))
                            if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                        {
                            self.eof = true;
                        }
                        Err(err) => {
                            return Err(EngineError::msg(format!("Avkodningsfel: {err}")));
                        }
                    }
                }
                Err(SymphoniaError::ResetRequired) => {
                    self.decoder.reset();
                }
                Err(SymphoniaError::IoError(err))
                    if err.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    self.eof = true;
                }
                Err(SymphoniaError::SeekError(_)) => {
                    self.eof = true;
                }
                Err(err) => {
                    return Err(EngineError::msg(format!("Kunde inte läsa paket: {err}")));
                }
            }
        }
        Ok(out)
    }

    pub fn unread(&mut self, samples: &[f32]) {
        let mut rest = samples.to_vec();
        rest.append(&mut self.leftover);
        self.leftover = rest;
    }

    pub fn seek_ms(&mut self, ms: u64) -> Result<u64, EngineError> {
        let seconds = ms / 1000;
        let frac = (ms % 1000) as f64 / 1000.0;
        let time = Time { seconds, frac };
        let ts = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|err| EngineError::msg(format!("Seek misslyckades: {err}")))?;
        self.decoder.reset();
        self.leftover.clear();
        self.eof = false;
        let rate = self.spec.sample_rate as u128;
        let actual_ms = ts.actual_ts as u128 * 1000 / rate.max(1);
        Ok(actual_ms as u64)
    }
}

fn copy_f32<'a>(
    decoded: AudioBufferRef<'_>,
    sample_buf: &'a mut Option<SampleBuffer<f32>>,
) -> &'a [f32] {
    let spec = *decoded.spec();
    let frames = decoded.capacity() as u64;
    let need_new = match sample_buf {
        None => true,
        Some(buf) => buf.capacity() < decoded.capacity(),
    };
    if need_new {
        *sample_buf = Some(SampleBuffer::<f32>::new(frames, spec));
    }
    let buf = sample_buf.as_mut().expect("sample buffer");
    buf.copy_interleaved_ref(decoded);
    buf.samples()
}

fn codec_label(codec: CodecType) -> String {
    let raw = format!("{codec:?}");
    let key = raw.to_ascii_lowercase();
    if key.contains("flac") {
        return "FLAC".into();
    }
    if key.contains("mp3") || key.contains("mpeg") {
        return "MP3".into();
    }
    if key.contains("aac") {
        return "AAC".into();
    }
    if key.contains("alac") {
        return "ALAC".into();
    }
    if key.contains("vorbis") {
        return "Vorbis".into();
    }
    if key.contains("opus") {
        return "Opus".into();
    }
    if key.contains("pcm") || key.contains("wav") {
        return "PCM".into();
    }
    if let Some(n) = raw
        .strip_prefix("CodecType(")
        .and_then(|s| s.strip_suffix(')'))
        .and_then(|s| s.parse::<u32>().ok())
    {
        // Symphonia PCM family is 0x100..=0x125; FLAC is 0x2000.
        if (0x100..=0x125).contains(&n) {
            return "PCM".into();
        }
        if n == 0x2000 {
            return "FLAC".into();
        }
        if n == 0x1000 {
            return "MP3".into();
        }
        if n == 0x1001 {
            return "AAC".into();
        }
        if n == 0x1002 {
            return "Vorbis".into();
        }
        if n == 0x1003 {
            return "Opus".into();
        }
        if n == 0x2003 {
            return "ALAC".into();
        }
    }
    raw
}

/// Bit depth from the container when Symphonia omits `bits_per_sample`.
/// Returns `None` rather than guessing (no misleading 16/24).
pub fn container_bits_per_sample(path: &Path) -> Option<u32> {
    wav_fmt_bits(path).or_else(|| flac_streaminfo_bits(path))
}

fn wav_fmt_bits(path: &Path) -> Option<u32> {
    let mut file = File::open(path).ok()?;
    let mut hdr = [0u8; 12];
    file.read_exact(&mut hdr).ok()?;
    if &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"WAVE" {
        return None;
    }
    loop {
        let mut chunk = [0u8; 8];
        if file.read_exact(&mut chunk).is_err() {
            return None;
        }
        let id = &chunk[0..4];
        let len = u32::from_le_bytes(chunk[4..8].try_into().ok()?) as u64;
        if id == b"fmt " {
            let mut body = vec![0u8; (len as usize).min(64)];
            file.read_exact(&mut body).ok()?;
            if body.len() < 16 {
                return None;
            }
            let bits = u16::from_le_bytes(body[14..16].try_into().ok()?);
            return if bits > 0 { Some(u32::from(bits)) } else { None };
        }
                file.seek(SeekFrom::Current(len as i64 + (len % 2) as i64))
                    .ok()?;
    }
}

fn flac_streaminfo_bits(path: &Path) -> Option<u32> {
    let mut file = File::open(path).ok()?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if &magic != b"fLaC" {
        return None;
    }
    loop {
        let mut header = [0u8; 4];
        file.read_exact(&mut header).ok()?;
        let last = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7f;
        let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let mut body = vec![0u8; len];
        file.read_exact(&mut body).ok()?;
        if block_type == 0 && body.len() >= 18 {
            let packed = u64::from_be_bytes(body[10..18].try_into().ok()?);
            let bits = ((packed >> 36) & 0x1F) as u32 + 1;
            return if (4..=32).contains(&bits) {
                Some(bits)
            } else {
                None
            };
        }
        if last {
            return None;
        }
    }
}

/// Downmix/upmix to stereo without extra gain. Mono is doubled; >2 channels fold to L/R.
pub fn to_stereo(interleaved: &[f32], channels: u16) -> Vec<f32> {
    match channels {
        0 => Vec::new(),
        1 => interleaved.iter().flat_map(|s| [*s, *s]).collect(),
        2 => interleaved.to_vec(),
        n => {
            let n = n as usize;
            let frames = interleaved.len() / n;
            let mut out = Vec::with_capacity(frames * 2);
            for frame in 0..frames {
                let base = frame * n;
                let l = interleaved[base];
                let r = if n > 1 { interleaved[base + 1] } else { l };
                out.push(l);
                out.push(r);
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_wav::write_sine_wav;
    use std::f32::consts::PI;

    #[test]
    fn wav_container_bits_are_16() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        write_sine_wav(&path, 44100, 2, 440.0, 0.05, 0.2);
        assert_eq!(container_bits_per_sample(&path), Some(16));
        assert_eq!(flac_streaminfo_bits(&path), None);
    }

    #[test]
    fn decodes_wav_sine_to_f32() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        write_sine_wav(&path, 44100, 1, 440.0, 0.25, 0.5);
        let mut dec = Decoder::open(&path).unwrap();
        assert_eq!(dec.spec().sample_rate, 44100);
        assert_eq!(dec.spec().channels, 1);
        assert_eq!(dec.spec().bits_per_sample, 16);
        assert_eq!(dec.spec().codec, "PCM");
        let samples = dec.read_interleaved(1000).unwrap();
        assert!(samples.len() >= 1000);
        let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.3 && peak < 0.7, "peak={peak}");
        let duration = dec.spec().duration_ms.unwrap();
        assert!((240..=260).contains(&duration), "{duration}");
    }

    #[test]
    fn stereo_downmix_keeps_first_two_channels() {
        let input = vec![1.0, -1.0, 0.5, 0.25];
        let out = to_stereo(&input, 4);
        assert_eq!(out, vec![1.0, -1.0]);
    }

    #[test]
    fn rejects_dsd_magic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hires.dsf");
        std::fs::write(&path, b"DSD not-a-real-file").unwrap();
        let err = Decoder::open(&path).err().expect("dsd should fail");
        assert!(err.to_string().contains("DSD"), "{err}");
    }

    #[test]
    fn seek_rewinds_sine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        write_sine_wav(&path, 48000, 2, 220.0, 1.0, 0.4);
        let mut dec = Decoder::open(&path).unwrap();
        let _ = dec.read_interleaved(48000).unwrap();
        dec.seek_ms(0).unwrap();
        let samples = dec.read_interleaved(64).unwrap();
        // first sample of a sine starting at 0 is ~0
        assert!(samples[0].abs() < 0.05, "{}", samples[0]);
        let expected = (2.0 * PI * 220.0 / 48000.0).sin() * 0.4;
        assert!((samples[2] - expected).abs() < 0.05);
    }
}
