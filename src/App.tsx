import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import {
  attachSkipSilence,
  assetUrlForLocalFile,
  blobUrlForLocalFile,
  configureHiFiElement,
} from "./audio/engine";
import { DriveModeView } from "./DriveModeView";
import {
  DRIVE_LABELS,
  driveNextIndex,
  drivePreviousIndex,
  driveSkipSeconds,
  visibleChrome,
} from "./driveMode";
import {
  AsrSettings,
  InboxView,
  StatsView,
  TranscriptPane,
} from "./IntelligenceViews";
import type {
  AdSegment,
  AppPaths,
  AsrStatus,
  AudioEngineSettings,
  Bookmark,
  EngineStatus,
  Episode,
  InboxItem,
  JobRow,
  LibraryStats,
  MusicAlbum,
  MusicPlayerPayload,
  MusicTrack,
  PlayerPayload,
  Podcast,
  QueueItem,
  TranscriptSearchHit,
  View,
} from "./types";

const SPEEDS = [0.8, 1, 1.2, 1.5, 1.75, 2, 2.5, 3];

/** Same rule as `intel::skip_ads_seek_ms`: enabled spans, never `not_ad`. */
function adsToSkip(ads: AdSegment[]): AdSegment[] {
  return ads.filter((ad) => ad.enabled && ad.user_label !== "not_ad");
}

function formatTime(seconds: number) {
  if (!Number.isFinite(seconds) || seconds < 0) return "0:00";
  const s = Math.floor(seconds % 60)
    .toString()
    .padStart(2, "0");
  const m = Math.floor(seconds / 60) % 60;
  const h = Math.floor(seconds / 3600);
  return h > 0 ? `${h}:${m.toString().padStart(2, "0")}:${s}` : `${m}:${s}`;
}

function formatDate(iso: string | null) {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleDateString("sv-SE");
}

