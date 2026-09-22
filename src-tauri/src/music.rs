//! Local music library: separate tables from podcasts, files under `media/music/`.

use crate::db;
use crate::download;
use crate::error::{AppError, AppResult};
use crate::models::{MusicAlbum, MusicImportOutcome, MusicTrack};
use id3::TagLike;
use rusqlite::Connection;
use std::fs;
use std::io::Read;
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
    db::upsert_music_folder(conn, &normalize_path(&root))?;
    let files = collect_audio_files(&root)?;
    let mut imported = 0i64;
    let mut skipped = 0i64;
    let mut warnings = Vec::new();
    for src in files {
        match import_one(conn, media_root, &root, &src) {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(err) => {
                warnings.push(format!("{}: {err}", src.display()));
            }
        }
    }
    Ok(MusicImportOutcome {
        imported_tracks: imported,
        skipped_tracks: skipped,
        album_count: db::count_music_albums(conn)?,
        track_count: db::count_music_tracks(conn)?,
        warnings,
    })
}

fn import_one(
    conn: &Connection,
    media_root: &Path,
    import_root: &Path,
    src: &Path,
) -> AppResult<bool> {
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "iso" {
        return Err(AppError::msg("SACD-ISO rippning ingår inte."));
    }
    let source = normalize_path(&fs::canonicalize(src).unwrap_or_else(|_| src.to_path_buf()));
    if db::music_track_by_source(conn, &source)?.is_some() {
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
    let dest = download::music_track_file(media_root, album_id, track_id, &ext);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Err(err) = fs::copy(src, &dest) {
        let _ = conn.execute("DELETE FROM music_track WHERE id = ?1", rusqlite::params![track_id]);
        return Err(err.into());
    }
    let bytes = fs::metadata(&dest).map(|m| m.len() as i64).ok();
    db::set_music_track_file(conn, track_id, &dest.display().to_string(), bytes)?;
    Ok(true)
}

fn collect_audio_files(root: &Path) -> AppResult<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk(root, &mut out, 0)?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) -> AppResult<()> {
    if depth > 24 {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            walk(&path, out, depth + 1)?;
            continue;
        }
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if AUDIO_EXTS.contains(&ext.as_str()) {
            out.push(path);
        }
    }
    Ok(())
}

fn apply_path_fallback(tags: &mut Tags, import_root: &Path, src: &Path) {
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
    if tags.album.is_none() {
        if let Some(album) = parts.last() {
            tags.album = Some(album.clone());
        }
    }
    if tags.artist.is_none() && parts.len() >= 2 {
        tags.artist = Some(parts[parts.len() - 2].clone());
    }
    if tags.title.is_none() {
        if let Some(stem) = src.file_stem().and_then(|s| s.to_str()) {
            let (num, title) = split_track_prefix(stem);
            tags.title = Some(title);
            if tags.track_no.is_none() {
                tags.track_no = num;
            }
        }
    }
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
}
