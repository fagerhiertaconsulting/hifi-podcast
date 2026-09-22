//! Local music library: separate tables from podcasts, files under `media/music/`.

use crate::db;
use crate::download;
use crate::error::{AppError, AppResult};
use crate::models::{MusicAlbum, MusicImportOutcome, MusicTrack};
use crate::net::MAX_MEDIA_BYTES;
use id3::TagLike;
use rusqlite::Connection;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const AUDIO_EXTS: &[&str] = &[
    "flac", "wav", "wave", "aiff", "aif", "dsf", "dff", "dsd", "mp3", "m4a", "m4b", "aac", "ogg",
    "oga", "opus",
];

#[derive(Debug, Default, Clone)]
struct Tags {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    track_no: Option<i64>,
}

pub fn import_folder(
    conn: &Connection,
    media_root: &Path,
    folder: &Path,
) -> AppResult<MusicImportOutcome> {
    if !folder.is_dir() {
        return Err(AppError::msg("Mappen finns inte."));
    }
    let root = fs::canonicalize(folder)?;
    let files = collect_audio_files(&root)?;
    let mut copied_new: Vec<PathBuf> = Vec::new();
    conn.execute("BEGIN IMMEDIATE", [])?;
    let outcome = (|| {
        let mut imported = 0i64;
        let mut skipped = 0i64;
        let mut errors = Vec::new();
        for src in &files {
            match import_one(conn, media_root, &root, src, &mut copied_new) {
                Ok(true) => imported += 1,
                Ok(false) => skipped += 1,
                Err(err) => {
                    errors.push(format!("{}: {err}", src.display()));
                }
            }
        }
        let outcome = MusicImportOutcome {
            imported_tracks: imported,
            skipped_tracks: skipped,
            failed_tracks: errors.len() as i64,
            album_count: db::count_music_albums(conn)?,
            track_count: db::count_music_tracks(conn)?,
            errors,
        };
        if outcome.failed_tracks > 0 {
            return Err(AppError::msg(format!(
                "Import misslyckades: {} fil(er). {} importerade, {} hoppades över. {}",
                outcome.failed_tracks,
                outcome.imported_tracks,
                outcome.skipped_tracks,
                outcome.errors.join(" ")
            )));
        }
        db::upsert_music_folder(conn, &normalize_path(&root))?;
        db::set_setting(conn, "music_last_import_root", &root.display().to_string())?;
        Ok(outcome)
    })();
    match outcome {
        Ok(ok) => {
            conn.execute("COMMIT", [])?;
            Ok(ok)
        }
        Err(err) => {
            let _ = conn.execute("ROLLBACK", []);
            cleanup_copied_files(&copied_new);
            Err(err)
        }
    }
}

fn cleanup_copied_files(paths: &[PathBuf]) {
    for dest in paths {
        let _ = fs::remove_file(dest);
        if let Some(dir) = dest.parent() {
            let _ = fs::remove_dir(dir);
        }
    }
}

