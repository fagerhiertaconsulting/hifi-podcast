use crate::error::AppResult;
use crate::models::{
    AdSegment, Bookmark, Chapter, DeleteEpisodeFileOutcome, Download, Episode,
    FinishedCleanupOutcome, FinishedCleanupSettings, InboxItem, JobRow, LibraryStats, MusicAlbum,
    MusicPlaybackState, MusicTrack, PlaybackState, Podcast, PodcastSettings, QueueItem, Transcript,
    TranscriptSegment, UnsubscribeOutcome,
};
use rusqlite::{params, Connection, OptionalExtension};

const SIDELOAD_FEED_URL: &str = "local://sideload";
const FINISHED_CLEANUP_ENABLED_KEY: &str = "delete_finished_enabled";
const FINISHED_CLEANUP_DAYS_KEY: &str = "delete_finished_days";
const FINISHED_CLEANUP_DEFAULT_DAYS: i64 = 7;
const FINISHED_CLEANUP_MIN_DAYS: i64 = 1;
const FINISHED_CLEANUP_MAX_DAYS: i64 = 365;

pub fn initialize(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;
        PRAGMA journal_mode = WAL;

        CREATE TABLE IF NOT EXISTS podcast (
          id INTEGER PRIMARY KEY,
          guid TEXT,
          title TEXT NOT NULL,
          author TEXT,
          feed_url TEXT NOT NULL UNIQUE,
          artwork_path TEXT,
          description TEXT,
          auto_download INTEGER NOT NULL DEFAULT 0,
          speed REAL NOT NULL DEFAULT 1.0,
          intro_skip_sec INTEGER NOT NULL DEFAULT 0,
          outro_skip_sec INTEGER NOT NULL DEFAULT 0,
          skip_silence INTEGER NOT NULL DEFAULT 0,
          language TEXT,
          asr_language TEXT,
          skip_ads INTEGER NOT NULL DEFAULT 0,
          auto_transcribe INTEGER NOT NULL DEFAULT 0,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          subscribed_at TEXT
        );

        CREATE TABLE IF NOT EXISTS episode (
          id INTEGER PRIMARY KEY,
          podcast_id INTEGER NOT NULL REFERENCES podcast(id) ON DELETE CASCADE,
          guid TEXT NOT NULL,
          title TEXT NOT NULL,
          description TEXT,
          audio_url TEXT,
          publish_at TEXT,
          duration_ms INTEGER,
          episode_image_path TEXT,
          status TEXT NOT NULL DEFAULT 'new',
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          UNIQUE(podcast_id, guid)
        );

        CREATE INDEX IF NOT EXISTS idx_episode_podcast_pub
          ON episode(podcast_id, publish_at);

        CREATE TABLE IF NOT EXISTS download (
          id INTEGER PRIMARY KEY,
          episode_id INTEGER NOT NULL UNIQUE REFERENCES episode(id) ON DELETE CASCADE,
          file_path TEXT,
          bytes INTEGER,
          checksum TEXT,
          mime TEXT,
          state TEXT NOT NULL,
          error_msg TEXT,
          completed_at TEXT
        );

        CREATE TABLE IF NOT EXISTS playback_state (
          episode_id INTEGER PRIMARY KEY REFERENCES episode(id) ON DELETE CASCADE,
          position_ms INTEGER NOT NULL DEFAULT 0,
          speed REAL NOT NULL DEFAULT 1.0,
          completed INTEGER NOT NULL DEFAULT 0,
          last_played_at TEXT
        );

        CREATE TABLE IF NOT EXISTS queue_item (
          id INTEGER PRIMARY KEY,
          episode_id INTEGER NOT NULL REFERENCES episode(id) ON DELETE CASCADE,
          position INTEGER NOT NULL UNIQUE,
          added_at TEXT NOT NULL
        );
        "#,
    )?;
    migrate_v1(conn)?;
    migrate_episode_guid_scope(conn)?;
    migrate_upnp(conn)?;
    migrate_music(conn)?;
    migrate_subscription_inbox(conn)?;
    Ok(())
}

fn migrate_music(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS music_folder (
          id INTEGER PRIMARY KEY,
          path TEXT NOT NULL UNIQUE,
          last_imported_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS music_album (
          id INTEGER PRIMARY KEY,
          title TEXT NOT NULL,
          artist TEXT NOT NULL,
          key TEXT NOT NULL UNIQUE,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS music_track (
          id INTEGER PRIMARY KEY,
          album_id INTEGER NOT NULL REFERENCES music_album(id) ON DELETE CASCADE,
          source_path TEXT NOT NULL UNIQUE,
          file_path TEXT NOT NULL DEFAULT '',
          title TEXT NOT NULL,
          artist TEXT NOT NULL,
          album TEXT NOT NULL,
          track_no INTEGER,
          duration_ms INTEGER,
          format TEXT NOT NULL,
          codec TEXT,
          sample_rate INTEGER,
          bits_per_sample INTEGER,
          channels INTEGER,
          bytes INTEGER,
          created_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_music_track_album
          ON music_track(album_id, track_no, title);

        CREATE TABLE IF NOT EXISTS music_playback_state (
          track_id INTEGER PRIMARY KEY REFERENCES music_track(id) ON DELETE CASCADE,
          position_ms INTEGER NOT NULL DEFAULT 0,
          completed INTEGER NOT NULL DEFAULT 0,
          last_played_at TEXT
        );
        "#,
    )?;
    Ok(())
}

fn migrate_upnp(conn: &Connection) -> AppResult<()> {
    let _ = conn.execute("ALTER TABLE download ADD COLUMN mime TEXT", []);
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS app_setting (
          key TEXT PRIMARY KEY,
          value TEXT NOT NULL
        );
        "#,
    )?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    let v = conn
        .query_row(
            "SELECT value FROM app_setting WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(v)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO app_setting (key, value) VALUES (?1, ?2)
        ON CONFLICT(key) DO UPDATE SET value = excluded.value
        "#,
        params![key, value],
    )?;
    Ok(())
}

pub fn upnp_settings(conn: &Connection) -> AppResult<crate::models::UpnpSettings> {
    let bind = get_setting(conn, "upnp_bind")?.unwrap_or_default();
    let port = get_setting(conn, "upnp_port")?
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .unwrap_or(8200);
    Ok(crate::models::UpnpSettings { bind, port })
}

pub fn save_upnp_settings(
    conn: &Connection,
    settings: &crate::models::UpnpSettings,
) -> AppResult<()> {
    if settings.port == 0 {
        return Err(crate::error::AppError::msg("Port måste vara 1–65535."));
    }
    set_setting(conn, "upnp_bind", settings.bind.trim())?;
    set_setting(conn, "upnp_port", &settings.port.to_string())?;
    Ok(())
}

pub fn audio_engine_settings(conn: &Connection) -> AppResult<crate::models::AudioEngineSettings> {
    let backend = get_setting(conn, "audio_backend")?.unwrap_or_else(|| "native".into());
    let backend = if backend == "webview" {
        "webview".into()
    } else {
        "native".into()
    };
    Ok(crate::models::AudioEngineSettings {
        backend,
        output_api: {
            let v = get_setting(conn, "audio_output_api")?.unwrap_or_else(|| "auto".into());
            match v.as_str() {
                "asio" | "wasapi_shared" | "wasapi_exclusive" => v,
                _ => "auto".into(),
            }
        },
        device_name: get_setting(conn, "audio_device")?.unwrap_or_default(),
        exclusive: get_setting(conn, "audio_exclusive")?.as_deref() == Some("1"),
        skip_silence: get_setting(conn, "audio_skip_silence")?.as_deref() == Some("1"),
    })
}

pub fn save_audio_engine_settings(
    conn: &Connection,
    settings: &crate::models::AudioEngineSettings,
) -> AppResult<()> {
    let backend = if settings.backend == "webview" {
        "webview"
    } else {
        "native"
    };
    set_setting(conn, "audio_backend", backend)?;
    let api = match settings.output_api.as_str() {
        "asio" | "wasapi_shared" | "wasapi_exclusive" => settings.output_api.as_str(),
        _ => "auto",
    };
    set_setting(conn, "audio_output_api", api)?;
    set_setting(conn, "audio_device", settings.device_name.trim())?;
    set_setting(
        conn,
        "audio_exclusive",
        if settings.exclusive { "1" } else { "0" },
    )?;
    set_setting(
        conn,
        "audio_skip_silence",
        if settings.skip_silence { "1" } else { "0" },
    )?;
    Ok(())
}

pub fn finished_cleanup_settings(conn: &Connection) -> AppResult<FinishedCleanupSettings> {
    let enabled = get_setting(conn, FINISHED_CLEANUP_ENABLED_KEY)?.as_deref() == Some("1");
    let days = get_setting(conn, FINISHED_CLEANUP_DAYS_KEY)?
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|days| (FINISHED_CLEANUP_MIN_DAYS..=FINISHED_CLEANUP_MAX_DAYS).contains(days))
        .unwrap_or(FINISHED_CLEANUP_DEFAULT_DAYS);
    Ok(FinishedCleanupSettings { enabled, days })
}

pub fn save_finished_cleanup_settings(
    conn: &Connection,
    settings: &FinishedCleanupSettings,
) -> AppResult<()> {
    if !(FINISHED_CLEANUP_MIN_DAYS..=FINISHED_CLEANUP_MAX_DAYS).contains(&settings.days) {
        return Err(crate::error::AppError::msg(
            "Antal dagar måste vara mellan 1 och 365.",
        ));
    }
    set_setting(
        conn,
        FINISHED_CLEANUP_ENABLED_KEY,
        if settings.enabled { "1" } else { "0" },
    )?;
    set_setting(conn, FINISHED_CLEANUP_DAYS_KEY, &settings.days.to_string())?;
    Ok(())
}

pub fn ensure_upnp_uuid(conn: &Connection) -> AppResult<String> {
    if let Some(existing) = get_setting(conn, "upnp_uuid")? {
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let uuid = uuid::Uuid::new_v4().to_string();
    set_setting(conn, "upnp_uuid", &uuid)?;
    Ok(uuid)
}

pub fn rewrite_legacy_download_paths(
    conn: &Connection,
    media_root: &std::path::Path,
) -> AppResult<u64> {
    let mut stmt =
        conn.prepare("SELECT episode_id, file_path FROM download WHERE file_path IS NOT NULL")?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    let mut n = 0u64;
    for (episode_id, path) in rows {
        if let Some(mapped) = crate::download::remap_legacy_file_path(media_root, &path) {
            let old_exists = std::path::Path::new(&path).is_file();
            let mapped_exists = std::path::Path::new(&mapped).is_file();
            // Collision or skipped move: keep the path that still has the file.
            if old_exists && mapped_exists {
                continue;
            }
            if old_exists && !mapped_exists {
                continue;
            }
            conn.execute(
                "UPDATE download SET file_path = ?1 WHERE episode_id = ?2",
                params![mapped, episode_id],
            )?;
            n += 1;
        }
    }
    Ok(n)
}

pub fn backfill_download_mime(conn: &Connection) -> AppResult<u64> {
    let mut stmt = conn.prepare(
        "SELECT episode_id, file_path FROM download
         WHERE state = 'done' AND file_path IS NOT NULL
           AND (mime IS NULL OR mime = '')",
    )?;
    let rows: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);
    let mut n = 0u64;
    for (episode_id, path) in rows {
        let mime = crate::media_mime::from_path(std::path::Path::new(&path));
        conn.execute(
            "UPDATE download SET mime = ?1 WHERE episode_id = ?2",
            params![mime, episode_id],
        )?;
        n += 1;
    }
    Ok(n)
}

pub fn list_upnp_podcasts(conn: &Connection) -> AppResult<Vec<(i64, String, Option<String>, i64)>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT p.id, p.title, p.author, COUNT(d.episode_id)
        FROM podcast p
        JOIN episode e ON e.podcast_id = p.id
        JOIN download d ON d.episode_id = e.id
        WHERE d.state = 'done' AND d.file_path IS NOT NULL
        GROUP BY p.id
        ORDER BY p.title COLLATE NOCASE
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn list_upnp_episodes(
    conn: &Connection,
    podcast_id: i64,
) -> AppResult<Vec<crate::models::UpnpEpisode>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT e.id, e.title, p.id, p.title, p.author, e.duration_ms, e.publish_at,
               d.file_path, d.bytes, d.mime
        FROM episode e
        JOIN podcast p ON p.id = e.podcast_id
        JOIN download d ON d.episode_id = e.id
        WHERE p.id = ?1 AND d.state = 'done' AND d.file_path IS NOT NULL
        ORDER BY CASE WHEN e.publish_at IS NULL THEN 1 ELSE 0 END, e.publish_at DESC, e.id DESC
        "#,
    )?;
    let rows = stmt.query_map(params![podcast_id], map_upnp_episode)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn get_upnp_episode(
    conn: &Connection,
    episode_id: i64,
) -> AppResult<Option<crate::models::UpnpEpisode>> {
    let row = conn
        .query_row(
            r#"
            SELECT e.id, e.title, p.id, p.title, p.author, e.duration_ms, e.publish_at,
                   d.file_path, d.bytes, d.mime
            FROM episode e
            JOIN podcast p ON p.id = e.podcast_id
            JOIN download d ON d.episode_id = e.id
            WHERE e.id = ?1 AND d.state = 'done' AND d.file_path IS NOT NULL
            "#,
            params![episode_id],
            map_upnp_episode,
        )
        .optional()?;
    Ok(row)
}

