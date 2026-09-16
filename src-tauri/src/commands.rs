use crate::asr;
use crate::db;
use crate::download;
use crate::error::AppResult;
use crate::feeds;
use crate::models::{
    AdSegment, AppPaths, AsrStatus, Bookmark, Chapter, Download, Episode, InboxItem, JobRow,
    LibraryStats, PlaybackState, Podcast, PodcastSettings, QueueItem, Transcript, TranscriptSegment,
};
use crate::AppState;
use tauri::State;

fn conn(state: &AppState) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
    state.db.lock().expect("databas-mutex")
}

#[tauri::command]
pub fn app_paths(state: State<AppState>) -> AppPaths {
    AppPaths {
        db_path: state.db_path.display().to_string(),
        media_root: state.media_root.display().to_string(),
        data_dir: state.data_dir.display().to_string(),
        models_dir: crate::asr::models_dir(&state.data_dir).display().to_string(),
        bin_dir: crate::asr::bin_dir(&state.data_dir).display().to_string(),
    }
}

#[tauri::command]
pub fn list_podcasts(state: State<AppState>) -> AppResult<Vec<Podcast>> {
    db::list_podcasts(&conn(&state))
}

#[tauri::command]
pub fn list_episodes(state: State<AppState>, podcast_id: i64) -> AppResult<Vec<Episode>> {
    db::list_episodes(&conn(&state), podcast_id)
}

#[tauri::command]
pub fn get_podcast(state: State<AppState>, podcast_id: i64) -> AppResult<Podcast> {
    db::get_podcast(&conn(&state), podcast_id)
}

#[tauri::command]
pub fn update_podcast_settings(
    state: State<AppState>,
    podcast_id: i64,
    settings: PodcastSettings,
) -> AppResult<Podcast> {
    let conn = conn(&state);
    db::update_podcast_settings(&conn, podcast_id, &settings)?;
    db::get_podcast(&conn, podcast_id)
}

async fn subscribe_url(state: &AppState, url: &str) -> AppResult<Podcast> {
    let xml = feeds::fetch_text(url).await?;
    let parsed = feeds::parse_feed(&xml, url)?;
    let (podcast, first_episode) = {
        let conn = conn(state);
        let podcast_id = db::upsert_podcast(
            &conn,
            url,
            &parsed.title,
            parsed.author.as_deref(),
            parsed.description.as_deref(),
            parsed.guid.as_deref(),
            parsed.language.as_deref(),
        )?;
        let mut first_episode = None;
        for episode in parsed.episodes {
            let eid = db::upsert_episode(
                &conn,
                podcast_id,
                &episode.guid,
                &episode.title,
                episode.description.as_deref(),
                episode.audio_url.as_deref(),
                episode.publish_at.as_deref(),
                episode.duration_ms,
            )?;
            if first_episode.is_none() {
                first_episode = Some(eid);
            }
        }
        (db::get_podcast(&conn, podcast_id)?, first_episode)
    };
    if podcast.auto_download {
        if let Some(eid) = first_episode {
            let _ = download_episode_inner(state, eid).await;
        }
    }
    Ok(podcast)
}

#[tauri::command]
pub async fn add_feed(state: State<'_, AppState>, url: String) -> AppResult<Podcast> {
    subscribe_url(&state, &url).await
}

#[tauri::command]
pub async fn refresh_feed(state: State<'_, AppState>, podcast_id: i64) -> AppResult<Podcast> {
    let feed_url = db::get_podcast(&conn(&state), podcast_id)?.feed_url;
    subscribe_url(&state, &feed_url).await
}

#[tauri::command]
pub async fn import_opml(state: State<'_, AppState>, xml: String) -> AppResult<Vec<Podcast>> {
    import_urls(&state, feeds::import_opml_urls(&xml)?).await
}

#[tauri::command]
pub async fn import_opml_file(state: State<'_, AppState>, path: String) -> AppResult<Vec<Podcast>> {
    let xml = tokio::fs::read_to_string(path).await?;
    import_urls(&state, feeds::import_opml_urls(&xml)?).await
}

async fn import_urls(state: &AppState, urls: Vec<String>) -> AppResult<Vec<Podcast>> {
    let mut imported = Vec::new();
    for url in urls {
        match subscribe_url(state, &url).await {
            Ok(podcast) => imported.push(podcast),
            Err(err) => eprintln!("OPML-prenumeration misslyckades för {url}: {err}"),
        }
    }
    Ok(imported)
}