fn import_one(
    conn: &Connection,
    media_root: &Path,
    import_root: &Path,
    src: &Path,
    copied_new: &mut Vec<PathBuf>,
) -> AppResult<bool> {
    ensure_importable_file(src)?;
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "iso" {
        return Err(AppError::msg("SACD-ISO rippning ingår inte."));
    }
    let source = normalize_path(&fs::canonicalize(src)?);
    if let Some(existing) = db::music_track_by_source(conn, &source)? {
        if track_needs_repair(&existing) {
            finish_copy(conn, media_root, src, existing.album_id, existing.id, &ext)?;
            return Ok(true);
        }
        return Ok(false);
    }
    let mut tags = read_tags(src);
    apply_path_fallback(&mut tags, import_root, src);
    let probe = crate::engine::Handle::probe(src).ok();
    let duration_ms = probe.as_ref().and_then(|p| p.duration_ms).map(|v| v as i64);
    let sample_rate = probe.as_ref().and_then(|p| p.sample_rate).map(|v| v as i64);
    let channels = probe.as_ref().and_then(|p| p.channels).map(|v| v as i64);
    let bits = probe.as_ref().and_then(|p| p.bits_per_sample).map(|v| v as i64);
    let codec = probe.as_ref().and_then(|p| p.codec.clone());
    let format = if ext.is_empty() {
        "unknown".into()
    } else {
        ext.clone()
    };
    let artist = tags.artist.unwrap_or_else(|| "Okänd artist".into());
    let album_title = tags.album.unwrap_or_else(|| "Okänt album".into());
    let title = tags.title.unwrap_or_else(|| {
        src.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Spår")
            .to_string()
    });
    let album_id = db::upsert_music_album(conn, &album_title, &artist)?;
    let track_id = db::insert_music_track_placeholder(
        conn,
        album_id,
        &source,
        &title,
        &artist,
        &album_title,
        tags.track_no,
        duration_ms,
        &format,
        codec.as_deref(),
        sample_rate,
        bits,
        channels,
    )?;
    match finish_copy(conn, media_root, src, album_id, track_id, &ext) {
        Ok(dest) => {
            copied_new.push(dest);
            Ok(true)
        }
        Err(err) => {
            let _ = db::delete_music_track(conn, track_id);
            Err(err)
        }
    }
}

fn track_needs_repair(track: &MusicTrack) -> bool {
    let path = track.file_path.trim();
    path.is_empty() || !Path::new(path).is_file()
}

fn finish_copy(
    conn: &Connection,
    media_root: &Path,
    src: &Path,
    album_id: i64,
    track_id: i64,
    ext: &str,
) -> AppResult<PathBuf> {
    let dest = download::music_track_file(media_root, album_id, track_id, ext);
    let bytes = copy_regular_limited(src, &dest, MAX_MEDIA_BYTES)?;
    db::set_music_track_file(conn, track_id, &dest.display().to_string(), Some(bytes as i64))?;
    Ok(dest)
}

fn ensure_importable_file(path: &Path) -> AppResult<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(AppError::msg("Symlänkar importeras inte."));
    }
    if !meta.is_file() {
        return Err(AppError::msg("Bara vanliga filer kan importeras."));
    }
    if meta.len() > MAX_MEDIA_BYTES {
        return Err(AppError::msg("Ljudfilen är större än tillåtet (2 GB)."));
    }
    Ok(meta)
}

