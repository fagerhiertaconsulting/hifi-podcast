//! Cut enabled ad spans to a new file with ffmpeg stream copy.
//! The original stays. Paths are argv entries, never a shell command.
//! The new file is only created inside the episode's show directory.

use crate::error::{AppError, AppResult};
use crate::intel::{self, AdDraft, RmsWindow, Segment};
use crate::models::CutAdsOutcome;
use crate::net::MAX_MEDIA_BYTES;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn silence_ads_in_file(path: &Path) -> AppResult<Vec<AdDraft>> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(AppError::msg(
            "Tystnadsanalys läser bara en vanlig fil. Symlänken lämnades orörd.",
        ));
    }
    if meta.len() > MAX_MEDIA_BYTES {
        return Err(AppError::msg(
            "Ljudfilen är större än tillåtet (2 GB). Ingen analys gjordes.",
        ));
    }
    let (raw, duration) = crate::engine::collect_rms_windows(path)?;
    let windows = raw
        .into_iter()
        .map(|(start_ms, end_ms, rms)| RmsWindow {
            start_ms,
            end_ms,
            rms,
        })
        .collect::<Vec<_>>();
    let gaps = intel::silence_gaps(&windows, intel::SILENCE_RMS);
    Ok(intel::detect_silence_ad_blocks(&gaps, duration))
}

pub fn combine_ads(segments: &[Segment], silence: &[AdDraft]) -> Vec<AdDraft> {
    let mut ads = intel::detect_ads(segments);
    ads.extend(silence.iter().cloned());
    intel::dedupe_ad_drafts(ads)
}

