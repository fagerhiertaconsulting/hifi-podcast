export type Podcast = {
  id: number;
  guid: string | null;
  title: string;
  author: string | null;
  feed_url: string;
  artwork_path: string | null;
  description: string | null;
  auto_download: boolean;
  speed: number;
  intro_skip_sec: number;
  outro_skip_sec: number;
  skip_silence: boolean;
  language: string | null;
  asr_language: string | null;
  skip_ads: boolean;
  auto_transcribe: boolean;
  created_at: string;
  updated_at: string;
};

export type PodcastSettings = {
  auto_download: boolean;
  speed: number;
  intro_skip_sec: number;
  outro_skip_sec: number;
  skip_silence: boolean;
  asr_language: string | null;
  skip_ads: boolean;
  auto_transcribe: boolean;
};

export type Episode = {
  id: number;
  podcast_id: number;
  guid: string;
  title: string;
  description: string | null;
  audio_url: string | null;
  publish_at: string | null;
  duration_ms: number | null;
  episode_image_path: string | null;
  status: string;
  inbox: boolean;
  archived: boolean;
  created_at: string;
  updated_at: string;
};

export type Download = {
  id: number;
  episode_id: number;
  file_path: string | null;
  bytes: number | null;
    checksum: string | null;
    mime: string | null;
    state: string;
  error_msg: string | null;
  completed_at: string | null;
};

export type PlaybackState = {
  episode_id: number;
  position_ms: number;
  speed: number;
  completed: boolean;
  last_played_at: string | null;
};

export type QueueItem = {
  id: number;
  episode_id: number;
  position: number;
  added_at: string;
  episode_title: string;
  podcast_title: string;
  file_path: string | null;
  download_state: string | null;
};

export type Chapter = {
  id: number;
  episode_id: number;
  start_ms: number;
  end_ms: number | null;
  title: string;
  source: string;
};

export type AdSegment = {
  id: number;
  episode_id: number;
  start_ms: number;
  end_ms: number;
  confidence: number;
  method: string;
  enabled: boolean;
};

export type TranscriptSegment = {
  id: number;
  transcript_id: number;
  start_ms: number;
  end_ms: number;
  text: string;
};

export type Transcript = {
  id: number;
  episode_id: number;
  source: string;
  model: string | null;
  language: string | null;
  status: string;
  created_at: string;
};

export type Bookmark = {
  id: number;
  episode_id: number;
  position_ms: number;
  note: string;
  created_at: string;
  episode_title: string;
};

export type JobRow = {
  id: number;
  job_type: string;
  episode_id: number;
  state: string;
  error_msg: string | null;
  created_at: string;
};

export type AsrStatus = {
  byok_enabled: boolean;
  model_name: string;
  model_present: boolean;
  model_path: string;
  cli_present: boolean;
  cli_path: string | null;
  cli_backend: string;
  nvidia_present: boolean;
  hint: string;
};

export type LibraryStats = {
  podcast_count: number;
  episode_count: number;
  downloaded_count: number;
  completed_count: number;
  inbox_count: number;
  bookmark_count: number;
  listened_ms: number;
};

export type InboxItem = {
  episode: Episode;
  podcast_title: string;
  download_state: string | null;
};

export type PlayerPayload = {
  episode: Episode;
  podcast: Podcast;
  download: Download | null;
  playback: PlaybackState | null;
  chapters: Chapter[];
  ads: AdSegment[];
};

export type AppPaths = {
  db_path: string;
  media_root: string;
  data_dir: string;
  models_dir: string;
  bin_dir: string;
};

export type UpnpSettings = {
  bind: string;
  port: number;
};

export type UpnpStatus = {
  running: boolean;
  bind: string;
  port: number;
  location: string | null;
  lan_ip: string | null;
  uuid: string;
  error: string | null;
};

export type AudioEngineSettings = {
  backend: "native" | "webview" | string;
  output_api: "auto" | "asio" | "wasapi_shared" | "wasapi_exclusive" | string;
  device_name: string;
  exclusive: boolean;
  skip_silence: boolean;
};

export type OutputDeviceInfo = {
  name: string;
  id: string;
  api: string;
  is_default: boolean;
  preferred: boolean;
  sample_rate: number;
  channels: number;
  exclusive_capable: boolean;
};

export type EngineCapabilities = {
  backend_default: string;
  pcm_mimes: string[];
  dsd_status_sv: string;
  asio_available: boolean;
  exclusive_available: boolean;
  asio_note_sv: string;
  exclusive_note_sv: string;
  stretch_note_sv: string;
};

export type EngineStatus = {
  loaded: boolean;
  playing: boolean;
  ended: boolean;
  position_ms: number;
  duration_ms: number;
  sample_rate: number;
  channels: number;
  codec: string;
  path: string;
  exclusive: boolean;
  output_api: string;
  device_name: string;
  output_rate: number;
  resampled: boolean;
  speed: number;
  skip_silence: boolean;
  underruns: number;
  error: string | null;
  format: string;
  dsd_rate: string;
  output_path: string;
  output_path_sv: string;
  callback_reentries: number;
  dsd_diag: string;
  file_bits: number;
  output_bits: number;
  output_sample: string;
  file_quality_sv: string;
  output_quality_sv: string;
  quality_delta: string[];
  quality_delta_sv: string;
};

export type ProbeInfo = {
  path: string;
  support: {
    kind: string;
    mime: string;
    extension: string;
    native_decode: boolean;
    dsd_plugin: boolean;
    label_sv: string;
    note_sv: string;
  };
  sample_rate: number | null;
  channels: number | null;
  duration_ms: number | null;
  codec: string | null;
  bits_per_sample: number | null;
};

export type View = "library" | "music" | "inbox" | "queue" | "stats" | "settings";

export type MusicAlbum = {
  id: number;
  title: string;
  artist: string;
  track_count: number;
  created_at: string;
  updated_at: string;
};

export type MusicTrack = {
  id: number;
  album_id: number;
  source_path: string;
  file_path: string;
  title: string;
  artist: string;
  album: string;
  track_no: number | null;
  duration_ms: number | null;
  format: string;
  codec: string | null;
  sample_rate: number | null;
  bits_per_sample: number | null;
  channels: number | null;
  bytes: number | null;
  created_at: string;
};

export type MusicPlaybackState = {
  track_id: number;
  position_ms: number;
  completed: boolean;
  last_played_at: string | null;
};

export type MusicPlayerPayload = {
  track: MusicTrack;
  album: MusicAlbum;
  playback: MusicPlaybackState | null;
};

export type MusicImportOutcome = {
  imported_tracks: number;
  skipped_tracks: number;
  album_count: number;
  track_count: number;
  warnings: string[];
};

export type SubscribeOutcome = {
  podcast: Podcast;
  warnings: string[];
};

export type ImportOutcome = {
  podcasts: Podcast[];
  warnings: string[];
};