fn copy_regular_limited(src: &Path, dest: &Path, max_bytes: u64) -> AppResult<u64> {
    let meta = ensure_importable_file(src)?;
    if meta.len() > max_bytes {
        return Err(AppError::msg("Ljudfilen är större än tillåtet (2 GB)."));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = {
        let mut name = dest.as_os_str().to_os_string();
        name.push(".part");
        PathBuf::from(name)
    };
    let mut input = fs::File::open(src)?;
    let mut output = fs::File::create(&tmp)?;
    let mut buf = [0u8; 64 * 1024];
    let mut written: u64 = 0;
    let copy_err = loop {
        let n = match input.read(&mut buf) {
            Ok(0) => break None,
            Ok(n) => n,
            Err(err) => break Some(err.into()),
        };
        written = written.saturating_add(n as u64);
        if written > max_bytes {
            break Some(AppError::msg("Ljudfilen är större än tillåtet (2 GB)."));
        }
        if let Err(err) = output.write_all(&buf[..n]) {
            break Some(err.into());
        }
    };
    if let Some(err) = copy_err {
        drop(output);
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    output.flush()?;
    drop(output);
    fs::rename(&tmp, dest)?;
    Ok(written)
}

fn collect_audio_files(root: &Path) -> AppResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(root, &mut out, 0)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) -> AppResult<()> {
    if depth > 24 {
        return Err(AppError::msg("Katalogstrukturen är för djup."));
    }
    let meta = fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(AppError::msg("Symlänkar importeras inte."));
    }
    if !meta.is_dir() {
        return Err(AppError::msg("Sökvägen är inte en mapp."));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() {
            return Err(AppError::msg(format!(
                "Symlänkar importeras inte ({}).",
                path.display()
            )));
        }
        if meta.is_dir() {
            walk(&path, out, depth + 1)?;
            continue;
        }
        if !meta.is_file() {
            return Err(AppError::msg(format!(
                "Specialfiler importeras inte ({}).",
                path.display()
            )));
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if AUDIO_EXTS.contains(&ext.as_str()) {
            if meta.len() > MAX_MEDIA_BYTES {
                return Err(AppError::msg(format!(
                    "{}: Ljudfilen är större än tillåtet (2 GB).",
                    path.display()
                )));
            }
            out.push(path);
        }
    }
    Ok(())
}

fn apply_path_fallback(tags: &mut Tags, import_root: &Path, src: &Path) {
    let parts = folder_chain(import_root, src);
    let meaningful: Vec<String> = parts
        .iter()
        .filter(|p| !is_generic_release_folder(p))
        .cloned()
        .collect();
    let album_folder = meaningful.last().or(parts.last());
    if tags.album.is_none() {
        if let Some(name) = album_folder {
            if let Some((artist, album)) = split_artist_title(name) {
                tags.album = Some(album);
                if tags.artist.is_none() {
                    tags.artist = Some(artist);
                }
            } else {
                tags.album = Some(name.clone());
            }
        }
    }
    if tags.artist.is_none() {
        if meaningful.len() >= 2 {
            tags.artist = Some(meaningful[meaningful.len() - 2].clone());
        } else if parts.len() >= 2 {
            tags.artist = Some(parts[parts.len() - 2].clone());
        }
    }
    if tags.title.is_none() {
        if let Some(stem) = src.file_stem().and_then(|s| s.to_str()) {
            let (num, title) = split_track_prefix(stem);
            if tags.track_no.is_none() {
                tags.track_no = num;
            }
            if tags.artist.is_none() {
                if let Some((artist, rest)) = split_artist_title(&title) {
                    tags.artist = Some(artist);
                    tags.title = Some(rest);
                } else {
                    tags.title = Some(title);
                }
            } else {
                tags.title = Some(title);
            }
        }
    }
}

fn folder_chain(import_root: &Path, src: &Path) -> Vec<String> {
    let rel = src.strip_prefix(import_root).unwrap_or(src);
    let mut parts: Vec<String> = rel
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    parts.retain(|p| !p.is_empty() && p != ".");
    if parts.is_empty() {
        if let Some(name) = import_root.file_name().and_then(|s| s.to_str()) {
            if !name.is_empty() && name != "." {
                parts.push(name.to_string());
            }
        }
    }
    parts
}

fn is_generic_release_folder(name: &str) -> bool {
    let compact: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    matches!(
        compact.as_str(),
        "stereo"
            | "multichannel"
            | "multi"
            | "scans"
            | "artwork"
            | "covers"
            | "bonus"
            | "sacd"
            | "iso"
            | "dsd"
            | "dsd64"
            | "dsd128"
            | "dsd256"
            | "dsd512"
    ) || compact
        .strip_prefix("cd")
        .or_else(|| compact.strip_prefix("disc"))
        .or_else(|| compact.strip_prefix("disk"))
        .or_else(|| compact.strip_prefix("skiva"))
        .is_some_and(|rest| rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit()))
}

fn split_artist_title(name: &str) -> Option<(String, String)> {
    let (left, right) = name.split_once(" - ")?;
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || right.is_empty() {
        return None;
    }
    if left.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((left.to_string(), right.to_string()))
}

fn split_track_prefix(stem: &str) -> (Option<i64>, String) {
    let trimmed = stem.trim();
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 || i > 3 {
        return (None, trimmed.to_string());
    }
    let num = trimmed[..i].parse::<i64>().ok();
    let rest = trimmed[i..].trim_start_matches(['.', '-', '_', ' ']);
    if rest.is_empty() {
        (num, trimmed.to_string())
    } else {
        (num, rest.to_string())
    }
}

