mod asr;
mod commands;
mod db;
mod download;
mod engine;
mod error;
mod feeds;
mod intel;
mod jobs;
mod media_mime;
mod models;
mod music;
mod net;
mod smtc;
mod upnp;

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
    pub upnp: upnp::Handle,
    pub engine: engine::Handle,
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
            let _ = download::ensure_media_layout(&media_root);
            let db_path = data_dir.join("db.sqlite");
            let conn = Connection::open(&db_path)?;
            db::initialize(&conn)?;
            let _ = db::rewrite_legacy_download_paths(&conn, &media_root);
            let _ = db::backfill_download_mime(&conn);
            let _ = db::requeue_running_jobs(&conn);
            let _ = db::purge_finished_episode_files(&conn, &media_root, chrono::Utc::now());
            let smtc = smtc::init(app);
            let engine = engine::Handle::new();
            app.manage(AppState {
                db: Mutex::new(conn),
                db_path,
                media_root,
                data_dir,
                download_lock: tokio::sync::Mutex::new(()),
                smtc,
                upnp: upnp::Handle::new(),
                engine: engine.clone(),
            });
            engine.bind_app(app.handle().clone());
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(hwnd) = window.hwnd() {
                    engine.set_hwnd(hwnd.0 as usize);
                }
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                jobs::worker_loop(handle).await;
            });
            let bind_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                for ms in [50_u64, 150, 400, 1000, 2500, 5000] {
                    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                    let app_ref = bind_app.clone();
                    let _ = bind_app.run_on_main_thread(move || {
                        if let Some(state) = app_ref.try_state::<AppState>() {
                            let _ = smtc::try_bind(&app_ref, &state.smtc);
                        }
                    });
                }
            });
            Ok(())
        })
        .on_page_load(|window, _payload| {
            let app = window.app_handle().clone();
            if let Some(state) = app.try_state::<AppState>() {
                let _ = smtc::try_bind(&app, &state.smtc);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_paths,
            commands::list_podcasts,
            commands::list_episodes,
            commands::get_podcast,
            commands::update_podcast_settings,
            commands::add_feed,
            commands::refresh_feed,
            commands::unsubscribe_podcast,
            commands::delete_episode_file,
            commands::get_finished_cleanup_settings,
            commands::save_finished_cleanup_settings,
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
            commands::upnp_status,
            commands::get_upnp_settings,
            commands::save_upnp_settings,
            commands::start_upnp_server,
            commands::stop_upnp_server,
            commands::engine_capabilities,
            commands::engine_list_devices,
            commands::engine_get_settings,
            commands::engine_save_settings,
            commands::engine_probe,
            commands::engine_open,
            commands::engine_play,
            commands::engine_pause,
            commands::engine_seek,
            commands::engine_set_speed,
            commands::engine_set_skip_silence,
            commands::engine_stop,
            commands::engine_status,
            commands::list_music_albums,
            commands::list_music_tracks,
            commands::last_music_import_root,
            commands::import_music_folder,
            commands::music_track_for_player,
            commands::save_music_playback,
            commands::next_music_track,
        ])
        .run(tauri::generate_context!())
        .expect("kunde inte starta HIFI podcast");
}
