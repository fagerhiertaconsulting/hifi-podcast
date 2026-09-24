use crate::error::AppResult;
use crate::models::{
    AdSegment, Bookmark, Chapter, Download, Episode, InboxItem, JobRow, LibraryStats,
    MusicAlbum, MusicPlaybackState, MusicTrack, PlaybackState, Podcast, PodcastSettings, QueueItem,
    Transcript, TranscriptSegment,
};
use rusqlite::{params, Connection, OptionalExtension};

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
          updated_at TEXT NOT NULL
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
        INSERT INTO podcast (guid, title, author, feed_url, description, language, auto_transcribe, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?7)
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
                language, asr_language, skip_ads, auto_transcribe, created_at, updated_at
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
    })
}

pub fn get_podcast(conn: &Connection, id: i64) -> AppResult<Podcast> {
    conn.query_row(
        "SELECT id, guid, title, author, feed_url, artwork_path, description,
                auto_download, speed, intro_skip_sec, outro_skip_sec, skip_silence,
                language, asr_language, skip_ads, auto_transcribe, created_at, updated_at
         FROM podcast WHERE id = ?1",
        params![id],
        map_podcast,
    )
    .map_err(|_| crate::error::AppError::msg("Podcasten hittades inte."))
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
        "local://sideload",
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

pub fn music_track_by_source(conn: &Connection, source_path: &str) -> AppResult<Option<MusicTrack>> {
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

pub fn get_music_playback(conn: &Connection, track_id: i64) -> AppResult<Option<MusicPlaybackState>> {
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
            params![current.album_id, current.track_no, current.title, current.id],
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
}
