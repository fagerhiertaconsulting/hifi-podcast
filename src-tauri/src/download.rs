use crate::error::{AppError, AppResult};
use crate::net;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Originalfilen skrivs byte-för-byte. Ingen omkodning, ingen container-ompackning.
pub async fn download_original(url: &str, dest: &Path) -> AppResult<(u64, String, String)> {
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

pub fn podcasts_dir(media_root: &Path) -> PathBuf {
    media_root.join("podcasts")
}

pub fn music_dir(media_root: &Path) -> PathBuf {
    media_root.join("music")
}

pub fn music_track_file(media_root: &Path, album_id: i64, track_id: i64, ext: &str) -> PathBuf {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    let ext = if ext.is_empty() { "bin".into() } else { ext };
    music_dir(media_root)
        .join(album_id.to_string())
        .join(format!("{track_id}.{ext}"))
}

pub fn media_file_path(media_root: &Path, podcast_id: i64, episode_id: i64, url: &str) -> PathBuf {
    let ext = extension_from_url(url);
    episode_file(media_root, podcast_id, episode_id, &ext)
}

pub fn episode_file(media_root: &Path, podcast_id: i64, episode_id: i64, ext: &str) -> PathBuf {
    podcasts_dir(media_root)
        .join(podcast_id.to_string())
        .join(format!("{episode_id}.{ext}"))
}

/// Creates `media/podcasts` and `media/music`, then moves legacy
/// `media/<podcast_id>/` trees into `media/podcasts/<podcast_id>/`.
pub fn ensure_media_layout(media_root: &Path) -> AppResult<Vec<(PathBuf, PathBuf)>> {
    std::fs::create_dir_all(podcasts_dir(media_root))?;
    std::fs::create_dir_all(music_dir(media_root))?;
    migrate_legacy_podcast_dirs(media_root)
}

fn is_numeric_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_digit())
}

fn migrate_legacy_podcast_dirs(media_root: &Path) -> AppResult<Vec<(PathBuf, PathBuf)>> {
    let mut moved = Vec::new();
    let entries = match std::fs::read_dir(media_root) {
        Ok(e) => e,
        Err(_) => return Ok(moved),
    };
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "podcasts" || name == "music" || !is_numeric_name(&name) {
            continue;
        }
        let dest_dir = podcasts_dir(media_root).join(&name);
        std::fs::create_dir_all(&dest_dir)?;
        if let Ok(files) = std::fs::read_dir(&src) {
            for file in files.flatten() {
                let from = file.path();
                let to = dest_dir.join(file.file_name());
                if to.exists() {
                    continue;
                }
                std::fs::rename(&from, &to)?;
                moved.push((from, to));
            }
        }
        let _ = std::fs::remove_dir(&src);
    }
    Ok(moved)
}

/// Rewrite an absolute/relative download path from the MVP layout
/// (`media/<id>/file`) to `media/podcasts/<id>/file`.
pub fn remap_legacy_file_path(media_root: &Path, stored: &str) -> Option<String> {
    let stored_path = PathBuf::from(stored);
    let podcasts = podcasts_dir(media_root);
    if path_is_within(&stored_path, &podcasts) {
        return None;
    }
    let file_name = stored_path.file_name()?.to_os_string();
    let parent = stored_path.parent()?;
    let show_id = parent.file_name()?.to_string_lossy();
    if !is_numeric_name(&show_id) {
        return None;
    }
    let grand = parent.parent()?;
    if !path_eq_or_within(grand, media_root) {
        return None;
    }
    let mapped = podcasts.join(show_id.as_ref()).join(file_name);
    Some(mapped.display().to_string())
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path.starts_with(root)
}

fn path_eq_or_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
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

    #[test]
    fn new_downloads_live_under_podcasts() {
        let root = PathBuf::from("/data/media");
        let p = media_file_path(&root, 7, 9, "https://cdn.example.com/a.m4a");
        assert_eq!(p, root.join("podcasts").join("7").join("9.m4a"));
    }

    #[test]
    fn remaps_legacy_show_dir() {
        let root = PathBuf::from("/data/media");
        let old = root.join("3").join("12.mp3");
        let mapped = remap_legacy_file_path(&root, &old.display().to_string()).unwrap();
        assert_eq!(
            mapped,
            root.join("podcasts")
                .join("3")
                .join("12.mp3")
                .display()
                .to_string()
        );
        assert!(remap_legacy_file_path(&root, &mapped).is_none());
    }

    #[test]
    fn migrates_numeric_dirs_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        let old = media.join("4");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("8.mp3"), b"ID3").unwrap();
        let moved = ensure_media_layout(&media).unwrap();
        assert_eq!(moved.len(), 1);
        assert!(media.join("podcasts").join("4").join("8.mp3").is_file());
        assert!(media.join("music").is_dir());
        assert!(
            !old.exists()
                || std::fs::read_dir(&old)
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(true)
        );
    }

    #[test]
    fn skips_move_when_destination_already_exists() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        let old = media.join("4");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("8.mp3"), b"OLD").unwrap();
        let dest = media.join("podcasts").join("4");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("8.mp3"), b"KEEP").unwrap();
        let moved = ensure_media_layout(&media).unwrap();
        assert!(moved.is_empty());
        assert_eq!(std::fs::read(old.join("8.mp3")).unwrap(), b"OLD");
        assert_eq!(std::fs::read(dest.join("8.mp3")).unwrap(), b"KEEP");
    }

    #[test]
    fn music_files_live_under_media_music() {
        let root = PathBuf::from("/data/media");
        let dest = music_track_file(&root, 7, 3, "dsf");
        assert_eq!(dest, root.join("music").join("7").join("3.dsf"));
    }
}
