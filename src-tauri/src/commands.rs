use crate::db;
use crate::download;
use crate::error::AppResult;
use crate::feeds;
use crate::models::{
    AppPaths, Download, Episode, PlaybackState, Podcast, PodcastSettings, QueueItem,
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
    Ok(PlayerPayload {
        episode,
        podcast,
        download,
        playback,
    })
}

#[derive(serde::Serialize)]
pub struct PlayerPayload {
    pub episode: Episode,
    pub podcast: Podcast,
    pub download: Option<Download>,
    pub playback: Option<PlaybackState>,
}
