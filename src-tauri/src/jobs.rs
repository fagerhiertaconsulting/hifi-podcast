use crate::asr;
use crate::db;
use crate::error::AppResult;
use crate::intel;
use crate::AppState;
use std::time::Duration;
use tauri::{AppHandle, Manager};

pub async fn worker_loop(app: AppHandle) {
    loop {
        let claimed = {
            let state = app.state::<AppState>();
            let conn = state.db.lock().expect("db");
            db::claim_next_job(&conn).ok().flatten()
        };
        if let Some((job_id, job_type, episode_id)) = claimed {
            let result = run_job(&app, &job_type, episode_id).await;
            let state = app.state::<AppState>();
            let conn = state.db.lock().expect("db");
            match result {
                Ok(()) => {
                    let _ = db::finish_job(&conn, job_id, true, None);
                }
                Err(err) => {
                    let _ = db::finish_job(&conn, job_id, false, Some(&err.to_string()));
                }
            }
        } else {
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
    }
}

async fn run_job(app: &AppHandle, job_type: &str, episode_id: i64) -> AppResult<()> {
    match job_type {
        "transcribe" => transcribe(app, episode_id).await,
        "generate_chapters" => chapters(app, episode_id).await,
        "detect_ads" => ads(app, episode_id).await,
        other => Err(crate::error::AppError::msg(format!("okänd jobbtyp: {other}"))),
    }
}

async fn transcribe(app: &AppHandle, episode_id: i64) -> AppResult<()> {
    let (data_dir, audio, language) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().expect("db");
        let episode = db::get_episode(&conn, episode_id)?;
        let download = db::get_download(&conn, episode_id)?
            .and_then(|d| d.file_path)
            .ok_or_else(|| crate::error::AppError::msg("Avsnittet är inte nedladdat."))?;
        let language = db::podcast_asr_language(&conn, episode.podcast_id)?;
        (state.data_dir.clone(), std::path::PathBuf::from(download), language)
    };
    let (segments, lang) = asr::transcribe_file(&data_dir, &audio, &language).await?;
    {
        let state = app.state::<AppState>();
        let conn = state.db.lock().expect("db");
        db::replace_transcript(
            &conn,
            episode_id,
            "local_whisper",
            Some(asr::MODEL_NAME),
            Some(&lang),
            &segments,
        )?;
        let _ = db::enqueue_job(&conn, "generate_chapters", episode_id, 5);
        let _ = db::enqueue_job(&conn, "detect_ads", episode_id, 4);
    }
    Ok(())
}

async fn chapters(app: &AppHandle, episode_id: i64) -> AppResult<()> {
    let (audio, segments) = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().expect("db");
        let download = db::get_download(&conn, episode_id)?.and_then(|d| d.file_path);
        let segs = db::list_segments(&conn, episode_id)?;
        (
            download.map(std::path::PathBuf::from),
            segs.into_iter()
                .map(|s| intel::Segment {
                    start_ms: s.start_ms,
                    end_ms: s.end_ms,
                    text: s.text,
                })
                .collect::<Vec<_>>(),
        )
    };
    let mut drafts = Vec::new();
    if let Some(path) = audio.as_ref() {
        drafts.extend(intel::read_id3_chapters(path));
    }
    if drafts.is_empty() {
        drafts = intel::generate_chapters(&segments);
    }
    let state = app.state::<AppState>();
    let conn = state.db.lock().expect("db");
    db::replace_generated_chapters(&conn, episode_id, &drafts)?;
    Ok(())
}

async fn ads(app: &AppHandle, episode_id: i64) -> AppResult<()> {
    let segments = {
        let state = app.state::<AppState>();
        let conn = state.db.lock().expect("db");
        db::list_segments(&conn, episode_id)?
            .into_iter()
            .map(|s| intel::Segment {
                start_ms: s.start_ms,
                end_ms: s.end_ms,
                text: s.text,
            })
            .collect::<Vec<_>>()
    };
    let ads = intel::detect_ads(&segments);
    let state = app.state::<AppState>();
    let conn = state.db.lock().expect("db");
    db::replace_ads(&conn, episode_id, &ads)?;
    Ok(())
}
