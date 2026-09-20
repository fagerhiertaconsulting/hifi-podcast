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
  Episode,
  InboxItem,
  JobRow,
  LibraryStats,
  PlayerPayload,
  Podcast,
  QueueItem,
  View,
} from "./types";

const SPEEDS = [0.8, 1, 1.2, 1.5, 1.75, 2, 2.5, 3];

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

  const [view, setView] = useState<View>("library");
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
  const [playing, setPlaying] = useState(false);
  playingRef.current = playing;
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState(0);
  const [speed, setSpeed] = useState(1);
  const [globalSkipSilence, setGlobalSkipSilence] = useState(false);
  const [engineSettings, setEngineSettings] = useState<AudioEngineSettings | null>(null);
  const nativeRef = useRef(true);
  const [sleepMode, setSleepMode] = useState<"off" | "minutes" | "episode">("off");
  const [sleepMinutes, setSleepMinutes] = useState(30);
  const [sleepUntil, setSleepUntil] = useState<number | null>(null);
  const outroFired = useRef(false);

  const selected = useMemo(
    () => podcasts.find((p) => p.id === selectedId) ?? null,
    [podcasts, selectedId],
  );

  const skipSilenceActive =
    globalSkipSilence || Boolean(payload?.podcast.skip_silence);
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

  const loadEpisodes = useCallback(async (podcastId: number) => {
    setEpisodes(await api.listEpisodes(podcastId));
  }, []);

  useEffect(() => {
    void refreshLibrary().catch((err) => setStatus(String(err)));
  }, [refreshLibrary]);

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
  }, [skipSilenceActive, loadPlaybackSource, speed]);

  const persistNow = useCallback(async () => {
    const episode = payload?.episode;
    if (!episode) return;
    const audio = audioRef.current;
    const posSec = nativeRef.current ? positionRef.current : (audio?.currentTime ?? positionRef.current);
    const dur = duration;
    await api.savePlaybackState({
      episode_id: episode.id,
      position_ms: Math.floor(posSec * 1000),
      speed,
      completed: dur > 0 && posSec >= dur - 2,
      last_played_at: new Date().toISOString(),
    });
  }, [payload, speed, duration]);

  const playEpisode = useCallback(
    async (episodeId: number, enqueue = true) => {
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
          next.playback && !next.playback.completed
            ? next.playback.position_ms / 1000
            : next.podcast.intro_skip_sec;
        const nextSpeed = next.playback?.speed || next.podcast.speed || 1;
        setSpeed(nextSpeed);
        setPayload(next);
        adsRef.current = next.podcast.skip_ads ? next.ads.filter((a) => a.enabled) : [];
        void api.smtcNowPlaying(next.episode.title, next.podcast.title, true);
        const useSkip = globalSkipSilence || next.podcast.skip_silence;
        skipModeRef.current = useSkip;
        filePathRef.current = filePath;
        const useNative = (engineSettings?.backend ?? "native") !== "webview";
        nativeRef.current = useNative;
        let statusLine: string;
        if (useNative) {
          audio.pause();
          audio.removeAttribute("src");
          audio.load();
          try {
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
            const apiLabel =
              engineSettings?.output_api === "asio" ||
              !engineSettings?.output_api ||
              engineSettings.output_api === "auto"
                ? "ASIO/WASAPI"
                : engineSettings.output_api;
            statusLine = `Spelar native (${apiLabel}): ${next.episode.title}`;
          } catch (nativeErr) {
            nativeRef.current = false;
            await api.engineStop();
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

  const onEnded = useCallback(async () => {
    await persistNow();
    if (sleepMode === "episode") {
      setSleepMode("off");
      setStatus("Sovtimer: avsnittet är slut.");
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
  }, [persistNow, playEpisode, payload, queue, sleepMode]);

  const playEpisodeRef = useRef(playEpisode);
  playEpisodeRef.current = playEpisode;
  const queueRef = useRef(queue);
  queueRef.current = queue;
  const payloadRef = useRef(payload);
  payloadRef.current = payload;

  useEffect(() => {
    const unTick = listen<import("./types").EngineStatus>("engine-tick", (event) => {
      if (!nativeRef.current) return;
      const st = event.payload;
      const pos = st.position_ms / 1000;
      positionRef.current = pos;
      setPosition(pos);
      setDuration((st.duration_ms || 0) / 1000);
      setPlaying(st.playing);
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

  useEffect(() => {
    const un = listen<string>("smtc", (event) => {
      const audio = audioRef.current;
      const cmd = event.payload;
      const native = nativeRef.current;
      if (cmd === "toggle") {
        if (native) {
          if (playingRef.current) void api.enginePause();
          else void api.enginePlay();
          return;
        }
        if (!audio) return;
        if (audio.paused) void audio.play();
        else audio.pause();
      } else if (cmd === "play") {
        if (native) void api.enginePlay();
        else if (audio) void audio.play();
      } else if (cmd === "pause") {
        if (native) void api.enginePause();
        else audio?.pause();
      } else if (cmd === "next") {
        const q = queueRef.current;
        const currentId = payloadRef.current?.episode.id;
        const idx = q.findIndex((item) => item.episode_id === currentId);
        const nextItem = idx >= 0 ? q[idx + 1] : q[0];
        if (nextItem && nextItem.episode_id !== currentId) {
          void playEpisodeRef.current(nextItem.episode_id, false);
        } else {
          console.info("SMTC next: ingen nästa post i kön.");
        }
      } else if (cmd === "previous") {
        if (native && positionRef.current > 3) {
          void api.engineSeek(0);
          return;
        }
        if (audio && !native && audio.currentTime > 3) {
          audio.currentTime = 0;
          return;
        }
        const q = queueRef.current;
        const currentId = payloadRef.current?.episode.id;
        const idx = q.findIndex((item) => item.episode_id === currentId);
        if (idx > 0) {
          void playEpisodeRef.current(q[idx - 1].episode_id, false);
        } else if (native) {
          void api.engineSeek(0);
        } else if (audio) {
          audio.currentTime = 0;
        } else {
          console.info("SMTC previous: ingen föregående post i kön.");
        }
      }
    });
    return () => {
      void un.then((fn) => fn());
    };
  }, []);

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

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="mark">HF</span>
          <div>
            <strong>HIFI podcast</strong>
            <p>Lokal hi-fi-spelare</p>
          </div>
        </div>
        <nav>
          <button className={view === "library" ? "active" : ""} onClick={() => setView("library")}>
            Bibliotek
          </button>
          <button className={view === "inbox" ? "active" : ""} onClick={() => setView("inbox")}>
            Inkorg ({inbox.length})
          </button>
          <button className={view === "queue" ? "active" : ""} onClick={() => setView("queue")}>
            Kö ({queue.length})
          </button>
          <button className={view === "stats" ? "active" : ""} onClick={() => setView("stats")}>
            Statistik
          </button>
          <button className={view === "settings" ? "active" : ""} onClick={() => setView("settings")}>
            Inställningar
          </button>
        </nav>
        <div className="sidebar-note">
          Inga konton. Allt stannar på den här datorn.
        </div>
      </aside>

      <main className="main">
        {view === "library" && (
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
                  "Transkribering startad. Ett jobb i taget — följ status under Statistik eller i transkriptpanelen. CPU kan ta 15–60 min för ett helt avsnitt.",
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

        {view === "stats" && (
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

        {view === "settings" && (
          <SettingsView
            paths={paths}
            asr={asr}
            busy={busy}
            globalSkipSilence={globalSkipSilence}
            setGlobalSkipSilence={(v) => {
              setGlobalSkipSilence(v);
              if (nativeRef.current) void api.engineSetSkipSilence(v);
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
                filters: [{ name: "Ljud", extensions: ["mp3", "m4a", "aac", "ogg", "opus", "flac", "wav"] }],
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

      <TranscriptPane
        episodeId={payload?.episode.id ?? null}
        positionMs={Math.floor(position * 1000)}
        jobs={jobs}
        onSeekMs={(ms) => {
          if (nativeRef.current) void api.engineSeek(ms);
          else if (audioRef.current) audioRef.current.currentTime = ms / 1000;
        }}
      />

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
            }
          }}
          onEnded={() => void onEnded()}
          onLoadedMetadata={(e) => setDuration(e.currentTarget.duration || 0)}
        />
        <div className="now">
          <strong>{payload?.episode.title ?? "Inget avsnitt"}</strong>
          <span>{payload?.podcast.title ?? "Välj ett avsnitt att spela"}</span>
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
            disabled={!payload}
            onClick={() => {
              if (nativeRef.current) {
                if (playing) void api.enginePause();
                else void api.enginePlay();
                return;
              }
              const audio = audioRef.current;
              if (!audio) return;
              if (audio.paused) void audio.play();
              else audio.pause();
            }}
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
            <option value="episode">Slut på avsnitt</option>
          </select>
        </label>
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
        <div className="status">
          {status}
          {nativeRef.current && payload ? " · Native" : payload ? " · WebView2" : ""}
        </div>
      </footer>
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
}) {
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
                <label className="check">
                  <input
                    type="checkbox"
                    checked={selected.auto_transcribe}
                    onChange={(e) =>
                      props.onPatchSelected({ auto_transcribe: e.target.checked })
                    }
                  />
                  Auto-transkribera (av som standard; kräver whisper-cli + modell)
                </label>
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
                  <li key={episode.id}>
                    <div>
                      <strong>{episode.title}</strong>
                      <span>
                        {formatDate(episode.publish_at)} · {episode.status}
                        {episode.inbox ? " · inkorg" : ""}
                      </span>
                    </div>
                    <div className="ep-actions">
                      <button onClick={() => props.onDownload(episode.id)}>Ladda ner</button>
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
            <option value="native">Native (Symphonia → ASIO/WASAPI)</option>
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
        <strong>Podcasts</strong> och spelar originalfilerna. Mappen{" "}
        <strong>Music</strong> är tom tills musikmodulen finns. Ingen omkodning.
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
        Windows-brandvägg: tillåt HIFI podcast för privat nätverk när Windows frågar.
        Öppna TCP-porten (8200) och UDP 1900 (SSDP) om receivern inte ser servern. Inget
        administratörskonto krävs för att starta servern.
      </p>
    </div>
  );
}
