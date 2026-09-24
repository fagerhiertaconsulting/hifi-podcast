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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowMediaDelete {
    pub removed_files: i64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeMediaDelete {
    pub removed_files: i64,
    pub warnings: Vec<String>,
    /// The download row may be cleared. False when the show directory was
    /// unsafe to treat as deleted (symlink, not a directory, or escaped).
    pub forget_download: bool,
}

enum ShowDirKind {
    Absent,
    Symlink(PathBuf),
    NotDirectory,
    Escapes,
    Ready(PathBuf),
}

/// Locate `media/podcasts/<podcast_id>/` without following a symlink.
fn classify_show_dir(media_root: &Path, podcast_id: i64) -> AppResult<ShowDirKind> {
    if podcast_id <= 0 {
        return Err(AppError::msg("Ogiltigt podcast-id."));
    }
    let id_name = podcast_id.to_string();
    if !id_name.chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::msg("Ogiltigt podcast-id."));
    }

    let root_meta = match std::fs::symlink_metadata(media_root) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ShowDirKind::Absent);
        }
        Err(err) => return Err(err.into()),
    };
    if root_meta.file_type().is_symlink() || !root_meta.is_dir() {
        return Err(AppError::msg(
            "Mediaroten är inte en vanlig katalog. Inga filer raderades.",
        ));
    }
    let root = std::fs::canonicalize(media_root)?;
    let podcasts = podcasts_dir(&root);
    let podcasts_meta = match std::fs::symlink_metadata(&podcasts) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ShowDirKind::Absent);
        }
        Err(err) => return Err(err.into()),
    };
    if podcasts_meta.file_type().is_symlink() || !podcasts_meta.is_dir() {
        return Err(AppError::msg(
            "Mediamappen för podcasts är inte en vanlig katalog. Inga filer raderades.",
        ));
    }
    let podcasts_canon = std::fs::canonicalize(&podcasts)?;
    if podcasts_canon != root.join("podcasts") || !podcasts_canon.starts_with(&root) {
        return Err(AppError::msg(
            "Mediamappen för podcasts ligger utanför mediaroten. Inga filer raderades.",
        ));
    }

    let show = podcasts_canon.join(&id_name);
    if show.parent() != Some(podcasts_canon.as_path())
        || show.file_name().map(|n| n == id_name.as_str()) != Some(true)
    {
        return Err(AppError::msg(
            "Poddens mediamapp ligger utanför tillåten katalog. Inga filer raderades.",
        ));
    }

    let show_meta = match std::fs::symlink_metadata(&show) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ShowDirKind::Absent);
        }
        Err(err) => return Err(err.into()),
    };

    if show_meta.file_type().is_symlink() {
        return Ok(ShowDirKind::Symlink(show));
    }
    if !show_meta.is_dir() {
        return Ok(ShowDirKind::NotDirectory);
    }

    let show_canon = std::fs::canonicalize(&show)?;
    if show_canon != show {
        return Ok(ShowDirKind::Escapes);
    }
    Ok(ShowDirKind::Ready(show_canon))
}

/// Delete files that belong to one show. The walk stays inside
/// `media/podcasts/<podcast_id>/`. Symlinks are unlinked and never followed.
/// Stored paths outside that directory are not accepted here at all.
pub fn delete_show_media(media_root: &Path, podcast_id: i64) -> AppResult<ShowMediaDelete> {
    match classify_show_dir(media_root, podcast_id)? {
        ShowDirKind::Absent => Ok(ShowMediaDelete {
            removed_files: 0,
            warnings: Vec::new(),
        }),
        ShowDirKind::Symlink(show) => {
            unlink_symlink(&show)?;
            Ok(ShowMediaDelete {
                removed_files: 0,
                warnings: vec!["Poddens mediamapp var en symlänk. Målet lämnades orört.".into()],
            })
        }
        ShowDirKind::NotDirectory => Ok(ShowMediaDelete {
            removed_files: 0,
            warnings: vec![
                "Poddens mediamapp är inte en vanlig katalog och lämnades orörd.".into(),
            ],
        }),
        ShowDirKind::Escapes => Ok(ShowMediaDelete {
            removed_files: 0,
            warnings: vec![
                "Poddens mediamapp pekar utanför den egna katalogen och lämnades orörd.".into(),
            ],
        }),
        ShowDirKind::Ready(show_canon) => {
            let removed = purge_show_tree(&show_canon, &show_canon, 0)?;
            if let Ok(meta) = std::fs::symlink_metadata(&show_canon) {
                if meta.is_dir() && !meta.file_type().is_symlink() {
                    if std::fs::canonicalize(&show_canon).ok().as_deref()
                        == Some(show_canon.as_path())
                    {
                        match std::fs::remove_dir(&show_canon) {
                            Ok(()) => {}
                            Err(err) if err.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
                            Err(err) => return Err(err.into()),
                        }
                    }
                }
            }
            Ok(ShowMediaDelete {
                removed_files: removed,
                warnings: Vec::new(),
            })
        }
    }
}