#[tauri::command]
pub fn export_opml(state: State<AppState>) -> AppResult<String> {
    let podcasts = db::list_podcasts(&conn(&state))?;
    let feeds: Vec<(String, String)> = podcasts
        .into_iter()
        .map(|p| (p.title, p.feed_url))
        .collect();
    feeds::export_opml(&feeds)
}

#[tauri::command]
pub fn export_opml_file(state: State<AppState>, path: String) -> AppResult<()> {
    let podcasts = db::list_podcasts(&conn(&state))?;
    let feeds: Vec<(String, String)> = podcasts
        .into_iter()
        .map(|p| (p.title, p.feed_url))
        .collect();
    std::fs::write(path, feeds::export_opml(&feeds)?)?;
    Ok(())
}

#[tauri::command]
pub async fn download_episode(state: State<'_, AppState>, episode_id: i64) -> AppResult<Download> {
    download_episode_inner(&state, episode_id).await
}

async fn download_episode_inner(state: &AppState, episode_id: i64) -> AppResult<Download> {
    let _guard = state.download_lock.lock().await;
    let (podcast_id, audio_url) = {
        let conn = conn(state);
        let episode = db::get_episode(&conn, episode_id)?;
        let url = episode
            .audio_url
            .clone()
            .ok_or_else(|| crate::error::AppError::msg("Avsnittet saknar ljudfil i feeden."))?;
        db::upsert_download_state(
            &conn,
            episode_id,
            "downloading",
            None,
            None,
            None,
            None,
            None,
        )?;
        (episode.podcast_id, url)
    };

    let dest = download::media_file_path(&state.media_root, podcast_id, episode_id, &audio_url);
    let result = download::download_original(&audio_url, &dest).await;

    let conn = conn(state);
    match result {
        Ok((bytes, checksum, _content_type)) => {
            let completed = db::now_rfc3339();
            db::upsert_download_state(
                &conn,
                episode_id,
                "done",
                Some(&dest.display().to_string()),
                Some(bytes as i64),
                Some(&checksum),
                None,
                Some(&completed),
            )?;
            db::set_episode_status(&conn, episode_id, "downloaded")?;
            if db::podcast_wants_transcribe(&conn, podcast_id).unwrap_or(false) {
                let _ = db::enqueue_job(&conn, "transcribe", episode_id, 10);
            }
        }
        Err(err) => {
            db::upsert_download_state(
                &conn,
                episode_id,
                "error",
                None,
                None,
                None,
                Some(&err.to_string()),
                None,
            )?;
            return Err(err);
        }
    }
    db::get_download(&conn, episode_id)?
        .ok_or_else(|| crate::error::AppError::msg("Nedladdningen saknas efter skrivning."))
}

#[tauri::command]
pub fn get_download(state: State<AppState>, episode_id: i64) -> AppResult<Option<Download>> {
    db::get_download(&conn(&state), episode_id)
}

#[tauri::command]
pub fn save_playback_state(state: State<AppState>, playback: PlaybackState) -> AppResult<()> {
    db::save_playback(&conn(&state), &playback)
}

#[tauri::command]
pub fn get_playback_state(
    state: State<AppState>,
    episode_id: i64,
) -> AppResult<Option<PlaybackState>> {
    db::get_playback(&conn(&state), episode_id)
}

#[tauri::command]
pub fn list_queue(state: State<AppState>) -> AppResult<Vec<QueueItem>> {
    db::list_queue(&conn(&state))
}

#[tauri::command]
pub fn enqueue_episode(state: State<AppState>, episode_id: i64) -> AppResult<Vec<QueueItem>> {
    let conn = conn(&state);
    db::add_to_queue(&conn, episode_id)?;
    db::list_queue(&conn)
}

#[tauri::command]
pub fn dequeue_item(state: State<AppState>, queue_id: i64) -> AppResult<Vec<QueueItem>> {
    let conn = conn(&state);
    db::remove_from_queue(&conn, queue_id)?;
    db::list_queue(&conn)
}

