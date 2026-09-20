use crate::error::{AppError, AppResult};
use crate::net;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Originalfilen skrivs byte-för-byte. Ingen omkodning, ingen container-ompackning.
pub async fn download_original(
    url: &str,
    dest: &Path,
) -> AppResult<(u64, String, String)> {
    net::validate_http_url(url)?;
    let client = net::http_client()?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(AppError::msg(format!(
            "Nedladdning misslyckades (HTTP {}).",
            response.status()
        )));
    }
    if let Some(len) = response.content_length() {
        if len > net::MAX_MEDIA_BYTES {
            return Err(AppError::msg(format!(
                "Ljudfilen är större än tillåtet ({len} byte)."
            )));
        }
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut hasher = Sha256::new();
    let mut bytes: u64 = 0;
    let tmp = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > net::MAX_MEDIA_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(AppError::msg("Ljudfilen är större än tillåtet (2 GB)."));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, dest).await?;
    let checksum = hex::encode(hasher.finalize());
    Ok((bytes, checksum, content_type))
}

pub fn media_file_path(media_root: &Path, podcast_id: i64, episode_id: i64, url: &str) -> PathBuf {
    let ext = extension_from_url(url);
    media_root
        .join(podcast_id.to_string())
        .join(format!("{episode_id}.{ext}"))
}

pub fn extension_from_url(url: &str) -> String {
    let parsed = url::Url::parse(url).ok();
    let path = parsed
        .as_ref()
        .map(|u| u.path().to_string())
        .unwrap_or_else(|| url.to_string());
    let ext = Path::new(&path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| {
            matches!(
                s.as_str(),
                "mp3" | "m4a" | "aac" | "ogg" | "opus" | "flac" | "wav" | "mp4" | "m4b"
            )
        });
    ext.unwrap_or_else(|| "bin".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_mp3_extension() {
        assert_eq!(
            extension_from_url("https://cdn.example.com/shows/ep-12.mp3?token=1"),
            "mp3"
        );
    }
}
