//! Local music library: separate tables from podcasts.
//! Tracks stay in the folder the user chose. Import indexes that path; it does not copy into appdata.

use crate::db;
use crate::error::{AppError, AppResult};
use crate::models::{MusicAlbum, MusicImportOutcome, MusicTrack};
use crate::net::MAX_MEDIA_BYTES;
use id3::TagLike;
use rusqlite::Connection;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

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

pub fn import_folder(conn: &Connection, folder: &Path) -> AppResult<MusicImportOutcome> {
    let root = prepare_import_root(folder)?;
    let files = collect_audio_files(&root)?;
    import_indexed_files(conn, &root, &files)
}

fn import_indexed_files(
    conn: &Connection,
    root: &Path,
    files: &[PathBuf],
) -> AppResult<MusicImportOutcome> {
    conn.execute("BEGIN IMMEDIATE", [])?;
    let outcome = (|| {
        let mut imported = 0i64;
        let mut skipped = 0i64;
        let mut errors = Vec::new();
        for src in files {
            match import_one(conn, root, src) {
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
        db::upsert_music_folder(conn, &normalize_path(root))?;
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
            Err(err)
        }
    }
}

fn import_one(conn: &Connection, import_root: &Path, src: &Path) -> AppResult<bool> {
    let (linked, byte_len) = link_source(import_root, src, MAX_MEDIA_BYTES)?;
    let source = normalize_path(&linked);
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "iso" {
        return Err(AppError::msg("SACD-ISO rippning ingår inte."));
    }
    let bytes = byte_len as i64;
    if let Some(existing) = db::music_track_by_source(conn, &source)? {
        if track_needs_relink(&existing, &source) {
            db::set_music_track_file(conn, existing.id, &source, Some(bytes))?;
            return Ok(true);
        }
        return Ok(false);
    }
    let mut tags = read_tags(&linked);
    apply_path_fallback(&mut tags, import_root, &linked);
    let probe = crate::engine::Handle::probe(&linked).ok();
    let duration_ms = probe.as_ref().and_then(|p| p.duration_ms).map(|v| v as i64);
    let sample_rate = probe.as_ref().and_then(|p| p.sample_rate).map(|v| v as i64);
    let channels = probe.as_ref().and_then(|p| p.channels).map(|v| v as i64);
    let bits = probe
        .as_ref()
        .and_then(|p| p.bits_per_sample)
        .map(|v| v as i64);
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
    if let Err(err) = db::set_music_track_file(conn, track_id, &source, Some(bytes)) {
        let _ = db::delete_music_track(conn, track_id);
        return Err(err);
    }
    Ok(true)
}

fn track_needs_relink(track: &MusicTrack, source: &str) -> bool {
    let path = track.file_path.trim().replace('\\', "/");
    path.is_empty() || path != source
}

/// Index `src` by its real path inside `import_root`. Does not copy bytes.
fn link_source(import_root: &Path, src: &Path, max_bytes: u64) -> AppResult<(PathBuf, u64)> {
    let path = contained_file(import_root, src)?;
    let meta = assert_regular_file(&path, max_bytes, false)?;
    Ok((path, meta.len()))
}

fn prepare_import_root(folder: &Path) -> AppResult<PathBuf> {
    let path = lexical_absolute(folder)?;
    let meta = fs::symlink_metadata(&path).map_err(|_| AppError::msg("Mappen finns inte."))?;
    if metadata_is_link(&meta) {
        return Err(AppError::msg("Symlänkar importeras inte."));
    }
    if !meta.is_dir() {
        return Err(AppError::msg("Mappen finns inte."));
    }
    Ok(path)
}

fn contained_file(import_root: &Path, src: &Path) -> AppResult<PathBuf> {
    let root = lexical_absolute(import_root)?;
    let file = lexical_absolute(src)?;
    if !file.starts_with(&root) {
        return Err(AppError::msg("Sökvägen lämnar den valda mappen."));
    }
    reject_links_between(&root, &file, false)?;
    Ok(file)
}

fn assert_regular_file(path: &Path, max_bytes: u64, playing: bool) -> AppResult<fs::Metadata> {
    let meta = fs::symlink_metadata(path).map_err(|_| {
        if playing {
            AppError::msg("Spåret saknar lokal fil.")
        } else {
            AppError::msg("Bara vanliga filer kan importeras.")
        }
    })?;
    if metadata_is_link(&meta) {
        return Err(symlink_err(playing));
    }
    if !meta.is_file() {
        return Err(AppError::msg("Bara vanliga filer kan importeras."));
    }
    if meta.len() > max_bytes {
        return Err(AppError::msg("Ljudfilen är större än tillåtet (2 GB)."));
    }
    confirm_open_nofollow(path, playing)?;
    Ok(meta)
}

fn symlink_err(playing: bool) -> AppError {
    if playing {
        AppError::msg("Symlänkar spelas inte.")
    } else {
        AppError::msg("Symlänkar importeras inte.")
    }
}

pub(crate) fn metadata_is_link(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

/// Refuse to follow a symlink (or, on Windows, any reparse point) between `root` and `file`.
fn reject_links_between(root: &Path, file: &Path, playing: bool) -> AppResult<()> {
    let root_meta = fs::symlink_metadata(root)?;
    if metadata_is_link(&root_meta) {
        return Err(symlink_err(playing));
    }
    let rel = file.strip_prefix(root).map_err(|_| {
        if playing {
            AppError::msg("Sökvägen lämnar den indexerade filen.")
        } else {
            AppError::msg("Sökvägen lämnar den valda mappen.")
        }
    })?;
    let mut acc = root.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::CurDir => continue,
            Component::ParentDir => {
                return Err(if playing {
                    AppError::msg("Sökvägen lämnar den indexerade filen.")
                } else {
                    AppError::msg("Sökvägen lämnar den valda mappen.")
                });
            }
            Component::Normal(name) => acc.push(name),
            Component::Prefix(_) | Component::RootDir => continue,
        }
        let meta = fs::symlink_metadata(&acc)?;
        if metadata_is_link(&meta) {
            return Err(if playing {
                symlink_err(true)
            } else {
                AppError::msg(format!("Symlänkar importeras inte ({}).", acc.display()))
            });
        }
    }
    Ok(())
}

fn confirm_open_nofollow(path: &Path, playing: bool) -> AppResult<()> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NOFOLLOW: i32 = 0x20000;
        match fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => {
                drop(file);
                Ok(())
            }
            Err(err) if err.raw_os_error() == Some(40) => {
                // Linux ELOOP: O_NOFOLLOW refused a symlink.
                Err(symlink_err(playing))
            }
            Err(err) => Err(err.into()),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (path, playing);
        Ok(())
    }
}

