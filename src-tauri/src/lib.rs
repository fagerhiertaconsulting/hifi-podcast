mod asr;
mod commands;
mod db;
mod download;
mod error;
mod feeds;
mod intel;
mod jobs;
mod models;
mod smtc;

use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::Manager;

pub struct AppState {
    pub db: Mutex<Connection>,
    pub db_path: PathBuf,
    pub media_root: PathBuf,
    pub data_dir: PathBuf,
    pub download_lock: tokio::sync::Mutex<()>,
    pub smtc: smtc::Handle,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_local_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            fs::create_dir_all(asr::models_dir(&data_dir))?;
            fs::create_dir_all(asr::bin_dir(&data_dir))?;
            let media_root = data_dir.join("media");
            fs::create_dir_all(&media_root)?;
            let db_path = data_dir.join("db.sqlite");
            let conn = Connection::open(&db_path)?;
            db::initialize(&conn)?;
            let smtc = smtc::init(app);
            app.manage(AppState {
                db: Mutex::new(conn),
                db_path,
                media_root,
                data_dir,
                download_lock: tokio::sync::Mutex::new(()),
                smtc,
            });
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                jobs::worker_loop(handle).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_paths,
            commands::list_podcasts,
            commands::list_episodes,
            commands::get_podcast,
            commands::update_podcast_settings,
            commands::add_feed,
            commands::refresh_feed,
            commands::import_opml,
            commands::import_opml_file,
            commands::export_opml,
            commands::export_opml_file,
            commands::download_episode,
            commands::get_download,
            commands::save_playback_state,
            commands::get_playback_state,
            commands::list_queue,
            commands::enqueue_episode,
            commands::dequeue_item,
            commands::move_queue_item,
            commands::episode_for_player,
            commands::read_media_file,
            commands::asr_status,
            commands::download_asr_model,
            commands::download_asr_cli,
            commands::download_asr_setup,
            commands::enqueue_transcribe,
            commands::list_jobs,
            commands::get_transcript,
            commands::list_segments,
            commands::search_transcripts,
            commands::list_chapters,
            commands::list_ads,
            commands::add_bookmark,
            commands::list_bookmarks,
            commands::delete_bookmark,
            commands::list_inbox,
            commands::triage_episode,
            commands::library_stats,
            commands::sideload_file,
            commands::smtc_now_playing,
        ])
        .run(tauri::generate_context!())
        .expect("kunde inte starta HIFI podcast");
}
