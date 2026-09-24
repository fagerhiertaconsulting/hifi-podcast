//! Optional ffmpeg next to whisper-cli. A download uses one pinned URL and
//! is not executed until the SHA-256 matches.

use crate::error::{AppError, AppResult};
use crate::models::FfmpegStatus;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const FFMPEG_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-09-23-14-55/ffmpeg-n8.1.3-win64-gpl-8.1.zip";
pub const FFMPEG_SHA256: &str = "fde13c8cf7c1b0a54bf661105652970810d94d140d867dd990fce4885a9f5d20";
const MAX_FFMPEG_BYTES: u64 = 220 * 1024 * 1024;

pub fn find_ffmpeg(data_dir: &Path) -> Option<PathBuf> {
    let dir = crate::asr::bin_dir(data_dir);
    for name in ["ffmpeg.exe", "ffmpeg"] {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let names = if cfg!(windows) {
        ["ffmpeg.exe", "ffmpeg"]
    } else {
        ["ffmpeg", "ffmpeg.exe"]
    };
    for name in names {
        if let Ok(path) = which::which(name) {
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub fn status(data_dir: &Path) -> FfmpegStatus {
    match find_ffmpeg(data_dir) {
        Some(path) => FfmpegStatus {
            present: true,
            hint: "ffmpeg finns. Klipp använder strömkopia, inte omkodning.".into(),
            path: Some(path.display().to_string()),
        },
        None => FfmpegStatus {
            present: false,
            path: None,
            hint: "ffmpeg saknas. På Windows kan du ladda ner den fästa versionen. Den körs inte förrän kontrollsumman stämmer.".into(),
        },
    }
}

pub fn verify_digest(digest: &str) -> AppResult<()> {
    if digest.eq_ignore_ascii_case(FFMPEG_SHA256) {
        Ok(())
    } else {
        Err(AppError::msg(format!(
            "SHA-256 stämmer inte för ffmpeg. Förväntat {FFMPEG_SHA256}, fick {digest}. Filen raderas."
        )))
    }
}

pub async fn download_windows_build(data_dir: &Path) -> AppResult<PathBuf> {
    if !cfg!(windows) {
        if let Some(path) = find_ffmpeg(data_dir) {
            return Ok(path);
        }
        return Err(AppError::msg(
            "ffmpeg saknas. Lägg ffmpeg i bin-mappen eller i PATH. Ingen fil hämtades.",
        ));
    }
    let dest_dir = crate::asr::bin_dir(data_dir);
    tokio::fs::create_dir_all(&dest_dir).await?;
    let zip_path = dest_dir.join("ffmpeg-n8.1.3-win64-gpl-8.1.zip");
    let client = crate::net::http_client()?;
    let mut hasher = Sha256::new();
    crate::net::download_to_path(
        &client,
        FFMPEG_URL,
        &zip_path,
        MAX_FFMPEG_BYTES,
        Some(&mut hasher),
    )
    .await?;
    let digest = hex::encode(hasher.finalize());
    if let Err(err) = verify_digest(&digest) {
        let _ = tokio::fs::remove_file(&zip_path).await;
        return Err(err);
    }
    let exe = extract_ffmpeg_exe(&zip_path, &dest_dir)?;
    let _ = tokio::fs::remove_file(&zip_path).await;
    Ok(exe)
}

fn extract_ffmpeg_exe(zip_path: &Path, dest_dir: &Path) -> AppResult<PathBuf> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|err| AppError::msg(format!("Kunde inte öppna ffmpeg-zip: {err}")))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|err| AppError::msg(format!("zip-fel: {err}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = PathBuf::from(entry.name());
        if name
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue;
        }
        let Some(file_name) = name.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !file_name.eq_ignore_ascii_case("ffmpeg.exe") {
            continue;
        }
        let dest = dest_dir.join("ffmpeg.exe");
        let mut out = std::fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut out)?;
        return Ok(dest);
    }
    Err(AppError::msg(
        "ffmpeg-zippen innehöll ingen ffmpeg.exe. Ingenting kördes.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_digest_rejects_a_mismatch() {
        assert!(verify_digest("deadbeef").is_err());
        assert!(verify_digest(FFMPEG_SHA256).is_ok());
        assert!(FFMPEG_URL
            .starts_with("https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-"));
        assert!(!FFMPEG_URL.contains("/latest/"));
    }
}