pub fn keep_spans(duration_ms: i64, cuts: &[(i64, i64)]) -> Vec<(i64, i64)> {
    if duration_ms <= 0 {
        return Vec::new();
    }
    let mut cuts: Vec<(i64, i64)> = cuts
        .iter()
        .map(|(a, b)| ((*a).max(0), (*b).min(duration_ms)))
        .filter(|(a, b)| *b > *a)
        .collect();
    cuts.sort_by_key(|(a, b)| (*a, *b));
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in cuts {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    let mut keep = Vec::new();
    let mut cursor = 0i64;
    for (start, end) in merged {
        if start > cursor {
            keep.push((cursor, start));
        }
        cursor = end.max(cursor);
    }
    if cursor < duration_ms {
        keep.push((cursor, duration_ms));
    }
    keep.into_iter().filter(|(a, b)| b - a >= 200).collect()
}

pub fn segment_copy_argv(
    start_ms: i64,
    end_ms: i64,
    input_name: &str,
    output_name: &str,
) -> Vec<String> {
    vec![
        "-nostdin".into(),
        "-y".into(),
        "-ss".into(),
        format_ms(start_ms),
        "-t".into(),
        format_ms((end_ms - start_ms).max(0)),
        "-i".into(),
        input_name.into(),
        "-c".into(),
        "copy".into(),
        "-avoid_negative_ts".into(),
        "make_zero".into(),
        output_name.into(),
    ]
}

pub fn concat_copy_argv(list_name: &str, output_name: &str) -> Vec<String> {
    vec![
        "-nostdin".into(),
        "-y".into(),
        "-f".into(),
        "concat".into(),
        "-safe".into(),
        "1".into(),
        "-i".into(),
        list_name.into(),
        "-c".into(),
        "copy".into(),
        output_name.into(),
    ]
}

fn format_ms(ms: i64) -> String {
    let ms = ms.max(0);
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// Destination for the cut. `requested` is refused when it leaves the show
/// directory. Nothing is created here.
pub fn resolve_cut_destination(
    show: &Path,
    episode_id: i64,
    ext: &str,
    requested: Option<&str>,
) -> AppResult<PathBuf> {
    if episode_id <= 0 {
        return Err(AppError::msg("Ogiltigt avsnitts-id."));
    }
    let ext = normalize_ext(ext)?;
    let show_meta = std::fs::symlink_metadata(show)
        .map_err(|_| AppError::msg("Poddens mediamapp saknas. Ingen ny fil skrevs."))?;
    if show_meta.file_type().is_symlink() || !show_meta.is_dir() {
        return Err(AppError::msg(
            "Poddens mediamapp är inte en vanlig katalog. Ingen ny fil skrevs.",
        ));
    }
    let show_canon = std::fs::canonicalize(show)?;
    if show_canon != show {
        return Err(AppError::msg(
            "Poddens mediamapp pekar utanför den egna katalogen. Ingen ny fil skrevs.",
        ));
    }
    let name = match requested.map(str::trim).filter(|s| !s.is_empty()) {
        None => default_cut_name(episode_id, &ext),
        Some(raw) => requested_file_name(raw, episode_id, &ext)?,
    };
    if !safe_simple_name(&name) {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    let dest = show_canon.join(&name);
    if dest.parent() != Some(show_canon.as_path()) || !dest.starts_with(&show_canon) {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    if dest.file_name().and_then(|n| n.to_str()) != Some(name.as_str()) {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    Ok(dest)
}

fn default_cut_name(episode_id: i64, ext: &str) -> String {
    format!("{episode_id}.utan-reklam.{ext}")
}

fn requested_file_name(raw: &str, episode_id: i64, ext: &str) -> AppResult<String> {
    if raw.contains('\0') || raw.contains('\n') || raw.contains('\r') {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    let path = Path::new(raw);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::CurDir
            )
        })
        || raw.contains('/')
        || raw.contains('\\')
        || raw.contains("..")
    {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    if path.components().count() != 1 {
        return Err(AppError::msg(
            "Klippfilen måste ligga i avsnittets mediamapp. Sökvägen lämnades orörd.",
        ));
    }
    let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        AppError::msg("Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.")
    })?;
    if !safe_simple_name(name) || !name.ends_with(&format!(".{ext}")) {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    if name == format!("{episode_id}.{ext}") {
        return Err(AppError::msg("Originalfilen skrivs inte över."));
    }
    Ok(name.to_string())
}

fn safe_simple_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && !name.contains("..")
        && name != "."
        && name != ".."
}

fn normalize_ext(ext: &str) -> AppResult<String> {
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if matches!(
        ext.as_str(),
        "mp3" | "m4a" | "m4b" | "mp4" | "aac" | "ogg" | "opus" | "flac" | "wav"
    ) {
        Ok(ext)
    } else {
        Err(AppError::msg("Filtypen kan inte klippas med strömkopia."))
    }
}

struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `dest_override` is only for the escape test. The command passes `None`.
pub fn cut_marked_ads(
    media_root: &Path,
    data_dir: &Path,
    podcast_id: i64,
    episode_id: i64,
    duration_ms: i64,
    cuts: &[(i64, i64)],
    dest_override: Option<&str>,
) -> AppResult<CutAdsOutcome> {
    if cuts.is_empty() {
        return Err(AppError::msg(
            "Inga markerade reklaminslag att klippa bort.",
        ));
    }
    let show = crate::download::ready_show_dir(media_root, podcast_id)?;
    let original = crate::download::episode_original_in_show(&show, episode_id)?;
    let ext = original
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_string();
    let dest = resolve_cut_destination(&show, episode_id, &ext, dest_override)?;
    if dest == original {
        return Err(AppError::msg("Originalfilen skrivs inte över."));
    }
    let source_meta = regular_media(&original)?;
    let source_len = source_meta.len();
    let duration = duration_for(&original, duration_ms)?;
    let keep = keep_spans(duration, cuts);
    if keep.is_empty() {
        return Err(AppError::msg("Inget ljud kvar efter klippet."));
    }
    let removed_ms: i64 = cuts
        .iter()
        .map(|(a, b)| ((*b).min(duration) - (*a).max(0)).max(0))
        .sum();
    let bin = crate::ffmpeg_bin::find_ffmpeg(data_dir).ok_or_else(|| {
        AppError::msg("ffmpeg saknas. Ladda ner ffmpeg eller lägg den i PATH. Ingen fil skrevs.")
    })?;
    let input_name = file_name_str(&original)?;
    let mut temps = Cleanup(Vec::new());
    let mut part_names = Vec::new();
    for (index, (start, end)) in keep.iter().enumerate() {
        let part_name = format!("{episode_id}.klipp.{index}.{ext}");
        let part = resolve_cut_destination(&show, episode_id, &ext, Some(&part_name))?;
        temps.0.push(part.clone());
        let args = segment_copy_argv(*start, *end, &input_name, &part_name);
        run_ffmpeg(&bin, &args, &show)?;
        let _ = regular_media(&part)?;
        part_names.push(part_name);
    }
    let list_name = format!("{episode_id}.klipp.txt");
    let list_path = show.join(&list_name);
    if list_path.parent() != Some(show.as_path()) || !safe_simple_name(&list_name) {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    temps.0.push(list_path.clone());
    {
        let mut list = std::fs::File::create(&list_path)?;
        for name in &part_names {
            if !safe_simple_name(name) {
                return Err(AppError::msg(
                    "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
                ));
            }
            writeln!(list, "file '{name}'")?;
        }
    }
    let output_name = file_name_str(&dest)?;
    if dest.exists() {
        let existing = std::fs::symlink_metadata(&dest)?;
        if existing.file_type().is_symlink() || !existing.is_file() {
            return Err(AppError::msg(
                "Målfilen är inte en vanlig fil och lämnades orörd.",
            ));
        }
    }
    temps.0.push(dest.clone());
    let args = concat_copy_argv(&list_name, &output_name);
    run_ffmpeg(&bin, &args, &show)?;
    let out_meta = regular_media(&dest)?;
    if out_meta.len() == 0 || out_meta.len() > MAX_MEDIA_BYTES {
        let _ = std::fs::remove_file(&dest);
        return Err(AppError::msg(
            "Klippet blev tomt eller större än tillåtet och togs bort.",
        ));
    }
    let again = regular_media(&original)?;
    if again.len() != source_len || again.file_type().is_symlink() {
        let _ = std::fs::remove_file(&dest);
        return Err(AppError::msg(
            "Originalfilen ändrades inte som väntat. Klippet togs bort.",
        ));
    }
    temps.0.retain(|p| p != &dest);
    let bytes = out_meta.len() as i64;
    Ok(CutAdsOutcome {
        original_path: original.display().to_string(),
        cut_path: dest.display().to_string(),
        removed_ms,
        bytes,
    })
}

fn duration_for(original: &Path, hint: i64) -> AppResult<i64> {
    if let Ok(ms) = crate::engine::file_duration_ms(original) {
        if ms > 0 {
            return Ok(ms);
        }
    }
    if hint > 0 {
        return Ok(hint);
    }
    Err(AppError::msg(
        "Avsnittets längd saknas. Ingen ny fil skrevs.",
    ))
}

fn regular_media(path: &Path) -> AppResult<std::fs::Metadata> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(AppError::msg(
            "Filen är inte en vanlig fil. Symlänken lämnades orörd.",
        ));
    }
    if meta.len() > MAX_MEDIA_BYTES {
        return Err(AppError::msg(
            "Ljudfilen är större än tillåtet (2 GB). Ingen ny fil skrevs.",
        ));
    }
    Ok(meta)
}

fn file_name_str(path: &Path) -> AppResult<String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| AppError::msg("Ogiltigt filnamn."))?;
    if !safe_simple_name(name) {
        return Err(AppError::msg(
            "Klippfilen hamnar utanför avsnittets mediamapp. Ingen ny fil skrevs.",
        ));
    }
    Ok(name.to_string())
}

