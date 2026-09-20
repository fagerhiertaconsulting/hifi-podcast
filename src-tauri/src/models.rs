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
    pub asr_language: Option<String>,
    pub skip_ads: bool,
    pub auto_transcribe: bool,
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
    pub asr_language: Option<String>,
    pub skip_ads: bool,
    pub auto_transcribe: bool,
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
    pub inbox: bool,
    pub archived: bool,
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
    pub mime: Option<String>,
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
    pub models_dir: String,
    pub bin_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpnpSettings {
    pub bind: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpnpStatus {
    pub running: bool,
    pub bind: String,
    pub port: u16,
    pub location: Option<String>,
    pub lan_ip: Option<String>,
    pub uuid: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UpnpEpisode {
    pub episode_id: i64,
    pub episode_title: String,
    pub podcast_id: i64,
    pub podcast_title: String,
    pub author: Option<String>,
    pub duration_ms: Option<i64>,
    pub publish_at: Option<String>,
    pub file_path: String,
    pub bytes: Option<i64>,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transcript {
    pub id: i64,
    pub episode_id: i64,
    pub source: String,
    pub model: Option<String>,
    pub language: Option<String>,
    pub status: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: i64,
    pub transcript_id: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chapter {
    pub id: i64,
    pub episode_id: i64,
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub title: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdSegment {
    pub id: i64,
    pub episode_id: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub confidence: f64,
    pub method: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub id: i64,
    pub episode_id: i64,
    pub position_ms: i64,
    pub note: String,
    pub created_at: String,
    pub episode_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRow {
    pub id: i64,
    pub job_type: String,
    pub episode_id: i64,
    pub state: String,
    pub error_msg: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AsrStatus {
    pub byok_enabled: bool,
    pub model_name: String,
    pub model_present: bool,
    pub model_path: String,
    pub cli_present: bool,
    pub cli_path: Option<String>,
    pub cli_backend: String,
    pub nvidia_present: bool,
    pub hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryStats {
    pub podcast_count: i64,
    pub episode_count: i64,
    pub downloaded_count: i64,
    pub completed_count: i64,
    pub inbox_count: i64,
    pub bookmark_count: i64,
    pub listened_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxItem {
    pub episode: Episode,
    pub podcast_title: String,
    pub download_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscribeOutcome {
    pub podcast: Podcast,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportOutcome {
    pub podcasts: Vec<Podcast>,
    pub warnings: Vec<String>,
}
