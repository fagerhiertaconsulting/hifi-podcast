use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Podcast {
    pub id: i64,
    pub guid: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub feed_url: String,
    pub artwork_path: Option<String>,
    pub description: Option<String>,
    pub auto_download: bool,
    pub speed: f64,
    pub intro_skip_sec: i64,
    pub outro_skip_sec: i64,
    pub skip_silence: bool,
    pub language: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PodcastSettings {
    pub auto_download: bool,
    pub speed: f64,
    pub intro_skip_sec: i64,
    pub outro_skip_sec: i64,
    pub skip_silence: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Episode {
    pub id: i64,
    pub podcast_id: i64,
    pub guid: String,
    pub title: String,
    pub description: Option<String>,
    pub audio_url: Option<String>,
    pub publish_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub episode_image_path: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: i64,
    pub episode_id: i64,
    pub file_path: Option<String>,
    pub bytes: Option<i64>,
    pub checksum: Option<String>,
    pub state: String,
    pub error_msg: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaybackState {
    pub episode_id: i64,
    pub position_ms: i64,
    pub speed: f64,
    pub completed: bool,
    pub last_played_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: i64,
    pub episode_id: i64,
    pub position: i64,
    pub added_at: String,
    pub episode_title: String,
    pub podcast_title: String,
    pub file_path: Option<String>,
    pub download_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppPaths {
    pub db_path: String,
    pub media_root: String,
    pub data_dir: String,
}