fn run_ffmpeg(bin: &Path, args: &[String], cwd: &Path) -> AppResult<()> {
    let argv: Vec<OsString> = args.iter().map(OsString::from).collect();
    let mut cmd = Command::new(bin);
    cmd.args(&argv);
    cmd.current_dir(cwd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let output = cmd.output()?;
    if output.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    let line = err
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("okänt fel");
    Err(AppError::msg(format!(
        "ffmpeg kunde inte klippa filen: {line}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_argv_is_stream_copy_without_a_shell_or_title() {
        let title = "Farlig titel; rm -rf /";
        let args = segment_copy_argv(0, 1500, "8.mp3", "8.klipp.0.mp3");
        assert!(args.windows(2).any(|w| w == ["-c", "copy"]));
        assert!(!args.iter().any(|a| a.contains("filter")));
        assert!(!args
            .iter()
            .any(|a| a == "sh" || a == "cmd" || a == "/bin/sh"));
        assert!(!args.iter().any(|a| a.contains(title) || a.contains(';')));
        assert_eq!(args.iter().filter(|a| *a == "-i").count(), 1);
        let input_at = args.iter().position(|a| a == "-i").unwrap();
        assert_eq!(args[input_at + 1], "8.mp3");
        let concat = concat_copy_argv("8.klipp.txt", "8.utan-reklam.mp3");
        assert!(concat.windows(2).any(|w| w == ["-c", "copy"]));
        assert!(concat.windows(2).any(|w| w == ["-safe", "1"]));
        assert!(!concat.iter().any(|a| a.contains(title)));
    }

    #[test]
    fn refuses_cut_path_that_escapes_episode_directory() {
        let dir = tempfile::tempdir().unwrap();
        let show = dir.path().join("podcasts").join("4");
        std::fs::create_dir_all(&show).unwrap();
        let outside = dir.path().join("escape.mp3");
        let escaped = show.join("..").join("..").join("escape.mp3");
        let err = resolve_cut_destination(&show, 8, "mp3", Some(&escaped.display().to_string()))
            .unwrap_err();
        assert!(err.to_string().contains("utanför"), "{err}");
        assert!(!outside.exists());
        let err =
            resolve_cut_destination(&show, 8, "mp3", Some(outside.to_str().unwrap())).unwrap_err();
        assert!(err.to_string().contains("utanför"), "{err}");
        assert!(!outside.exists());
        let err = resolve_cut_destination(&show, 8, "mp3", Some("..\\escape.mp3")).unwrap_err();
        assert!(!outside.exists());
        assert!(err.to_string().contains("utanför"));

        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let podcasts = media.join("podcasts");
        let real_show = podcasts.join("4");
        std::fs::create_dir_all(&real_show).unwrap();
        std::fs::write(real_show.join("8.mp3"), b"original").unwrap();
        let outside_keep = dir.path().join("keep.mp3");
        std::fs::write(&outside_keep, b"keep").unwrap();
        let err = cut_marked_ads(
            &media,
            dir.path(),
            4,
            8,
            1_000,
            &[(0, 100)],
            Some("../escape.mp3"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("utanför"), "{err}");
        assert!(!outside.exists());
        assert_eq!(std::fs::read(real_show.join("8.mp3")).unwrap(), b"original");
        assert_eq!(std::fs::read(&outside_keep).unwrap(), b"keep");
        assert!(!real_show.join("escape.mp3").exists());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_cut_when_show_dir_is_a_symlink() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let target = dir.path().join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        let secret = target.join("secret.mp3");
        std::fs::write(&secret, b"secret").unwrap();
        symlink(&target, media.join("podcasts").join("9")).unwrap();
        let err = cut_marked_ads(&media, dir.path(), 9, 3, 1_000, &[(0, 100)], None).unwrap_err();
        assert!(err.to_string().contains("symlänk"), "{err}");
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert!(target.is_dir());
    }

    #[test]
    fn stream_copy_writes_a_new_file_and_leaves_the_original() {
        let bin_parent = tempfile::tempdir().unwrap();
        if crate::ffmpeg_bin::find_ffmpeg(bin_parent.path()).is_none() {
            eprintln!("ffmpeg saknas i den här miljön");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let show = media.join("podcasts").join("4");
        std::fs::create_dir_all(&show).unwrap();
        let original = show.join("8.wav");
        crate::engine::test_wav::write_sine_wav(&original, 8_000, 1, 440.0, 3.0, 0.4);
        let before = std::fs::read(&original).unwrap();
        let outcome = cut_marked_ads(
            &media,
            bin_parent.path(),
            4,
            8,
            3_000,
            &[(1_000, 2_000)],
            None,
        )
        .unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), before);
        let cut = show.join("8.utan-reklam.wav");
        assert_eq!(PathBuf::from(&outcome.cut_path), cut);
        assert!(cut.is_file());
        assert!(outcome.bytes > 0);
        assert!(outcome.removed_ms >= 1_000);
        let duration = crate::engine::file_duration_ms(&cut).unwrap();
        assert!(
            (1_500..2_600).contains(&duration),
            "cut duration {duration}"
        );
        assert!(!show.join("8.klipp.0.wav").exists());
        assert!(!show.join("8.klipp.txt").exists());
    }
}