#[tauri::command]
pub fn move_queue_item(
    state: State<AppState>,
    queue_id: i64,
    direction: i64,
) -> AppResult<Vec<QueueItem>> {
    let conn = conn(&state);
    db::move_queue_item(&conn, queue_id, direction)?;
    db::list_queue(&conn)
}

#[tauri::command]
pub fn episode_for_player(
    state: State<AppState>,
    episode_id: i64,
) -> AppResult<PlayerPayload> {
    let conn = conn(&state);
    let episode = db::get_episode(&conn, episode_id)?;
    let podcast = db::get_podcast(&conn, episode.podcast_id)?;
    let download = db::get_download(&conn, episode_id)?;
    let playback = db::get_playback(&conn, episode_id)?;
    let chapters = db::list_chapters(&conn, episode_id).unwrap_or_default();
    let ads = db::list_ads(&conn, episode_id).unwrap_or_default();
    Ok(PlayerPayload {
        episode,
        podcast,
        download,
        playback,
        chapters,
        ads,
    })
}

#[derive(serde::Serialize)]
pub struct PlayerPayload {
    pub episode: Episode,
    pub podcast: Podcast,
    pub download: Option<Download>,
    pub playback: Option<PlaybackState>,
    pub chapters: Vec<Chapter>,
    pub ads: Vec<AdSegment>,
}

/// Reads a downloaded original under media-root. Used only for opt-in skip-silence
/// so playback can use a same-origin blob URL (Web Audio + asset.localhost CORS).
#[tauri::command]
pub fn read_media_file(state: State<AppState>, path: String) -> AppResult<Vec<u8>> {
    read_media_file_from(state.media_root.as_path(), std::path::Path::new(&path))
}

pub(crate) fn read_media_file_from(
    media_root: &std::path::Path,
    path: &std::path::Path,
) -> AppResult<Vec<u8>> {
    let root = std::fs::canonicalize(media_root)?;
    let file = std::fs::canonicalize(path).map_err(|_| {
        crate::error::AppError::msg("Ljudfilen hittades inte under media-root.")
    })?;
    if !file.starts_with(&root) {
        return Err(crate::error::AppError::msg("Otillåten sökväg."));
    }
    Ok(std::fs::read(file)?)
}

#[tauri::command]
pub fn asr_status(state: State<AppState>) -> AsrStatus {
    asr::status(&state.data_dir)
}

#[tauri::command]
pub async fn download_asr_model(state: State<'_, AppState>) -> AppResult<AsrStatus> {
    asr::download_model(&state.data_dir).await?;
    Ok(asr::status(&state.data_dir))
}

#[tauri::command]
pub async fn download_asr_cli(state: State<'_, AppState>) -> AppResult<AsrStatus> {
    asr::download_cli(&state.data_dir).await?;
    Ok(asr::status(&state.data_dir))
}

#[tauri::command]
pub async fn download_asr_setup(state: State<'_, AppState>) -> AppResult<AsrStatus> {
    asr::download_cli(&state.data_dir).await?;
    asr::download_model(&state.data_dir).await?;
    Ok(asr::status(&state.data_dir))
}

#[tauri::command]
pub fn enqueue_transcribe(state: State<AppState>, episode_id: i64) -> AppResult<Vec<JobRow>> {
    let conn = conn(&state);
    db::enqueue_job(&conn, "transcribe", episode_id, 10)?;
    db::list_jobs(&conn, 30)
}

#[tauri::command]
pub fn list_jobs(state: State<AppState>) -> AppResult<Vec<JobRow>> {
    db::list_jobs(&conn(&state), 40)
}

#[tauri::command]
pub fn get_transcript(state: State<AppState>, episode_id: i64) -> AppResult<Option<Transcript>> {
    db::get_transcript(&conn(&state), episode_id)
}

#[tauri::command]
pub fn list_segments(state: State<AppState>, episode_id: i64) -> AppResult<Vec<TranscriptSegment>> {
    db::list_segments(&conn(&state), episode_id)
}

#[tauri::command]
pub fn search_transcripts(state: State<AppState>, query: String) -> AppResult<Vec<TranscriptSegment>> {
    db::search_segments(&conn(&state), &query)
}

#[tauri::command]
pub fn list_chapters(state: State<AppState>, episode_id: i64) -> AppResult<Vec<Chapter>> {
    db::list_chapters(&conn(&state), episode_id)
}

