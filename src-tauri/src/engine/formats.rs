//! Format detection for the native engine. DSD (.dsf/.dff) is native; SACD ISO is not ripped.

use crate::media_mime;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatKind {
    Pcm,
    Dsd,
    Unknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct FormatSupport {
    pub kind: FormatKind,
    pub mime: String,
    pub extension: String,
    pub native_decode: bool,
    pub dsd_plugin: bool,
    pub label_sv: String,
    pub note_sv: String,
}

const PCM_EXTS: &[&str] = &[
    "wav", "wave", "flac", "mp3", "mpga", "m4a", "m4b", "aac", "mp4", "ogg", "oga", "opus", "aiff",
    "aif",
];

const DSD_EXTS: &[&str] = &["dsf", "dff", "dsd"];

pub fn extension_of(path: &Path) -> String {
    path.extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
}

pub fn classify_extension(ext: &str) -> FormatKind {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if PCM_EXTS.contains(&ext.as_str()) {
        FormatKind::Pcm
    } else if DSD_EXTS.contains(&ext.as_str()) || ext == "iso" {
        FormatKind::Dsd
    } else {
        FormatKind::Unknown
    }
}

pub fn classify_mime(mime: &str) -> FormatKind {
    let mime = mime.trim().to_ascii_lowercase();
    if mime == "audio/x-dsd"
        || mime == "audio/dsd"
        || mime == "audio/x-dsf"
        || mime == "audio/x-dff"
    {
        return FormatKind::Dsd;
    }
    if media_mime::is_known_renderer_audio(&mime) {
        if mime == "audio/x-ms-wma" {
            return FormatKind::Unknown;
        }
        return FormatKind::Pcm;
    }
    FormatKind::Unknown
}

pub fn classify_path(path: &Path) -> FormatKind {
    classify_extension(&extension_of(path))
}

/// Magic-byte probe. DSD is recognized so we never hand .dsf to Symphonia.
pub fn classify_magic(header: &[u8], path: Option<&Path>) -> FormatKind {
    if header.len() >= 4 {
        if &header[..4] == b"DSD " {
            return FormatKind::Dsd;
        }
        if &header[..4] == b"FRM8" {
            return FormatKind::Dsd;
        }
        if &header[..4] == b"fLaC" || &header[..4] == b"OggS" || &header[..4] == b"RIFF" {
            return FormatKind::Pcm;
        }
        if header.len() >= 3 && &header[..3] == b"ID3" {
            return FormatKind::Pcm;
        }
        if header.len() >= 8 && &header[4..8] == b"ftyp" {
            return FormatKind::Pcm;
        }
        // MPEG frame sync
        if header[0] == 0xff && (header[1] & 0xe0) == 0xe0 {
            return FormatKind::Pcm;
        }
    }
    if let Some(path) = path {
        return classify_path(path);
    }
    FormatKind::Unknown
}

pub fn support_for(path: &Path, stored_mime: Option<&str>) -> FormatSupport {
    let ext = extension_of(path);
    let mime = media_mime::for_serving(stored_mime, path);
    let mut kind = classify_path(path);
    if kind == FormatKind::Unknown {
        kind = classify_mime(&mime);
    }
    let flac = ext == "flac";
    match kind {
        FormatKind::Pcm => FormatSupport {
            kind,
            mime,
            extension: ext,
            native_decode: true,
            dsd_plugin: false,
            label_sv: "PCM (Symphonia)".into(),
            note_sv: if flac {
                "FLAC: heltals-bitkopia via ASIO när enhetens format rymmer bitdjupet. f32 bara som reserv. Originalfilen rörs inte.".into()
            } else {
                "Avkodas native till f32. Originalfilen rörs inte.".into()
            },
        },
        FormatKind::Dsd => {
            let iso = ext == "iso";
            FormatSupport {
                kind,
                mime,
                extension: ext,
                native_decode: !iso,
                dsd_plugin: iso,
                label_sv: if iso {
                    "SACD ISO (ej rippning)".into()
                } else {
                    "DSD (.dsf/.dff)".into()
                },
                note_sv: if iso {
                    "SACD-ISO rippning ingår inte. Använd .dsf/.dff. Filen bevaras orörd.".into()
                } else {
                    "Rå DSD avkodas native. Primär utgång: native DSD via ASIO (FiiO). DoP om DAC kräver det — då visas DoP, aldrig \"native DSD\". WASAPI exclusive DoP är dokumenterad fallback. Delat WASAPI vägras. Originalfilen rörs inte.".into()
                },
            }
        }
        FormatKind::Unknown => FormatSupport {
            kind,
            mime,
            extension: ext,
            native_decode: false,
            dsd_plugin: false,
            label_sv: "Okänt format".into(),
            note_sv: "Native-motorn avkodar inte det här formatet. Prova WebView2 för vanliga poddfiler (MP3/M4A) eller konvertera inte originalet.".into(),
        },
    }
}

pub fn native_pcm_mimes() -> &'static [&'static str] {
    &[
        "audio/mpeg",
        "audio/mp4",
        "audio/flac",
        "audio/ogg",
        "audio/wav",
        "audio/aiff",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn pcm_extensions_are_native() {
        for ext in ["mp3", "m4a", "flac", "ogg", "wav", "opus", "aac"] {
            assert_eq!(classify_extension(ext), FormatKind::Pcm, "{ext}");
            let support = support_for(Path::new(&format!("ep.{ext}")), None);
            assert!(support.native_decode, "{ext}");
        }
    }

    #[test]
    fn dsd_files_are_native_iso_is_not() {
        for ext in ["dsf", "dff", "dsd"] {
            let support = support_for(Path::new(&format!("track.{ext}")), None);
            assert_eq!(support.kind, FormatKind::Dsd);
            assert!(support.native_decode, "{ext}");
            assert!(!support.dsd_plugin);
        }
        let iso = support_for(Path::new("album.iso"), None);
        assert_eq!(iso.kind, FormatKind::Dsd);
        assert!(!iso.native_decode);
        assert_eq!(
            classify_magic(b"DSD ", Some(Path::new("x.bin"))),
            FormatKind::Dsd
        );
        assert_eq!(
            classify_magic(b"FRM8....", Some(Path::new("x.bin"))),
            FormatKind::Dsd
        );
        assert_eq!(classify_mime("audio/x-dsd"), FormatKind::Dsd);
    }

    #[test]
    fn flac_magic_and_mp4_ftyp() {
        assert_eq!(classify_magic(b"fLaC....", None), FormatKind::Pcm);
        let mut mp4 = vec![0u8; 12];
        mp4[4..8].copy_from_slice(b"ftyp");
        assert_eq!(classify_magic(&mp4, None), FormatKind::Pcm);
        assert_eq!(classify_mime("audio/x-ms-wma"), FormatKind::Unknown);
    }
}
