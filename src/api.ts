import { invoke } from "@tauri-apps/api/core";
import type {
  AdSegment,
  AppPaths,
  AsrStatus,
  Bookmark,
  Chapter,
  Download,
  Episode,
  ImportOutcome,
  InboxItem,
  JobRow,
  LibraryStats,
  PlaybackState,
  PlayerPayload,
  Podcast,
  PodcastSettings,
  QueueItem,
  SubscribeOutcome,
  DeleteEpisodeFileOutcome,
  FinishedCleanupOutcome,
  FinishedCleanupSettings,
  UnsubscribeOutcome,
  Transcript,
  TranscriptSegment,
  UpnpSettings,
  UpnpStatus,
  AudioEngineSettings,
  EngineCapabilities,
  EngineStatus,
  OutputDeviceInfo,
  ProbeInfo,
  MusicAlbum,
  MusicTrack,
  MusicPlayerPayload,
  MusicPlaybackState,
  MusicImportOutcome,
  OnlineMetadataOutcome,
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
  addFeed: (url: string) => invoke<SubscribeOutcome>("add_feed", { url }),
  refreshFeed: (podcastId: number) =>
    invoke<SubscribeOutcome>("refresh_feed", { podcastId }),
  unsubscribePodcast: (podcastId: number, deleteFiles: boolean) =>
    invoke<UnsubscribeOutcome>("unsubscribe_podcast", { podcastId, deleteFiles }),
  deleteEpisodeFile: (episodeId: number) =>
    invoke<DeleteEpisodeFileOutcome>("delete_episode_file", { episodeId }),
  getFinishedCleanupSettings: () =>
    invoke<FinishedCleanupSettings>("get_finished_cleanup_settings"),
  saveFinishedCleanupSettings: (settings: FinishedCleanupSettings) =>
    invoke<FinishedCleanupOutcome>("save_finished_cleanup_settings", { settings }),
  importOpml: (xml: string) => invoke<ImportOutcome>("import_opml", { xml }),
  importOpmlFile: (path: string) =>
    invoke<ImportOutcome>("import_opml_file", { path }),
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
  asrStatus: () => invoke<AsrStatus>("asr_status"),
  downloadAsrModel: () => invoke<AsrStatus>("download_asr_model"),
  downloadAsrCli: () => invoke<AsrStatus>("download_asr_cli"),
  downloadAsrSetup: () => invoke<AsrStatus>("download_asr_setup"),
  enqueueTranscribe: (episodeId: number) =>
    invoke<JobRow[]>("enqueue_transcribe", { episodeId }),
  listJobs: () => invoke<JobRow[]>("list_jobs"),
  getTranscript: (episodeId: number) =>
    invoke<Transcript | null>("get_transcript", { episodeId }),
  listSegments: (episodeId: number) =>
    invoke<TranscriptSegment[]>("list_segments", { episodeId }),
  searchTranscripts: (query: string) =>
    invoke<TranscriptSegment[]>("search_transcripts", { query }),
  listChapters: (episodeId: number) =>
    invoke<Chapter[]>("list_chapters", { episodeId }),
  listAds: (episodeId: number) => invoke<AdSegment[]>("list_ads", { episodeId }),
  addBookmark: (episodeId: number, positionMs: number, note: string) =>
    invoke<Bookmark[]>("add_bookmark", { episodeId, positionMs, note }),
  listBookmarks: () => invoke<Bookmark[]>("list_bookmarks"),
  deleteBookmark: (id: number) => invoke<Bookmark[]>("delete_bookmark", { id }),
  listInbox: () => invoke<InboxItem[]>("list_inbox"),
  triageEpisode: (episodeId: number, inbox: boolean, archived: boolean) =>
    invoke<void>("triage_episode", { episodeId, inbox, archived }),
  libraryStats: () => invoke<LibraryStats>("library_stats"),
  sideloadFile: (path: string) => invoke<Episode>("sideload_file", { path }),
  smtcNowPlaying: (title: string, artist: string, playing: boolean) =>
    invoke<void>("smtc_now_playing", { title, artist, playing }),
  upnpStatus: () => invoke<UpnpStatus>("upnp_status"),
  getUpnpSettings: () => invoke<UpnpSettings>("get_upnp_settings"),
  saveUpnpSettings: (settings: UpnpSettings) =>
    invoke<UpnpSettings>("save_upnp_settings", { settings }),
  startUpnpServer: () => invoke<UpnpStatus>("start_upnp_server"),
  stopUpnpServer: () => invoke<UpnpStatus>("stop_upnp_server"),
  engineCapabilities: () => invoke<EngineCapabilities>("engine_capabilities"),
  engineListDevices: () => invoke<OutputDeviceInfo[]>("engine_list_devices"),
  engineGetSettings: () => invoke<AudioEngineSettings>("engine_get_settings"),
  engineSaveSettings: (settings: AudioEngineSettings) =>
    invoke<AudioEngineSettings>("engine_save_settings", { settings }),
  engineProbe: (path: string) => invoke<ProbeInfo>("engine_probe", { path }),
  engineOpen: (args: {
    path: string;
    speed: number;
    skipSilence: boolean;
    outputApi?: string;
    deviceName?: string;
  }) =>
    invoke<EngineStatus>("engine_open", {
      path: args.path,
      speed: args.speed,
      skipSilence: args.skipSilence,
      outputApi: args.outputApi,
      deviceName: args.deviceName,
    }),
  enginePlay: () => invoke<void>("engine_play"),
  enginePause: () => invoke<void>("engine_pause"),
  engineSeek: (positionMs: number) => invoke<void>("engine_seek", { positionMs }),
  engineSetSpeed: (speed: number) => invoke<void>("engine_set_speed", { speed }),
  engineSetSkipSilence: (enabled: boolean) =>
    invoke<void>("engine_set_skip_silence", { enabled }),
  engineStop: () => invoke<void>("engine_stop"),
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  listMusicAlbums: () => invoke<MusicAlbum[]>("list_music_albums"),
  listMusicTracks: (albumId?: number | null) =>
    invoke<MusicTrack[]>("list_music_tracks", { albumId: albumId ?? null }),
  lastMusicImportRoot: () => invoke<string | null>("last_music_import_root"),
  importMusicFolder: (path: string) =>
    invoke<MusicImportOutcome>("import_music_folder", { path }),
  musicTrackForPlayer: (trackId: number) =>
    invoke<MusicPlayerPayload>("music_track_for_player", { trackId }),
  saveMusicPlayback: (playback: MusicPlaybackState) =>
    invoke<void>("save_music_playback", { playback }),
  nextMusicTrack: (trackId: number) =>
    invoke<MusicTrack | null>("next_music_track", { trackId }),
  getOnlineMetadataConsent: () => invoke<boolean>("get_online_metadata_consent"),
  saveOnlineMetadataConsent: (enabled: boolean) =>
    invoke<boolean>("save_online_metadata_consent", { enabled }),
  fetchAlbumOnlineMetadata: (albumId: number) =>
    invoke<OnlineMetadataOutcome>("fetch_album_online_metadata", { albumId }),
};
