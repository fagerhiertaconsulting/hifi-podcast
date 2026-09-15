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
  created_at: string;
  updated_at: string;
};

export type PodcastSettings = {
  auto_download: boolean;
  speed: number;
  intro_skip_sec: number;
  outro_skip_sec: number;
  skip_silence: boolean;
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

export type PlayerPayload = {
  episode: Episode;
  podcast: Podcast;
  download: Download | null;
  playback: PlaybackState | null;
};

export type AppPaths = {
  db_path: string;
  media_root: string;
  data_dir: string;
};

export type View = "library" | "queue" | "settings";