fn lexical_absolute(path: &Path) -> AppResult<PathBuf> {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(AppError::msg("Sökvägen lämnar den valda mappen."));
    }
    Ok(out)
}

/// Play `requested` only when it is the indexed music file, or a podcast file under media-root.
/// A symlink that resolves outside the indexed file is refused.
pub(crate) fn authorize_playback_path(
    conn: &Connection,
    media_root: &Path,
    requested: &Path,
) -> AppResult<PathBuf> {
    let lexical = lexical_absolute(requested)?;
    let key = normalize_path(&lexical);
    if let Some(track) = db::music_track_by_file_path(conn, &key)? {
        return require_indexed(conn, media_root, &track, &lexical);
    }
    let raw = normalize_path(requested);
    if raw != key {
        if let Some(track) = db::music_track_by_file_path(conn, &raw)? {
            let stored = lexical_absolute(Path::new(track.file_path.trim()))?;
            return require_indexed(conn, media_root, &track, &stored);
        }
    }
    Ok(crate::engine::ensure_under_root(media_root, requested)?)
}

pub(crate) fn require_playable_track(
    conn: &Connection,
    media_root: &Path,
    track: &MusicTrack,
) -> AppResult<PathBuf> {
    if track.file_path.trim().is_empty() {
        return Err(AppError::msg("Spåret saknar lokal fil."));
    }
    let lexical = lexical_absolute(Path::new(track.file_path.trim()))?;
    require_indexed(conn, media_root, track, &lexical)
}