fn read_tags(path: &Path) -> Tags {
    let mut tags = Tags::default();
    if let Ok(tag) = id3::Tag::read_from_path(path) {
        tags.title = nonempty(tag.title());
        tags.artist = nonempty(tag.artist());
        tags.album = nonempty(tag.album());
        tags.track_no = tag.track().map(|n| n as i64);
    }
    if tags.title.is_none() || tags.artist.is_none() || tags.album.is_none() {
        merge_tags(&mut tags, read_flac_vorbis(path));
    }
    tags
}

fn merge_tags(into: &mut Tags, other: Tags) {
    if into.title.is_none() {
        into.title = other.title;
    }
    if into.artist.is_none() {
        into.artist = other.artist;
    }
    if into.album.is_none() {
        into.album = other.album;
    }
    if into.track_no.is_none() {
        into.track_no = other.track_no;
    }
}

fn nonempty(v: Option<&str>) -> Option<String> {
    v.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string())
}

fn read_flac_vorbis(path: &Path) -> Tags {
    let mut tags = Tags::default();
    let Ok(mut file) = fs::File::open(path) else {
        return tags;
    };
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() || &magic != b"fLaC" {
        return tags;
    }
    loop {
        let mut header = [0u8; 4];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        let last = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7f;
        let len = u32::from_be_bytes([0, header[1], header[2], header[3]]) as usize;
        let mut body = vec![0u8; len];
        if file.read_exact(&mut body).is_err() {
            break;
        }
        if block_type == 4 {
            tags = parse_vorbis_comment(&body);
            break;
        }
        if last {
            break;
        }
    }
    tags
}

