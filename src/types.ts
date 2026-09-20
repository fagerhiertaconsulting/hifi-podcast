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

export type View = "library" | "inbox" | "queue" | "stats" | "settings";

export type SubscribeOutcome = {
  podcast: Podcast;
  warnings: string[];
};

export type ImportOutcome = {
  podcasts: Podcast[];
  warnings: string[];
};