/// Delete one episode file directly under `media/podcasts/<podcast_id>/`.
/// The stored path is never opened. A path that leaves the show directory
/// is ignored. Symlinks are unlinked and not followed. The show directory
/// itself is left in place so other episodes stay on disk.
pub fn delete_episode_media(
    media_root: &Path,
    podcast_id: i64,
    episode_id: i64,
    stored_path: Option<&str>,
) -> AppResult<EpisodeMediaDelete> {
    if episode_id <= 0 || !episode_id.to_string().chars().all(|c| c.is_ascii_digit()) {
        return Err(AppError::msg("Ogiltigt avsnitts-id."));
    }
    let kind = classify_show_dir(media_root, podcast_id)?;
    let ShowDirKind::Ready(show) = kind else {
        return Ok(match kind {
            ShowDirKind::Absent => EpisodeMediaDelete {
                removed_files: 0,
                warnings: Vec::new(),
                forget_download: true,
            },
            ShowDirKind::Symlink(_) => EpisodeMediaDelete {
                removed_files: 0,
                warnings: vec![
                    "Poddens mediamapp var en symlänk. Ingen avsnittsfil raderades.".into(),
                ],
                forget_download: false,
            },
            ShowDirKind::NotDirectory => EpisodeMediaDelete {
                removed_files: 0,
                warnings: vec![
                    "Poddens mediamapp är inte en vanlig katalog och lämnades orörd.".into(),
                ],
                forget_download: false,
            },
            ShowDirKind::Escapes => EpisodeMediaDelete {
                removed_files: 0,
                warnings: vec![
                    "Poddens mediamapp pekar utanför den egna katalogen och lämnades orörd.".into(),
                ],
                forget_download: false,
            },
            ShowDirKind::Ready(_) => unreachable!(),
        });
    };

    let mut warnings = Vec::new();
    if let Some(stored) = stored_path.map(str::trim).filter(|s| !s.is_empty()) {
        if !stored_path_stays_in_show(&show, stored) {
            warnings
                .push("Sparad sökväg ligger utanför poddens mediamapp och lämnades orörd.".into());
        }
    }

    let (removed, mut file_warnings, blocked) = delete_matching_episode_files(&show, episode_id)?;
    warnings.append(&mut file_warnings);
    Ok(EpisodeMediaDelete {
        removed_files: removed,
        warnings,
        forget_download: !blocked,
    })
}

fn stored_path_stays_in_show(show: &Path, stored: &str) -> bool {
    let path = Path::new(stored);
    if path.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if !name_is_safe(name) || !path.starts_with(show) {
        return false;
    }
    path.parent() == Some(show)
}

fn name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

fn episode_media_name(episode_id: i64, name: &str) -> bool {
    if episode_id <= 0 {
        return false;
    }
    let prefix = format!("{episode_id}.");
    let lower = name.to_ascii_lowercase();
    let Some(ext) = lower.strip_prefix(&prefix) else {
        return false;
    };
    matches!(
        ext,
        "mp3" | "m4a" | "aac" | "ogg" | "opus" | "flac" | "wav" | "mp4" | "m4b" | "bin" | "part"
    )
}

fn ensure_show_still_ready(show: &Path) -> AppResult<()> {
    let meta = std::fs::symlink_metadata(show)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(AppError::msg("Kan inte radera utanför poddens mediamapp."));
    }
    let canon = std::fs::canonicalize(show)?;
    if canon != show {
        return Err(AppError::msg("Kan inte radera utanför poddens mediamapp."));
    }
    Ok(())
}

fn delete_matching_episode_files(
    show: &Path,
    episode_id: i64,
) -> AppResult<(i64, Vec<String>, bool)> {
    ensure_show_still_ready(show)?;
    let mut removed = 0i64;
    let mut warnings = Vec::new();
    let mut blocked = false;
    for entry in std::fs::read_dir(show)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if !name_is_safe(&name) || !episode_media_name(episode_id, &name) {
            continue;
        }
        let path = show.join(&name);
        if path.parent() != Some(show) {
            continue;
        }
        ensure_show_still_ready(show)?;
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            unlink_symlink(&path)?;
            removed += 1;
            continue;
        }
        if meta.is_dir() {
            blocked = true;
            warnings.push("En avsnittsfil är en katalog och lämnades orörd.".into());
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let again = std::fs::symlink_metadata(&path)?;
        if again.file_type().is_symlink() {
            unlink_symlink(&path)?;
            removed += 1;
            continue;
        }
        if !again.is_file() {
            continue;
        }
        std::fs::remove_file(&path)?;
        removed += 1;
    }
    Ok((removed, warnings, blocked))
}

fn unlink_symlink(path: &Path) -> AppResult<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_symlink() {
        return Err(AppError::msg(
            "Vägrade radera en sökväg som inte längre är en symlänk.",
        ));
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(file_err) => {
            let again = std::fs::symlink_metadata(path)?;
            if !again.file_type().is_symlink() {
                return Err(AppError::msg(
                    "Vägrade radera en sökväg som inte längre är en symlänk.",
                ));
            }
            match std::fs::remove_dir(path) {
                Ok(()) => Ok(()),
                Err(_) => Err(file_err.into()),
            }
        }
    }
}

