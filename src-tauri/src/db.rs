use crate::error::AppResult;
use crate::models::{
    AdSegment, Bookmark, Chapter, Download, Episode, InboxItem, JobRow, LibraryStats, PlaybackState,
    Podcast, PodcastSettings, QueueItem, Transcript, TranscriptSegment,
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
    Ok(())
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
) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO download (episode_id, file_path, bytes, checksum, state, error_msg, completed_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(episode_id) DO UPDATE SET
          file_path = excluded.file_path,
          bytes = excluded.bytes,
          checksum = excluded.checksum,
          state = excluded.state,
          error_msg = excluded.error_msg,
          completed_at = excluded.completed_at
        "#,
        params![episode_id, file_path, bytes, checksum, state, error_msg, completed_at],
    )?;
    Ok(())
}

pub fn get_download(conn: &Connection, episode_id: i64) -> AppResult<Option<Download>> {
    let row = conn
        .query_row(
            "SELECT id, episode_id, file_path, bytes, checksum, state, error_msg, completed_at
             FROM download WHERE episode_id = ?1",
            params![episode_id],
            |row| {
                Ok(Download {
                    id: row.get(0)?,
                    episode_id: row.get(1)?,
                    file_path: row.get(2)?,
                    bytes: row.get(3)?,
                    checksum: row.get(4)?,
                    state: row.get(5)?,
                    error_msg: row.get(6)?,
                    completed_at: row.get(7)?,
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

pub fn enqueue_job(conn: &Connection, job_type: &str, episode_id: i64, priority: i64) -> AppResult<i64> {
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
        params![
            if ok { "done" } else { "error" },
            error,
            now_rfc3339(),
            id
        ],
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

pub fn replace_ads(conn: &Connection, episode_id: i64, ads: &[crate::intel::AdDraft]) -> AppResult<()> {
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

pub fn add_bookmark(conn: &Connection, episode_id: i64, position_ms: i64, note: &str) -> AppResult<()> {
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
        assert_eq!(
            get_playback(&conn, eid).unwrap().unwrap().position_ms,
            1234
        );
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
        let a = upsert_podcast(&conn, "https://a.example/feed.xml", "A", None, None, None, None)
            .unwrap();
        let b = upsert_podcast(&conn, "https://b.example/feed.xml", "B", None, None, None, None)
            .unwrap();
        let e1 = upsert_episode(&conn, a, "same-guid", "Ep A", None, None, None, None).unwrap();
        let e2 = upsert_episode(&conn, b, "same-guid", "Ep B", None, None, None, None).unwrap();
        assert_ne!(e1, e2);
        let e1b = upsert_episode(&conn, a, "same-guid", "Ep A uppdaterad", None, None, None, None)
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
        let id = upsert_episode(&conn, other, "g1", "Same guid other show", None, None, None, None)
            .unwrap();
        assert!(id > 0);
        assert_eq!(get_episode(&conn, 1).unwrap().title, "E1");
    }
}
