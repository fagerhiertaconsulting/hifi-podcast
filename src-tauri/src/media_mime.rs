//! MIME for original podcast files. Never inferred as a reason to transcode.

/// Maps a file extension or HTTP Content-Type onto a UPnP `res` MIME.
/// Unknown types stay `application/octet-stream` so the original bytes are
/// still served; the renderer may refuse them (no v1 transcoder).
pub fn from_extension(ext: &str) -> &'static str {
    match ext
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" | "mpga" => "audio/mpeg",
        "m4a" | "m4b" | "aac" => "audio/mp4",
        "mp4" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/ogg",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        "wma" => "audio/x-ms-wma",
        "dsf" | "dff" => "audio/x-dsd",
        _ => "application/octet-stream",
    }
}

pub fn from_path(path: &std::path::Path) -> &'static str {
    path.extension()
        .and_then(|s| s.to_str())
        .map(from_extension)
        .unwrap_or("application/octet-stream")
}

/// Keep a plausible audio MIME from the download response; drop parameters
/// (`codecs=`, charset) and reject empty / generic-binary unless we have
/// nothing else.
pub fn from_content_type(content_type: &str, fallback_path: Option<&std::path::Path>) -> String {
    let primary = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if primary.starts_with("audio/") {
        let normalized = normalize_audio_mime(&primary);
        if is_allowed_media_mime(&normalized) {
            return normalized;
        }
    }
    if primary == "video/mp4" || primary == "application/mp4" {
        return "audio/mp4".into();
    }
    fallback_path
        .map(from_path)
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// MIME we will store/serve from `download.mime` (plus octet-stream fallback).
pub fn is_allowed_media_mime(mime: &str) -> bool {
    is_known_renderer_audio(mime)
}

pub fn for_serving(stored: Option<&str>, path: &std::path::Path) -> String {
    if let Some(mime) = stored
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| m.to_ascii_lowercase())
    {
        if is_allowed_media_mime(&mime) {
            return mime;
        }
        if mime == "application/octet-stream" || mime == "binary/octet-stream" {
            let from_ext = from_path(path);
            if from_ext != "application/octet-stream" {
                return from_ext.to_string();
            }
            return "application/octet-stream".into();
        }
    }
    from_path(path).to_string()
}

fn normalize_audio_mime(mime: &str) -> String {
    match mime {
        "audio/mp3" | "audio/mpeg3" | "audio/x-mpeg" => "audio/mpeg".into(),
        "audio/x-m4a" | "audio/m4a" | "audio/aac" | "audio/x-aac" => "audio/mp4".into(),
        "audio/x-flac" => "audio/flac".into(),
        "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => "audio/wav".into(),
        "audio/vorbis" => "audio/ogg".into(),
        "audio/aiff" | "audio/x-aiff" => "audio/aiff".into(),
        other => other.to_string(),
    }
}

pub fn is_known_renderer_audio(mime: &str) -> bool {
    matches!(
        mime,
        "audio/mpeg"
            | "audio/mp4"
            | "audio/flac"
            | "audio/ogg"
            | "audio/wav"
            | "audio/aiff"
            | "audio/x-ms-wma"
            | "audio/x-dsd"
    )
}

/// DLNA `protocolInfo` for an HTTP original (no conversion, Range/seek).
/// `DLNA.ORG_OP=01` = Range supported. `CI=0` = not transcoded.
pub fn protocol_info(mime: &str) -> String {
    let pn = dlna_pn(mime);
    match pn {
        Some(pn) => format!(
            "http-get:*:{mime}:DLNA.ORG_PN={pn};DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000"
        ),
        None => format!(
            "http-get:*:{mime}:DLNA.ORG_OP=01;DLNA.ORG_CI=0;DLNA.ORG_FLAGS=01700000000000000000000000000000"
        ),
    }
}

fn dlna_pn(mime: &str) -> Option<&'static str> {
    match mime {
        "audio/mpeg" => Some("MP3"),
        "audio/mp4" => Some("AAC_ISO_320"),
        "audio/flac" => Some("FLAC"),
        "audio/wav" | "audio/aiff" => Some("LPCM"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn maps_common_podcast_extensions() {
        assert_eq!(from_extension("mp3"), "audio/mpeg");
        assert_eq!(from_extension(".M4A"), "audio/mp4");
        assert_eq!(from_extension("flac"), "audio/flac");
        assert_eq!(from_extension("ogg"), "audio/ogg");
        assert_eq!(from_extension("wav"), "audio/wav");
        assert_eq!(from_extension("xyz"), "application/octet-stream");
    }

    #[test]
    fn prefers_audio_content_type() {
        assert_eq!(
            from_content_type("audio/mpeg; charset=utf-8", Some(Path::new("x.m4a"))),
            "audio/mpeg"
        );
        assert_eq!(
            from_content_type("application/octet-stream", Some(Path::new("ep.flac"))),
            "audio/flac"
        );
        assert_eq!(from_content_type("audio/x-m4a", None), "audio/mp4");
        assert_eq!(
            from_content_type("text/html", Some(Path::new("ep.mp3"))),
            "audio/mpeg"
        );
        assert_eq!(
            from_content_type("application/javascript", None),
            "application/octet-stream"
        );
        assert_eq!(
            for_serving(Some("text/html"), Path::new("ep.flac")),
            "audio/flac"
        );
        assert_eq!(
            for_serving(Some("audio/mpeg"), Path::new("ep.flac")),
            "audio/mpeg"
        );
    }

    #[test]
    fn protocol_info_marks_original_and_range() {
        let p = protocol_info("audio/mpeg");
        assert!(p.contains("audio/mpeg"));
        assert!(p.contains("DLNA.ORG_OP=01"));
        assert!(p.contains("DLNA.ORG_CI=0"));
        assert!(p.contains("MP3"));
        assert!(is_known_renderer_audio("audio/mpeg"));
        assert!(!is_known_renderer_audio("application/octet-stream"));
    }
}