fn parse_vorbis_comment(body: &[u8]) -> Tags {
    let mut tags = Tags::default();
    if body.len() < 8 {
        return tags;
    }
    let vendor_len = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
    let mut i = 4 + vendor_len;
    if i + 4 > body.len() {
        return tags;
    }
    let count = u32::from_le_bytes(body[i..i + 4].try_into().unwrap()) as usize;
    i += 4;
    for _ in 0..count {
        if i + 4 > body.len() {
            break;
        }
        let n = u32::from_le_bytes(body[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if i + n > body.len() {
            break;
        }
        if let Ok(s) = std::str::from_utf8(&body[i..i + n]) {
            if let Some((k, v)) = s.split_once('=') {
                let v = v.trim();
                if v.is_empty() {
                    i += n;
                    continue;
                }
                match k.to_ascii_uppercase().as_str() {
                    "TITLE" => tags.title = Some(v.to_string()),
                    "ARTIST" | "ALBUMARTIST" => {
                        if tags.artist.is_none() {
                            tags.artist = Some(v.to_string());
                        }
                    }
                    "ALBUM" => tags.album = Some(v.to_string()),
                    "TRACKNUMBER" => {
                        tags.track_no = v
                            .split(['/', '-'])
                            .next()
                            .and_then(|t| t.trim().parse().ok());
                    }
                    _ => {}
                }
            }
        }
        i += n;
    }
    tags
}

fn normalize_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

pub fn list_albums(conn: &Connection) -> AppResult<Vec<MusicAlbum>> {
    db::list_music_albums(conn)
}

pub fn list_tracks(conn: &Connection, album_id: Option<i64>) -> AppResult<Vec<MusicTrack>> {
    db::list_music_tracks(conn, album_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use rusqlite::Connection;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        db::initialize(&c).unwrap();
        c
    }

    #[test]
    fn splits_track_numbers_from_filenames() {
        assert_eq!(
            split_track_prefix("01 - Blue in Green"),
            (Some(1), "Blue in Green".into())
        );
        assert_eq!(split_track_prefix("Song"), (None, "Song".into()));
    }

    #[test]
    fn path_fallback_uses_folders_filename_and_skips_disc() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        let src = root
            .join("Miles Davis")
            .join("Kind of Blue")
            .join("Disc 1")
            .join("DSD256")
            .join("01 - So What.dsf");
        let mut tags = Tags::default();
        apply_path_fallback(&mut tags, &root, &src);
        assert_eq!(tags.artist.as_deref(), Some("Miles Davis"));
        assert_eq!(tags.album.as_deref(), Some("Kind of Blue"));
        assert_eq!(tags.title.as_deref(), Some("So What"));
        assert_eq!(tags.track_no, Some(1));
    }

    #[test]
    fn path_fallback_splits_artist_album_folder_and_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Bill Evans - Waltz for Debby");
        let src = root.join("02 Bill Evans - My Foolish Heart.flac");
        let mut tags = Tags::default();
        apply_path_fallback(&mut tags, &root, &src);
        assert_eq!(tags.artist.as_deref(), Some("Bill Evans"));
        assert_eq!(tags.album.as_deref(), Some("Waltz for Debby"));
        assert_eq!(tags.title.as_deref(), Some("My Foolish Heart"));
        assert_eq!(tags.track_no, Some(2));
    }

    #[test]
    fn path_fallback_uses_import_root_name_when_files_are_flat() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Kind of Blue");
        let src = root.join("So What.dsf");
        let mut tags = Tags::default();
        apply_path_fallback(&mut tags, &root, &src);
        assert_eq!(tags.album.as_deref(), Some("Kind of Blue"));
        assert_eq!(tags.title.as_deref(), Some("So What"));
    }

    #[test]
    fn import_uses_music_tables_not_episodes() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("Downloads").join("Miles").join("Kind of Blue");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("01 So What.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 2, 440.0, 0.2, 0.2);
        fs::write(folder.join("notes.txt"), "ignore").unwrap();

        let out = import_folder(&db, &media, tmp.path().join("Downloads").as_path()).unwrap();
        assert_eq!(out.imported_tracks, 1);
        assert_eq!(out.skipped_tracks, 0);
        assert_eq!(out.album_count, 1);
        assert_eq!(out.track_count, 1);

        let albums = list_albums(&db).unwrap();
        assert_eq!(albums[0].artist, "Miles");
        assert_eq!(albums[0].title, "Kind of Blue");
        let tracks = list_tracks(&db, Some(albums[0].id)).unwrap();
        assert_eq!(tracks[0].title, "So What");
        assert_eq!(tracks[0].track_no, Some(1));
        assert_eq!(tracks[0].format, "wav");
        assert!(Path::new(&tracks[0].file_path).starts_with(&media.join("music")));
        assert!(Path::new(&tracks[0].file_path).is_file());

        let shows: i64 = db
            .query_row("SELECT COUNT(*) FROM episode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(shows, 0);

        let again = import_folder(&db, &media, tmp.path().join("Downloads").as_path()).unwrap();
        assert_eq!(again.imported_tracks, 0);
        assert_eq!(again.skipped_tracks, 1);
        assert_eq!(again.track_count, 1);
    }

    #[test]
    fn parses_flac_vorbis_comments() {
        let mut body = Vec::new();
        body.extend_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(b"vendor");
        body.extend_from_slice(&2u32.to_le_bytes());
        let c1 = b"TITLE=Flamenco Sketches";
        body.extend_from_slice(&(c1.len() as u32).to_le_bytes());
        body.extend_from_slice(c1);
        let c2 = b"ARTIST=Miles Davis";
        body.extend_from_slice(&(c2.len() as u32).to_le_bytes());
        body.extend_from_slice(c2);
        let tags = parse_vorbis_comment(&body);
        assert_eq!(tags.title.as_deref(), Some("Flamenco Sketches"));
        assert_eq!(tags.artist.as_deref(), Some("Miles Davis"));
    }

    #[test]
    fn repairs_empty_file_path_on_reimport() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("lib");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("tune.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 1, 440.0, 0.1, 0.2);
        import_folder(&db, &media, &folder).unwrap();
        let track = list_tracks(&db, None).unwrap().remove(0);
        fs::remove_file(&track.file_path).unwrap();
        db::set_music_track_file(&db, track.id, "", None).unwrap();
        let again = import_folder(&db, &media, &folder).unwrap();
        assert_eq!(again.imported_tracks, 1);
        assert_eq!(again.skipped_tracks, 0);
        let repaired = list_tracks(&db, None).unwrap().remove(0);
        assert_eq!(repaired.id, track.id);
        assert!(Path::new(&repaired.file_path).is_file());
    }

    #[test]
    fn copy_rejects_oversize_and_leaves_no_dest() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("big.wav");
        fs::write(&src, vec![0u8; 64]).unwrap();
        let dest = tmp.path().join("out.wav");
        let err = copy_regular_limited(&src, &dest, 8).unwrap_err();
        assert!(err.to_string().contains("större än tillåtet"));
        assert!(!dest.exists());
        assert!(!PathBuf::from(format!("{}.part", dest.display())).exists());
    }

    #[test]
    fn import_fails_closed_when_a_file_cannot_be_copied() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("mix");
        fs::create_dir_all(&folder).unwrap();
        let good = folder.join("ok.wav");
        crate::engine::test_wav::write_sine_wav(&good, 44100, 1, 440.0, 0.1, 0.2);
        fs::remove_dir_all(media.join("music")).unwrap();
        fs::write(media.join("music"), b"not-a-directory").unwrap();
        let err = import_folder(&db, &media, &folder).unwrap_err();
        assert!(err.to_string().contains("Import misslyckades"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
        assert!(db::last_music_import_root(&db).unwrap().is_none());
    }

    #[test]
    fn import_rolls_back_earlier_tracks_and_copied_files() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("mix");
        fs::create_dir_all(&folder).unwrap();
        let a = folder.join("a.wav");
        let b = folder.join("b.wav");
        crate::engine::test_wav::write_sine_wav(&a, 44100, 1, 440.0, 0.05, 0.2);
        crate::engine::test_wav::write_sine_wav(&b, 44100, 1, 440.0, 0.05, 0.2);
        fs::create_dir_all(media.join("music").join("1").join("2.wav")).unwrap();
        let err = import_folder(&db, &media, &folder).unwrap_err();
        assert!(err.to_string().contains("Import misslyckades"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
        assert_eq!(db::count_music_albums(&db).unwrap(), 0);
        assert!(!media.join("music").join("1").join("1.wav").is_file());
        assert!(db::last_music_import_root(&db).unwrap().is_none());
    }

    #[test]
    fn successful_import_remembers_root_and_failed_run_keeps_previous() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let good = tmp.path().join("good");
        fs::create_dir_all(&good).unwrap();
        let wav = good.join("tune.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 1, 440.0, 0.05, 0.2);
        import_folder(&db, &media, &good).unwrap();
        let remembered = db::last_music_import_root(&db).unwrap().unwrap();
        assert!(remembered.contains("good"));

        let bad = tmp.path().join("bad");
        fs::create_dir_all(&bad).unwrap();
        let ok = bad.join("ok.wav");
        let later = bad.join("later.wav");
        crate::engine::test_wav::write_sine_wav(&ok, 44100, 1, 440.0, 0.05, 0.2);
        crate::engine::test_wav::write_sine_wav(&later, 44100, 1, 440.0, 0.05, 0.2);
        fs::create_dir_all(media.join("music").join("2").join("3.wav")).unwrap();
        let err = import_folder(&db, &media, &bad).unwrap_err();
        assert!(err.to_string().contains("Import misslyckades"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 1);
        assert_eq!(
            db::last_music_import_root(&db).unwrap().as_deref(),
            Some(remembered.as_str())
        );
    }

    #[cfg(unix)]
    #[test]
    fn import_rejects_symlinked_audio() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("src");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("real.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 1, 440.0, 0.1, 0.2);
        std::os::unix::fs::symlink(&wav, folder.join("alias.wav")).unwrap();
        let err = import_folder(&db, &media, &folder).unwrap_err();
        assert!(err.to_string().contains("Symlänk"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
    }
}
