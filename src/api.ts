import { invoke } from "@tauri-apps/api/core";
import type {
  AppPaths,
  Download,
  Episode,
  PlaybackState,
  PlayerPayload,
  Podcast,
  PodcastSettings,
  QueueItem,
} from "./types";

export const api = {
  appPaths: () => invoke<AppPaths>("app_paths"),
  listPodcasts: () => invoke<Podcast[]>("list_podcasts"),
  listEpisodes: (podcastId: number) =>
    invoke<Episode[]>("list_episodes", { podcastId }),
  getPodcast: (podcastId: number) =>
    invoke<Podcast>("get_podcast", { podcastId }),
  updatePodcastSettings: (podcastId: number, settings: PodcastSettings) =>
    invoke<Podcast>("update_podcast_settings", { podcastId, settings }),
  addFeed: (url: string) => invoke<Podcast>("add_feed", { url }),
  refreshFeed: (podcastId: number) =>
    invoke<Podcast>("refresh_feed", { podcastId }),
  importOpml: (xml: string) => invoke<Podcast[]>("import_opml", { xml }),
  importOpmlFile: (path: string) =>
    invoke<Podcast[]>("import_opml_file", { path }),
  exportOpml: () => invoke<string>("export_opml"),
  exportOpmlFile: (path: string) => invoke<void>("export_opml_file", { path }),
  downloadEpisode: (episodeId: number) =>
    invoke<Download>("download_episode", { episodeId }),
  getDownload: (episodeId: number) =>
    invoke<Download | null>("get_download", { episodeId }),
  savePlaybackState: (playback: PlaybackState) =>
    invoke<void>("save_playback_state", { playback }),
  getPlaybackState: (episodeId: number) =>
    invoke<PlaybackState | null>("get_playback_state", { episodeId }),
  listQueue: () => invoke<QueueItem[]>("list_queue"),
  enqueueEpisode: (episodeId: number) =>
    invoke<QueueItem[]>("enqueue_episode", { episodeId }),
  dequeueItem: (queueId: number) =>
    invoke<QueueItem[]>("dequeue_item", { queueId }),
  moveQueueItem: (queueId: number, direction: number) =>
    invoke<QueueItem[]>("move_queue_item", { queueId, direction }),
  episodeForPlayer: (episodeId: number) =>
    invoke<PlayerPayload>("episode_for_player", { episodeId }),
};
