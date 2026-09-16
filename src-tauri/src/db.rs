use crate::error::AppResult;
use crate::models::{Download, Episode, PlaybackState, Podcast, PodcastSettings, QueueItem};
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
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS episode (
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
    Ok(())
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
        INSERT INTO podcast (guid, title, author, feed_url, description, language, created_at, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
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
                language, created_at, updated_at
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
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

pub fn get_podcast(conn: &Connection, id: i64) -> AppResult<Podcast> {
    conn.query_row(
        "SELECT id, guid, title, author, feed_url, artwork_path, description,
                auto_download, speed, intro_skip_sec, outro_skip_sec, skip_silence,
                language, created_at, updated_at
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
          updated_at = ?6
        WHERE id = ?7
        "#,
        params![
            settings.auto_download as i64,
            settings.speed,
            settings.intro_skip_sec,
            settings.outro_skip_sec,
            settings.skip_silence as i64,
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
        ON CONFLICT(guid) DO UPDATE SET
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
        "SELECT id FROM episode WHERE guid = ?1",
        params![guid],
        |row| row.get(0),
    )?;
    Ok(id)
}

pub fn list_episodes(conn: &Connection, podcast_id: i64) -> AppResult<Vec<Episode>> {
    let mut stmt = conn.prepare(
        "SELECT id, podcast_id, guid, title, description, audio_url, publish_at,
                duration_ms, episode_image_path, status, created_at, updated_at
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
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

pub fn get_episode(conn: &Connection, id: i64) -> AppResult<Episode> {
    conn.query_row(
        "SELECT id, podcast_id, guid, title, description, audio_url, publish_at,
                duration_ms, episode_image_path, status, created_at, updated_at
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
    }
}