#[tauri::command]
pub fn list_ads(state: State<AppState>, episode_id: i64) -> AppResult<Vec<AdSegment>> {
    db::list_ads(&conn(&state), episode_id)
}

#[tauri::command]
pub fn add_bookmark(
    state: State<AppState>,
    episode_id: i64,
    position_ms: i64,
    note: String,
) -> AppResult<Vec<Bookmark>> {
    let conn = conn(&state);
    db::add_bookmark(&conn, episode_id, position_ms, &note)?;
    db::list_bookmarks(&conn)
}

#[tauri::command]
pub fn list_bookmarks(state: State<AppState>) -> AppResult<Vec<Bookmark>> {
    db::list_bookmarks(&conn(&state))
}

#[tauri::command]
pub fn delete_bookmark(state: State<AppState>, id: i64) -> AppResult<Vec<Bookmark>> {
    let conn = conn(&state);
    db::delete_bookmark(&conn, id)?;
    db::list_bookmarks(&conn)
}

#[tauri::command]
pub fn list_inbox(state: State<AppState>) -> AppResult<Vec<InboxItem>> {
    db::list_inbox(&conn(&state))
}

#[tauri::command]
pub fn triage_episode(
    state: State<AppState>,
    episode_id: i64,
    inbox: bool,
    archived: bool,
) -> AppResult<()> {
    db::set_inbox(&conn(&state), episode_id, inbox, archived)
}

#[tauri::command]
pub fn library_stats(state: State<AppState>) -> AppResult<LibraryStats> {
    db::library_stats(&conn(&state))
}

#[tauri::command]
pub fn sideload_file(state: State<AppState>, path: String) -> AppResult<Episode> {
    let src = std::path::PathBuf::from(&path);
    if !src.is_file() {
        return Err(crate::error::AppError::msg("Filen finns inte."));
    }
    let episode_id = {
        let dbconn = conn(&state);
        let podcast_id = db::ensure_sideload_podcast(&dbconn)?;
        let title = src
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Importerat avsnitt")
            .to_string();
        let guid = format!("sideload:{}", path.replace('\\', "/"));
        let id = db::upsert_episode(
            &dbconn,
            podcast_id,
            &guid,
            &title,
            None,
            Some(&format!("file://{}", path)),
            None,
            None,
        )?;
        (podcast_id, id)
    };
    let (podcast_id, episode_id) = episode_id;
    let ext = src
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("bin")
        .to_ascii_lowercase();
    let dest = state
        .media_root
        .join(podcast_id.to_string())
        .join(format!("{episode_id}.{ext}"));
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&src, &dest)?;
    let meta = std::fs::metadata(&dest)?;
    let conn = conn(&state);
    let completed = db::now_rfc3339();
    db::upsert_download_state(
        &conn,
        episode_id,
        "done",
        Some(&dest.display().to_string()),
        Some(meta.len() as i64),
        None,
        None,
        Some(&completed),
    )?;
    db::set_episode_status(&conn, episode_id, "downloaded")?;
    if db::podcast_wants_transcribe(&conn, podcast_id).unwrap_or(false) {
        let _ = db::enqueue_job(&conn, "transcribe", episode_id, 10);
    }
    db::get_episode(&conn, episode_id)
}

#[tauri::command]
pub fn smtc_now_playing(
    state: State<AppState>,
    title: String,
    artist: String,
    playing: bool,
) {
    crate::smtc::set_playing(&state.smtc, &title, &artist, playing);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    #[test]
    fn rejects_path_outside_media_root() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        fs::create_dir_all(&media).unwrap();
        let outside = dir.path().join("secret.bin");
        fs::write(&outside, b"nope").unwrap();
        let err = read_media_file_from(&media, &outside).unwrap_err();
        assert!(err.to_string().contains("Otillåten"));
    }

    #[test]
    fn reads_file_inside_media_root() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        fs::create_dir_all(media.join("1")).unwrap();
        let file = media.join("1").join("1.mp3");
        let mut f = fs::File::create(&file).unwrap();
        f.write_all(b"ID3fake").unwrap();
        drop(f);
        let bytes = read_media_file_from(&media, &file).unwrap();
        assert_eq!(bytes, b"ID3fake");
    }
}