/// True only when `requested` is the same lexical path as the indexed file.
/// Symlinks are not resolved, and this function does not open `requested`.
pub(crate) fn request_path_is_indexed_file(requested: &Path, indexed: &Path) -> bool {
    let Ok(requested) = lexical_absolute(requested) else {
        return false;
    };
    let Ok(indexed) = lexical_absolute(indexed) else {
        return false;
    };
    normalize_path(&requested) == normalize_path(&indexed)
}

fn require_indexed(
    conn: &Connection,
    media_root: &Path,
    track: &MusicTrack,
    lexical: &Path,
) -> AppResult<PathBuf> {
    let stored = track.file_path.trim().replace('\\', "/");
    if stored.is_empty() || normalize_path(lexical) != stored {
        return Err(AppError::msg("Sökvägen lämnar den indexerade filen."));
    }
    let folders = db::list_music_folder_paths(conn)?;
    let boundary = playback_boundary(lexical, &folders, media_root)?;
    reject_links_between(&boundary, lexical, true)?;
    assert_regular_file(lexical, MAX_MEDIA_BYTES, true)?;
    Ok(lexical.to_path_buf())
}

fn playback_boundary(file: &Path, folders: &[String], media_root: &Path) -> AppResult<PathBuf> {
    let mut best: Option<&str> = None;
    for folder in folders {
        let folder_path = Path::new(folder.trim());
        if file.starts_with(folder_path)
            && best
                .map(|prev| folder_path.components().count() > Path::new(prev).components().count())
                .unwrap_or(true)
        {
            best = Some(folder.trim());
        }
    }
    if let Some(folder) = best {
        return lexical_absolute(Path::new(folder));
    }
    let root = lexical_absolute(media_root)?;
    if file.starts_with(&root) {
        return Ok(root);
    }
    Err(AppError::msg("Sökvägen lämnar den indexerade filen."))
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
    if metadata_is_link(&meta) {
        return Err(AppError::msg("Symlänkar importeras inte."));
    }
    if !meta.is_dir() {
        return Err(AppError::msg("Sökvägen är inte en mapp."));
    }
    confirm_open_nofollow(dir, false)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let meta = fs::symlink_metadata(&path)?;
        if metadata_is_link(&meta) {
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
            let linked = contained_file(dir, &path)?;
            out.push(linked);
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
            if let Some((artist, rest)) = split_artist_title(&title) {
                if tags.artist.is_none() {
                    tags.artist = Some(artist);
                    tags.title = Some(rest);
                } else if tags.artist.as_deref() == Some(artist.as_str()) {
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
    v.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
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
    use crate::download;
    use rusqlite::Connection;

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if !dir.exists() {
            return out;
        }
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            let Ok(rd) = fs::read_dir(dir) else {
                return;
            };
            for entry in rd.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.push(path);
                }
            }
        }
        walk(dir, &mut out);
        out
    }

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
    fn import_links_file_in_place_and_does_not_copy() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp
            .path()
            .join("Downloads")
            .join("Miles")
            .join("Kind of Blue");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("01 So What.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 2, 440.0, 0.2, 0.2);
        let original = fs::read(&wav).unwrap();
        fs::write(folder.join("notes.txt"), "ignore").unwrap();

        let out = import_folder(&db, tmp.path().join("Downloads").as_path()).unwrap();
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
        assert_eq!(tracks[0].file_path, normalize_path(&wav));
        assert_eq!(tracks[0].source_path, tracks[0].file_path);
        assert_eq!(tracks[0].bytes, Some(original.len() as i64));
        assert_eq!(fs::read(&wav).unwrap(), original);
        assert!(files_under(&media.join("music")).is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let src = fs::metadata(&wav).unwrap();
            let linked = fs::metadata(&tracks[0].file_path).unwrap();
            assert_eq!(src.dev(), linked.dev());
            assert_eq!(src.ino(), linked.ino());
        }

        let shows: i64 = db
            .query_row("SELECT COUNT(*) FROM episode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(shows, 0);

        let again = import_folder(&db, tmp.path().join("Downloads").as_path()).unwrap();
        assert_eq!(again.imported_tracks, 0);
        assert_eq!(again.skipped_tracks, 1);
        assert_eq!(again.track_count, 1);
        assert!(files_under(&media.join("music")).is_empty());

        let playable =
            authorize_playback_path(&db, &media, Path::new(&tracks[0].file_path)).unwrap();
        assert_eq!(normalize_path(&playable), tracks[0].file_path);
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
    fn relinks_empty_file_path_without_copying() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("lib");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("tune.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 1, 440.0, 0.1, 0.2);
        let original = fs::read(&wav).unwrap();
        import_folder(&db, &folder).unwrap();
        let track = list_tracks(&db, None).unwrap().remove(0);
        db::set_music_track_file(&db, track.id, "", None).unwrap();
        let again = import_folder(&db, &folder).unwrap();
        assert_eq!(again.imported_tracks, 1);
        assert_eq!(again.skipped_tracks, 0);
        let repaired = list_tracks(&db, None).unwrap().remove(0);
        assert_eq!(repaired.id, track.id);
        assert_eq!(repaired.file_path, normalize_path(&wav));
        assert_eq!(fs::read(&wav).unwrap(), original);
        assert!(files_under(&media.join("music")).is_empty());
    }

    #[test]
    fn link_rejects_oversize_and_leaves_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("lib");
        fs::create_dir_all(&root).unwrap();
        let src = root.join("big.wav");
        fs::write(&src, vec![0u8; 64]).unwrap();
        let err = link_source(&root, &src, 8).unwrap_err();
        assert!(err.to_string().contains("större än tillåtet"));
        assert_eq!(fs::read(&src).unwrap(), vec![0u8; 64]);
        assert_eq!(files_under(&root).len(), 1);
    }

    #[test]
    fn import_rejects_file_over_media_limit_without_copying() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("lib");
        fs::create_dir_all(&folder).unwrap();
        let big = folder.join("huge.flac");
        let file = fs::File::create(&big).unwrap();
        file.set_len(MAX_MEDIA_BYTES + 1).unwrap();
        let err = import_folder(&db, &folder).unwrap_err();
        assert!(err.to_string().contains("större än tillåtet"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
        assert!(db::last_music_import_root(&db).unwrap().is_none());
        assert!(files_under(&media.join("music")).is_empty());
        assert_eq!(fs::metadata(&big).unwrap().len(), MAX_MEDIA_BYTES + 1);
    }

    #[test]
    fn import_rolls_back_when_a_later_path_leaves_the_folder() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("mix");
        fs::create_dir_all(&folder).unwrap();
        let inside = folder.join("a.wav");
        crate::engine::test_wav::write_sine_wav(&inside, 44100, 1, 440.0, 0.05, 0.2);
        let outside = tmp.path().join("outside.wav");
        crate::engine::test_wav::write_sine_wav(&outside, 44100, 1, 440.0, 0.05, 0.2);
        let err =
            import_indexed_files(&db, &folder, &[inside.clone(), outside.clone()]).unwrap_err();
        assert!(err.to_string().contains("Import misslyckades"));
        assert!(err.to_string().contains("lämnar den valda mappen"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
        assert_eq!(db::count_music_albums(&db).unwrap(), 0);
        assert!(db::last_music_import_root(&db).unwrap().is_none());
        assert!(files_under(&media.join("music")).is_empty());
        assert!(inside.is_file());
        assert!(outside.is_file());
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
        import_folder(&db, &good).unwrap();
        let remembered = db::last_music_import_root(&db).unwrap().unwrap();
        assert!(remembered.contains("good"));

        let bad = tmp.path().join("bad");
        fs::create_dir_all(&bad).unwrap();
        let huge = bad.join("huge.flac");
        let file = fs::File::create(&huge).unwrap();
        file.set_len(MAX_MEDIA_BYTES + 1).unwrap();
        let err = import_folder(&db, &bad).unwrap_err();
        assert!(err.to_string().contains("större än tillåtet"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 1);
        assert_eq!(
            db::last_music_import_root(&db).unwrap().as_deref(),
            Some(remembered.as_str())
        );
        assert!(files_under(&media.join("music")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn import_rejects_symlinked_audio_and_does_not_follow_a_link_out() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("src");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("real.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 1, 440.0, 0.1, 0.2);
        std::os::unix::fs::symlink(&wav, folder.join("alias.wav")).unwrap();
        let err = import_folder(&db, &folder).unwrap_err();
        assert!(err.to_string().contains("Symlänk"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
        assert!(files_under(&media.join("music")).is_empty());

        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.wav");
        crate::engine::test_wav::write_sine_wav(&secret, 44100, 1, 880.0, 0.1, 0.2);
        let chosen = tmp.path().join("chosen");
        fs::create_dir_all(&chosen).unwrap();
        std::os::unix::fs::symlink(&outside, chosen.join("linked")).unwrap();
        let err = import_folder(&db, &chosen).unwrap_err();
        assert!(err.to_string().contains("Symlänk"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
        let stored = db::list_music_tracks(&db, None).unwrap();
        assert!(stored.iter().all(|t| !t.file_path.contains("secret")));

        let real_only = tmp.path().join("real-only");
        fs::create_dir_all(&real_only).unwrap();
        let only = real_only.join("only.wav");
        crate::engine::test_wav::write_sine_wav(&only, 44100, 1, 440.0, 0.05, 0.2);
        let via = tmp.path().join("via-link");
        std::os::unix::fs::symlink(&real_only, &via).unwrap();
        let err = import_folder(&db, &via).unwrap_err();
        assert!(err.to_string().contains("Symlänk"));
        assert_eq!(db::count_music_tracks(&db).unwrap(), 0);
        assert!(files_under(&media.join("music")).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn play_rejects_symlink_that_escapes_the_indexed_file() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("lib");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("tune.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 1, 440.0, 0.1, 0.2);
        import_folder(&db, &folder).unwrap();
        let track = list_tracks(&db, None).unwrap().remove(0);
        let outside = tmp.path().join("outside.wav");
        crate::engine::test_wav::write_sine_wav(&outside, 44100, 1, 880.0, 0.1, 0.2);
        fs::remove_file(&wav).unwrap();
        std::os::unix::fs::symlink(&outside, &wav).unwrap();

        let err = authorize_playback_path(&db, &media, Path::new(&track.file_path)).unwrap_err();
        assert!(err.to_string().contains("Symlänk"));
        let escaped = authorize_playback_path(&db, &media, &outside).unwrap_err();
        assert!(
            escaped.to_string().contains("Otillåten")
                || escaped.to_string().contains("indexerade filen")
                || escaped.to_string().contains("hittades inte")
        );
        let dotted = folder
            .join("nope")
            .join("..")
            .join("..")
            .join("outside.wav");
        let dotted_err = authorize_playback_path(&db, &media, &dotted).unwrap_err();
        let dotted_msg = dotted_err.to_string();
        assert!(
            dotted_msg.contains("Otillåten")
                || dotted_msg.contains("indexerade filen")
                || dotted_msg.contains("hittades inte"),
            "{dotted_msg}"
        );
    }

    #[test]
    fn play_rejects_indexed_file_that_grew_past_the_size_limit() {
        let db = conn();
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        download::ensure_media_layout(&media).unwrap();
        let folder = tmp.path().join("lib");
        fs::create_dir_all(&folder).unwrap();
        let wav = folder.join("tune.wav");
        crate::engine::test_wav::write_sine_wav(&wav, 44100, 1, 440.0, 0.05, 0.2);
        import_folder(&db, &folder).unwrap();
        let track = list_tracks(&db, None).unwrap().remove(0);
        let file = fs::OpenOptions::new().write(true).open(&wav).unwrap();
        file.set_len(MAX_MEDIA_BYTES + 1).unwrap();
        let err = authorize_playback_path(&db, &media, Path::new(&track.file_path)).unwrap_err();
        assert!(err.to_string().contains("större än tillåtet"));
        assert!(files_under(&media.join("music")).is_empty());
    }
}
