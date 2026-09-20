//! Native hi-fi playback engine: Symphonia decode → f32 → ASIO / WASAPI.

mod asio_host;
mod decode;
mod exclusive;
mod formats;
mod output;
mod player;
mod resample;
mod ring;
mod skip;
mod stretch;

#[cfg(test)]
mod test_wav;

#[allow(unused_imports)]
pub use asio_host::{is_preferred_hi_fi_name, list_asio_drivers, AsioDriverInfo};
#[allow(unused_imports)]
pub use formats::{classify_extension, classify_magic, classify_mime, FormatKind, FormatSupport};
pub use output::OutputDeviceInfo;
pub use player::{EngineCapabilities, EngineStatus, Handle, OpenParams, ProbeInfo};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl EngineError {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }

    pub fn dsd() -> Self {
        Self::Message(
            "DSD/SACD avkodas inte i native-motorn ännu (plugin-yta). Filen lämnas orörd. FLAC och vanliga poddformat spelas native.".into(),
        )
    }
}

pub fn ensure_under_root(media_root: &Path, path: &Path) -> Result<PathBuf, EngineError> {
    let root = std::fs::canonicalize(media_root)?;
    let file = std::fs::canonicalize(path).map_err(|_| {
        EngineError::msg("Ljudfilen hittades inte under media-root.")
    })?;
    if !file.starts_with(&root) {
        return Err(EngineError::msg("Otillåten sökväg."));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::decode::Decoder;
    use crate::engine::test_wav::write_sine_wav;

    #[test]
    fn probe_wav_reports_pcm() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.wav");
        write_sine_wav(&path, 48000, 2, 1000.0, 0.2, 0.3);
        let probe = Handle::probe(&path).unwrap();
        assert_eq!(probe.support.kind, FormatKind::Pcm);
        assert_eq!(probe.sample_rate, Some(48000));
        assert_eq!(probe.channels, Some(2));
        assert!(probe.support.native_decode);
    }

    #[test]
    fn decode_path_roundtrip_not_silent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.wav");
        write_sine_wav(&path, 44100, 1, 880.0, 0.15, 0.6);
        let mut dec = Decoder::open(&path).unwrap();
        let s = dec.read_interleaved(2048).unwrap();
        let rms = skip::SkipSilence::rms(&s);
        assert!(rms > 0.1, "{rms}");
    }

    #[test]
    fn path_escape_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        let secret = dir.path().join("secret.wav");
        write_sine_wav(&secret, 8000, 1, 100.0, 0.05, 0.2);
        let err = ensure_under_root(&media, &secret).unwrap_err();
        assert_eq!(err.to_string().contains("Otillåten"), true);
    }

    #[test]
    fn lists_devices_without_panic() {
        let _ = Handle::list_devices();
        let caps = Handle::capabilities();
        assert!(caps.pcm_mimes.iter().any(|m| m == "audio/flac"));
        assert!(caps.asio_note_sv.contains("ASIO"));
        assert!(!crate::engine::is_preferred_hi_fi_name("Realtek"));
        assert!(crate::engine::is_preferred_hi_fi_name("FiiO ASIO Driver"));
    }
}