fn entry_name_is_safe(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let name = name.to_string_lossy();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

fn parent_is_inside(path: &Path, show_root: &Path) -> AppResult<bool> {
    if !entry_name_is_safe(path) {
        return Ok(false);
    }
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    let parent_canon = std::fs::canonicalize(parent)?;
    Ok(parent_canon == show_root || parent_canon.starts_with(show_root))
}

fn purge_show_tree(dir: &Path, show_root: &Path, depth: u32) -> AppResult<i64> {
    if depth > 24 {
        return Err(AppError::msg("Katalogstrukturen är för djup."));
    }
    let dir_meta = std::fs::symlink_metadata(dir)?;
    if dir_meta.file_type().is_symlink() || !dir_meta.is_dir() {
        return Err(AppError::msg("Kan inte radera utanför poddens mediamapp."));
    }
    let dir_canon = std::fs::canonicalize(dir)?;
    if dir_canon != show_root && !dir_canon.starts_with(show_root) {
        return Err(AppError::msg("Kan inte radera utanför poddens mediamapp."));
    }

    let mut removed = 0i64;
    for entry in std::fs::read_dir(&dir_canon)? {
        let entry = entry?;
        let path = entry.path();
        if !entry_name_is_safe(&path) || !parent_is_inside(&path, show_root)? {
            continue;
        }
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            unlink_symlink(&path)?;
            removed += 1;
            continue;
        }
        if meta.is_dir() {
            let child = std::fs::canonicalize(&path)?;
            if !child.starts_with(show_root) || child == show_root {
                continue;
            }
            let again = std::fs::symlink_metadata(&path)?;
            if again.file_type().is_symlink() {
                unlink_symlink(&path)?;
                removed += 1;
                continue;
            }
            if !again.is_dir() {
                continue;
            }
            removed += purge_show_tree(&child, show_root, depth + 1)?;
            if let Ok(after) = std::fs::symlink_metadata(&path) {
                if after.is_dir() && !after.file_type().is_symlink() {
                    if std::fs::canonicalize(&path)
                        .ok()
                        .is_some_and(|canon| canon.starts_with(show_root) && canon != show_root)
                    {
                        let _ = std::fs::remove_dir(&path);
                    }
                }
            }
            continue;
        }
        if meta.is_file() {
            let canon = match std::fs::canonicalize(&path) {
                Ok(canon) => canon,
                Err(_) => continue,
            };
            if !canon.starts_with(show_root) || canon == show_root {
                continue;
            }
            let again = std::fs::symlink_metadata(&path)?;
            if again.file_type().is_symlink() {
                unlink_symlink(&path)?;
                removed += 1;
                continue;
            }
            if !again.is_file() {
                continue;
            }
            std::fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
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

    #[test]
    fn delete_episode_removes_only_that_file() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        ensure_media_layout(&media).unwrap();
        let show = media.join("podcasts").join("4");
        std::fs::create_dir_all(&show).unwrap();
        let own = show.join("8.mp3");
        let part = show.join("8.part");
        let sibling = show.join("9.mp3");
        let cover = show.join("cover.jpg");
        let prefixed = show.join("80.mp3");
        std::fs::write(&own, b"own").unwrap();
        std::fs::write(&part, b"part").unwrap();
        std::fs::write(&sibling, b"sib").unwrap();
        std::fs::write(&cover, b"jpeg").unwrap();
        std::fs::write(&prefixed, b"prefix").unwrap();
        let stored = own.display().to_string();

        let outcome = delete_episode_media(&media, 4, 8, Some(&stored)).unwrap();
        assert_eq!(outcome.removed_files, 2);
        assert!(outcome.warnings.is_empty());
        assert!(outcome.forget_download);
        assert!(!own.exists());
        assert!(!part.exists());
        assert_eq!(std::fs::read(&sibling).unwrap(), b"sib");
        assert_eq!(std::fs::read(&cover).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(&prefixed).unwrap(), b"prefix");
        assert!(show.is_dir());
    }

    #[test]
    fn delete_episode_does_not_use_stored_path_that_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        ensure_media_layout(&media).unwrap();
        let show = media.join("podcasts").join("4");
        std::fs::create_dir_all(&show).unwrap();
        let own = show.join("8.mp3");
        std::fs::write(&own, b"own").unwrap();
        let secret = dir.path().join("secret.mp3");
        std::fs::write(&secret, b"secret").unwrap();
        let escaped = show.join("..").join("..").join("secret.mp3");
        let sibling = media.join("podcasts").join("5").join("8.mp3");
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, b"other-show").unwrap();
        let music = media.join("music").join("1.flac");
        std::fs::write(&music, b"flac").unwrap();

        let outcome =
            delete_episode_media(&media, 4, 8, Some(&escaped.display().to_string())).unwrap();
        assert_eq!(outcome.removed_files, 1);
        assert!(outcome.warnings.iter().any(|w| w.contains("utanför")));
        assert!(outcome.forget_download);
        assert!(!own.exists());
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert_eq!(std::fs::read(&sibling).unwrap(), b"other-show");
        assert_eq!(std::fs::read(&music).unwrap(), b"flac");
        assert!(show.is_dir());
    }
}