fn map_upnp_episode(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::models::UpnpEpisode> {
    Ok(crate::models::UpnpEpisode {
        episode_id: row.get(0)?,
        episode_title: row.get(1)?,
        podcast_id: row.get(2)?,
        podcast_title: row.get(3)?,
        author: row.get(4)?,
        duration_ms: row.get(5)?,
        publish_at: row.get(6)?,
        file_path: row.get(7)?,
        bytes: row.get(8)?,
        mime: row.get(9)?,
    })
}

fn migrate_v1(conn: &Connection) -> AppResult<()> {
    for sql in [
        "ALTER TABLE podcast ADD COLUMN asr_language TEXT",
        "ALTER TABLE podcast ADD COLUMN skip_ads INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE podcast ADD COLUMN auto_transcribe INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE episode ADD COLUMN inbox INTEGER NOT NULL DEFAULT 1",
        "ALTER TABLE episode ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = conn.execute(sql, []);
    }
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS transcript (
          id INTEGER PRIMARY KEY,
          episode_id INTEGER NOT NULL UNIQUE REFERENCES episode(id) ON DELETE CASCADE,
          source TEXT NOT NULL,
          model TEXT,
          language TEXT,
          status TEXT NOT NULL,
          created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS transcript_segment (
          id INTEGER PRIMARY KEY,
          transcript_id INTEGER NOT NULL REFERENCES transcript(id) ON DELETE CASCADE,
          start_ms INTEGER NOT NULL,
          end_ms INTEGER NOT NULL,
          text TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_segment_transcript ON transcript_segment(transcript_id, start_ms);
        CREATE TABLE IF NOT EXISTS chapter (
          id INTEGER PRIMARY KEY,
          episode_id INTEGER NOT NULL REFERENCES episode(id) ON DELETE CASCADE,
          start_ms INTEGER NOT NULL,
          end_ms INTEGER,
          title TEXT NOT NULL,
          source TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chapter_episode ON chapter(episode_id, start_ms);
        CREATE TABLE IF NOT EXISTS ad_segment (
          id INTEGER PRIMARY KEY,
          episode_id INTEGER NOT NULL REFERENCES episode(id) ON DELETE CASCADE,
          start_ms INTEGER NOT NULL,
          end_ms INTEGER NOT NULL,
          confidence REAL NOT NULL,
          method TEXT NOT NULL,
          enabled INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE IF NOT EXISTS job (
          id INTEGER PRIMARY KEY,
          type TEXT NOT NULL,
          episode_id INTEGER NOT NULL REFERENCES episode(id) ON DELETE CASCADE,
          priority INTEGER NOT NULL DEFAULT 0,
          state TEXT NOT NULL,
          attempts INTEGER NOT NULL DEFAULT 0,
          payload_json TEXT,
          error_msg TEXT,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_job_state_pri ON job(state, priority, id);
        CREATE TABLE IF NOT EXISTS bookmark (
          id INTEGER PRIMARY KEY,
          episode_id INTEGER NOT NULL REFERENCES episode(id) ON DELETE CASCADE,
          position_ms INTEGER NOT NULL,
          note TEXT NOT NULL DEFAULT '',
          created_at TEXT NOT NULL
        );
        "#,
    )?;
    Ok(())
}

/// Older DBs had UNIQUE(guid) globally. Rebuild so the same GUID can exist
/// on two podcasts, and ON CONFLICT(podcast_id, guid) works after upgrade.
fn migrate_episode_guid_scope(conn: &Connection) -> AppResult<()> {
    if !episode_guid_is_globally_unique(conn) {
        return Ok(());
    }
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = OFF;
        CREATE TABLE episode_new (
          id INTEGER PRIMARY KEY,
          podcast_id INTEGER NOT NULL REFERENCES podcast(id) ON DELETE CASCADE,
          guid TEXT NOT NULL,
          title TEXT NOT NULL,
          description TEXT,
          audio_url TEXT,
          publish_at TEXT,
          duration_ms INTEGER,
          episode_image_path TEXT,
          status TEXT NOT NULL DEFAULT 'new',
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          inbox INTEGER NOT NULL DEFAULT 1,
          archived INTEGER NOT NULL DEFAULT 0,
          UNIQUE(podcast_id, guid)
        );
        INSERT INTO episode_new (
          id, podcast_id, guid, title, description, audio_url, publish_at, duration_ms,
          episode_image_path, status, created_at, updated_at, inbox, archived
        )
        SELECT
          id, podcast_id, guid, title, description, audio_url, publish_at, duration_ms,
          episode_image_path, status, created_at, updated_at, inbox, archived
        FROM episode;
        DROP TABLE episode;
        ALTER TABLE episode_new RENAME TO episode;
        CREATE INDEX IF NOT EXISTS idx_episode_podcast_pub
          ON episode(podcast_id, publish_at);
        PRAGMA foreign_keys = ON;
        "#,
    )?;
    Ok(())
}

/// Existing feeds keep their original subscription time. Episodes that were
/// not published after that time leave the inbox. Sideloaded files are left
/// alone, and episodes the user already removed from the inbox stay removed.
fn migrate_subscription_inbox(conn: &Connection) -> AppResult<()> {
    let _ = conn.execute("ALTER TABLE podcast ADD COLUMN subscribed_at TEXT", []);
    conn.execute(
        "UPDATE podcast SET subscribed_at = created_at
         WHERE subscribed_at IS NULL OR TRIM(subscribed_at) = ''",
        [],
    )?;
    let mut stmt = conn.prepare(
        r#"
        SELECT e.id, e.publish_at, p.subscribed_at
        FROM episode e
        JOIN podcast p ON p.id = e.podcast_id
        WHERE e.inbox = 1 AND p.feed_url != ?1
        "#,
    )?;
    let rows: Vec<(i64, Option<String>, Option<String>)> = stmt
        .query_map(params![SIDELOAD_FEED_URL], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for (id, publish_at, subscribed_at) in rows {
        let cutoff = subscribed_at.unwrap_or_default();
        if !published_after_subscription(publish_at.as_deref(), &cutoff) {
            conn.execute(
                "UPDATE episode SET inbox = 0 WHERE id = ?1 AND inbox = 1",
                params![id],
            )?;
        }
    }
    Ok(())
}

fn published_after_subscription(publish_at: Option<&str>, subscribed_at: &str) -> bool {
    match (
        publish_at.and_then(parse_utc_timestamp),
        parse_utc_timestamp(subscribed_at),
    ) {
        (Some(published), Some(cutoff)) => published > cutoff,
        _ => false,
    }
}

fn parse_utc_timestamp(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    chrono::DateTime::parse_from_rfc2822(raw)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
}

fn episode_guid_is_globally_unique(conn: &Connection) -> bool {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'episode'",
            [],
            |row| row.get(0),
        )
        .ok();
    let Some(sql) = sql else {
        return false;
    };
    let compact: String = sql
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    compact.contains("guidtextnotnullunique") && !compact.contains("unique(podcast_id,guid)")
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn upsert_podcast(
    conn: &Connection,
    feed_url: &str,
    title: &str,
    author: Option<&str>,
    description: Option<&str>,
    guid: Option<&str>,
    language: Option<&str>,
) -> AppResult<i64> {
    let ts = now_rfc3339();
    conn.execute(
        r#"
        INSERT INTO podcast (
          guid, title, author, feed_url, description, language, auto_transcribe,
          created_at, updated_at, subscribed_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7, ?7)
        ON CONFLICT(feed_url) DO UPDATE SET
          title = excluded.title,
          author = COALESCE(excluded.author, podcast.author),
          description = COALESCE(excluded.description, podcast.description),
          guid = COALESCE(excluded.guid, podcast.guid),
          language = COALESCE(excluded.language, podcast.language),
          updated_at = excluded.updated_at
        "#,
        params![guid, title, author, feed_url, description, language, ts],
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM podcast WHERE feed_url = ?1",
        params![feed_url],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn list_podcasts(conn: &Connection) -> AppResult<Vec<Podcast>> {
    let mut stmt = conn.prepare(
        "SELECT id, guid, title, author, feed_url, artwork_path, description,
                auto_download, speed, intro_skip_sec, outro_skip_sec, skip_silence,
                language, asr_language, skip_ads, auto_transcribe, created_at, updated_at,
                subscribed_at
         FROM podcast ORDER BY title COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], map_podcast)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn map_podcast(row: &rusqlite::Row<'_>) -> rusqlite::Result<Podcast> {
    Ok(Podcast {
        id: row.get(0)?,
        guid: row.get(1)?,
        title: row.get(2)?,
        author: row.get(3)?,
        feed_url: row.get(4)?,
        artwork_path: row.get(5)?,
        description: row.get(6)?,
        auto_download: row.get::<_, i64>(7)? != 0,
        speed: row.get(8)?,
        intro_skip_sec: row.get(9)?,
        outro_skip_sec: row.get(10)?,
        skip_silence: row.get::<_, i64>(11)? != 0,
        language: row.get(12)?,
        asr_language: row.get(13)?,
        skip_ads: row.get::<_, i64>(14)? != 0,
        auto_transcribe: row.get::<_, i64>(15)? != 0,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        subscribed_at: row.get::<_, Option<String>>(18)?.unwrap_or_default(),
    })
}

pub fn get_podcast(conn: &Connection, id: i64) -> AppResult<Podcast> {
    conn.query_row(
        "SELECT id, guid, title, author, feed_url, artwork_path, description,
                auto_download, speed, intro_skip_sec, outro_skip_sec, skip_silence,
                language, asr_language, skip_ads, auto_transcribe, created_at, updated_at,
                subscribed_at
         FROM podcast WHERE id = ?1",
        params![id],
        map_podcast,
    )
    .map_err(|_| crate::error::AppError::msg("Podcasten hittades inte."))
}

/// Remove the feed. Local files are deleted only when `delete_files` is set,
/// and then only inside `media/podcasts/<id>/`.
pub fn unsubscribe_podcast(
    conn: &Connection,
    media_root: &std::path::Path,
    podcast_id: i64,
    delete_files: bool,
) -> AppResult<UnsubscribeOutcome> {
    let podcast = get_podcast(conn, podcast_id)?;
    let (files_removed, warnings) = if delete_files {
        let deleted = crate::download::delete_show_media(media_root, podcast_id)?;
        (deleted.removed_files, deleted.warnings)
    } else {
        (0, Vec::new())
    };
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    let n = conn.execute("DELETE FROM podcast WHERE id = ?1", params![podcast_id])?;
    if n == 0 {
        return Err(crate::error::AppError::msg("Podcasten hittades inte."));
    }
    Ok(UnsubscribeOutcome {
        podcast_id,
        title: podcast.title,
        files_removed,
        warnings,
    })
}

/// Delete one episode's local file. The podcast and the episode row stay
/// so the episode can be downloaded again.
pub fn delete_episode_local_file(
    conn: &Connection,
    media_root: &std::path::Path,
    episode_id: i64,
) -> AppResult<DeleteEpisodeFileOutcome> {
    let episode = get_episode(conn, episode_id)?;
    let _podcast = get_podcast(conn, episode.podcast_id)?;
    let stored = get_download(conn, episode_id)?.and_then(|download| download.file_path);
    let deleted = crate::download::delete_episode_media(
        media_root,
        episode.podcast_id,
        episode.id,
        stored.as_deref(),
    )?;
    if deleted.forget_download {
        conn.execute(
            "DELETE FROM download WHERE episode_id = ?1",
            params![episode_id],
        )?;
        if episode.status == "downloaded" {
            set_episode_status(conn, episode_id, "new")?;
        }
    }
    Ok(DeleteEpisodeFileOutcome {
        episode_id: episode.id,
        podcast_id: episode.podcast_id,
        title: episode.title,
        files_removed: deleted.removed_files,
        warnings: deleted.warnings,
    })
}

fn playback_finished_before(
    finished: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
    days: i64,
) -> bool {
    if !(FINISHED_CLEANUP_MIN_DAYS..=FINISHED_CLEANUP_MAX_DAYS).contains(&days) {
        return false;
    }
    match finished.checked_add_signed(chrono::Duration::days(days)) {
        Some(limit) => limit < now,
        None => false,
    }
}

/// When the setting is on, delete local files for episodes whose playback
/// `completed` flag is set and whose `last_played_at` is strictly older than
/// N days. Each file goes through the single-episode delete path.
pub fn purge_finished_episode_files(
    conn: &Connection,
    media_root: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
) -> AppResult<FinishedCleanupOutcome> {
    let settings = finished_cleanup_settings(conn)?;
    if !settings.enabled {
        return Ok(FinishedCleanupOutcome {
            enabled: false,
            days: settings.days,
            files_removed: 0,
            warnings: Vec::new(),
        });
    }

    let mut stmt =
        conn.prepare("SELECT episode_id, last_played_at FROM playback_state WHERE completed = 1")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut due = Vec::new();
    for row in rows {
        let (episode_id, last_played_at) = row?;
        let Some(raw) = last_played_at
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(finished) = parse_utc_timestamp(raw) else {
            continue;
        };
        if playback_finished_before(finished, now, settings.days) {
            due.push(episode_id);
        }
    }
    drop(stmt);

    let mut files_removed = 0i64;
    let mut warnings = Vec::new();
    for episode_id in due {
        match delete_episode_local_file(conn, media_root, episode_id) {
            Ok(outcome) => {
                files_removed += outcome.files_removed;
                for warning in outcome.warnings {
                    push_cleanup_warning(&mut warnings, warning);
                }
            }
            Err(_) => {
                push_cleanup_warning(
                    &mut warnings,
                    "En färdigspelad fil kunde inte raderas och lämnades kvar.".into(),
                );
            }
        }
    }

    Ok(FinishedCleanupOutcome {
        enabled: true,
        days: settings.days,
        files_removed,
        warnings,
    })
}

fn push_cleanup_warning(warnings: &mut Vec<String>, warning: String) {
    const MAX_WARNINGS: usize = 24;
    if warnings.len() < MAX_WARNINGS {
        warnings.push(warning);
    } else if warnings.len() == MAX_WARNINGS {
        warnings.push("Ytterligare varningar utelämnades.".into());
    }
}

pub fn update_podcast_settings(
    conn: &Connection,
    id: i64,
    settings: &PodcastSettings,
) -> AppResult<()> {
    let ts = now_rfc3339();
    let n = conn.execute(
        r#"
        UPDATE podcast SET
          auto_download = ?1,
          speed = ?2,
          intro_skip_sec = ?3,
          outro_skip_sec = ?4,
          skip_silence = ?5,
          asr_language = ?6,
          skip_ads = ?7,
          auto_transcribe = ?8,
          updated_at = ?9
        WHERE id = ?10
        "#,
        params![
            settings.auto_download as i64,
            settings.speed,
            settings.intro_skip_sec,
            settings.outro_skip_sec,
            settings.skip_silence as i64,
            settings.asr_language,
            settings.skip_ads as i64,
            settings.auto_transcribe as i64,
            ts,
            id
        ],
    )?;
    if n == 0 {
        return Err(crate::error::AppError::msg("Podcasten hittades inte."));
    }
    Ok(())
}

pub fn upsert_episode(
    conn: &Connection,
    podcast_id: i64,
    guid: &str,
    title: &str,
    description: Option<&str>,
    audio_url: Option<&str>,
    publish_at: Option<&str>,
    duration_ms: Option<i64>,
) -> AppResult<i64> {
    let ts = now_rfc3339();
    conn.execute(
        r#"
        INSERT INTO episode (podcast_id, guid, title, description, audio_url, publish_at, duration_ms, status, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'new', ?8, ?8)
        ON CONFLICT(podcast_id, guid) DO UPDATE SET
          title = excluded.title,
          description = COALESCE(excluded.description, episode.description),
          audio_url = COALESCE(excluded.audio_url, episode.audio_url),
          publish_at = COALESCE(excluded.publish_at, episode.publish_at),
          duration_ms = COALESCE(excluded.duration_ms, episode.duration_ms),
          updated_at = excluded.updated_at
        "#,
        params![
            podcast_id,
            guid,
            title,
            description,
            audio_url,
            publish_at,
            duration_ms,
            ts
        ],
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM episode WHERE podcast_id = ?1 AND guid = ?2",
        params![podcast_id, guid],
        |row| row.get(0),
    )?;
    Ok(id)
}

/// Insert or refresh one feed episode. Inbox is decided only on insert:
/// the episode must be published strictly after the feed's subscription time.
/// A later refresh does not move that cutoff or put a back-catalog episode
/// into the inbox.
pub fn upsert_subscribed_episode(
    conn: &Connection,
    podcast_id: i64,
    guid: &str,
    title: &str,
    description: Option<&str>,
    audio_url: Option<&str>,
    publish_at: Option<&str>,
    duration_ms: Option<i64>,
) -> AppResult<i64> {
    let subscribed_at: String = conn
        .query_row(
            "SELECT COALESCE(subscribed_at, '') FROM podcast WHERE id = ?1",
            params![podcast_id],
            |row| row.get(0),
        )
        .map_err(|_| crate::error::AppError::msg("Podcasten hittades inte."))?;
    let inbox = published_after_subscription(publish_at, &subscribed_at);
    let ts = now_rfc3339();
    conn.execute(
        r#"
        INSERT INTO episode (
          podcast_id, guid, title, description, audio_url, publish_at, duration_ms,
          status, inbox, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'new', ?8, ?9, ?9)
        ON CONFLICT(podcast_id, guid) DO UPDATE SET
          title = excluded.title,
          description = COALESCE(excluded.description, episode.description),
          audio_url = COALESCE(excluded.audio_url, episode.audio_url),
          publish_at = COALESCE(excluded.publish_at, episode.publish_at),
          duration_ms = COALESCE(excluded.duration_ms, episode.duration_ms),
          updated_at = excluded.updated_at
        "#,
        params![
            podcast_id,
            guid,
            title,
            description,
            audio_url,
            publish_at,
            duration_ms,
            inbox as i64,
            ts
        ],
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM episode WHERE podcast_id = ?1 AND guid = ?2",
        params![podcast_id, guid],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn list_episodes(conn: &Connection, podcast_id: i64) -> AppResult<Vec<Episode>> {
    let mut stmt = conn.prepare(
        "SELECT id, podcast_id, guid, title, description, audio_url, publish_at,
                duration_ms, episode_image_path, status, inbox, archived, created_at, updated_at
         FROM episode WHERE podcast_id = ?1
         ORDER BY CASE WHEN publish_at IS NULL THEN 1 ELSE 0 END, publish_at DESC, id DESC",
    )?;
    let rows = stmt.query_map(params![podcast_id], map_episode)?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn map_episode(row: &rusqlite::Row<'_>) -> rusqlite::Result<Episode> {
    Ok(Episode {
        id: row.get(0)?,
        podcast_id: row.get(1)?,
        guid: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        audio_url: row.get(5)?,
        publish_at: row.get(6)?,
        duration_ms: row.get(7)?,
        episode_image_path: row.get(8)?,
        status: row.get(9)?,
        inbox: row.get::<_, i64>(10)? != 0,
        archived: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

pub fn get_episode(conn: &Connection, id: i64) -> AppResult<Episode> {
    conn.query_row(
        "SELECT id, podcast_id, guid, title, description, audio_url, publish_at,
                duration_ms, episode_image_path, status, inbox, archived, created_at, updated_at
         FROM episode WHERE id = ?1",
        params![id],
        map_episode,
    )
    .map_err(|_| crate::error::AppError::msg("Avsnittet hittades inte."))
}

pub fn set_episode_status(conn: &Connection, id: i64, status: &str) -> AppResult<()> {
    let ts = now_rfc3339();
    conn.execute(
        "UPDATE episode SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status, ts, id],
    )?;
    Ok(())
}

pub fn upsert_download_state(
    conn: &Connection,
    episode_id: i64,
    state: &str,
    file_path: Option<&str>,
    bytes: Option<i64>,
    checksum: Option<&str>,
    error_msg: Option<&str>,
    completed_at: Option<&str>,
    mime: Option<&str>,
) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO download (episode_id, file_path, bytes, checksum, mime, state, error_msg, completed_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        ON CONFLICT(episode_id) DO UPDATE SET
          file_path = COALESCE(excluded.file_path, download.file_path),
          bytes = COALESCE(excluded.bytes, download.bytes),
          checksum = COALESCE(excluded.checksum, download.checksum),
          mime = COALESCE(excluded.mime, download.mime),
          state = excluded.state,
          error_msg = excluded.error_msg,
          completed_at = COALESCE(excluded.completed_at, download.completed_at)
        "#,
        params![
            episode_id,
            file_path,
            bytes,
            checksum,
            mime,
            state,
            error_msg,
            completed_at
        ],
    )?;
    Ok(())
}

pub fn get_download(conn: &Connection, episode_id: i64) -> AppResult<Option<Download>> {
    let row = conn
        .query_row(
            "SELECT id, episode_id, file_path, bytes, checksum, mime, state, error_msg, completed_at
             FROM download WHERE episode_id = ?1",
            params![episode_id],
            |row| {
                Ok(Download {
                    id: row.get(0)?,
                    episode_id: row.get(1)?,
                    file_path: row.get(2)?,
                    bytes: row.get(3)?,
                    checksum: row.get(4)?,
                    mime: row.get(5)?,
                    state: row.get(6)?,
                    error_msg: row.get(7)?,
                    completed_at: row.get(8)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

pub fn save_playback(conn: &Connection, state: &PlaybackState) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO playback_state (episode_id, position_ms, speed, completed, last_played_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(episode_id) DO UPDATE SET
          position_ms = excluded.position_ms,
          speed = excluded.speed,
          completed = excluded.completed,
          last_played_at = excluded.last_played_at
        "#,
        params![
            state.episode_id,
            state.position_ms,
            state.speed,
            state.completed as i64,
            state.last_played_at
        ],
    )?;
    Ok(())
}

pub fn get_playback(conn: &Connection, episode_id: i64) -> AppResult<Option<PlaybackState>> {
    let row = conn
        .query_row(
            "SELECT episode_id, position_ms, speed, completed, last_played_at
             FROM playback_state WHERE episode_id = ?1",
            params![episode_id],
            |row| {
                Ok(PlaybackState {
                    episode_id: row.get(0)?,
                    position_ms: row.get(1)?,
                    speed: row.get(2)?,
                    completed: row.get::<_, i64>(3)? != 0,
                    last_played_at: row.get(4)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

pub fn list_queue(conn: &Connection) -> AppResult<Vec<QueueItem>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT q.id, q.episode_id, q.position, q.added_at,
               e.title, p.title, d.file_path, d.state
        FROM queue_item q
        JOIN episode e ON e.id = q.episode_id
        JOIN podcast p ON p.id = e.podcast_id
        LEFT JOIN download d ON d.episode_id = e.id
        ORDER BY q.position
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(QueueItem {
            id: row.get(0)?,
            episode_id: row.get(1)?,
            position: row.get(2)?,
            added_at: row.get(3)?,
            episode_title: row.get(4)?,
            podcast_title: row.get(5)?,
            file_path: row.get(6)?,
            download_state: row.get(7)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn add_to_queue(conn: &Connection, episode_id: i64) -> AppResult<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM queue_item WHERE episode_id = ?1",
        params![episode_id],
        |row| row.get(0),
    )?;
    if exists > 0 {
        return Ok(());
    }
    let next: i64 = conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM queue_item",
        [],
        |row| row.get(0),
    )?;
    conn.execute(
        "INSERT INTO queue_item (episode_id, position, added_at) VALUES (?1, ?2, ?3)",
        params![episode_id, next, now_rfc3339()],
    )?;
    Ok(())
}

pub fn remove_from_queue(conn: &Connection, queue_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM queue_item WHERE id = ?1", params![queue_id])?;
    reindex_queue(conn)
}

pub fn move_queue_item(conn: &Connection, queue_id: i64, direction: i64) -> AppResult<()> {
    let current: i64 = conn.query_row(
        "SELECT position FROM queue_item WHERE id = ?1",
        params![queue_id],
        |row| row.get(0),
    )?;
    let target = current + direction;
    let swap: Option<i64> = conn
        .query_row(
            "SELECT id FROM queue_item WHERE position = ?1",
            params![target],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(other_id) = swap {
        conn.execute(
            "UPDATE queue_item SET position = -1 WHERE id = ?1",
            params![queue_id],
        )?;
        conn.execute(
            "UPDATE queue_item SET position = ?1 WHERE id = ?2",
            params![current, other_id],
        )?;
        conn.execute(
            "UPDATE queue_item SET position = ?1 WHERE id = ?2",
            params![target, queue_id],
        )?;
    }
    Ok(())
}

fn reindex_queue(conn: &Connection) -> AppResult<()> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM queue_item ORDER BY position")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.filter_map(|r| r.ok()).collect()
    };
    for (i, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE queue_item SET position = ?1 WHERE id = ?2",
            params![i as i64, id],
        )?;
    }
    Ok(())
}

pub fn enqueue_job(
    conn: &Connection,
    job_type: &str,
    episode_id: i64,
    priority: i64,
) -> AppResult<i64> {
    let existing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM job WHERE episode_id = ?1 AND type = ?2 AND state IN ('queued','running')",
        params![episode_id, job_type],
        |row| row.get(0),
    )?;
    if existing > 0 {
        return Ok(0);
    }
    let ts = now_rfc3339();
    conn.execute(
        "INSERT INTO job (type, episode_id, priority, state, attempts, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'queued', 0, ?4, ?4)",
        params![job_type, episode_id, priority, ts],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn claim_next_job(conn: &Connection) -> AppResult<Option<(i64, String, i64)>> {
    let row: Option<(i64, String, i64)> = conn
        .query_row(
            "SELECT id, type, episode_id FROM job WHERE state = 'queued' ORDER BY priority DESC, id ASC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some((id, ty, eid)) = &row {
        conn.execute(
            "UPDATE job SET state = 'running', attempts = attempts + 1, updated_at = ?1 WHERE id = ?2",
            params![now_rfc3339(), id],
        )?;
        let _ = (ty, eid);
    }
    Ok(row)
}

pub fn requeue_running_jobs(conn: &Connection) -> AppResult<usize> {
    let n = conn.execute(
        "UPDATE job SET state = 'queued', error_msg = 'Omstart: jobbet låg kvar som running.', updated_at = ?1 WHERE state = 'running'",
        params![now_rfc3339()],
    )?;
    Ok(n)
}

pub fn set_job_progress(conn: &Connection, id: i64, msg: &str) -> AppResult<()> {
    let clipped: String = msg.chars().take(400).collect();
    conn.execute(
        "UPDATE job SET error_msg = ?1, updated_at = ?2 WHERE id = ?3 AND state = 'running'",
        params![clipped, now_rfc3339(), id],
    )?;
    Ok(())
}

pub fn finish_job(conn: &Connection, id: i64, ok: bool, error: Option<&str>) -> AppResult<()> {
    conn.execute(
        "UPDATE job SET state = ?1, error_msg = ?2, updated_at = ?3 WHERE id = ?4",
        params![if ok { "done" } else { "error" }, error, now_rfc3339(), id],
    )?;
    Ok(())
}

pub fn list_jobs(conn: &Connection, limit: i64) -> AppResult<Vec<JobRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, type, episode_id, state, error_msg, created_at FROM job ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(JobRow {
            id: row.get(0)?,
            job_type: row.get(1)?,
            episode_id: row.get(2)?,
            state: row.get(3)?,
            error_msg: row.get(4)?,
            created_at: row.get(5)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn replace_transcript(
    conn: &Connection,
    episode_id: i64,
    source: &str,
    model: Option<&str>,
    language: Option<&str>,
    segments: &[crate::intel::Segment],
) -> AppResult<i64> {
    conn.execute(
        "DELETE FROM transcript WHERE episode_id = ?1",
        params![episode_id],
    )?;
    let ts = now_rfc3339();
    conn.execute(
        "INSERT INTO transcript (episode_id, source, model, language, status, created_at)
         VALUES (?1, ?2, ?3, ?4, 'ready', ?5)",
        params![episode_id, source, model, language, ts],
    )?;
    let tid = conn.last_insert_rowid();
    for seg in segments {
        conn.execute(
            "INSERT INTO transcript_segment (transcript_id, start_ms, end_ms, text) VALUES (?1, ?2, ?3, ?4)",
            params![tid, seg.start_ms, seg.end_ms, seg.text],
        )?;
    }
    Ok(tid)
}

pub fn get_transcript(conn: &Connection, episode_id: i64) -> AppResult<Option<Transcript>> {
    conn.query_row(
        "SELECT id, episode_id, source, model, language, status, created_at FROM transcript WHERE episode_id = ?1",
        params![episode_id],
        |row| {
            Ok(Transcript {
                id: row.get(0)?,
                episode_id: row.get(1)?,
                source: row.get(2)?,
                model: row.get(3)?,
                language: row.get(4)?,
                status: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_segments(conn: &Connection, episode_id: i64) -> AppResult<Vec<TranscriptSegment>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT s.id, s.transcript_id, s.start_ms, s.end_ms, s.text
        FROM transcript_segment s
        JOIN transcript t ON t.id = s.transcript_id
        WHERE t.episode_id = ?1
        ORDER BY s.start_ms
        "#,
    )?;
    let rows = stmt.query_map(params![episode_id], |row| {
        Ok(TranscriptSegment {
            id: row.get(0)?,
            transcript_id: row.get(1)?,
            start_ms: row.get(2)?,
            end_ms: row.get(3)?,
            text: row.get(4)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn search_segments(conn: &Connection, query: &str) -> AppResult<Vec<TranscriptSegment>> {
    let like = format!("%{}%", query);
    let mut stmt = conn.prepare(
        "SELECT s.id, s.transcript_id, s.start_ms, s.end_ms, s.text
         FROM transcript_segment s
         WHERE s.text LIKE ?1 COLLATE NOCASE
         ORDER BY s.start_ms
         LIMIT 200",
    )?;
    let rows = stmt.query_map(params![like], |row| {
        Ok(TranscriptSegment {
            id: row.get(0)?,
            transcript_id: row.get(1)?,
            start_ms: row.get(2)?,
            end_ms: row.get(3)?,
            text: row.get(4)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn replace_generated_chapters(
    conn: &Connection,
    episode_id: i64,
    chapters: &[crate::intel::ChapterDraft],
) -> AppResult<()> {
    conn.execute(
        "DELETE FROM chapter WHERE episode_id = ?1 AND source IN ('generated','embedded')",
        params![episode_id],
    )?;
    for ch in chapters {
        conn.execute(
            "INSERT INTO chapter (episode_id, start_ms, end_ms, title, source) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![episode_id, ch.start_ms, ch.end_ms, ch.title, ch.source],
        )?;
    }
    Ok(())
}

pub fn list_chapters(conn: &Connection, episode_id: i64) -> AppResult<Vec<Chapter>> {
    let mut stmt = conn.prepare(
        "SELECT id, episode_id, start_ms, end_ms, title, source FROM chapter WHERE episode_id = ?1 ORDER BY start_ms",
    )?;
    let rows = stmt.query_map(params![episode_id], |row| {
        Ok(Chapter {
            id: row.get(0)?,
            episode_id: row.get(1)?,
            start_ms: row.get(2)?,
            end_ms: row.get(3)?,
            title: row.get(4)?,
            source: row.get(5)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn replace_ads(
    conn: &Connection,
    episode_id: i64,
    ads: &[crate::intel::AdDraft],
) -> AppResult<()> {
    conn.execute(
        "DELETE FROM ad_segment WHERE episode_id = ?1 AND method != 'user'",
        params![episode_id],
    )?;
    for ad in ads {
        conn.execute(
            "INSERT INTO ad_segment (episode_id, start_ms, end_ms, confidence, method, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![episode_id, ad.start_ms, ad.end_ms, ad.confidence, ad.method],
        )?;
    }
    Ok(())
}

pub fn list_ads(conn: &Connection, episode_id: i64) -> AppResult<Vec<AdSegment>> {
    let mut stmt = conn.prepare(
        "SELECT id, episode_id, start_ms, end_ms, confidence, method, enabled FROM ad_segment WHERE episode_id = ?1 ORDER BY start_ms",
    )?;
    let rows = stmt.query_map(params![episode_id], |row| {
        Ok(AdSegment {
            id: row.get(0)?,
            episode_id: row.get(1)?,
            start_ms: row.get(2)?,
            end_ms: row.get(3)?,
            confidence: row.get(4)?,
            method: row.get(5)?,
            enabled: row.get::<_, i64>(6)? != 0,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn add_bookmark(
    conn: &Connection,
    episode_id: i64,
    position_ms: i64,
    note: &str,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO bookmark (episode_id, position_ms, note, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![episode_id, position_ms, note, now_rfc3339()],
    )?;
    Ok(())
}

pub fn list_bookmarks(conn: &Connection) -> AppResult<Vec<Bookmark>> {
    let mut stmt = conn.prepare(
        "SELECT b.id, b.episode_id, b.position_ms, b.note, b.created_at, e.title
         FROM bookmark b JOIN episode e ON e.id = b.episode_id
         ORDER BY b.created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Bookmark {
            id: row.get(0)?,
            episode_id: row.get(1)?,
            position_ms: row.get(2)?,
            note: row.get(3)?,
            created_at: row.get(4)?,
            episode_title: row.get(5)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn delete_bookmark(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM bookmark WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn set_inbox(conn: &Connection, episode_id: i64, inbox: bool, archived: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE episode SET inbox = ?1, archived = ?2, updated_at = ?3 WHERE id = ?4",
        params![inbox as i64, archived as i64, now_rfc3339(), episode_id],
    )?;
    Ok(())
}

pub fn list_inbox(conn: &Connection) -> AppResult<Vec<InboxItem>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT e.id, e.podcast_id, e.guid, e.title, e.description, e.audio_url, e.publish_at,
               e.duration_ms, e.episode_image_path, e.status, e.inbox, e.archived, e.created_at, e.updated_at,
               p.title, d.state
        FROM episode e
        JOIN podcast p ON p.id = e.podcast_id
        LEFT JOIN download d ON d.episode_id = e.id
        WHERE e.inbox = 1 AND e.archived = 0
        ORDER BY CASE WHEN e.publish_at IS NULL THEN 1 ELSE 0 END, e.publish_at DESC
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(InboxItem {
            episode: Episode {
                id: row.get(0)?,
                podcast_id: row.get(1)?,
                guid: row.get(2)?,
                title: row.get(3)?,
                description: row.get(4)?,
                audio_url: row.get(5)?,
                publish_at: row.get(6)?,
                duration_ms: row.get(7)?,
                episode_image_path: row.get(8)?,
                status: row.get(9)?,
                inbox: row.get::<_, i64>(10)? != 0,
                archived: row.get::<_, i64>(11)? != 0,
                created_at: row.get(12)?,
                updated_at: row.get(13)?,
            },
            podcast_title: row.get(14)?,
            download_state: row.get(15)?,
        })
    })?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

pub fn library_stats(conn: &Connection) -> AppResult<LibraryStats> {
    let podcast_count: i64 = conn.query_row("SELECT COUNT(*) FROM podcast", [], |r| r.get(0))?;
    let episode_count: i64 = conn.query_row("SELECT COUNT(*) FROM episode", [], |r| r.get(0))?;
    let downloaded_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM download WHERE state = 'done'",
        [],
        |r| r.get(0),
    )?;
    let completed_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM playback_state WHERE completed = 1",
        [],
        |r| r.get(0),
    )?;
    let inbox_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM episode WHERE inbox = 1 AND archived = 0",
        [],
        |r| r.get(0),
    )?;
    let bookmark_count: i64 = conn.query_row("SELECT COUNT(*) FROM bookmark", [], |r| r.get(0))?;
    let listened_ms: i64 = conn.query_row(
        "SELECT COALESCE(SUM(position_ms), 0) FROM playback_state",
        [],
        |r| r.get(0),
    )?;
    Ok(LibraryStats {
        podcast_count,
        episode_count,
        downloaded_count,
        completed_count,
        inbox_count,
        bookmark_count,
        listened_ms,
    })
}

pub fn ensure_sideload_podcast(conn: &Connection) -> AppResult<i64> {
    upsert_podcast(
        conn,
        SIDELOAD_FEED_URL,
        "Importerade filer",
        Some("Lokal"),
        Some("Ljudfiler importerade utan RSS."),
        Some("sideload"),
        None,
    )
}

pub fn podcast_wants_transcribe(conn: &Connection, podcast_id: i64) -> AppResult<bool> {
    let v: i64 = conn.query_row(
        "SELECT auto_transcribe FROM podcast WHERE id = ?1",
        params![podcast_id],
        |row| row.get(0),
    )?;
    Ok(v != 0)
}

pub fn podcast_asr_language(conn: &Connection, podcast_id: i64) -> AppResult<String> {
    let v: Option<String> = conn.query_row(
        "SELECT asr_language FROM podcast WHERE id = ?1",
        params![podcast_id],
        |row| row.get(0),
    )?;
    Ok(v.unwrap_or_else(|| "auto".into()))
}

pub fn download_is_done(conn: &Connection, episode_id: i64) -> bool {
    matches!(get_download(conn, episode_id), Ok(Some(d)) if d.state == "done")
}

/// Episodes already stored for a show, before the current fetch upserts.
/// `file_present` is true only when the download row is completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeSeenBeforeFetch {
    pub guid: String,
    pub file_present: bool,
}

pub fn episodes_seen_before_fetch(
    conn: &Connection,
    podcast_id: i64,
) -> AppResult<Vec<EpisodeSeenBeforeFetch>> {
    let mut stmt = conn.prepare(
        "SELECT e.guid, CASE WHEN d.state = 'done' THEN 1 ELSE 0 END
         FROM episode e
         LEFT JOIN download d ON d.episode_id = e.id
         WHERE e.podcast_id = ?1
         ORDER BY e.id",
    )?;
    let rows = stmt.query_map(params![podcast_id], |row| {
        Ok(EpisodeSeenBeforeFetch {
            guid: row.get(0)?,
            file_present: row.get::<_, i64>(1)? != 0,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn music_album_key(title: &str, artist: &str) -> String {
    format!(
        "{}|||{}",
        artist.trim().to_lowercase(),
        title.trim().to_lowercase()
    )
}

pub fn last_music_import_root(conn: &Connection) -> AppResult<Option<String>> {
    get_setting(conn, "music_last_import_root")
}

pub fn upsert_music_folder(conn: &Connection, path: &str) -> AppResult<i64> {
    let ts = now_rfc3339();
    conn.execute(
        r#"
        INSERT INTO music_folder (path, last_imported_at) VALUES (?1, ?2)
        ON CONFLICT(path) DO UPDATE SET last_imported_at = excluded.last_imported_at
        "#,
        params![path, ts],
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM music_folder WHERE path = ?1",
        params![path],
        |r| r.get(0),
    )?;
    Ok(id)
}

pub fn upsert_music_album(conn: &Connection, title: &str, artist: &str) -> AppResult<i64> {
    let ts = now_rfc3339();
    let key = music_album_key(title, artist);
    conn.execute(
        r#"
        INSERT INTO music_album (title, artist, key, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?4)
        ON CONFLICT(key) DO UPDATE SET
          title = excluded.title,
          artist = excluded.artist,
          updated_at = excluded.updated_at
        "#,
        params![title, artist, key, ts],
    )?;
    let id: i64 = conn.query_row(
        "SELECT id FROM music_album WHERE key = ?1",
        params![key],
        |r| r.get(0),
    )?;
    Ok(id)
}

pub fn insert_music_track_placeholder(
    conn: &Connection,
    album_id: i64,
    source_path: &str,
    title: &str,
    artist: &str,
    album: &str,
    track_no: Option<i64>,
    duration_ms: Option<i64>,
    format: &str,
    codec: Option<&str>,
    sample_rate: Option<i64>,
    bits_per_sample: Option<i64>,
    channels: Option<i64>,
) -> AppResult<i64> {
    let ts = now_rfc3339();
    conn.execute(
        r#"
        INSERT INTO music_track (
          album_id, source_path, file_path, title, artist, album, track_no,
          duration_ms, format, codec, sample_rate, bits_per_sample, channels, created_at
        ) VALUES (?1, ?2, '', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        "#,
        params![
            album_id,
            source_path,
            title,
            artist,
            album,
            track_no,
            duration_ms,
            format,
            codec,
            sample_rate,
            bits_per_sample,
            channels,
            ts
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn set_music_track_file(
    conn: &Connection,
    track_id: i64,
    file_path: &str,
    bytes: Option<i64>,
) -> AppResult<()> {
    conn.execute(
        "UPDATE music_track SET file_path = ?1, bytes = ?2 WHERE id = ?3",
        params![file_path, bytes, track_id],
    )?;
    Ok(())
}

pub fn delete_music_track(conn: &Connection, track_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM music_track WHERE id = ?1", params![track_id])?;
    Ok(())
}

pub fn music_track_by_source(
    conn: &Connection,
    source_path: &str,
) -> AppResult<Option<MusicTrack>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, album_id, source_path, file_path, title, artist, album, track_no,
               duration_ms, format, codec, sample_rate, bits_per_sample, channels, bytes, created_at
        FROM music_track WHERE source_path = ?1
        "#,
    )?;
    let row = stmt
        .query_row(params![source_path], map_music_track)
        .optional()?;
    Ok(row)
}

pub fn get_music_track(conn: &Connection, id: i64) -> AppResult<MusicTrack> {
    conn.query_row(
        r#"
        SELECT id, album_id, source_path, file_path, title, artist, album, track_no,
               duration_ms, format, codec, sample_rate, bits_per_sample, channels, bytes, created_at
        FROM music_track WHERE id = ?1
        "#,
        params![id],
        map_music_track,
    )
    .map_err(|_| crate::error::AppError::msg("Spåret finns inte."))
}

pub fn get_music_album(conn: &Connection, id: i64) -> AppResult<MusicAlbum> {
    conn.query_row(
        r#"
        SELECT a.id, a.title, a.artist,
               (SELECT COUNT(*) FROM music_track t WHERE t.album_id = a.id),
               a.created_at, a.updated_at
        FROM music_album a WHERE a.id = ?1
        "#,
        params![id],
        map_music_album,
    )
    .map_err(|_| crate::error::AppError::msg("Albumet finns inte."))
}

pub fn list_music_albums(conn: &Connection) -> AppResult<Vec<MusicAlbum>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT a.id, a.title, a.artist,
               (SELECT COUNT(*) FROM music_track t WHERE t.album_id = a.id),
               a.created_at, a.updated_at
        FROM music_album a
        ORDER BY a.artist COLLATE NOCASE, a.title COLLATE NOCASE
        "#,
    )?;
    let rows = stmt.query_map([], map_music_album)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn list_music_tracks(conn: &Connection, album_id: Option<i64>) -> AppResult<Vec<MusicTrack>> {
    if let Some(album_id) = album_id {
        let mut stmt = conn.prepare(
            r#"
            SELECT id, album_id, source_path, file_path, title, artist, album, track_no,
                   duration_ms, format, codec, sample_rate, bits_per_sample, channels, bytes, created_at
            FROM music_track WHERE album_id = ?1
            ORDER BY track_no IS NULL, track_no, title COLLATE NOCASE, id
            "#,
        )?;
        let rows = stmt.query_map(params![album_id], map_music_track)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        return Ok(out);
    }
    let mut stmt = conn.prepare(
        r#"
        SELECT id, album_id, source_path, file_path, title, artist, album, track_no,
               duration_ms, format, codec, sample_rate, bits_per_sample, channels, bytes, created_at
        FROM music_track
        ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, track_no IS NULL, track_no, title COLLATE NOCASE, id
        "#,
    )?;
    let rows = stmt.query_map([], map_music_track)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn count_music_albums(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM music_album", [], |r| r.get(0))?)
}

pub fn count_music_tracks(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("SELECT COUNT(*) FROM music_track", [], |r| r.get(0))?)
}

pub fn save_music_playback(conn: &Connection, state: &MusicPlaybackState) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO music_playback_state (track_id, position_ms, completed, last_played_at)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(track_id) DO UPDATE SET
          position_ms = excluded.position_ms,
          completed = excluded.completed,
          last_played_at = excluded.last_played_at
        "#,
        params![
            state.track_id,
            state.position_ms,
            state.completed as i64,
            state.last_played_at
        ],
    )?;
    Ok(())
}

pub fn get_music_playback(
    conn: &Connection,
    track_id: i64,
) -> AppResult<Option<MusicPlaybackState>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT track_id, position_ms, completed, last_played_at
        FROM music_playback_state WHERE track_id = ?1
        "#,
    )?;
    let row = stmt
        .query_row(params![track_id], |row| {
            Ok(MusicPlaybackState {
                track_id: row.get(0)?,
                position_ms: row.get(1)?,
                completed: row.get::<_, i64>(2)? != 0,
                last_played_at: row.get(3)?,
            })
        })
        .optional()?;
    Ok(row)
}

pub fn next_music_track(conn: &Connection, track_id: i64) -> AppResult<Option<MusicTrack>> {
    let current = get_music_track(conn, track_id)?;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, album_id, source_path, file_path, title, artist, album, track_no,
               duration_ms, format, codec, sample_rate, bits_per_sample, channels, bytes, created_at
        FROM music_track
        WHERE album_id = ?1 AND (
          (COALESCE(track_no, 999999) > COALESCE(?2, 999999))
          OR (
            COALESCE(track_no, 999999) = COALESCE(?2, 999999)
            AND (
              title COLLATE NOCASE > ?3 COLLATE NOCASE
              OR (title COLLATE NOCASE = ?3 COLLATE NOCASE AND id > ?4)
            )
          )
        )
        ORDER BY track_no IS NULL, track_no, title COLLATE NOCASE, id
        LIMIT 1
        "#,
    )?;
    let row = stmt
        .query_row(
            params![
                current.album_id,
                current.track_no,
                current.title,
                current.id
            ],
            map_music_track,
        )
        .optional()?;
    Ok(row)
}

fn map_music_album(row: &rusqlite::Row<'_>) -> rusqlite::Result<MusicAlbum> {
    Ok(MusicAlbum {
        id: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        track_count: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn map_music_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<MusicTrack> {
    Ok(MusicTrack {
        id: row.get(0)?,
        album_id: row.get(1)?,
        source_path: row.get(2)?,
        file_path: row.get(3)?,
        title: row.get(4)?,
        artist: row.get(5)?,
        album: row.get(6)?,
        track_no: row.get(7)?,
        duration_ms: row.get(8)?,
        format: row.get(9)?,
        codec: row.get(10)?,
        sample_rate: row.get(11)?,
        bits_per_sample: row.get(12)?,
        channels: row.get(13)?,
        bytes: row.get(14)?,
        created_at: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_and_queue_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let pid = upsert_podcast(
            &conn,
            "https://example.com/feed.xml",
            "Testpodd",
            Some("A"),
            None,
            None,
            Some("sv"),
        )
        .unwrap();
        let eid = upsert_episode(
            &conn,
            pid,
            "guid-1",
            "Avsnitt 1",
            None,
            Some("https://example.com/a.mp3"),
            None,
            Some(60_000),
        )
        .unwrap();
        add_to_queue(&conn, eid).unwrap();
        add_to_queue(&conn, eid).unwrap();
        assert_eq!(list_queue(&conn).unwrap().len(), 1);
        save_playback(
            &conn,
            &PlaybackState {
                episode_id: eid,
                position_ms: 1234,
                speed: 1.5,
                completed: false,
                last_played_at: Some(now_rfc3339()),
            },
        )
        .unwrap();
        assert_eq!(get_playback(&conn, eid).unwrap().unwrap().position_ms, 1234);
        enqueue_job(&conn, "transcribe", eid, 10).unwrap();
        assert!(claim_next_job(&conn).unwrap().is_some());
        assert_eq!(requeue_running_jobs(&conn).unwrap(), 1);
        assert_eq!(list_jobs(&conn, 5).unwrap()[0].state, "queued");
        assert!(!get_podcast(&conn, pid).unwrap().auto_transcribe);
    }

    #[test]
    fn episode_guid_unique_per_podcast_not_global() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let a = upsert_podcast(
            &conn,
            "https://a.example/feed.xml",
            "A",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let b = upsert_podcast(
            &conn,
            "https://b.example/feed.xml",
            "B",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let e1 = upsert_episode(&conn, a, "same-guid", "Ep A", None, None, None, None).unwrap();
        let e2 = upsert_episode(&conn, b, "same-guid", "Ep B", None, None, None, None).unwrap();
        assert_ne!(e1, e2);
        let e1b = upsert_episode(
            &conn,
            a,
            "same-guid",
            "Ep A uppdaterad",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert_eq!(e1, e1b);
        assert_eq!(get_episode(&conn, e1).unwrap().title, "Ep A uppdaterad");
    }

    #[test]
    fn upgrades_global_guid_unique_without_losing_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE podcast (
              id INTEGER PRIMARY KEY,
              guid TEXT,
              title TEXT NOT NULL,
              author TEXT,
              feed_url TEXT NOT NULL UNIQUE,
              artwork_path TEXT,
              description TEXT,
              auto_download INTEGER NOT NULL DEFAULT 0,
              speed REAL NOT NULL DEFAULT 1.0,
              intro_skip_sec INTEGER NOT NULL DEFAULT 0,
              outro_skip_sec INTEGER NOT NULL DEFAULT 0,
              skip_silence INTEGER NOT NULL DEFAULT 0,
              language TEXT,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE episode (
              id INTEGER PRIMARY KEY,
              podcast_id INTEGER NOT NULL REFERENCES podcast(id) ON DELETE CASCADE,
              guid TEXT NOT NULL UNIQUE,
              title TEXT NOT NULL,
              description TEXT,
              audio_url TEXT,
              publish_at TEXT,
              duration_ms INTEGER,
              episode_image_path TEXT,
              status TEXT NOT NULL DEFAULT 'new',
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            "#,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO podcast (title, feed_url, created_at, updated_at) VALUES ('P','https://old.example/feed', 't','t')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO episode (podcast_id, guid, title, status, created_at, updated_at)
             VALUES (1, 'g1', 'E1', 'new', 't', 't')",
            [],
        )
        .unwrap();
        initialize(&conn).unwrap();
        let other = upsert_podcast(
            &conn,
            "https://other.example/feed.xml",
            "Other",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let id = upsert_episode(
            &conn,
            other,
            "g1",
            "Same guid other show",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        assert!(id > 0);
        assert_eq!(get_episode(&conn, 1).unwrap().title, "E1");
    }

    #[test]
    fn stores_mime_and_lists_upnp_tree() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let pid = upsert_podcast(
            &conn,
            "https://example.com/feed.xml",
            "Show",
            Some("Host"),
            None,
            None,
            None,
        )
        .unwrap();
        let eid = upsert_episode(
            &conn,
            pid,
            "g",
            "Ep",
            None,
            Some("https://example.com/a.mp3"),
            None,
            Some(90_000),
        )
        .unwrap();
        upsert_download_state(
            &conn,
            eid,
            "done",
            Some("/data/media/podcasts/1/1.mp3"),
            Some(12),
            Some("ab"),
            None,
            Some("t"),
            Some("audio/mpeg"),
        )
        .unwrap();
        let d = get_download(&conn, eid).unwrap().unwrap();
        assert_eq!(d.mime.as_deref(), Some("audio/mpeg"));
        let shows = list_upnp_podcasts(&conn).unwrap();
        assert_eq!(shows.len(), 1);
        assert_eq!(shows[0].3, 1);
        let eps = list_upnp_episodes(&conn, pid).unwrap();
        assert_eq!(eps[0].episode_title, "Ep");
        assert_eq!(eps[0].mime.as_deref(), Some("audio/mpeg"));
    }

    #[test]
    fn backfills_mime_from_path_and_rewrites_legacy_paths() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(media.join("podcasts")).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let pid = upsert_podcast(&conn, "https://e/x", "S", None, None, None, None).unwrap();
        let eid = upsert_episode(&conn, pid, "g", "E", None, None, None, None).unwrap();
        let legacy = media.join("1").join("1.m4a");
        upsert_download_state(
            &conn,
            eid,
            "done",
            Some(&legacy.display().to_string()),
            Some(1),
            None,
            None,
            Some("t"),
            None,
        )
        .unwrap();
        assert_eq!(rewrite_legacy_download_paths(&conn, &media).unwrap(), 1);
        let d = get_download(&conn, eid).unwrap().unwrap();
        assert!(d.file_path.unwrap().contains("podcasts"));
        assert_eq!(backfill_download_mime(&conn).unwrap(), 1);
        assert_eq!(
            get_download(&conn, eid).unwrap().unwrap().mime.as_deref(),
            Some("audio/mp4")
        );
    }

    #[test]
    fn rewrite_skips_when_legacy_file_was_not_moved() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        let dest_dir = media.join("podcasts").join("1");
        std::fs::create_dir_all(media.join("1")).unwrap();
        std::fs::create_dir_all(&dest_dir).unwrap();
        let legacy = media.join("1").join("1.mp3");
        std::fs::write(&legacy, b"OLD").unwrap();
        std::fs::write(dest_dir.join("1.mp3"), b"NEW").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let pid = upsert_podcast(&conn, "https://e/x", "S", None, None, None, None).unwrap();
        let eid = upsert_episode(&conn, pid, "g", "E", None, None, None, None).unwrap();
        upsert_download_state(
            &conn,
            eid,
            "done",
            Some(&legacy.display().to_string()),
            Some(1),
            None,
            None,
            Some("t"),
            Some("audio/mpeg"),
        )
        .unwrap();
        assert_eq!(rewrite_legacy_download_paths(&conn, &media).unwrap(), 0);
        let kept = get_download(&conn, eid)
            .unwrap()
            .unwrap()
            .file_path
            .unwrap();
        assert_eq!(kept, legacy.display().to_string());
        assert!(!kept.contains("podcasts"));
    }

    #[test]
    fn upnp_settings_roundtrip() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let s = upnp_settings(&conn).unwrap();
        assert_eq!(s.port, 8200);
        assert!(s.bind.is_empty());
        save_upnp_settings(
            &conn,
            &crate::models::UpnpSettings {
                bind: "192.168.1.20".into(),
                port: 8201,
            },
        )
        .unwrap();
        let s = upnp_settings(&conn).unwrap();
        assert_eq!(s.bind, "192.168.1.20");
        assert_eq!(s.port, 8201);
        let u1 = ensure_upnp_uuid(&conn).unwrap();
        let u2 = ensure_upnp_uuid(&conn).unwrap();
        assert_eq!(u1, u2);
    }

    #[test]
    fn audio_engine_settings_default_native() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let s = audio_engine_settings(&conn).unwrap();
        assert_eq!(s.backend, "native");
        assert!(!s.exclusive);
        save_audio_engine_settings(
            &conn,
            &crate::models::AudioEngineSettings {
                backend: "webview".into(),
                output_api: "asio".into(),
                device_name: "FiiO ASIO Driver".into(),
                exclusive: true,
                skip_silence: true,
            },
        )
        .unwrap();
        let s = audio_engine_settings(&conn).unwrap();
        assert_eq!(s.backend, "webview");
        assert_eq!(s.output_api, "asio");
        assert_eq!(s.device_name, "FiiO ASIO Driver");
        assert!(s.exclusive);
        assert!(s.skip_silence);
    }

    #[test]
    fn music_tables_are_separate_from_episodes() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let album = upsert_music_album(&conn, "Kind of Blue", "Miles Davis").unwrap();
        let tid = insert_music_track_placeholder(
            &conn,
            album,
            "/tmp/source.flac",
            "So What",
            "Miles Davis",
            "Kind of Blue",
            Some(1),
            Some(5000),
            "flac",
            Some("FLAC"),
            Some(96000),
            Some(24),
            Some(2),
        )
        .unwrap();
        set_music_track_file(&conn, tid, "/media/music/1/1.flac", Some(12)).unwrap();
        let albums = list_music_albums(&conn).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].track_count, 1);
        let eps: i64 = conn
            .query_row("SELECT COUNT(*) FROM episode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(eps, 0);
        let next = next_music_track(&conn, tid).unwrap();
        assert!(next.is_none());
    }

    #[test]
    fn next_music_track_uses_nocase_title_order() {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        let album = upsert_music_album(&conn, "Untit", "A").unwrap();
        let mut ids = Vec::new();
        for title in ["alpha", "Beta", "gamma"] {
            ids.push(
                insert_music_track_placeholder(
                    &conn,
                    album,
                    &format!("/tmp/{title}.flac"),
                    title,
                    "A",
                    "Untit",
                    None,
                    Some(1000),
                    "flac",
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap(),
            );
        }
        // NOCASE order: alpha, Beta, gamma. Binary `title > 'alpha'` would skip Beta.
        let nxt = next_music_track(&conn, ids[0]).unwrap().unwrap();
        assert_eq!(nxt.title, "Beta");
        let nxt = next_music_track(&conn, nxt.id).unwrap().unwrap();
        assert_eq!(nxt.title, "gamma");
    }

    fn open_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        initialize(&conn).unwrap();
        conn
    }

    fn add_show(conn: &Connection, url: &str, title: &str) -> (i64, i64) {
        let pid = upsert_podcast(conn, url, title, None, None, None, None).unwrap();
        let eid = upsert_episode(
            conn,
            pid,
            "ep",
            "Avsnitt",
            None,
            Some("https://cdn.example/ep.mp3"),
            None,
            Some(1_000),
        )
        .unwrap();
        (pid, eid)
    }

    fn remember_file(conn: &Connection, episode_id: i64, path: &std::path::Path) {
        upsert_download_state(
            conn,
            episode_id,
            "done",
            Some(&path.display().to_string()),
            Some(4),
            None,
            None,
            Some("t"),
            Some("audio/mpeg"),
        )
        .unwrap();
    }

    #[test]
    fn unsubscribe_keeps_local_files_when_not_requested() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://keep.example/feed.xml", "Behåll");
        let (other_id, other_ep) = add_show(&conn, "https://other.example/feed.xml", "Annan");
        let show = media.join("podcasts").join(pid.to_string());
        let own = show.join(format!("{eid}.mp3"));
        let cover = show.join("cover.jpg");
        std::fs::create_dir_all(&show).unwrap();
        std::fs::write(&own, b"audio").unwrap();
        std::fs::write(&cover, b"jpeg").unwrap();
        remember_file(&conn, eid, &own);
        add_to_queue(&conn, eid).unwrap();
        let other_file = media
            .join("podcasts")
            .join(other_id.to_string())
            .join(format!("{other_ep}.mp3"));
        std::fs::create_dir_all(other_file.parent().unwrap()).unwrap();
        std::fs::write(&other_file, b"other").unwrap();
        remember_file(&conn, other_ep, &other_file);
        add_to_queue(&conn, other_ep).unwrap();

        let outcome = unsubscribe_podcast(&conn, &media, pid, false).unwrap();
        assert_eq!(outcome.title, "Behåll");
        assert_eq!(outcome.files_removed, 0);
        assert!(outcome.warnings.is_empty());
        assert!(get_podcast(&conn, pid).is_err());
        assert!(list_episodes(&conn, pid).unwrap().is_empty());
        assert_eq!(std::fs::read(&own).unwrap(), b"audio");
        assert_eq!(std::fs::read(&cover).unwrap(), b"jpeg");
        assert!(show.is_dir());
        assert_eq!(get_podcast(&conn, other_id).unwrap().title, "Annan");
        assert_eq!(std::fs::read(&other_file).unwrap(), b"other");
        let queue = list_queue(&conn).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].episode_id, other_ep);
    }

    #[test]
    fn unsubscribe_deletes_show_files_when_requested() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://drop.example/feed.xml", "Ta bort");
        let (other_id, other_ep) = add_show(&conn, "https://stay.example/feed.xml", "Kvar");
        let show = media.join("podcasts").join(pid.to_string());
        let own = show.join(format!("{eid}.mp3"));
        let nested = show.join("extra").join("note.txt");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&own, b"audio").unwrap();
        std::fs::write(&nested, b"note").unwrap();
        remember_file(&conn, eid, &own);
        add_to_queue(&conn, eid).unwrap();
        let other_file = media
            .join("podcasts")
            .join(other_id.to_string())
            .join(format!("{other_ep}.mp3"));
        std::fs::create_dir_all(other_file.parent().unwrap()).unwrap();
        std::fs::write(&other_file, b"stay").unwrap();
        let music = media.join("music").join("1").join("1.flac");
        std::fs::create_dir_all(music.parent().unwrap()).unwrap();
        std::fs::write(&music, b"flac").unwrap();

        let outcome = unsubscribe_podcast(&conn, &media, pid, true).unwrap();
        assert_eq!(outcome.podcast_id, pid);
        assert!(outcome.files_removed >= 2);
        assert!(outcome.warnings.is_empty());
        assert!(get_podcast(&conn, pid).is_err());
        assert!(list_episodes(&conn, pid).unwrap().is_empty());
        assert!(list_queue(&conn).unwrap().is_empty());
        assert!(!own.exists());
        assert!(!nested.exists());
        assert!(!show.exists());
        assert!(media.join("podcasts").is_dir());
        assert_eq!(std::fs::read(&other_file).unwrap(), b"stay");
        assert_eq!(std::fs::read(&music).unwrap(), b"flac");
        assert_eq!(get_podcast(&conn, other_id).unwrap().title, "Kvar");
    }

    #[test]
    fn unsubscribe_delete_cannot_escape_show_media_dir() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://one.example/feed.xml", "Ett");
        let (sibling_id, sibling_ep) = add_show(&conn, "https://two.example/feed.xml", "Två");
        let (prefixed_id, _) = add_show(&conn, "https://twelve.example/feed.xml", "Tolv");
        assert_ne!(prefixed_id.to_string(), pid.to_string());

        let show = media.join("podcasts").join(pid.to_string());
        let own = show.join("own.mp3");
        std::fs::create_dir_all(&show).unwrap();
        std::fs::write(&own, b"own").unwrap();

        let sibling = media
            .join("podcasts")
            .join(sibling_id.to_string())
            .join(format!("{sibling_ep}.mp3"));
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, b"sibling").unwrap();
        remember_file(&conn, eid, &sibling);

        let prefixed = media
            .join("podcasts")
            .join(format!("{pid}0"))
            .join("keep.mp3");
        assert_ne!(prefixed_id.to_string(), format!("{pid}0"));
        std::fs::create_dir_all(prefixed.parent().unwrap()).unwrap();
        std::fs::write(&prefixed, b"prefix").unwrap();

        let secret = dir.path().join("secret.mp3");
        std::fs::write(&secret, b"secret").unwrap();
        let poison_ep = upsert_episode(
            &conn,
            pid,
            "poison",
            "Giftig sökväg",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let escaped = show.join("..").join("..").join("secret.mp3");
        remember_file(&conn, poison_ep, &escaped);

        let oops = media.join("podcasts").join("oops.mp3");
        std::fs::write(&oops, b"oops").unwrap();
        let music = media.join("music").join("album.flac");
        std::fs::write(&music, b"music").unwrap();

        let outcome = unsubscribe_podcast(&conn, &media, pid, true).unwrap();
        assert!(outcome.files_removed >= 1);
        assert!(!own.exists());
        assert!(!show.exists());
        assert_eq!(std::fs::read(&sibling).unwrap(), b"sibling");
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert_eq!(std::fs::read(&oops).unwrap(), b"oops");
        assert_eq!(std::fs::read(&music).unwrap(), b"music");
        assert_eq!(std::fs::read(&prefixed).unwrap(), b"prefix");
        assert_eq!(get_podcast(&conn, sibling_id).unwrap().title, "Två");
        assert_eq!(get_podcast(&conn, prefixed_id).unwrap().title, "Tolv");
        assert!(get_podcast(&conn, pid).is_err());
        assert!(media.join("podcasts").is_dir());
        assert!(media.join("music").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn unsubscribe_delete_does_not_follow_symlinks_outside_show_dir() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://link.example/feed.xml", "Länk");
        let show = media.join("podcasts").join(pid.to_string());
        std::fs::create_dir_all(&show).unwrap();
        let own = show.join(format!("{eid}.mp3"));
        std::fs::write(&own, b"own").unwrap();
        remember_file(&conn, eid, &own);

        let outside = dir.path().join("outside.mp3");
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, show.join("escape.mp3")).unwrap();

        let outside_dir = dir.path().join("outside-dir");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let kept = outside_dir.join("keep.mp3");
        std::fs::write(&kept, b"keep").unwrap();
        symlink(&outside_dir, show.join("escape-dir")).unwrap();

        let outcome = unsubscribe_podcast(&conn, &media, pid, true).unwrap();
        assert!(outcome.warnings.is_empty());
        assert!(!show.exists());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        assert_eq!(std::fs::read(&kept).unwrap(), b"keep");
        assert!(outside_dir.is_dir());

        let (linked_id, _) = add_show(&conn, "https://linked.example/feed.xml", "Symlänkad");
        let linked_target = dir.path().join("linked-target");
        let linked_file = linked_target.join("keep.mp3");
        std::fs::create_dir_all(&linked_target).unwrap();
        std::fs::write(&linked_file, b"linked").unwrap();
        let linked_show = media.join("podcasts").join(linked_id.to_string());
        symlink(&linked_target, &linked_show).unwrap();
        let linked = unsubscribe_podcast(&conn, &media, linked_id, true).unwrap();
        assert!(linked.warnings.iter().any(|w| w.contains("symlänk")));
        assert_eq!(std::fs::read(&linked_file).unwrap(), b"linked");
        assert!(linked_target.is_dir());
        assert!(!linked_show.exists());
        assert!(get_podcast(&conn, linked_id).is_err());

        let (root_id, _) = add_show(&conn, "https://rootlink.example/feed.xml", "Rot");
        let parked = dir.path().join("parked-podcasts");
        std::fs::rename(media.join("podcasts"), &parked).unwrap();
        symlink(&parked, media.join("podcasts")).unwrap();
        let err = unsubscribe_podcast(&conn, &media, root_id, true).unwrap_err();
        assert!(err.to_string().contains("vanlig katalog"));
        assert_eq!(get_podcast(&conn, root_id).unwrap().title, "Rot");
        assert!(parked.is_dir());
    }

    #[test]
    fn delete_episode_file_removes_only_that_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://drop.example/feed.xml", "Kvar");
        let sibling = upsert_episode(
            &conn,
            pid,
            "other",
            "Syskon",
            None,
            Some("https://cdn.example/other.mp3"),
            None,
            Some(1_000),
        )
        .unwrap();
        let show = media.join("podcasts").join(pid.to_string());
        let own = show.join(format!("{eid}.mp3"));
        let part = show.join(format!("{eid}.part"));
        let other = show.join(format!("{sibling}.mp3"));
        let cover = show.join("cover.jpg");
        let prefixed = show.join(format!("{eid}0.mp3"));
        std::fs::create_dir_all(&show).unwrap();
        std::fs::write(&own, b"own").unwrap();
        std::fs::write(&part, b"part").unwrap();
        std::fs::write(&other, b"other").unwrap();
        std::fs::write(&cover, b"jpeg").unwrap();
        std::fs::write(&prefixed, b"prefix").unwrap();
        remember_file(&conn, eid, &own);
        set_episode_status(&conn, eid, "downloaded").unwrap();
        remember_file(&conn, sibling, &other);
        set_episode_status(&conn, sibling, "downloaded").unwrap();
        let music = media.join("music").join("1.flac");
        std::fs::write(&music, b"flac").unwrap();

        let outcome = delete_episode_local_file(&conn, &media, eid).unwrap();
        assert_eq!(outcome.episode_id, eid);
        assert_eq!(outcome.podcast_id, pid);
        assert_eq!(outcome.files_removed, 2);
        assert!(outcome.warnings.is_empty());
        assert!(!own.exists());
        assert!(!part.exists());
        assert_eq!(std::fs::read(&other).unwrap(), b"other");
        assert_eq!(std::fs::read(&cover).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(&prefixed).unwrap(), b"prefix");
        assert_eq!(std::fs::read(&music).unwrap(), b"flac");
        assert!(show.is_dir());
        assert!(get_download(&conn, sibling)
            .unwrap()
            .unwrap()
            .file_path
            .is_some());
    }

    #[test]
    fn delete_episode_file_keeps_episode_row() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://keep.example/feed.xml", "Prenumererad");
        let show = media.join("podcasts").join(pid.to_string());
        let own = show.join(format!("{eid}.mp3"));
        std::fs::create_dir_all(&show).unwrap();
        std::fs::write(&own, b"own").unwrap();
        remember_file(&conn, eid, &own);
        set_episode_status(&conn, eid, "downloaded").unwrap();
        add_to_queue(&conn, eid).unwrap();
        save_playback(
            &conn,
            &PlaybackState {
                episode_id: eid,
                position_ms: 1500,
                speed: 1.0,
                completed: false,
                last_played_at: None,
            },
        )
        .unwrap();

        let outcome = delete_episode_local_file(&conn, &media, eid).unwrap();
        assert_eq!(outcome.title, "Avsnitt");
        assert_eq!(outcome.files_removed, 1);
        assert!(!own.exists());
        let episode = get_episode(&conn, eid).unwrap();
        assert_eq!(episode.podcast_id, pid);
        assert_eq!(episode.title, "Avsnitt");
        assert_eq!(episode.status, "new");
        assert_eq!(
            episode.audio_url.as_deref(),
            Some("https://cdn.example/ep.mp3")
        );
        assert!(get_download(&conn, eid).unwrap().is_none());
        assert!(!download_is_done(&conn, eid));
        assert_eq!(get_podcast(&conn, pid).unwrap().title, "Prenumererad");
        assert_eq!(list_episodes(&conn, pid).unwrap().len(), 1);
        assert_eq!(list_podcasts(&conn).unwrap().len(), 1);
        let queue = list_queue(&conn).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].episode_id, eid);
        assert_eq!(get_playback(&conn, eid).unwrap().unwrap().position_ms, 1500);
    }

    #[test]
    fn delete_episode_file_ignores_stored_path_outside_show() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://one.example/feed.xml", "Ett");
        let (other_id, other_ep) = add_show(&conn, "https://two.example/feed.xml", "Två");
        let show = media.join("podcasts").join(pid.to_string());
        let own = show.join(format!("{eid}.mp3"));
        std::fs::create_dir_all(&show).unwrap();
        std::fs::write(&own, b"own").unwrap();
        set_episode_status(&conn, eid, "downloaded").unwrap();

        let secret = dir.path().join("secret.mp3");
        std::fs::write(&secret, b"secret").unwrap();
        let escaped = show.join("..").join("..").join("secret.mp3");
        remember_file(&conn, eid, &escaped);

        let sibling = media
            .join("podcasts")
            .join(other_id.to_string())
            .join(format!("{other_ep}.mp3"));
        std::fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        std::fs::write(&sibling, b"sibling").unwrap();
        remember_file(&conn, other_ep, &sibling);
        let same_name = media.join("podcasts").join("oops.mp3");
        std::fs::write(&same_name, b"oops").unwrap();
        let music = media.join("music").join("album.flac");
        std::fs::write(&music, b"music").unwrap();

        let outcome = delete_episode_local_file(&conn, &media, eid).unwrap();
        assert_eq!(outcome.files_removed, 1);
        assert!(outcome.warnings.iter().any(|w| w.contains("utanför")));
        assert!(!own.exists());
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert_eq!(std::fs::read(&sibling).unwrap(), b"sibling");
        assert_eq!(std::fs::read(&same_name).unwrap(), b"oops");
        assert_eq!(std::fs::read(&music).unwrap(), b"music");
        assert!(show.is_dir());
        assert_eq!(get_episode(&conn, eid).unwrap().status, "new");
        assert!(get_download(&conn, eid).unwrap().is_none());
        assert_eq!(get_podcast(&conn, pid).unwrap().title, "Ett");
        assert_eq!(get_podcast(&conn, other_id).unwrap().title, "Två");
        assert!(get_download(&conn, other_ep).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn delete_episode_file_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        let (pid, eid) = add_show(&conn, "https://link.example/feed.xml", "Länk");
        let show = media.join("podcasts").join(pid.to_string());
        std::fs::create_dir_all(&show).unwrap();
        let outside = dir.path().join("outside.mp3");
        std::fs::write(&outside, b"outside").unwrap();
        let link = show.join(format!("{eid}.mp3"));
        symlink(&outside, &link).unwrap();
        remember_file(&conn, eid, &link);
        set_episode_status(&conn, eid, "downloaded").unwrap();

        let outcome = delete_episode_local_file(&conn, &media, eid).unwrap();
        assert_eq!(outcome.files_removed, 1);
        assert!(outcome.warnings.is_empty());
        assert!(!link.exists());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        assert!(get_download(&conn, eid).unwrap().is_none());
        assert_eq!(get_episode(&conn, eid).unwrap().title, "Avsnitt");

        let (linked_id, linked_ep) =
            add_show(&conn, "https://linked.example/feed.xml", "Symlänkad");
        let linked_target = dir.path().join("linked-target");
        let linked_file = linked_target.join(format!("{linked_ep}.mp3"));
        std::fs::create_dir_all(&linked_target).unwrap();
        std::fs::write(&linked_file, b"linked").unwrap();
        let linked_show = media.join("podcasts").join(linked_id.to_string());
        symlink(&linked_target, &linked_show).unwrap();
        remember_file(&conn, linked_ep, &linked_file);
        set_episode_status(&conn, linked_ep, "downloaded").unwrap();
        let linked = delete_episode_local_file(&conn, &media, linked_ep).unwrap();
        assert_eq!(linked.files_removed, 0);
        assert!(linked.warnings.iter().any(|w| w.contains("symlänk")));
        assert_eq!(std::fs::read(&linked_file).unwrap(), b"linked");
        assert!(linked_show
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(get_episode(&conn, linked_ep).unwrap().status, "downloaded");
        assert!(get_download(&conn, linked_ep).unwrap().is_some());
        assert_eq!(get_podcast(&conn, linked_id).unwrap().title, "Symlänkad");
    }

    fn mark_played(conn: &Connection, episode_id: i64, completed: bool, at: Option<&str>) {
        save_playback(
            conn,
            &PlaybackState {
                episode_id,
                position_ms: if completed { 1_000 } else { 10 },
                speed: 1.0,
                completed,
                last_played_at: at.map(str::to_string),
            },
        )
        .unwrap();
    }

    fn place_downloaded(
        conn: &Connection,
        show: &std::path::Path,
        episode_id: i64,
    ) -> std::path::PathBuf {
        std::fs::create_dir_all(show).unwrap();
        let path = show.join(format!("{episode_id}.mp3"));
        std::fs::write(&path, b"audio").unwrap();
        remember_file(conn, episode_id, &path);
        set_episode_status(conn, episode_id, "downloaded").unwrap();
        path
    }

    #[test]
    fn finished_cleanup_defaults_off() {
        let conn = open_db();
        let settings = finished_cleanup_settings(&conn).unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.days, 7);

        set_setting(&conn, "delete_finished_enabled", "true").unwrap();
        set_setting(&conn, "delete_finished_days", "0").unwrap();
        let settings = finished_cleanup_settings(&conn).unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.days, 7);

        set_setting(&conn, "delete_finished_enabled", "1").unwrap();
        set_setting(&conn, "delete_finished_days", "9999").unwrap();
        let settings = finished_cleanup_settings(&conn).unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.days, 7);

        let rejected = save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: true,
                days: 0,
            },
        )
        .unwrap_err();
        assert!(rejected.to_string().contains("1 och 365"));
        assert!(save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: true,
                days: 366,
            },
        )
        .is_err());
        let settings = finished_cleanup_settings(&conn).unwrap();
        assert!(settings.enabled);
        assert_eq!(settings.days, 7);

        save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: true,
                days: 1,
            },
        )
        .unwrap();
        assert!(finished_cleanup_settings(&conn).unwrap().enabled);
        assert_eq!(finished_cleanup_settings(&conn).unwrap().days, 1);

        save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: false,
                days: 14,
            },
        )
        .unwrap();
        let settings = finished_cleanup_settings(&conn).unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.days, 14);

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let (pid, eid) = add_show(&conn, "https://off.example/feed.xml", "Av");
        let show = media.join("podcasts").join(pid.to_string());
        let file = place_downloaded(&conn, &show, eid);
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::days(30)).to_rfc3339();
        mark_played(&conn, eid, true, Some(&old));

        let outcome = purge_finished_episode_files(&conn, &media, now).unwrap();
        assert!(!outcome.enabled);
        assert_eq!(outcome.days, 14);
        assert_eq!(outcome.files_removed, 0);
        assert!(outcome.warnings.is_empty());
        assert_eq!(std::fs::read(&file).unwrap(), b"audio");
        assert_eq!(get_episode(&conn, eid).unwrap().status, "downloaded");
        assert!(get_download(&conn, eid)
            .unwrap()
            .unwrap()
            .file_path
            .is_some());
        assert!(get_playback(&conn, eid).unwrap().unwrap().completed);
    }

    #[test]
    fn purge_finished_deletes_only_files_older_than_n_days() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: true,
                days: 7,
            },
        )
        .unwrap();

        let (pid, old_id) = add_show(&conn, "https://age.example/feed.xml", "Ålder");
        let show = media.join("podcasts").join(pid.to_string());
        let now = chrono::Utc::now();
        let ago = |days: i64, extra_secs: i64| {
            (now - chrono::Duration::days(days) - chrono::Duration::seconds(extra_secs))
                .to_rfc3339()
        };
        let add = |guid: &str, title: &str| {
            upsert_episode(
                &conn,
                pid,
                guid,
                title,
                None,
                Some("https://cdn.example/ep.mp3"),
                None,
                Some(1_000),
            )
            .unwrap()
        };

        let exact_id = add("exact", "Exakt");
        let recent_id = add("recent", "Nyligen");
        let just_id = add("just", "Precis");
        let open_id = add("open", "Pågår");
        let notime_id = add("notime", "Utan tid");
        let bad_id = add("bad", "Dålig tid");

        let old_file = place_downloaded(&conn, &show, old_id);
        let exact_file = place_downloaded(&conn, &show, exact_id);
        let recent_file = place_downloaded(&conn, &show, recent_id);
        let just_file = place_downloaded(&conn, &show, just_id);
        let open_file = place_downloaded(&conn, &show, open_id);
        let notime_file = place_downloaded(&conn, &show, notime_id);
        let bad_file = place_downloaded(&conn, &show, bad_id);
        let cover = show.join("cover.jpg");
        std::fs::write(&cover, b"jpeg").unwrap();
        let music = media.join("music").join("done.flac");
        std::fs::write(&music, b"flac").unwrap();

        mark_played(&conn, old_id, true, Some(&ago(8, 0)));
        mark_played(&conn, exact_id, true, Some(&ago(7, 0)));
        mark_played(&conn, recent_id, true, Some(&ago(6, 0)));
        mark_played(&conn, just_id, true, Some(&ago(7, 1)));
        mark_played(&conn, open_id, false, Some(&ago(40, 0)));
        mark_played(&conn, notime_id, true, None);
        mark_played(&conn, bad_id, true, Some("inte-ett-datum"));

        let (other_pid, other_id) = add_show(&conn, "https://other-age.example/feed.xml", "Annan");
        let other_file = place_downloaded(
            &conn,
            &media.join("podcasts").join(other_pid.to_string()),
            other_id,
        );
        mark_played(&conn, other_id, false, Some(&ago(90, 0)));

        let outcome = purge_finished_episode_files(&conn, &media, now).unwrap();
        assert!(outcome.enabled);
        assert_eq!(outcome.days, 7);
        assert_eq!(outcome.files_removed, 2);
        assert!(outcome.warnings.is_empty());
        assert!(!old_file.exists());
        assert!(!just_file.exists());
        assert_eq!(std::fs::read(&exact_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&recent_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&open_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&notime_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&bad_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&cover).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(&music).unwrap(), b"flac");
        assert_eq!(std::fs::read(&other_file).unwrap(), b"audio");
        assert!(show.is_dir());
        let removed = get_episode(&conn, old_id).unwrap();
        assert_eq!(removed.title, "Avsnitt");
        assert_eq!(removed.status, "new");
        assert!(get_download(&conn, old_id).unwrap().is_none());
        assert!(get_playback(&conn, old_id).unwrap().unwrap().completed);
        assert_eq!(get_episode(&conn, exact_id).unwrap().status, "downloaded");
        assert_eq!(get_episode(&conn, open_id).unwrap().status, "downloaded");
        assert_eq!(get_podcast(&conn, pid).unwrap().title, "Ålder");
        assert_eq!(list_episodes(&conn, pid).unwrap().len(), 7);

        save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: true,
                days: 3,
            },
        )
        .unwrap();
        let tighter = purge_finished_episode_files(&conn, &media, now).unwrap();
        assert_eq!(tighter.days, 3);
        assert_eq!(tighter.files_removed, 2);
        assert!(!exact_file.exists());
        assert!(!recent_file.exists());
        assert_eq!(std::fs::read(&open_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&notime_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&bad_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&other_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&music).unwrap(), b"flac");
        assert_eq!(get_episode(&conn, open_id).unwrap().status, "downloaded");
    }

    #[test]
    fn purge_finished_keeps_paths_outside_the_episode_file() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: true,
                days: 7,
            },
        )
        .unwrap();
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::days(10)).to_rfc3339();

        let (pid, eid) = add_show(&conn, "https://path.example/feed.xml", "Sökväg");
        let show = media.join("podcasts").join(pid.to_string());
        let own = place_downloaded(&conn, &show, eid);
        let part = show.join(format!("{eid}.part"));
        std::fs::write(&part, b"part").unwrap();
        let exe = show.join(format!("{eid}.exe"));
        std::fs::write(&exe, b"exe").unwrap();
        let nested = show.join("extra").join(format!("{eid}.mp3"));
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, b"nested").unwrap();
        let cover = show.join("cover.jpg");
        std::fs::write(&cover, b"jpeg").unwrap();
        let secret = dir.path().join("secret.mp3");
        std::fs::write(&secret, b"secret").unwrap();
        let escaped = show.join("..").join("..").join("secret.mp3");
        remember_file(&conn, eid, &escaped);
        mark_played(&conn, eid, true, Some(&old));

        let sibling = upsert_episode(
            &conn,
            pid,
            "sibling",
            "Syskon",
            None,
            Some("https://cdn.example/other.mp3"),
            None,
            Some(1_000),
        )
        .unwrap();
        let sibling_file = place_downloaded(&conn, &show, sibling);
        mark_played(&conn, sibling, false, Some(&old));

        let (other_pid, other_id) = add_show(&conn, "https://decoy.example/feed.xml", "Lockbete");
        let other_show = media.join("podcasts").join(other_pid.to_string());
        std::fs::create_dir_all(&other_show).unwrap();
        let decoy = other_show.join(format!("{eid}.mp3"));
        std::fs::write(&decoy, b"decoy").unwrap();
        let other_file = place_downloaded(&conn, &other_show, other_id);
        mark_played(&conn, other_id, false, Some(&old));

        let (dir_pid, dir_eid) = add_show(&conn, "https://dir.example/feed.xml", "Katalog");
        let fake_dir = media
            .join("podcasts")
            .join(dir_pid.to_string())
            .join(format!("{dir_eid}.mp3"));
        std::fs::create_dir_all(&fake_dir).unwrap();
        let inside = fake_dir.join("inside.txt");
        std::fs::write(&inside, b"inside").unwrap();
        remember_file(&conn, dir_eid, &fake_dir);
        set_episode_status(&conn, dir_eid, "downloaded").unwrap();
        mark_played(&conn, dir_eid, true, Some(&old));

        let music = media.join("music").join("album.flac");
        std::fs::write(&music, b"music").unwrap();

        let outcome = purge_finished_episode_files(&conn, &media, now).unwrap();
        assert_eq!(outcome.files_removed, 2);
        assert!(outcome.warnings.iter().any(|w| w.contains("utanför")));
        assert!(outcome.warnings.iter().any(|w| w.contains("katalog")));
        assert!(!own.exists());
        assert!(!part.exists());
        assert_eq!(std::fs::read(&exe).unwrap(), b"exe");
        assert_eq!(std::fs::read(&nested).unwrap(), b"nested");
        assert_eq!(std::fs::read(&cover).unwrap(), b"jpeg");
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
        assert_eq!(std::fs::read(&sibling_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&decoy).unwrap(), b"decoy");
        assert_eq!(std::fs::read(&other_file).unwrap(), b"audio");
        assert_eq!(std::fs::read(&inside).unwrap(), b"inside");
        assert_eq!(std::fs::read(&music).unwrap(), b"music");
        assert!(show.is_dir());
        assert!(fake_dir.is_dir());
        assert_eq!(get_episode(&conn, eid).unwrap().status, "new");
        assert!(get_download(&conn, eid).unwrap().is_none());
        assert_eq!(get_episode(&conn, dir_eid).unwrap().status, "downloaded");
        assert!(get_download(&conn, dir_eid).unwrap().is_some());
        assert_eq!(get_podcast(&conn, pid).unwrap().title, "Sökväg");
        assert_eq!(get_podcast(&conn, other_pid).unwrap().title, "Lockbete");
    }

    #[cfg(unix)]
    #[test]
    fn purge_finished_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let conn = open_db();
        save_finished_cleanup_settings(
            &conn,
            &FinishedCleanupSettings {
                enabled: true,
                days: 7,
            },
        )
        .unwrap();
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::days(10)).to_rfc3339();

        let (pid, eid) = add_show(&conn, "https://link-purge.example/feed.xml", "Länk");
        let show = media.join("podcasts").join(pid.to_string());
        std::fs::create_dir_all(&show).unwrap();
        let outside = dir.path().join("outside.mp3");
        std::fs::write(&outside, b"outside").unwrap();
        let link = show.join(format!("{eid}.mp3"));
        symlink(&outside, &link).unwrap();
        remember_file(&conn, eid, &link);
        set_episode_status(&conn, eid, "downloaded").unwrap();
        mark_played(&conn, eid, true, Some(&old));

        let outcome = purge_finished_episode_files(&conn, &media, now).unwrap();
        assert_eq!(outcome.files_removed, 1);
        assert!(outcome.warnings.is_empty());
        assert!(!link.exists());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        assert!(get_download(&conn, eid).unwrap().is_none());
        assert_eq!(get_episode(&conn, eid).unwrap().title, "Avsnitt");

        let (linked_id, linked_ep) =
            add_show(&conn, "https://linked-purge.example/feed.xml", "Symlänkad");
        let linked_target = dir.path().join("linked-target");
        let linked_file = linked_target.join(format!("{linked_ep}.mp3"));
        std::fs::create_dir_all(&linked_target).unwrap();
        std::fs::write(&linked_file, b"linked").unwrap();
        let linked_show = media.join("podcasts").join(linked_id.to_string());
        symlink(&linked_target, &linked_show).unwrap();
        remember_file(&conn, linked_ep, &linked_file);
        set_episode_status(&conn, linked_ep, "downloaded").unwrap();
        mark_played(&conn, linked_ep, true, Some(&old));

        let linked = purge_finished_episode_files(&conn, &media, now).unwrap();
        assert_eq!(linked.files_removed, 0);
        assert!(linked.warnings.iter().any(|w| w.contains("symlänk")));
        assert_eq!(std::fs::read(&linked_file).unwrap(), b"linked");
        assert!(linked_show
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(get_episode(&conn, linked_ep).unwrap().status, "downloaded");
        assert!(get_download(&conn, linked_ep).unwrap().is_some());
        assert_eq!(get_podcast(&conn, linked_id).unwrap().title, "Symlänkad");
    }

    fn inbox_titles(conn: &Connection) -> Vec<String> {
        let mut titles: Vec<String> = list_inbox(conn)
            .unwrap()
            .into_iter()
            .map(|item| item.episode.title)
            .collect();
        titles.sort();
        titles
    }

    #[test]
    fn new_subscription_inbox_only_includes_episodes_published_after() {
        let conn = open_db();
        let pid = upsert_podcast(
            &conn,
            "https://example.com/feed.xml",
            "Testpodd",
            None,
            None,
            None,
            Some("sv"),
        )
        .unwrap();
        let show = get_podcast(&conn, pid).unwrap();
        let cutoff = chrono::DateTime::parse_from_rfc3339(&show.subscribed_at).unwrap();
        assert_eq!(show.subscribed_at, show.created_at);
        let same = cutoff.to_rfc3339();
        let before = (cutoff - chrono::Duration::seconds(1)).to_rfc3339();
        let after = (cutoff + chrono::Duration::seconds(1)).to_rfc3339();
        let later = (cutoff + chrono::Duration::days(2)).to_rfc3339();
        let rfc_before = (cutoff - chrono::Duration::days(1)).to_rfc2822();
        let rfc_after = (cutoff + chrono::Duration::days(1)).to_rfc2822();
        let same_instant_other_offset =
            cutoff.with_timezone(&chrono::FixedOffset::east_opt(2 * 3600).unwrap());

        let add = |guid: &str, title: &str, publish: Option<&str>| {
            upsert_subscribed_episode(
                &conn,
                pid,
                guid,
                title,
                None,
                Some("https://cdn.example/a.mp3"),
                publish,
                Some(1_000),
            )
            .unwrap();
        };
        add("old", "Före", Some(&before));
        add("same", "Samma", Some(&same));
        add(
            "offset",
            "Samma ögonblick",
            Some(&same_instant_other_offset.to_rfc3339()),
        );
        add("new", "Efter", Some(&after));
        add("undated", "Utan datum", None);
        add("bad", "Ogiltigt datum", Some("inte-ett-datum"));
        add("rfc-old", "Rfc före", Some(&rfc_before));
        add("rfc-new", "Rfc efter", Some(&rfc_after));

        assert_eq!(
            inbox_titles(&conn),
            vec!["Efter".to_string(), "Rfc efter".to_string()]
        );
        assert_eq!(list_episodes(&conn, pid).unwrap().len(), 8);

        set_inbox(&conn, get_episode_by_guid(&conn, pid, "new"), false, false).unwrap();
        add("new", "Efter", Some(&after));
        add("old", "Före uppdaterad", Some(&before));
        add("later", "Nyare", Some(&later));

        assert_eq!(
            inbox_titles(&conn),
            vec!["Nyare".to_string(), "Rfc efter".to_string()]
        );
        assert_eq!(
            get_episode(&conn, get_episode_by_guid(&conn, pid, "old"))
                .unwrap()
                .title,
            "Före uppdaterad"
        );
        assert!(
            !get_episode(&conn, get_episode_by_guid(&conn, pid, "old"))
                .unwrap()
                .inbox
        );

        let cutoff_before_refresh = get_podcast(&conn, pid).unwrap().subscribed_at;
        upsert_podcast(
            &conn,
            "https://example.com/feed.xml",
            "Nytt namn från feeden",
            None,
            None,
            None,
            Some("sv"),
        )
        .unwrap();
        let refreshed = get_podcast(&conn, pid).unwrap();
        assert_eq!(refreshed.title, "Nytt namn från feeden");
        assert_eq!(refreshed.subscribed_at, cutoff_before_refresh);
        assert_eq!(
            inbox_titles(&conn),
            vec!["Nyare".to_string(), "Rfc efter".to_string()]
        );
    }

    fn get_episode_by_guid(conn: &Connection, podcast_id: i64, guid: &str) -> i64 {
        conn.query_row(
            "SELECT id FROM episode WHERE podcast_id = ?1 AND guid = ?2",
            params![podcast_id, guid],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn migration_drops_back_catalog_from_inbox_for_existing_feeds() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE podcast (
              id INTEGER PRIMARY KEY,
              guid TEXT,
              title TEXT NOT NULL,
              author TEXT,
              feed_url TEXT NOT NULL UNIQUE,
              artwork_path TEXT,
              description TEXT,
              auto_download INTEGER NOT NULL DEFAULT 0,
              speed REAL NOT NULL DEFAULT 1.0,
              intro_skip_sec INTEGER NOT NULL DEFAULT 0,
              outro_skip_sec INTEGER NOT NULL DEFAULT 0,
              skip_silence INTEGER NOT NULL DEFAULT 0,
              language TEXT,
              asr_language TEXT,
              skip_ads INTEGER NOT NULL DEFAULT 0,
              auto_transcribe INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE episode (
              id INTEGER PRIMARY KEY,
              podcast_id INTEGER NOT NULL REFERENCES podcast(id) ON DELETE CASCADE,
              guid TEXT NOT NULL,
              title TEXT NOT NULL,
              description TEXT,
              audio_url TEXT,
              publish_at TEXT,
              duration_ms INTEGER,
              episode_image_path TEXT,
              status TEXT NOT NULL DEFAULT 'new',
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              inbox INTEGER NOT NULL DEFAULT 1,
              archived INTEGER NOT NULL DEFAULT 0,
              UNIQUE(podcast_id, guid)
            );
            "#,
        )
        .unwrap();
        let subscribed = "2024-06-01T12:00:00+00:00";
        conn.execute(
            "INSERT INTO podcast (title, feed_url, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params!["Befintlig", "https://old.example/feed.xml", subscribed],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO podcast (title, feed_url, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params!["Importerade filer", SIDELOAD_FEED_URL, subscribed],
        )
        .unwrap();
        let insert_ep = |podcast_id: i64,
                         guid: &str,
                         title: &str,
                         publish: Option<&str>,
                         inbox: i64,
                         archived: i64| {
            conn.execute(
                "INSERT INTO episode (podcast_id, guid, title, publish_at, inbox, archived, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![podcast_id, guid, title, publish, inbox, archived, subscribed],
            )
            .unwrap();
        };
        insert_ep(1, "efter", "Efter", Some("2024-07-01T00:00:00+00:00"), 1, 0);
        insert_ep(1, "fore", "Före", Some("2024-05-01T00:00:00+00:00"), 1, 1);
        insert_ep(1, "samma", "Samma", Some(subscribed), 1, 0);
        insert_ep(1, "zulu", "Zulu före", Some("2024-06-01T11:30:00Z"), 1, 0);
        insert_ep(
            1,
            "offset",
            "Offset före",
            Some("2024-06-01T13:30:00+02:00"),
            1,
            0,
        );
        insert_ep(1, "utan", "Utan datum", None, 1, 0);
        insert_ep(
            1,
            "ogiltigt",
            "Ogiltigt datum",
            Some("inte-ett-datum"),
            1,
            0,
        );
        insert_ep(
            1,
            "rfc-efter",
            "Rfc efter",
            Some("Tue, 02 Jul 2024 15:00:00 +0000"),
            1,
            0,
        );
        insert_ep(
            1,
            "redan",
            "Redan borttagen",
            Some("2024-08-01T00:00:00+00:00"),
            0,
            0,
        );
        insert_ep(2, "lokal", "Lokal fil", None, 1, 0);

        initialize(&conn).unwrap();

        let show = get_podcast(&conn, 1).unwrap();
        assert_eq!(show.subscribed_at, subscribed);
        assert_eq!(
            inbox_titles(&conn),
            vec![
                "Efter".to_string(),
                "Lokal fil".to_string(),
                "Rfc efter".to_string()
            ]
        );
        let old = get_episode(&conn, get_episode_by_guid(&conn, 1, "fore")).unwrap();
        assert!(!old.inbox);
        assert!(old.archived);
        assert!(
            !get_episode(&conn, get_episode_by_guid(&conn, 1, "redan"))
                .unwrap()
                .inbox
        );
        assert!(
            get_episode(&conn, get_episode_by_guid(&conn, 2, "lokal"))
                .unwrap()
                .inbox
        );

        initialize(&conn).unwrap();
        assert_eq!(get_podcast(&conn, 1).unwrap().subscribed_at, subscribed);
        assert_eq!(
            inbox_titles(&conn),
            vec![
                "Efter".to_string(),
                "Lokal fil".to_string(),
                "Rfc efter".to_string()
            ]
        );

        upsert_subscribed_episode(
            &conn,
            1,
            "ny-efter",
            "Ny efter",
            None,
            Some("https://cdn.example/new.mp3"),
            Some("2024-09-01T00:00:00+00:00"),
            Some(1_000),
        )
        .unwrap();
        upsert_subscribed_episode(
            &conn,
            1,
            "ny-fore",
            "Ny före",
            None,
            Some("https://cdn.example/old.mp3"),
            Some("2024-01-01T00:00:00+00:00"),
            Some(1_000),
        )
        .unwrap();
        assert_eq!(
            inbox_titles(&conn),
            vec![
                "Efter".to_string(),
                "Lokal fil".to_string(),
                "Ny efter".to_string(),
                "Rfc efter".to_string()
            ]
        );
        assert!(
            !get_episode(&conn, get_episode_by_guid(&conn, 1, "ny-fore"))
                .unwrap()
                .inbox
        );
    }
}
