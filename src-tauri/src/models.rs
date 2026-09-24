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
    /// Set once, when the feed is first added. Inbox episodes must be published after this.
    pub subscribed_at: String,
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
pub struct AudioEngineSettings {
    /// `native` or `webview`
    pub backend: String,
    /// `auto` | `asio` | `wasapi_shared` | `wasapi_exclusive`
    pub output_api: String,
    /// Empty = prefer FiiO ASIO if present, else system default
    pub device_name: String,
    pub exclusive: bool,
    pub skip_silence: bool,
}

impl Default for AudioEngineSettings {
    fn default() -> Self {
        Self {
            backend: "native".into(),
            output_api: "auto".into(),
            device_name: String::new(),
            exclusive: false,
            skip_silence: false,
        }
    }
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
pub struct UnsubscribeOutcome {
    pub podcast_id: i64,
    pub title: String,
    pub files_removed: i64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteEpisodeFileOutcome {
    pub episode_id: i64,
    pub podcast_id: i64,
    pub title: String,
    pub files_removed: i64,
    pub warnings: Vec<String>,
}

/// Optional cleanup of local files for episodes whose playback reached the end.
/// Off unless the user turns it on. `days` is only used when `enabled` is true.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishedCleanupSettings {
    pub enabled: bool,
    pub days: i64,
}

impl Default for FinishedCleanupSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            days: 7,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishedCleanupOutcome {
    pub enabled: bool,
    pub days: i64,
    pub files_removed: i64,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportOutcome {
    pub podcasts: Vec<Podcast>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicAlbum {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub track_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicTrack {
    pub id: i64,
    pub album_id: i64,
    pub source_path: String,
    pub file_path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track_no: Option<i64>,
    pub duration_ms: Option<i64>,
    pub format: String,
    pub codec: Option<String>,
    pub sample_rate: Option<i64>,
    pub bits_per_sample: Option<i64>,
    pub channels: Option<i64>,
    pub bytes: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicPlaybackState {
    pub track_id: i64,
    pub position_ms: i64,
    pub completed: bool,
    pub last_played_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicPlayerPayload {
    pub track: MusicTrack,
    pub album: MusicAlbum,
    pub playback: Option<MusicPlaybackState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MusicImportOutcome {
    pub imported_tracks: i64,
    pub skipped_tracks: i64,
    pub failed_tracks: i64,
    pub album_count: i64,
    pub track_count: i64,
    pub errors: Vec<String>,
}