export default function App() {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const skipRef = useRef<ReturnType<typeof attachSkipSilence> | null>(null);
  const persistTimer = useRef<number | null>(null);
  const filePathRef = useRef<string | null>(null);
  const blobUrlRef = useRef<string | null>(null);
  const positionRef = useRef(0);
  const playingRef = useRef(false);
  const musicPayloadRef = useRef<MusicPlayerPayload | null>(null);
  const driveModeRef = useRef(false);
  const sessionPlayingRef = useRef<boolean | null>(null);
  const [driveMode, setDriveMode] = useState(() => {
    try {
      return localStorage.getItem("hifi-drive-mode") === "1";
    } catch {
      return false;
    }
  });
  const [view, setView] = useState<View>("library");
  useEffect(() => {
    try {
      localStorage.setItem("hifi-drive-mode", driveMode ? "1" : "0");
    } catch {
      /* visningsflagga på den här datorn */
    }
  }, [driveMode]);
  const [filter, setFilter] = useState("");
  const [onlyDownloaded, setOnlyDownloaded] = useState(false);
  const [inbox, setInbox] = useState<InboxItem[]>([]);
  const [bookmarks, setBookmarks] = useState<Bookmark[]>([]);
  const [jobs, setJobs] = useState<JobRow[]>([]);
  const [stats, setStats] = useState<LibraryStats | null>(null);
  const [asr, setAsr] = useState<AsrStatus | null>(null);
  const adsRef = useRef<AdSegment[]>([]);
  const [status, setStatus] = useState("Redo.");
  const [busy, setBusy] = useState(false);
  const [feedUrl, setFeedUrl] = useState("");
  const [opmlText, setOpmlText] = useState("");
  const [podcasts, setPodcasts] = useState<Podcast[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [episodes, setEpisodes] = useState<Episode[]>([]);
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [paths, setPaths] = useState<AppPaths | null>(null);
  const [payload, setPayload] = useState<PlayerPayload | null>(null);
  const [musicPayload, setMusicPayload] = useState<MusicPlayerPayload | null>(null);
  const [musicAlbums, setMusicAlbums] = useState<MusicAlbum[]>([]);
  const [musicTracks, setMusicTracks] = useState<MusicTrack[]>([]);
  const [selectedAlbumId, setSelectedAlbumId] = useState<number | null>(null);
  const [musicFilter, setMusicFilter] = useState("");
  const [playing, setPlaying] = useState(false);
  playingRef.current = playing;
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState(0);
  const [speed, setSpeed] = useState(1);
  const [globalSkipSilence, setGlobalSkipSilence] = useState(false);
  const [engineSettings, setEngineSettings] = useState<AudioEngineSettings | null>(null);
  const [engineQa, setEngineQa] = useState("");
  const [engineStatus, setEngineStatus] = useState<EngineStatus | null>(null);
  const [outputIsWebview, setOutputIsWebview] = useState(false);
  const nativeRef = useRef(true);
  const [sleepMode, setSleepMode] = useState<"off" | "minutes" | "episode">("off");
  const [sleepMinutes, setSleepMinutes] = useState(30);
  const [sleepUntil, setSleepUntil] = useState<number | null>(null);
  const outroFired = useRef(false);

  const selected = useMemo(
    () => podcasts.find((p) => p.id === selectedId) ?? null,
    [podcasts, selectedId],
  );

  const musicPlayingRef = useRef(false);
  musicPlayingRef.current = musicPayload != null;
  const skipSilenceActive =
    musicPayload == null &&
    (globalSkipSilence || Boolean(payload?.podcast.skip_silence));
  const skipEnabledRef = useRef(skipSilenceActive);
  skipEnabledRef.current = skipSilenceActive;
  const skipModeRef = useRef(skipSilenceActive);

  const refreshLibrary = useCallback(async () => {
    const [list, q, p, box, marks, jobList, st, asrStatus, audioSet] = await Promise.all([
      api.listPodcasts(),
      api.listQueue(),
      api.appPaths(),
      api.listInbox(),
      api.listBookmarks(),
      api.listJobs(),
      api.libraryStats(),
      api.asrStatus(),
      api.engineGetSettings(),
    ]);
    setPodcasts(list);
    setQueue(q);
    setPaths(p);
    setInbox(box);
    setBookmarks(marks);
    setJobs(jobList);
    setStats(st);
    setAsr(asrStatus);
    setEngineSettings(audioSet);
    nativeRef.current = audioSet.backend !== "webview";
    setGlobalSkipSilence(audioSet.skip_silence);
    if (list.length && selectedId === null) {
      setSelectedId(list[0].id);
    }
  }, [selectedId]);

  const refreshMusic = useCallback(async () => {
    const albums = await api.listMusicAlbums();
    setMusicAlbums(albums);
    setSelectedAlbumId((current) => {
      if (current != null && albums.some((a) => a.id === current)) return current;
      return albums.length ? albums[0].id : null;
    });
  }, []);

  const loadEpisodes = useCallback(async (podcastId: number) => {
    setEpisodes(await api.listEpisodes(podcastId));
  }, []);

  useEffect(() => {
    void refreshLibrary().catch((err) => setStatus(String(err)));
    void refreshMusic().catch((err) => setStatus(String(err)));
  }, [refreshLibrary, refreshMusic]);

  useEffect(() => {
    if (selectedAlbumId == null) {
      setMusicTracks([]);
      return;
    }
    void api.listMusicTracks(selectedAlbumId).then(setMusicTracks).catch((err) => setStatus(String(err)));
  }, [selectedAlbumId]);

  useEffect(() => {
    if (selectedId != null) {
      void loadEpisodes(selectedId).catch((err) => setStatus(String(err)));
    }
  }, [selectedId, loadEpisodes]);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;
    configureHiFiElement(audio, speed);
  }, [speed]);

  useEffect(() => {
    const un = listen("jobs-changed", () => {
      void refreshLibrary().catch((err) => setStatus(String(err)));
    });
    return () => {
      void un.then((fn) => fn());
    };
  }, [refreshLibrary]);

  useEffect(() => {
    const un = listen<{ refreshed: number; skipped: number; warnings: string[] }>(
      "feeds-refreshed",
      (event) => {
        void refreshLibrary().catch((err) => setStatus(String(err)));
        if (selectedId != null) {
          void loadEpisodes(selectedId).catch((err) => setStatus(String(err)));
        }
        const { refreshed, warnings } = event.payload;
        if (refreshed === 0 && warnings.length === 0) return;
        setStatus((current) => {
          if (
            current.startsWith("Spelar") ||
            current.startsWith("Musik") ||
            current.startsWith("Native") ||
            current.startsWith("Laddar")
          ) {
            return current;
          }
          if (warnings.length > 0) {
            return `Automatisk uppdatering: ${refreshed} flöden. Varningar: ${warnings.join(" ")}`;
          }
          return refreshed === 1
            ? "Flödet uppdaterades automatiskt."
            : `Flödena uppdaterades automatiskt (${refreshed}).`;
        });
      },
    );
    return () => {
      void un.then((fn) => fn());
    };
  }, [refreshLibrary, loadEpisodes, selectedId]);

  useEffect(() => {
    const active = jobs.some((j) => j.state === "queued" || j.state === "running");
    if (!active) return;
    const id = window.setInterval(() => {
      void refreshLibrary().catch((err) => setStatus(String(err)));
    }, 2500);
    return () => window.clearInterval(id);
  }, [jobs, refreshLibrary]);

  useEffect(() => {
    if (sleepMode !== "minutes" || !sleepUntil) return;
    const id = window.setInterval(() => {
      if (Date.now() >= sleepUntil) {
        if (nativeRef.current) void api.enginePause();
        else audioRef.current?.pause();
        setSleepMode("off");
        setSleepUntil(null);
        setStatus("Sovtimer: pausad.");
      }
    }, 1000);
    return () => window.clearInterval(id);
  }, [sleepMode, sleepUntil]);

  const loadPlaybackSource = useCallback(
    async (
      audio: HTMLAudioElement,
      filePath: string,
      useSkipSilence: boolean,
      speedValue: number,
    ) => {
      configureHiFiElement(audio, speedValue);
      // Once Web Audio is bound to this element it must keep a same-origin
      // (blob:) src. Never point a MediaElementSource at asset.localhost.
      const mustUseBlob = useSkipSilence || skipRef.current !== null;
      if (!mustUseBlob) {
        if (blobUrlRef.current) {
          URL.revokeObjectURL(blobUrlRef.current);
          blobUrlRef.current = null;
        }
        audio.src = assetUrlForLocalFile(filePath);
        return;
      }
      if (blobUrlRef.current) {
        URL.revokeObjectURL(blobUrlRef.current);
      }
      const blobUrl = await blobUrlForLocalFile(filePath);
      blobUrlRef.current = blobUrl;
      audio.src = blobUrl;
      if (!skipRef.current) {
        skipRef.current = attachSkipSilence(audio, () => skipEnabledRef.current);
      }
    },
    [],
  );

  useEffect(() => {
    if (musicPayload) {
      skipEnabledRef.current = false;
      skipModeRef.current = false;
      if (nativeRef.current) {
        void api.engineSetSkipSilence(false).catch((err) => setStatus(String(err)));
      }
      return;
    }
    if (skipModeRef.current === skipSilenceActive) return;
    skipModeRef.current = skipSilenceActive;
    const audio = audioRef.current;
    const path = filePathRef.current;
    if (!audio || !path) return;
    if (nativeRef.current) {
      void api.engineSetSkipSilence(skipSilenceActive).catch((err) => setStatus(String(err)));
      return;
    }
    const pos = positionRef.current;
    const wasPlaying = playingRef.current;
    void loadPlaybackSource(audio, path, skipSilenceActive, speed)
      .then(async () => {
        audio.currentTime = pos;
        if (wasPlaying) {
          await audio.play();
        }
      })
      .catch((err) => setStatus(String(err)));
  }, [skipSilenceActive, musicPayload, loadPlaybackSource, speed]);

  const persistNow = useCallback(async () => {
    const audio = audioRef.current;
    const posSec = nativeRef.current ? positionRef.current : (audio?.currentTime ?? positionRef.current);
    const dur = duration;
    const completed = dur > 0 && posSec >= dur - 2;
    const last = new Date().toISOString();
    if (musicPayload) {
      await api.saveMusicPlayback({
        track_id: musicPayload.track.id,
        position_ms: Math.floor(posSec * 1000),
        completed,
        last_played_at: last,
      });
      return;
    }
    const episode = payload?.episode;
    if (!episode) return;
    await api.savePlaybackState({
      episode_id: episode.id,
      position_ms: Math.floor(posSec * 1000),
      speed,
      completed,
      last_played_at: last,
    });
  }, [payload, musicPayload, speed, duration]);

  const playEpisode = useCallback(
    async (episodeId: number, enqueue = true, startMsOverride?: number) => {
      setBusy(true);
      try {
        if (enqueue) {
          setQueue(await api.enqueueEpisode(episodeId));
        }
        let next = await api.episodeForPlayer(episodeId);
        if (!next.download?.file_path) {
          setStatus("Laddar ner originalfil (ingen omkodning)…");
          await api.downloadEpisode(episodeId);
          next = await api.episodeForPlayer(episodeId);
        }
        const filePath = next.download?.file_path;
        if (!filePath) {
          throw new Error("Ingen lokal fil att spela.");
        }
        const audio = audioRef.current;
        if (!audio) return;
        const startSec =
          startMsOverride != null
            ? Math.max(0, startMsOverride) / 1000
            : next.playback && !next.playback.completed
              ? next.playback.position_ms / 1000
              : next.podcast.intro_skip_sec;
        const nextSpeed = next.playback?.speed || next.podcast.speed || 1;
        setSpeed(nextSpeed);
        setPayload(next);
        adsRef.current = next.podcast.skip_ads ? adsToSkip(next.ads) : [];
        void api.smtcNowPlaying(next.episode.title, next.podcast.title, true);
        const useSkip = globalSkipSilence || next.podcast.skip_silence;
        skipModeRef.current = useSkip;
        filePathRef.current = filePath;
        const useNative = (engineSettings?.backend ?? "native") !== "webview";
        nativeRef.current = useNative;
        setMusicPayload(null);
        let statusLine: string;
        if (useNative) {
          audio.pause();
          audio.removeAttribute("src");
          audio.load();
          try {
            setEngineStatus(await qualityStripPending(filePath));
            const st = await api.engineOpen({
              path: filePath,
              speed: nextSpeed,
              skipSilence: useSkip,
              outputApi: engineSettings?.output_api,
              deviceName: engineSettings?.device_name,
            });
            const startMs = Math.max(0, startSec) * 1000;
            await api.engineSeek(Math.floor(startMs));
            await api.enginePlay();
            setDuration((st.duration_ms || 0) / 1000);
            setPosition(startSec);
            positionRef.current = startSec;
            const apiLabel = nativeOutputLabel(st, engineSettings);
            const diag = st.dsd_diag ? ` [${st.dsd_diag}]` : "";
            statusLine = `Spelar native (${apiLabel}): ${next.episode.title}${diag}`;
            setEngineStatus(st);
            setOutputIsWebview(false);
          } catch (nativeErr) {
            const isDsd = /\.(dsf|dff|dsd)$/i.test(filePath);
            if (isDsd) {
              nativeRef.current = true;
              await api.engineStop();
              setPlaying(false);
              setEngineStatus(null);
              setEngineQa("");
              setOutputIsWebview(false);
              setStatus(
                `DSD kunde inte spelas: ${String(nativeErr)}. Ingen WebView2-fallback (skulle bli tystnad).`,
              );
              return;
            }
            nativeRef.current = false;
            await api.engineStop();
            setOutputIsWebview(true);
            setEngineStatus(await probeFileQuality(filePath));
            await loadPlaybackSource(audio, filePath, useSkip, nextSpeed);
            audio.currentTime = Math.max(0, startSec);
            await audio.play();
            setDuration(Number.isFinite(audio.duration) ? audio.duration : 0);
            setPosition(startSec);
            positionRef.current = startSec;
            statusLine = `Native utgång misslyckades (${String(nativeErr)}). Spelar via WebView2: ${next.episode.title}`;
          }
        } else {
          await api.engineStop();
          setOutputIsWebview(true);
          setEngineStatus(await probeFileQuality(filePath));
          await loadPlaybackSource(audio, filePath, useSkip, nextSpeed);
          audio.currentTime = Math.max(0, startSec);
          await audio.play();
          statusLine = `Spelar (WebView2): ${next.episode.title}`;
        }
        setPlaying(true);
        setStatus(statusLine);
        if (selectedId !== next.podcast.id) {
          setSelectedId(next.podcast.id);
        }
        await loadEpisodes(next.podcast.id);
        await refreshLibrary();
      } catch (err) {
        setStatus(String(err));
      } finally {
        setBusy(false);
      }
    },
    [loadEpisodes, loadPlaybackSource, refreshLibrary, selectedId, globalSkipSilence, engineSettings],
  );

  const playMusicTrack = useCallback(
    async (trackId: number) => {
      setBusy(true);
      try {
        const next = await api.musicTrackForPlayer(trackId);
        const filePath = next.track.file_path;
        if (!filePath) {
          throw new Error("Ingen lokal fil att spela.");
        }
        const audio = audioRef.current;
        if (!audio) return;
        const startSec =
          next.playback && !next.playback.completed ? next.playback.position_ms / 1000 : 0;
        setSpeed(1);
        setPayload(null);
        adsRef.current = [];
        setMusicPayload(next);
        void api.smtcNowPlaying(next.track.title, next.track.artist, true);
        skipModeRef.current = false;
        filePathRef.current = filePath;
        const useNative = (engineSettings?.backend ?? "native") !== "webview";
        nativeRef.current = useNative;
        let statusLine: string;
        if (useNative) {
          audio.pause();
          audio.removeAttribute("src");
          audio.load();
          try {
            setEngineStatus(await qualityStripPending(filePath));
            const st = await api.engineOpen({
              path: filePath,
              speed: 1,
              skipSilence: false,
              outputApi: engineSettings?.output_api,
              deviceName: engineSettings?.device_name,
            });
            const startMs = Math.max(0, startSec) * 1000;
            await api.engineSeek(Math.floor(startMs));
            await api.enginePlay();
            setDuration((st.duration_ms || 0) / 1000);
            setPosition(startSec);
            positionRef.current = startSec;
            const apiLabel = nativeOutputLabel(st, engineSettings);
            const diag = st.dsd_diag ? ` [${st.dsd_diag}]` : "";
            statusLine = `Musik native (${apiLabel}): ${next.track.title}${diag}`;
            setEngineStatus(st);
            setOutputIsWebview(false);
          } catch (nativeErr) {
            const isDsd = /\.(dsf|dff|dsd)$/i.test(filePath);
            if (isDsd) {
              nativeRef.current = true;
              await api.engineStop();
              setPlaying(false);
              setEngineStatus(null);
              setEngineQa("");
              setOutputIsWebview(false);
              setStatus(
                `DSD kunde inte spelas: ${String(nativeErr)}. Ingen WebView2-fallback (skulle bli tystnad).`,
              );
              return;
            }
            nativeRef.current = false;
            await api.engineStop();
            setOutputIsWebview(true);
            setEngineStatus(await probeFileQuality(filePath));
            await loadPlaybackSource(audio, filePath, false, 1);
            audio.currentTime = Math.max(0, startSec);
            await audio.play();
            setDuration(Number.isFinite(audio.duration) ? audio.duration : 0);
            setPosition(startSec);
            positionRef.current = startSec;
            statusLine = `Native utgång misslyckades (${String(nativeErr)}). Spelar via WebView2: ${next.track.title}`;
          }
        } else {
          await api.engineStop();
          setOutputIsWebview(true);
          setEngineStatus(await probeFileQuality(filePath));
          await loadPlaybackSource(audio, filePath, false, 1);
          audio.currentTime = Math.max(0, startSec);
          await audio.play();
          statusLine = `Spelar (WebView2): ${next.track.title}`;
        }
        setPlaying(true);
        setStatus(statusLine);
        if (selectedAlbumId !== next.album.id) {
          setSelectedAlbumId(next.album.id);
        }
      } catch (err) {
        setStatus(String(err));
      } finally {
        setBusy(false);
      }
    },
    [engineSettings, loadPlaybackSource, selectedAlbumId],
  );

  const onEnded = useCallback(async () => {
    await persistNow();
    if (sleepMode === "episode") {
      setSleepMode("off");
      setStatus("Sovtimer: spåret är slut.");
      return;
    }
    if (musicPayload) {
      const next = await api.nextMusicTrack(musicPayload.track.id);
      if (next) {
        await playMusicTrack(next.id);
      } else {
        setPlaying(false);
        setStatus("Albumet är slut.");
      }
      return;
    }
    const currentId = payload?.episode.id;
    const remaining = queue.filter((item) => item.episode_id !== currentId);
    if (remaining.length) {
      await playEpisode(remaining[0].episode_id, false);
    } else {
      setPlaying(false);
      setStatus("Kön är tom.");
    }
  }, [persistNow, playEpisode, playMusicTrack, payload, musicPayload, queue, sleepMode]);

  const playEpisodeRef = useRef(playEpisode);
  playEpisodeRef.current = playEpisode;
  const queueRef = useRef(queue);
  queueRef.current = queue;
  const payloadRef = useRef(payload);
  payloadRef.current = payload;
  musicPayloadRef.current = musicPayload;
  driveModeRef.current = driveMode;

  useEffect(() => {
    void api.setAudioPlaying(playing).catch(() => undefined);
  }, [playing]);

  const openSearchHit = useCallback((hit: TranscriptSearchHit) => {
    setView("library");
    setFilter("");
    setOnlyDownloaded(false);
    setSelectedId(hit.podcast_id);
    const playingId = payloadRef.current?.episode.id ?? null;
    if (playingId === hit.episode_id && filePathRef.current) {
      if (nativeRef.current) void api.engineSeek(hit.start_ms);
      else if (audioRef.current) audioRef.current.currentTime = hit.start_ms / 1000;
      setPosition(hit.start_ms / 1000);
      positionRef.current = hit.start_ms / 1000;
      setStatus(`Öppnade träffen i ${hit.episode_title}.`);
      return;
    }
    void playEpisodeRef.current(hit.episode_id, true, hit.start_ms);
  }, []);

  useEffect(() => {
    const unTick = listen<import("./types").EngineStatus>("engine-tick", (event) => {
      if (!nativeRef.current) return;
      const st = event.payload;
      const pos = st.position_ms / 1000;
      positionRef.current = pos;
      setPosition(pos);
      setDuration((st.duration_ms || 0) / 1000);
      setPlaying(st.playing);
      if (sessionPlayingRef.current !== st.playing) {
        sessionPlayingRef.current = st.playing;
        const music = musicPayloadRef.current;
        const episode = payloadRef.current;
        if (music) {
          void api.smtcNowPlaying(music.track.title, music.track.artist, st.playing);
        } else if (episode) {
          void api.smtcNowPlaying(episode.episode.title, episode.podcast.title, st.playing);
        }
      }
      if (st.loaded) {
        setEngineStatus(st);
      }
      if (st.format === "dsd") {
        const re =
          st.callback_reentries > 0 ? ` · re-entry ${st.callback_reentries} (inte underrun)` : "";
        setEngineQa(`underrun ${st.underruns}${re}${st.dsd_diag ? ` · ${st.dsd_diag}` : ""}`);
      } else if (st.loaded) {
        const re =
          st.callback_reentries > 0 ? ` · re-entry ${st.callback_reentries} (inte underrun)` : "";
        setEngineQa(`underrun ${st.underruns}${re}`);
      }
      if (st.error) {
        setStatus(st.error);
      }
      const posMs = st.position_ms;
      const ad = adsRef.current.find((a) => posMs >= a.start_ms && posMs < a.end_ms - 200);
      if (ad) {
        void api.engineSeek(ad.end_ms);
        return;
      }
      const outro = payloadRef.current?.podcast.outro_skip_sec ?? 0;
      if (
        !outroFired.current &&
        outro > 0 &&
        st.duration_ms > outro * 1000 &&
        posMs >= st.duration_ms - outro * 1000
      ) {
        outroFired.current = true;
        void api.enginePause();
        void onEnded();
      }
    });
    const unEnd = listen("engine-ended", () => {
      if (!nativeRef.current) return;
      void onEnded();
    });
    return () => {
      void unTick.then((fn) => fn());
      void unEnd.then((fn) => fn());
    };
  }, [onEnded]);

  const goNext = useCallback(() => {
    const q = queueRef.current;
    const currentId = payloadRef.current?.episode.id;
    const idx = q.findIndex((item) => item.episode_id === currentId);
    const next = driveNextIndex(idx, q.length);
    if (next == null) return;
    void playEpisodeRef.current(q[next].episode_id, false);
  }, []);

  const goPrevious = useCallback(() => {
    const audio = audioRef.current;
    const native = nativeRef.current;
    const q = queueRef.current;
    const currentId = payloadRef.current?.episode.id;
    const idx = q.findIndex((item) => item.episode_id === currentId);
    if (driveModeRef.current) {
      const prev = drivePreviousIndex(idx);
      if (prev != null) void playEpisodeRef.current(q[prev].episode_id, false);
      return;
    }
    if (native && positionRef.current > 3) {
      void api.engineSeek(0);
      return;
    }
    if (audio && !native && audio.currentTime > 3) {
      audio.currentTime = 0;
      return;
    }
    if (idx > 0) {
      void playEpisodeRef.current(q[idx - 1].episode_id, false);
    } else if (native) {
      void api.engineSeek(0);
    } else if (audio) {
      audio.currentTime = 0;
    }
  }, []);

  const togglePlayback = useCallback(() => {
    if (nativeRef.current) {
      const next = !playingRef.current;
      if (next) void api.enginePlay();
      else void api.enginePause();
      const music = musicPayloadRef.current;
      const episode = payloadRef.current;
      if (music) void api.smtcNowPlaying(music.track.title, music.track.artist, next);
      else if (episode) void api.smtcNowPlaying(episode.episode.title, episode.podcast.title, next);
      return;
    }
    const audio = audioRef.current;
    if (!audio) return;
    if (audio.paused) void audio.play();
    else audio.pause();
  }, []);

  const resumeWhereLeft = useCallback(async () => {
    if (payloadRef.current || musicPayloadRef.current) {
      if (!playingRef.current) togglePlayback();
      return;
    }
    try {
      const episodeId = await api.resumeEpisodeId();
      if (episodeId != null) {
        await playEpisodeRef.current(episodeId, false);
        return;
      }
    } catch (err) {
      setStatus(String(err));
      return;
    }
    const first = queueRef.current[0];
    if (first) {
      await playEpisodeRef.current(first.episode_id, false);
      return;
    }
    setStatus(DRIVE_LABELS.nothingToResume);
  }, [togglePlayback]);

  useEffect(() => {
    const un = listen<string>("smtc", (event) => {
      const audio = audioRef.current;
      const cmd = event.payload;
      const native = nativeRef.current;
      if (cmd === "toggle" || cmd === "play" || cmd === "pause") {
        if (native) {
          // Motorn har redan fått kommandot, så låsskärmen inte väntar på webview.
          return;
        }
        if (!audio) return;
        if (cmd === "pause") audio.pause();
        else if (cmd === "play" || audio.paused) void audio.play();
        else audio.pause();
        return;
      }
      if (cmd === "next") goNext();
      else if (cmd === "previous") goPrevious();
    });
    return () => {
      void un.then((fn) => fn());
    };
  }, [goNext, goPrevious]);

  const skipSeconds = (delta: number) => {
    if (nativeRef.current) {
      const next = Math.max(0, Math.min(duration || 0, positionRef.current + delta));
      void api.engineSeek(Math.floor(next * 1000));
      setPosition(next);
      positionRef.current = next;
      return;
    }
    skip(audioRef.current, delta);
  };

  const chrome = visibleChrome(driveMode);
  const nowTitle = musicPayload?.track.title ?? payload?.episode.title ?? "Inget spår";
  const nowSubtitle = musicPayload
    ? musicPayload.track.artist
    : (payload?.podcast.title ?? "");

  return (
    <div className={chrome.drive ? "app app-drive" : "app"}>
      {!chrome.drive && (
      <aside className="sidebar">
        <div className="brand">
          <span className="mark">HF</span>
          <div>
            <strong>HIFI podcast</strong>
            <p>Lokal hi-fi-spelare</p>
          </div>
        </div>
        <button type="button" className="drive-enter" onClick={() => setDriveMode(true)}>
          {DRIVE_LABELS.enter}
        </button>
        <nav>
          {chrome.library ? (
          <button className={view === "library" ? "active" : ""} onClick={() => setView("library")}>
            Bibliotek
          </button>
          ) : null}
          {chrome.library && chrome.import ? (
          <button className={view === "music" ? "active" : ""} onClick={() => setView("music")}>
            Musik
          </button>
          ) : null}
          <button className={view === "inbox" ? "active" : ""} onClick={() => setView("inbox")}>
            Inkorg ({inbox.length})
          </button>
          <button className={view === "queue" ? "active" : ""} onClick={() => setView("queue")}>
            Kö ({queue.length})
          </button>
          {chrome.statistics ? (
          <button className={view === "stats" ? "active" : ""} onClick={() => setView("stats")}>
            Statistik
          </button>
          ) : null}
          {chrome.settings ? (
          <button className={view === "settings" ? "active" : ""} onClick={() => setView("settings")}>
            Inställningar
          </button>
          ) : null}
        </nav>
        <div className="sidebar-note">
          Inga konton. Allt stannar på den här datorn.
        </div>
      </aside>
      )}

      {chrome.drive ? (
        <DriveModeView
          playing={playing}
          title={nowTitle}
          subtitle={nowSubtitle}
          positionLabel={formatTime(position)}
          durationLabel={formatTime(duration)}
          queue={queue}
          onPlayPause={togglePlayback}
          onSkip={(seconds) => {
            const allowed = driveSkipSeconds(seconds);
            if (allowed == null) return;
            skipSeconds(allowed);
          }}
          onPrevious={goPrevious}
          onNext={goNext}
          onResume={() => {
            void resumeWhereLeft();
          }}
          onPlayQueue={(id) => {
            void playEpisode(id, false);
          }}
          onRemoveQueue={(id) => {
            void api.dequeueItem(id).then(setQueue).catch((err) => setStatus(String(err)));
          }}
          onExit={() => setDriveMode(false)}
        />
      ) : (
      <main className="main">
        {chrome.library && chrome.import && view === "library" && (
          <Library
            feedUrl={feedUrl}
            setFeedUrl={setFeedUrl}
            opmlText={opmlText}
            setOpmlText={setOpmlText}
            filter={filter}
            setFilter={setFilter}
            onlyDownloaded={onlyDownloaded}
            setOnlyDownloaded={setOnlyDownloaded}
            podcasts={podcasts}
            selected={selected}
            episodes={episodes}
            activeEpisodeId={payload?.episode.id ?? null}
            busy={busy}
            onSelect={(id) => setSelectedId(id)}
            onAddFeed={async () => {
              setBusy(true);
              try {
                const result = await api.addFeed(feedUrl.trim());
                setFeedUrl("");
                setSelectedId(result.podcast.id);
                await refreshLibrary();
                setStatus(
                  result.warnings.length
                    ? `Prenumererar på ${result.podcast.title}. Varningar: ${result.warnings.join(" ")}`
                    : `Prenumererar på ${result.podcast.title}`,
                );
              } catch (err) {
                await refreshLibrary().catch(() => undefined);
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onImportOpml={async () => {
              setBusy(true);
              try {
                const imported = await api.importOpml(opmlText);
                setOpmlText("");
                await refreshLibrary();
                setStatus(
                  imported.warnings.length
                    ? `Importerade ${imported.podcasts.length} flöden. Varningar: ${imported.warnings.join(" ")}`
                    : `Importerade ${imported.podcasts.length} flöden från OPML.`,
                );
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onImportFile={async () => {
              const path = await open({
                multiple: false,
                filters: [{ name: "OPML", extensions: ["opml", "xml"] }],
              });
              if (!path || Array.isArray(path)) return;
              setBusy(true);
              try {
                const imported = await api.importOpmlFile(path);
                await refreshLibrary();
                setStatus(
                  imported.warnings.length
                    ? `Importerade ${imported.podcasts.length} flöden. Varningar: ${imported.warnings.join(" ")}`
                    : `Importerade ${imported.podcasts.length} flöden.`,
                );
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onRefresh={async () => {
              if (!selected) return;
              setBusy(true);
              try {
                const result = await api.refreshFeed(selected.id);
                await loadEpisodes(selected.id);
                await refreshLibrary();
                setStatus(
                  result.warnings.length
                    ? `Flödet uppdaterades. Varningar: ${result.warnings.join(" ")}`
                    : "Flödet uppdaterades.",
                );
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onDownload={async (id) => {
              setBusy(true);
              try {
                await api.downloadEpisode(id);
                if (selected) await loadEpisodes(selected.id);
                setStatus("Originalfilen är sparad utan omkodning.");
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onPlay={(id) => void playEpisode(id)}
            onEnqueue={async (id) => {
              setQueue(await api.enqueueEpisode(id));
              await api.triageEpisode(id, false, false);
              setStatus("Tillagd i kön.");
            }}
            onTranscribe={async (id) => {
              try {
                setJobs(await api.enqueueTranscribe(id));
                setStatus(
                  "Transkribering köad. Den pausas medan ljud spelas och fortsätter när uppspelningen stannar. NVIDIA GPU används när CUDA-klienten är installerad.",
                );
                await refreshLibrary();
              } catch (err) {
                setStatus(String(err));
              }
            }}
            onSaveSettings={async () => {
              if (!selected) return;
              await api.updatePodcastSettings(selected.id, {
                auto_download: selected.auto_download,
                speed: selected.speed,
                intro_skip_sec: selected.intro_skip_sec,
                outro_skip_sec: selected.outro_skip_sec,
                skip_silence: selected.skip_silence,
                asr_language: selected.asr_language,
                skip_ads: selected.skip_ads,
                auto_transcribe: selected.auto_transcribe,
              });
              await refreshLibrary();
              setStatus("Podcastinställningar sparade.");
            }}
            onPatchSelected={(patch) => {
              setPodcasts((current) =>
                current.map((p) => (p.id === selected?.id ? { ...p, ...patch } : p)),
              );
            }}
            onDeleteFile={async (id) => {
              setBusy(true);
              try {
                if (payload?.episode.id === id) {
                  const audio = audioRef.current;
                  if (audio) {
                    audio.pause();
                    audio.removeAttribute("src");
                    audio.load();
                  }
                  await api.engineStop().catch(() => undefined);
                  setPlaying(false);
                  setPayload(null);
                  filePathRef.current = null;
                  void api.smtcNowPlaying("", "", false);
                }
                const result = await api.deleteEpisodeFile(id);
                if (selected) await loadEpisodes(selected.id);
                await refreshLibrary();
                const warn = result.warnings.length
                  ? ` ${result.warnings.join(" ")}`
                  : "";
                const note =
                  result.files_removed > 0
                    ? "Lokal fil raderad. Avsnittet finns kvar och kan laddas ner igen."
                    : "Ingen lokal fil raderades. Avsnittet finns kvar.";
                setStatus(`${note}${warn}`);
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onUnsubscribe={async (deleteFiles) => {
              if (!selected) return;
              const id = selected.id;
              const title = selected.title;
              setBusy(true);
              try {
                if (payload?.podcast.id === id) {
                  const audio = audioRef.current;
                  if (audio) {
                    audio.pause();
                    audio.removeAttribute("src");
                    audio.load();
                  }
                  await api.engineStop().catch(() => undefined);
                  setPlaying(false);
                  setPayload(null);
                  filePathRef.current = null;
                  void api.smtcNowPlaying(title, "", false);
                }
                const result = await api.unsubscribePodcast(id, deleteFiles);
                setEpisodes([]);
                setSelectedId((current) => (current === id ? null : current));
                await refreshLibrary();
                const fileNote = !deleteFiles
                  ? "Lokala filer behölls."
                  : result.warnings.length
                    ? "Lokala filer raderades bara där det var säkert."
                    : "Lokala filer raderades.";
                const warn = result.warnings.length
                  ? ` ${result.warnings.join(" ")}`
                  : "";
                setStatus(
                  `Avslutade prenumerationen på ${result.title}. ${fileNote}${warn}`,
                );
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
          />
        )}

        {chrome.library && chrome.import && view === "music" && (
          <MusicView
            albums={musicAlbums}
            tracks={musicTracks}
            selectedAlbumId={selectedAlbumId}
            filter={musicFilter}
            setFilter={setMusicFilter}
            busy={busy}
            onSelectAlbum={setSelectedAlbumId}
            onPlay={(id) => void playMusicTrack(id)}
            onStatus={setStatus}
            onRefresh={refreshMusic}
            onImport={async () => {
              const lastRoot = await api.lastMusicImportRoot().catch(() => null);
              const path = await open({
                directory: true,
                multiple: false,
                title: "Välj mapp med FLAC/DSD",
                defaultPath: lastRoot || undefined,
              });
              if (!path || Array.isArray(path)) return;
              setBusy(true);
              try {
                const result = await api.importMusicFolder(path);
                await refreshMusic();
                if (result.failed_tracks > 0) {
                  const detail = result.errors.slice(0, 3).join(" ");
                  setStatus(
                    `Import misslyckades: ${result.failed_tracks} fil(er). ${result.imported_tracks} importerade, ${result.skipped_tracks} hoppades över. ${detail}`,
                  );
                  return;
                }
                setStatus(
                  `Importerade ${result.imported_tracks} spår (${result.skipped_tracks} hoppades över). ${result.album_count} album, ${result.track_count} i biblioteket.`,
                );
              } catch (err) {
                await refreshMusic().catch(() => undefined);
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
          />
        )}

        {view === "inbox" && (
          <InboxView
            items={inbox}
            onPlay={(id) => void playEpisode(id)}
            onQueue={async (id) => {
              setQueue(await api.enqueueEpisode(id));
              await api.triageEpisode(id, false, false);
              await refreshLibrary();
            }}
            onArchive={async (id) => {
              await api.triageEpisode(id, false, true);
              await refreshLibrary();
            }}
          />
        )}

        {view === "queue" && (
          <QueueView
            queue={queue}
            onPlay={(id) => void playEpisode(id, false)}
            onRemove={async (id) => setQueue(await api.dequeueItem(id))}
            onMove={async (id, dir) => setQueue(await api.moveQueueItem(id, dir))}
          />
        )}

        {chrome.statistics && view === "stats" && (
          <StatsView
            stats={stats}
            bookmarks={bookmarks}
            jobs={jobs}
            onSeekBookmark={(episodeId, ms) => {
              void playEpisode(episodeId, false).then(() => {
                if (nativeRef.current) void api.engineSeek(ms);
                else if (audioRef.current) audioRef.current.currentTime = ms / 1000;
              });
            }}
            onDeleteBookmark={async (id) => setBookmarks(await api.deleteBookmark(id))}
          />
        )}

        {chrome.settings && view === "settings" && (
          <SettingsView
            paths={paths}
            asr={asr}
            busy={busy}
            globalSkipSilence={globalSkipSilence}
            setGlobalSkipSilence={(v) => {
              setGlobalSkipSilence(v);
              if (nativeRef.current && !musicPlayingRef.current) {
                void api.engineSetSkipSilence(v);
              }
            }}
            engineSettings={engineSettings}
            onEngineSettings={setEngineSettings}
            nativeRef={nativeRef}
            onDownloadCli={async () => {
              const cuda = Boolean(asr?.nvidia_present);
              const ok = window.confirm(
                cuda
                  ? "Detta hämtar whisper-cli med CUDA, ca 640 MB. Nedladdningen startar bara om du bekräftar. Fortsätt?"
                  : "Detta hämtar whisper-cli (Windows-zip från whisper.cpp). Fortsätt?",
              );
              if (!ok) return;
              setBusy(true);
              try {
                setStatus(
                  cuda
                    ? "Laddar ner whisper-cli CUDA (ca 640 MB)…"
                    : "Laddar ner whisper-cli (Windows-zip från whisper.cpp)…",
                );
                setAsr(await api.downloadAsrCli());
                setStatus("whisper-cli är nerladdad.");
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onDownloadModel={async () => {
              const ok = window.confirm(
                "Detta hämtar ggml-small (ca 466 MB) från Hugging Face. SHA-256 kontrolleras. Fortsätt?",
              );
              if (!ok) return;
              setBusy(true);
              try {
                setStatus("Laddar ner ggml-small (ca 500 MB)…");
                setAsr(await api.downloadAsrModel());
                setStatus("Modellen är nerladdad.");
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onDownloadAll={async () => {
              const cuda = Boolean(asr?.nvidia_present);
              const ok = window.confirm(
                cuda
                  ? "Detta hämtar whisper-cli CUDA (ca 640 MB) och ggml-small (ca 466 MB). Startas bara efter bekräftelse. Fortsätt?"
                  : "Detta hämtar whisper-cli och ggml-small (hundratals MB). Fortsätt?",
              );
              if (!ok) return;
              setBusy(true);
              try {
                setStatus("Laddar ner whisper-cli och ggml-small…");
                setAsr(await api.downloadAsrSetup());
                setStatus("Whisper är redo.");
              } catch (err) {
                setStatus(String(err));
              } finally {
                setBusy(false);
              }
            }}
            onSideload={async () => {
              const path = await open({
                multiple: false,
                filters: [{ name: "Ljud", extensions: ["mp3", "m4a", "aac", "ogg", "opus", "flac", "wav", "dsf", "dff", "dsd"] }],
              });
              if (!path || Array.isArray(path)) return;
              const ep = await api.sideloadFile(path);
              setSelectedId(ep.podcast_id);
              await refreshLibrary();
              setView("library");
              setStatus(`Importerade ${ep.title}`);
            }}
            onExport={async () => {
              const path = await save({
                defaultPath: "hifi-podcast.opml",
                filters: [{ name: "OPML", extensions: ["opml"] }],
              });
              if (!path) return;
              await api.exportOpmlFile(path);
              setStatus("OPML exporterad.");
            }}
            onStatus={setStatus}
          />
        )}
      </main>
      )}

      {chrome.whisper && (
      <TranscriptPane
        episodeId={payload?.episode.id ?? null}
        positionMs={Math.floor(position * 1000)}
        jobs={jobs}
        onSeekMs={(ms) => {
          if (nativeRef.current) void api.engineSeek(ms);
          else if (audioRef.current) audioRef.current.currentTime = ms / 1000;
        }}
        onOpenHit={openSearchHit}
        onNotice={setStatus}
        onAdsChanged={(next) => {
          if (payloadRef.current?.podcast.skip_ads) {
            adsRef.current = adsToSkip(next);
          }
        }}
      />
      )}

      <footer className="player">
        <audio
          ref={audioRef}
          crossOrigin="anonymous"
          onTimeUpdate={(e) => {
            const el = e.currentTarget;
            positionRef.current = el.currentTime;
            setPosition(el.currentTime);
            setDuration(el.duration || 0);
            const posMs = el.currentTime * 1000;
            const ad = adsRef.current.find((a) => posMs >= a.start_ms && posMs < a.end_ms - 200);
            if (ad) {
              el.currentTime = ad.end_ms / 1000;
              return;
            }
            const outro = payload?.podcast.outro_skip_sec ?? 0;
            if (
              !outroFired.current &&
              outro > 0 &&
              el.duration > outro &&
              el.currentTime >= el.duration - outro
            ) {
              outroFired.current = true;
              el.pause();
              void onEnded();
              return;
            }
            if (persistTimer.current) return;
            persistTimer.current = window.setTimeout(() => {
              persistTimer.current = null;
              void persistNow();
            }, 2500);
          }}
          onPlay={() => setPlaying(true)}
          onPause={() => {
            setPlaying(false);
            void persistNow();
            if (payload) {
              void api.smtcNowPlaying(payload.episode.title, payload.podcast.title, false);
            } else if (musicPayload) {
              void api.smtcNowPlaying(musicPayload.track.title, musicPayload.track.artist, false);
            }
          }}
          onEnded={() => void onEnded()}
          onLoadedMetadata={(e) => setDuration(e.currentTarget.duration || 0)}
        />
        {!chrome.drive && (
        <>
        <div className="now">
          <strong>
            {musicPayload?.track.title ?? payload?.episode.title ?? "Inget spår"}
          </strong>
          <span>
            {musicPayload
              ? `${musicPayload.track.artist} · ${musicPayload.album.title}`
              : (payload?.podcast.title ?? "Välj ett avsnitt eller ett musikspår")}
          </span>
        </div>
        <div className="transport">
          <button onClick={() => skipSeconds(-30)} title="Bakåt 30 s">
            −30
          </button>
          <button onClick={() => skipSeconds(-15)} title="Bakåt 15 s">
            −15
          </button>
          <button
            className="play"
            disabled={!payload && !musicPayload}
            onClick={togglePlayback}
          >
            {playing ? "Paus" : "Spela"}
          </button>
          <button onClick={() => skipSeconds(15)} title="Framåt 15 s">
            +15
          </button>
          <button onClick={() => skipSeconds(30)} title="Framåt 30 s">
            +30
          </button>
        </div>
        {chrome.seekSlider && (
        <div className="seek">
          <span>{formatTime(position)}</span>
          <input
            type="range"
            min={0}
            max={duration || 0}
            step={0.1}
            value={Math.min(position, duration || 0)}
            onChange={(e) => {
              const value = Number(e.target.value);
              if (nativeRef.current) void api.engineSeek(Math.floor(value * 1000));
              else if (audioRef.current) audioRef.current.currentTime = value;
              setPosition(value);
              positionRef.current = value;
            }}
          />
          <span>{formatTime(duration)}</span>
        </div>
        )}
        <label className="speed">
          Hastighet
          <select
            value={speed}
            onChange={(e) => {
              const next = Number(e.target.value);
              setSpeed(next);
              if (nativeRef.current) void api.engineSetSpeed(next);
              else if (audioRef.current) configureHiFiElement(audioRef.current, next);
            }}
          >
            {SPEEDS.map((s) => (
              <option key={s} value={s}>
                {s.toFixed(2).replace(/\.00$/, "")}×
              </option>
            ))}
          </select>
        </label>
        <label className="sleep">
          Sovtimer
          <select
            value={sleepMode === "minutes" ? String(sleepMinutes) : sleepMode}
            onChange={(e) => {
              const value = e.target.value;
              if (value === "off") {
                setSleepMode("off");
                setSleepUntil(null);
              } else if (value === "episode") {
                setSleepMode("episode");
                setSleepUntil(null);
              } else {
                const minutes = Number(value);
                setSleepMinutes(minutes);
                setSleepMode("minutes");
                setSleepUntil(Date.now() + minutes * 60_000);
              }
            }}
          >
            <option value="off">Av</option>
            <option value="5">5 min</option>
            <option value="15">15 min</option>
            <option value="30">30 min</option>
            <option value="45">45 min</option>
            <option value="60">60 min</option>
            <option value="episode">{musicPayload ? "Slut på spår" : "Slut på avsnitt"}</option>
          </select>
        </label>
        {musicPayload ? (
          <span className="player-note" title="Bokmärken är knutna till podcastavsnitt.">
            Bokmärken gäller podcasts
          </span>
        ) : (
          <button
            disabled={!payload}
            onClick={() => {
              if (!payload) return;
              const note = window.prompt("Anteckning (valfritt)") ?? "";
              void api
                .addBookmark(payload.episode.id, Math.floor(position * 1000), note)
                .then(setBookmarks);
            }}
          >
            Bokmärke
          </button>
        )}
        <QualityStrip status={engineStatus} webview={outputIsWebview} />
        <div className="status">
          {status}
          {nativeRef.current && (payload || musicPayload)
            ? " · Native"
            : payload || musicPayload
              ? " · WebView2"
              : ""}
          {engineQa ? ` · ${engineQa}` : ""}
        </div>
        </>
        )}
      </footer>
    </div>
  );
}

async function probeFileQuality(path: string): Promise<EngineStatus | null> {
  try {
    const p = await api.engineProbe(path);
    const dsd = p.support.kind === "dsd";
    const rate = p.sample_rate ?? 0;
    const bits = p.bits_per_sample ?? 0;
    const codec = p.codec ?? (dsd ? "DSD" : "");
    const dsdRate = dsd ? dsdRateLabel(rate) : "";
    return {
      loaded: true,
      playing: false,
      ended: false,
      position_ms: 0,
      duration_ms: p.duration_ms ?? 0,
      sample_rate: rate,
      channels: p.channels ?? 0,
      codec,
      path,
      exclusive: false,
      output_api: "webview",
      device_name: "",
      output_rate: 0,
      resampled: false,
      speed: 1,
      skip_silence: false,
      underruns: 0,
      error: null,
      format: dsd ? "dsd" : "pcm",
      dsd_rate: dsdRate,
      output_path: "webview",
      output_path_sv: "WebView2 (systemmixer) — inte native ASIO/WASAPI.",
      callback_reentries: 0,
      dsd_diag: "",
      file_bits: bits,
      output_bits: 0,
      output_sample: "",
      file_quality_sv: fileQualityFromProbe(dsd, codec, rate, bits, dsdRate),
      output_quality_sv: "WebView2 (systemmixer)",
      quality_delta: [],
      quality_delta_sv: "",
    };
  } catch {
    return null;
  }
}

async function qualityStripPending(path: string): Promise<EngineStatus> {
  const probed = await probeFileQuality(path);
  if (probed) {
    return {
      ...probed,
      output_api: "",
      output_path: "",
      output_path_sv: "Öppnar utgång, väntar på enhet.",
      output_quality_sv: "Förhandlar ASIO/WASAPI…",
      output_rate: 0,
      output_bits: 0,
      output_sample: "",
      quality_delta: [],
      quality_delta_sv: "",
    };
  }
  return {
    loaded: true,
    playing: false,
    ended: false,
    position_ms: 0,
    duration_ms: 0,
    sample_rate: 0,
    channels: 0,
    codec: "",
    path,
    exclusive: false,
    output_api: "",
    device_name: "",
    output_rate: 0,
    resampled: false,
    speed: 1,
    skip_silence: false,
    underruns: 0,
    error: null,
    format: "pcm",
    dsd_rate: "",
    output_path: "",
    output_path_sv: "Öppnar utgång, väntar på enhet.",
    callback_reentries: 0,
    dsd_diag: "",
    file_bits: 0,
    output_bits: 0,
    output_sample: "",
    file_quality_sv: "Läser fil…",
    output_quality_sv: "Förhandlar ASIO/WASAPI…",
    quality_delta: [],
    quality_delta_sv: "",
  };
}

function nativeOutputLabel(st: EngineStatus, settings: AudioEngineSettings | null): string {
  if (st.output_path === "native_dsd") {
    return `Native DSD ${st.dsd_rate} / ${st.device_name}`.trim();
  }
  if (st.output_path === "dop") {
    return `DoP ${st.dsd_rate} (inte native DSD) / ${st.output_api}`;
  }
  if (
    settings?.output_api === "asio" ||
    !settings?.output_api ||
    settings.output_api === "auto"
  ) {
    return "ASIO/WASAPI";
  }
  return settings.output_api;
}

function dsdRateLabel(hz: number): string {
  switch (hz) {
    case 2_822_400:
      return "DSD64";
    case 5_644_800:
      return "DSD128";
    case 11_289_600:
      return "DSD256";
    case 22_579_200:
      return "DSD512";
    default:
      return hz ? `${hz} Hz` : "";
  }
}

function fileQualityFromProbe(
  dsd: boolean,
  codec: string,
  rate: number,
  bits: number,
  dsdRate: string,
): string {
  if (dsd) {
    const label = dsdRate || (rate ? `${rate} Hz` : "");
    return codec ? `${codec} · ${label}` : `DSD · ${label}`;
  }
  const khz =
    rate % 1000 === 0
      ? `${rate / 1000} kHz`
      : rate
        ? `${(rate / 1000).toLocaleString("sv-SE", { maximumFractionDigits: 1 })} kHz`
        : "";
  let s = codec || "PCM";
  if (khz) s += ` · ${khz}`;
  if (bits > 0) s += ` / ${bits}-bit`;
  return s;
}

function QualityStrip(props: { status: EngineStatus | null; webview: boolean }) {
  const st = props.status;
  if (!st) return null;
  if (!st.file_quality_sv && !st.output_quality_sv) return null;
  const pending =
    !props.webview &&
    (!st.output_path || st.output_quality_sv.startsWith("Förhandlar"));
  const changed = !props.webview && !pending && (st.quality_delta?.length ?? 0) > 0;
  const output = props.webview
    ? "WebView2 (systemmixer)"
    : st.output_quality_sv || "Förhandlar ASIO/WASAPI…";
  const original = st.file_quality_sv || "Läser fil…";
  return (
    <div
      className={`quality${changed ? " quality-changed" : ""}${pending ? " quality-pending" : ""}`}
      title={st.output_path_sv || undefined}
    >
      <span className="quality-pair">
        <span className="quality-k">Original</span>
        <strong>{original}</strong>
      </span>
      <span className="quality-arrow" aria-hidden>
        →
      </span>
      <span className="quality-pair">
        <span className="quality-k">Utgång</span>
        <strong>{output}</strong>
      </span>
      {changed ? <span className="quality-delta">{st.quality_delta_sv}</span> : null}
      {!props.webview && !pending && st.device_name ? (
        <span className="quality-dev">{st.device_name}</span>
      ) : null}
    </div>
  );
}

function skip(audio: HTMLAudioElement | null, delta: number) {
  if (!audio) return;
  audio.currentTime = Math.max(0, Math.min(audio.duration || 0, audio.currentTime + delta));
}

function Library(props: {
  feedUrl: string;
  setFeedUrl: (v: string) => void;
  opmlText: string;
  setOpmlText: (v: string) => void;
  filter: string;
  setFilter: (v: string) => void;
  onlyDownloaded: boolean;
  setOnlyDownloaded: (v: boolean) => void;
  podcasts: Podcast[];
  selected: Podcast | null;
  episodes: Episode[];
  activeEpisodeId: number | null;
  busy: boolean;
  onSelect: (id: number) => void;
  onAddFeed: () => void;
  onImportOpml: () => void;
  onImportFile: () => void;
  onRefresh: () => void;
  onDownload: (id: number) => void;
  onPlay: (id: number) => void;
  onEnqueue: (id: number) => void;
  onTranscribe: (id: number) => void;
  onSaveSettings: () => void;
  onPatchSelected: (patch: Partial<Podcast>) => void;
  onUnsubscribe: (deleteFiles: boolean) => void;
  onDeleteFile: (episodeId: number) => void;
}) {
  const [confirmUnsubscribe, setConfirmUnsubscribe] = useState(false);
  const [deleteLocalFiles, setDeleteLocalFiles] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<number | null>(null);
  useEffect(() => {
    setConfirmUnsubscribe(false);
    setDeleteLocalFiles(false);
    setConfirmDeleteId(null);
  }, [props.selected?.id]);
  const selected = props.selected;
  const visible = props.episodes.filter((ep) => {
    const q = props.filter.trim().toLowerCase();
    if (q && !ep.title.toLowerCase().includes(q)) return false;
    if (props.onlyDownloaded && ep.status !== "downloaded") return false;
    return true;
  });
  return (
    <section className="library">
      <header className="toolbar">
        <input
          placeholder="RSS-adress"
          value={props.feedUrl}
          onChange={(e) => props.setFeedUrl(e.target.value)}
        />
        <button disabled={props.busy || !props.feedUrl.trim()} onClick={props.onAddFeed}>
          Prenumerera
        </button>
        <button disabled={props.busy} onClick={props.onImportFile}>
          Importera OPML…
        </button>
        <button disabled={props.busy || !selected} onClick={props.onRefresh}>
          Uppdatera flöde
        </button>
      </header>
      <p className="auto-refresh-note">
        Flöden hämtas vid start och var 60:e minut. Knappen uppdaterar det valda flödet direkt.
      </p>
      <div className="toolbar">
        <input
          placeholder="Filtrera avsnitt"
          value={props.filter}
          onChange={(e) => props.setFilter(e.target.value)}
        />
        <label className="check">
          <input
            type="checkbox"
            checked={props.onlyDownloaded}
            onChange={(e) => props.setOnlyDownloaded(e.target.checked)}
          />
          Bara nedladdade
        </label>
      </div>
      <textarea
        placeholder="Klistra in OPML här och importera"
        value={props.opmlText}
        onChange={(e) => props.setOpmlText(e.target.value)}
      />
      <div className="row-actions">
        <button disabled={props.busy || !props.opmlText.trim()} onClick={props.onImportOpml}>
          Importera inklistrad OPML
        </button>
      </div>
      <div className="split">
        <ul className="podcast-list">
          {props.podcasts.length === 0 && <li className="empty">Inga podcasts ännu.</li>}
          {props.podcasts.map((podcast) => (
            <li key={podcast.id}>
              <button
                className={selected?.id === podcast.id ? "active" : ""}
                onClick={() => props.onSelect(podcast.id)}
              >
                <strong>{podcast.title}</strong>
                <span>{podcast.author ?? podcast.feed_url}</span>
              </button>
            </li>
          ))}
        </ul>
        <div className="detail">
          {selected ? (
            <>
              <div className="podcast-head">
                <div>
                  <h1>{selected.title}</h1>
                  <p>{selected.description ?? "Ingen beskrivning i feeden."}</p>
                  <p className="empty">
                    Prenumerationen gäller från {formatDate(selected.subscribed_at)}.
                    Inkorgen tar bara med avsnitt publicerade efter det. Äldre avsnitt
                    ligger kvar i listan.
                  </p>
                </div>
                <div className="unsubscribe">
                  {confirmUnsubscribe ? (
                    <div className="unsubscribe-panel">
                      <p>
                        Flödet tas bort från biblioteket. Avsnitt, kö och sparad
                        uppspelning för den här podden försvinner.
                      </p>
                      <label className="check">
                        <input
                          type="checkbox"
                          checked={deleteLocalFiles}
                          onChange={(e) => setDeleteLocalFiles(e.target.checked)}
                        />
                        Radera även lokala filer för den här podden
                      </label>
                      <div className="row-actions">
                        <button
                          disabled={props.busy}
                          onClick={() => {
                            setConfirmUnsubscribe(false);
                            setDeleteLocalFiles(false);
                          }}
                        >
                          Avbryt
                        </button>
                        <button
                          className="danger"
                          disabled={props.busy}
                          onClick={() => props.onUnsubscribe(deleteLocalFiles)}
                        >
                          Avsluta prenumeration
                        </button>
                      </div>
                    </div>
                  ) : (
                    <button
                      disabled={props.busy}
                      onClick={() => {
                        setDeleteLocalFiles(false);
                        setConfirmUnsubscribe(true);
                      }}
                    >
                      Avsluta prenumeration
                    </button>
                  )}
                </div>
              </div>
              <div className="settings-grid">
                <label>
                  Hastighet
                  <select
                    value={selected.speed}
                    onChange={(e) =>
                      props.onPatchSelected({ speed: Number(e.target.value) })
                    }
                  >
                    {SPEEDS.map((s) => (
                      <option key={s} value={s}>
                        {s}×
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  Hoppa intro (s)
                  <input
                    type="number"
                    min={0}
                    value={selected.intro_skip_sec}
                    onChange={(e) =>
                      props.onPatchSelected({ intro_skip_sec: Number(e.target.value) })
                    }
                  />
                </label>
                <label>
                  Hoppa outro (s)
                  <input
                    type="number"
                    min={0}
                    value={selected.outro_skip_sec}
                    onChange={(e) =>
                      props.onPatchSelected({ outro_skip_sec: Number(e.target.value) })
                    }
                  />
                </label>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={selected.auto_download}
                    onChange={(e) =>
                      props.onPatchSelected({ auto_download: e.target.checked })
                    }
                  />
                  Auto-nedladdning
                </label>
                <p className="auto-download-note">
                  Bara avsnitt som är nya sedan förra hämtningen. Äldre avsnitt
                  utan fil laddas inte ner automatiskt.
                </p>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={selected.skip_silence}
                    onChange={(e) =>
                      props.onPatchSelected({ skip_silence: e.target.checked })
                    }
                  />
                  Hoppa tystnad (opt-in)
                </label>
                <label className="check">
                  <input
                    type="checkbox"
                    checked={selected.skip_ads}
                    onChange={(e) =>
                      props.onPatchSelected({ skip_ads: e.target.checked })
                    }
                  />
                  Hoppa reklam (heuristik)
                </label>
                <p className="auto-download-note">
                  Transkribering med ggml-small köas automatiskt när nedladdningen
                  är klar. Knappen Transkribera finns kvar. Jobbet pausas medan
                  ljud spelas och fortsätter när uppspelningen stannar.
                </p>
                <label>
                  ASR-språk
                  <select
                    value={selected.asr_language ?? "auto"}
                    onChange={(e) =>
                      props.onPatchSelected({
                        asr_language: e.target.value === "auto" ? "auto" : e.target.value,
                      })
                    }
                  >
                    <option value="auto">auto</option>
                    <option value="sv">sv</option>
                    <option value="en">en</option>
                  </select>
                </label>
                <button onClick={props.onSaveSettings}>Spara inställningar</button>
              </div>
              <ul className="episodes">
                {visible.map((episode) => (
                  <li
                    key={episode.id}
                    className={props.activeEpisodeId === episode.id ? "active" : ""}
                  >
                    <div>
                      <strong>{episode.title}</strong>
                      <span>
                        {formatDate(episode.publish_at)} · {episode.status}
                        {episode.inbox ? " · inkorg" : ""}
                      </span>
                    </div>
                    <div className="ep-actions">
                      <button onClick={() => props.onDownload(episode.id)}>Ladda ner</button>
                      {episode.status === "downloaded" &&
                        (confirmDeleteId === episode.id ? (
                          <>
                            <button
                              disabled={props.busy}
                              onClick={() => setConfirmDeleteId(null)}
                            >
                              Avbryt
                            </button>
                            <button
                              className="danger"
                              disabled={props.busy}
                              onClick={() => {
                                setConfirmDeleteId(null);
                                props.onDeleteFile(episode.id);
                              }}
                            >
                              Radera fil
                            </button>
                          </>
                        ) : (
                          <button
                            disabled={props.busy}
                            onClick={() => setConfirmDeleteId(episode.id)}
                          >
                            Radera fil
                          </button>
                        ))}
                      <button onClick={() => props.onTranscribe(episode.id)}>Transkribera</button>
                      <button onClick={() => props.onEnqueue(episode.id)}>Köa</button>
                      <button className="primary" onClick={() => props.onPlay(episode.id)}>
                        Spela
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p className="empty">Prenumerera på ett RSS-flöde för att börja.</p>
          )}
        </div>
      </div>
    </section>
  );
}

function formatDurationMs(ms: number | null) {
  if (ms == null || ms <= 0) return "";
  return formatTime(ms / 1000);
}

function trackQualityHint(track: MusicTrack): string {
  const fmt = (track.format || "").toLowerCase();
  const dsd = fmt === "dsf" || fmt === "dff" || fmt === "dsd";
  if (dsd) {
    const label = dsdRateLabel(track.sample_rate ?? 0);
    return label ? ` · ${label}` : "";
  }
  const rate = track.sample_rate
    ? track.sample_rate % 1000 === 0
      ? `${track.sample_rate / 1000} kHz`
      : `${track.sample_rate} Hz`
    : "";
  const bits = track.bits_per_sample && track.bits_per_sample > 0 ? `${track.bits_per_sample}-bit` : "";
  if (rate && bits) return ` · ${rate} / ${bits}`;
  if (rate) return ` · ${rate}`;
  if (bits) return ` · ${bits}`;
  return "";
}

function localCoverSrc(path: string | null): string | null {
  if (!path || path.includes("://")) return null;
  return assetUrlForLocalFile(path);
}

function MusicView(props: {
  albums: MusicAlbum[];
  tracks: MusicTrack[];
  selectedAlbumId: number | null;
  filter: string;
  setFilter: (v: string) => void;
  busy: boolean;
  onSelectAlbum: (id: number) => void;
  onPlay: (id: number) => void;
  onImport: () => void;
  onStatus: (message: string) => void;
  onRefresh: () => Promise<void>;
}) {
  const [savedConsent, setSavedConsent] = useState(false);
  const [draftConsent, setDraftConsent] = useState(false);
  const [consentBusy, setConsentBusy] = useState(false);
  const [fetchBusy, setFetchBusy] = useState(false);
  const selected = props.albums.find((a) => a.id === props.selectedAlbumId) ?? null;
  const coverSrc = localCoverSrc(selected?.cover_path ?? null);

  useEffect(() => {
    let cancelled = false;
    void api
      .getOnlineMetadataConsent()
      .then((enabled) => {
        if (cancelled) return;
        setSavedConsent(enabled);
        setDraftConsent(enabled);
      })
      .catch((err) => props.onStatus(String(err)));
    return () => {
      cancelled = true;
    };
    // Local setting only. This does not contact MusicBrainz or Cover Art Archive.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const saveConsent = async () => {
    setConsentBusy(true);
    try {
      const enabled = await api.saveOnlineMetadataConsent(draftConsent);
      setSavedConsent(enabled);
      setDraftConsent(enabled);
      props.onStatus(
        enabled
          ? "Internet-samtycke är på. MusicBrainz och Cover Art Archive kontaktas bara när du hämtar metadata."
          : "Internet-samtycke är av. Ingen metadataförfrågan skickas.",
      );
    } catch (err) {
      props.onStatus(String(err));
    } finally {
      setConsentBusy(false);
    }
  };

  const fetchMetadata = async () => {
    if (!savedConsent || selected == null) {
      props.onStatus("Internet-samtycke saknas. Ingen förfrågan skickades.");
      return;
    }
    setFetchBusy(true);
    try {
      const outcome = await api.fetchAlbumOnlineMetadata(selected.id);
      await props.onRefresh();
      const date = outcome.release_date ? ` Utgivning ${outcome.release_date}.` : "";
      const cover = outcome.cover_saved ? " Omslag sparat." : "";
      const warning = outcome.warnings.length ? ` ${outcome.warnings.join(" ")}` : "";
      props.onStatus(
        `Metadata hämtad från MusicBrainz (${outcome.mbid}).${date}${cover}${warning}`,
      );
    } catch (err) {
      props.onStatus(String(err));
    } finally {
      setFetchBusy(false);
    }
  };
  const q = props.filter.trim().toLowerCase();
  const visible = props.tracks.filter((t) => {
    if (!q) return true;
    return (
      t.title.toLowerCase().includes(q) ||
      t.artist.toLowerCase().includes(q) ||
      t.album.toLowerCase().includes(q) ||
      t.format.toLowerCase().includes(q)
    );
  });
  return (
    <section className="library">
      <header className="toolbar">
        <h1 className="toolbar-title">Musik</h1>
        <button className="primary" disabled={props.busy} onClick={props.onImport}>
          Importera mapp…
        </button>
      </header>
      <p className="empty music-lead">
        Eget bibliotek, skilt från podcasts. Importera mappar med FLAC/DSD (t.ex. under
        Downloads). Filerna lämnas kvar där de ligger: appen indexerar och länkar dem,
        utan kopia till appdata och utan omkodning, och spelar dem i samma hi-fi-motor
        (ASIO/FiiO, WASAPI, native DSD). SACD-ISO rippning ingår inte.
      </p>
      <div className="consent-panel">
        <h2>Internet-metadata</h2>
        <p>
          Av som standard. Appen kontaktar bara musicbrainz.org och
          coverartarchive.org, och bara efter att du har sparat samtycke.
          Ingen förfrågan skickas innan dess. Lokala taggar skrivs inte över.
          Omslag sparas bara om Cover Art Archive svarar med själva bilden.
          En omdirigering till en annan värd, till exempel archive.org, avvisas.
        </p>
        <label className="check">
          <input
            type="checkbox"
            checked={draftConsent}
            onChange={(e) => setDraftConsent(e.target.checked)}
          />
          Jag samtycker till att hämta albummetadata och omslag från MusicBrainz
          och Cover Art Archive
        </label>
        <div className="row-actions">
          <button onClick={() => void saveConsent()} disabled={consentBusy}>
            Spara samtycke
          </button>
        </div>
      </div>
      <div className="toolbar">
        <input
          placeholder="Filtrera spår"
          value={props.filter}
          onChange={(e) => props.setFilter(e.target.value)}
        />
      </div>
      <div className="split">
        <ul className="podcast-list">
          {props.albums.length === 0 && (
            <li className="empty">Inga album ännu. Importera en mapp.</li>
          )}
          {props.albums.map((album) => (
            <li key={album.id}>
              <button
                className={selected?.id === album.id ? "active" : ""}
                onClick={() => props.onSelectAlbum(album.id)}
              >
                <strong>{album.title}</strong>
                <span>
                  {album.artist} · {album.track_count} spår
                </span>
              </button>
            </li>
          ))}
        </ul>
        <div className="detail">
          {selected ? (
            <>
              <div className="podcast-head">
                {coverSrc ? (
                  <img className="album-cover" src={coverSrc} alt={`Omslag för ${selected.title}`} />
                ) : null}
                <div>
                  <h1>{selected.title}</h1>
                  <p>{selected.artist}</p>
                  {selected.release_date ? <p>Utgivning {selected.release_date}</p> : null}
                  {selected.mbid ? <p>MusicBrainz {selected.mbid}</p> : null}
                </div>
                <button
                  onClick={() => void fetchMetadata()}
                  disabled={!savedConsent || fetchBusy || props.busy}
                  title={savedConsent ? undefined : "Kräver sparat internet-samtycke"}
                >
                  Hämta metadata och omslag
                </button>
              </div>
              {!savedConsent ? (
                <p className="meta-note">
                  Spara internet-samtycke först. Ingen förfrågan skickas innan dess.
                </p>
              ) : null}
              <ul className="episodes">
                {visible.map((track) => (
                  <li key={track.id}>
                    <div>
                      <strong>
                        {track.track_no != null ? `${track.track_no}. ` : ""}
                        {track.title}
                      </strong>
                      <span>
                        {track.artist}
                        {track.format ? ` · ${track.format.toUpperCase()}` : ""}
                        {trackQualityHint(track)}
                        {track.duration_ms ? ` · ${formatDurationMs(track.duration_ms)}` : ""}
                      </span>
                    </div>
                    <div className="ep-actions">
                      <button className="primary" onClick={() => props.onPlay(track.id)}>
                        Spela
                      </button>
                    </div>
                  </li>
                ))}
              </ul>
            </>
          ) : (
            <p className="empty">Importera en mapp för att bygga musikbiblioteket.</p>
          )}
        </div>
      </div>
    </section>
  );
}

function QueueView(props: {
  queue: QueueItem[];
  onPlay: (episodeId: number) => void;
  onRemove: (queueId: number) => void;
  onMove: (queueId: number, direction: number) => void;
}) {
  return (
    <section className="queue">
      <h1>Up Next</h1>
      {props.queue.length === 0 && <p className="empty">Kön är tom.</p>}
      <ol>
        {props.queue.map((item) => (
          <li key={item.id}>
            <div>
              <strong>{item.episode_title}</strong>
              <span>
                {item.podcast_title}
                {item.download_state ? ` · ${item.download_state}` : ""}
              </span>
            </div>
            <div className="ep-actions">
              <button onClick={() => props.onMove(item.id, -1)}>Upp</button>
              <button onClick={() => props.onMove(item.id, 1)}>Ner</button>
              <button className="primary" onClick={() => props.onPlay(item.episode_id)}>
                Spela
              </button>
              <button onClick={() => props.onRemove(item.id)}>Ta bort</button>
            </div>
          </li>
        ))}
      </ol>
    </section>
  );
}

function FinishedCleanupPanel(props: { onStatus: (s: string) => void }) {
  const [enabled, setEnabled] = useState(false);
  const [days, setDays] = useState(7);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    const settings = await api.getFinishedCleanupSettings();
    setEnabled(settings.enabled);
    setDays(settings.days);
  }, []);

  useEffect(() => {
    void refresh().catch((err) => props.onStatus(String(err)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);

  const save = async () => {
    const nextDays = Math.round(days);
    if (!Number.isInteger(nextDays) || nextDays < 1 || nextDays > 365) {
      props.onStatus("Antal dagar måste vara mellan 1 och 365.");
      return;
    }
    setBusy(true);
    try {
      const outcome = await api.saveFinishedCleanupSettings({
        enabled,
        days: nextDays,
      });
      setEnabled(outcome.enabled);
      setDays(outcome.days);
      const removed =
        outcome.files_removed === 0
          ? "Inga filer raderades."
          : outcome.files_removed === 1
            ? "1 lokal fil raderades."
            : `${outcome.files_removed} lokala filer raderades.`;
      const extra = outcome.warnings[0] ? ` ${outcome.warnings[0]}` : "";
      props.onStatus(
        outcome.enabled
          ? `Färdigspelade avsnitt raderas efter ${outcome.days} dagar. ${removed}${extra}`
          : `Automatisk radering av färdigspelade är av. ${removed}${extra}`,
      );
    } catch (err) {
      props.onStatus(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="upnp-panel">
      <h2>Färdigspelade avsnitt</h2>
      <p>
        Valfritt. Av som standard. När det är på tas bara den lokala filen bort
        för avsnitt där uppspelningen redan har nått slutet, och bara om det
        var för mer än det valda antalet dagar sedan. Standard är 7 dagar.
        Avsnittet finns kvar och kan laddas ner igen. Pågående avsnitt och
        musik rörs inte.
      </p>
      <label className="check">
        <input
          type="checkbox"
          checked={enabled}
          onChange={(e) => setEnabled(e.target.checked)}
        />
        Radera färdigspelade avsnitt efter ett antal dagar
      </label>
      <div className="settings-grid">
        <label>
          Antal dagar
          <input
            type="number"
            min={1}
            max={365}
            step={1}
            value={days}
            disabled={!enabled}
            onChange={(e) => setDays(Number(e.target.value))}
          />
        </label>
      </div>
      <p className="sidebar-note">
        Filen måste ligga direkt i poddens mediamapp. En sparad sökväg utanför
        den mappen används inte, och symlänkar följs inte.
      </p>
      <div className="row-actions">
        <button onClick={() => void save()} disabled={busy}>
          Spara
        </button>
      </div>
    </div>
  );
}

function SettingsView(props: {
  paths: AppPaths | null;
  asr: AsrStatus | null;
  busy: boolean;
  globalSkipSilence: boolean;
  setGlobalSkipSilence: (v: boolean) => void;
  engineSettings: AudioEngineSettings | null;
  onEngineSettings: (s: AudioEngineSettings) => void;
  nativeRef: { current: boolean };
  onDownloadModel: () => void;
  onDownloadCli: () => void;
  onDownloadAll: () => void;
  onSideload: () => void;
  onExport: () => void;
  onStatus: (s: string) => void;
}) {
  return (
    <section className="settings">
      <h1>Inställningar</h1>
      <p>
        HIFI podcast är lokal-first. Ingen molnsynk, inga konton och ingen
        omkodning av nedladdade original. BYOK moln-ASR är avstängt.
      </p>
      <label className="check">
        <input
          type="checkbox"
          checked={props.globalSkipSilence}
          onChange={(e) => props.setGlobalSkipSilence(e.target.checked)}
        />
        Hoppa tystnad globalt (av som standard — påverkar inte dynamik)
      </label>
      <FinishedCleanupPanel onStatus={props.onStatus} />
      <AudioEnginePanel
        settings={props.engineSettings}
        onSettings={(s) => {
          props.onEngineSettings(s);
          props.nativeRef.current = s.backend !== "webview";
        }}
        onStatus={props.onStatus}
      />
      <button onClick={props.onSideload}>Importera ljudfil (sideload)…</button>
      <button onClick={props.onExport}>Exportera OPML…</button>
      <UpnpSettingsPanel onStatus={props.onStatus} />
      <AsrSettings
        status={props.asr}
        pathsBin={props.paths?.bin_dir}
        onDownloadCli={props.onDownloadCli}
        onDownloadModel={props.onDownloadModel}
        onDownloadAll={props.onDownloadAll}
        busy={props.busy}
      />
      {props.paths && (
        <dl className="paths">
          <dt>Databas</dt>
          <dd>{props.paths.db_path}</dd>
          <dt>Media-root</dt>
          <dd>{props.paths.media_root}</dd>
          <dt>Appdata</dt>
          <dd>{props.paths.data_dir}</dd>
        </dl>
      )}
    </section>
  );
}

function AudioEnginePanel(props: {
  settings: AudioEngineSettings | null;
  onSettings: (s: AudioEngineSettings) => void;
  onStatus: (s: string) => void;
}) {
  const [devices, setDevices] = useState<import("./types").OutputDeviceInfo[]>([]);
  const [caps, setCaps] = useState<import("./types").EngineCapabilities | null>(null);
  const [busy, setBusy] = useState(false);
  const [backend, setBackend] = useState("native");
  const [outputApi, setOutputApi] = useState("auto");
  const [deviceName, setDeviceName] = useState("");

  const refresh = useCallback(async () => {
    const [list, c, s] = await Promise.all([
      api.engineListDevices(),
      api.engineCapabilities(),
      api.engineGetSettings(),
    ]);
    setDevices(list);
    setCaps(c);
    setBackend(s.backend);
    setOutputApi(s.output_api || "auto");
    setDeviceName(s.device_name);
    props.onSettings(s);
  }, [props]);

  useEffect(() => {
    void refresh().catch((err) => props.onStatus(String(err)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const save = async () => {
    setBusy(true);
    try {
      const next = await api.engineSaveSettings({
        backend,
        output_api: outputApi,
        device_name: deviceName,
        exclusive: outputApi === "wasapi_exclusive",
        skip_silence: props.settings?.skip_silence ?? false,
      });
      props.onSettings(next);
      props.onStatus(
        next.output_api === "asio"
          ? "Ljudmotor sparad. ASIO (FiiO) används när du spelar ett nedladdat avsnitt."
          : "Ljudmotor sparad.",
      );
    } catch (err) {
      props.onStatus(String(err));
    } finally {
      setBusy(false);
    }
  };

  const asioDevices = devices.filter((d) => d.api === "asio");
  const wasapiDevices = devices.filter((d) => d.api === "wasapi");

  return (
    <div className="upnp-panel">
      <h2>Hi-fi-motor (native)</h2>
      <p>
        Primär utgång är <strong>ASIO</strong> — välj <strong>FiiO ASIO Driver</strong> (samma som
        Qobuz Preferred Audio Output). WASAPI delat/exclusive är fallback om ASIO saknas eller
        nekas. WebView2 finns kvar som omkopplare för podd-MVP.
      </p>
      <div className="settings-grid">
        <label>
          Uppspelningsväg
          <select value={backend} onChange={(e) => setBackend(e.target.value)}>
            <option value="native">Native (Symphonia PCM / DSD → ASIO)</option>
            <option value="webview">WebView2 (HTMLMediaElement)</option>
          </select>
        </label>
        <label>
          Utgångs-API
          <select value={outputApi} onChange={(e) => setOutputApi(e.target.value)}>
            <option value="auto">Auto (ASIO först, därefter WASAPI)</option>
            <option value="asio">ASIO (FiiO / andra drivrutiner)</option>
            <option value="wasapi_shared">WASAPI delat</option>
            <option value="wasapi_exclusive">WASAPI exclusive</option>
          </select>
        </label>
        <label>
          Enhet
          <select value={deviceName} onChange={(e) => setDeviceName(e.target.value)}>
            <option value="">
              {asioDevices.some((d) => d.preferred)
                ? "Auto — föredra FiiO ASIO Driver"
                : "Auto — första ASIO, annars systemstandard"}
            </option>
            {asioDevices.map((d) => (
              <option key={d.id} value={d.name}>
                ASIO: {d.name}
                {d.preferred ? " (rekommenderad)" : ""}
              </option>
            ))}
            {wasapiDevices.map((d) => (
              <option key={d.id} value={d.name}>
                WASAPI: {d.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      {asioDevices.length === 0 ? (
        <p className="upnp-warn">
          Inga ASIO-drivrutiner i Windows-registret. Installera <strong>FiiO ASIO Driver</strong>{" "}
          och starta om appen. Tills dess används WASAPI (delat) om du valt Auto.
        </p>
      ) : (
        <p className="sidebar-note">
          Hittade ASIO: {asioDevices.map((d) => d.name).join(", ")}. Öppning sker via IASIO
          (CoCreateInstance), inte bara en etikett.
        </p>
      )}
      <p className="sidebar-note">{caps?.asio_note_sv}</p>
      <p className="sidebar-note">{caps?.dsd_status_sv}</p>
      <p className="sidebar-note">{caps?.stretch_note_sv}</p>
      <div className="row-actions">
        <button onClick={() => void save()} disabled={busy}>
          Spara ljudmotor
        </button>
        <button onClick={() => void refresh()} disabled={busy}>
          Uppdatera enheter
        </button>
      </div>
    </div>
  );
}

function UpnpSettingsPanel(props: { onStatus: (s: string) => void }) {
  const [bind, setBind] = useState("");
  const [port, setPort] = useState(8200);
  const [busy, setBusy] = useState(false);
  const [status, setLocalStatus] = useState<import("./types").UpnpStatus | null>(null);

  const refresh = useCallback(async () => {
    const [settings, st] = await Promise.all([api.getUpnpSettings(), api.upnpStatus()]);
    setBind(settings.bind);
    setPort(settings.port);
    setLocalStatus(st);
  }, []);

  useEffect(() => {
    void refresh().catch((err) => props.onStatus(String(err)));
    // Intentionally not depending on props: onStatus is a stable setState.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh]);

  const save = async () => {
    setBusy(true);
    try {
      const next = await api.saveUpnpSettings({ bind: bind.trim(), port });
      setBind(next.bind);
      setPort(next.port);
      props.onStatus("UPnP-inställningar sparade.");
    } catch (err) {
      props.onStatus(String(err));
    } finally {
      setBusy(false);
    }
  };

  const start = async () => {
    if (bind.trim() === "0.0.0.0") {
      const ok = window.confirm(
        "Du har valt 0.0.0.0. Mediaservern lyssnar då på alla nätverksgränssnitt och biblioteket kan nås utanför LAN om nätverket tillåter det. Fortsätt?",
      );
      if (!ok) return;
    }
    setBusy(true);
    try {
      await api.saveUpnpSettings({ bind: bind.trim(), port });
      const st = await api.startUpnpServer();
      setLocalStatus(st);
      props.onStatus(
        st.location
          ? `Mediaserver igång: ${st.location}`
          : "Mediaserver startad.",
      );
    } catch (err) {
      props.onStatus(String(err));
      await refresh().catch(() => undefined);
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      const st = await api.stopUpnpServer();
      setLocalStatus(st);
      props.onStatus("Mediaserver stoppad.");
    } catch (err) {
      props.onStatus(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="upnp-panel">
      <h2>UPnP/DLNA-mediaserver</h2>
      <p>
        HIFI podcast är servern. Onkyo TX-RZ50 (eller annan renderer) bläddrar i{" "}
        <strong>Podcasts</strong> och <strong>Music</strong> och spelar originalfilerna.
        Music fylls med indexerade album och spår. Musikfilerna ligger kvar där de
        importerades — de kopieras inte till appdata och omkodas inte. Poddträdet
        är oförändrat.
      </p>
      <p className="upnp-warn">
        Servern är bara för ett <strong>betrott hemmanät (LAN)</strong>. SOAP, katalog
        och mediafiler har ingen inloggning (normalt för DLNA). Starta den bara när
        receivern ska spela, och stoppa den efteråt. Den lyssnar inte på internet som
        standard.
      </p>
      <div className="settings-grid">
        <label>
          Bindningsadress
          <input
            value={bind}
            onChange={(e) => setBind(e.target.value)}
            placeholder="Tom = upptäckt LAN-IPv4"
            spellCheck={false}
          />
        </label>
        <label>
          Port
          <input
            type="number"
            min={1}
            max={65535}
            value={port}
            onChange={(e) => setPort(Number(e.target.value) || 8200)}
          />
        </label>
      </div>
      {bind.trim() === "0.0.0.0" ? (
        <p className="upnp-warn">
          Du har valt <strong>0.0.0.0</strong> (alla nätverksgränssnitt). Biblioteket
          kan då nås även utanför LAN om brandvägg eller routing tillåter det. Använd
          det bara medvetet. Lämna fältet tomt för att binda enbart till upptäckt
          LAN-IPv4.
        </p>
      ) : (
        <p className="sidebar-note">
          Tom bindningsadress = HTTP och SSDP (UDP 1900) på upptäckt LAN-IPv4, inte
          0.0.0.0. Skriv 0.0.0.0 bara om du medvetet vill lyssna på alla gränssnitt.
        </p>
      )}
      <div className="row-actions">
        <button onClick={() => void save()} disabled={busy}>
          Spara
        </button>
        <button className="primary" onClick={() => void start()} disabled={busy || status?.running}>
          Starta mediaserver
        </button>
        <button onClick={() => void stop()} disabled={busy || !status?.running}>
          Stoppa mediaserver
        </button>
      </div>
      <p className="sidebar-note">
        {status?.running
          ? `Igång på ${status.location ?? `${status.bind}:${status.port}`}${
              status.lan_ip ? ` (LAN ${status.lan_ip})` : ""
            } — lyssnar på ${status.bind}:${status.port}.`
          : `Stoppad. Standardport 8200. Upptäckt LAN: ${status?.lan_ip ?? "okänt"}.`}
        {status?.error ? ` Fel: ${status.error}` : ""}
      </p>
      <p className="sidebar-note">
        På receivern: HIFI podcast → Music → album → spår. Servern öppnar bara den
        indexerade filen för spår-id:t, med samma spärrar som lokal uppspelning. En
        annan sökväg i adressen avvisas. Ingen symlink ut, inte utanför den indexerade
        filen, och högst 2 GB.
      </p>
      <p className="sidebar-note">
        Windows-brandvägg: tillåt HIFI podcast för privat nätverk när Windows frågar.
        Öppna TCP-porten (8200) och UDP 1900 (SSDP) om receivern inte ser servern. Inget
        administratörskonto krävs för att starta servern. Appen lägger inte till
        brandväggsregler.
      </p>
    </div>
  );
}
