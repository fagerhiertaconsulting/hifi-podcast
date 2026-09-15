import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { attachSkipSilence, configureHiFiElement } from "./audio/engine";
import type {
  AppPaths,
  Episode,
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

  const [view, setView] = useState<View>("library");
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
  const [position, setPosition] = useState(0);
  const [duration, setDuration] = useState(0);
  const [speed, setSpeed] = useState(1);
  const [globalSkipSilence, setGlobalSkipSilence] = useState(false);
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

  const refreshLibrary = useCallback(async () => {
    const [list, q, p] = await Promise.all([
      api.listPodcasts(),
      api.listQueue(),
      api.appPaths(),
    ]);
    setPodcasts(list);
    setQueue(q);
    setPaths(p);
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
    const audio = audioRef.current;
    if (!audio) return;
    skipRef.current = attachSkipSilence(audio, () => skipEnabledRef.current);
    return () => skipRef.current?.stop();
  }, []);

  useEffect(() => {
    if (sleepMode !== "minutes" || !sleepUntil) return;
    const id = window.setInterval(() => {
      if (Date.now() >= sleepUntil) {
        audioRef.current?.pause();
        setSleepMode("off");
        setSleepUntil(null);
        setStatus("Sovtimer: pausad.");
      }
    }, 1000);
    return () => window.clearInterval(id);
  }, [sleepMode, sleepUntil]);

  const persistNow = useCallback(async () => {
    const audio = audioRef.current;
    const episode = payload?.episode;
    if (!audio || !episode) return;
    await api.savePlaybackState({
      episode_id: episode.id,
      position_ms: Math.floor(audio.currentTime * 1000),
      speed,
      completed: audio.ended || (audio.duration > 0 && audio.currentTime >= audio.duration - 2),
      last_played_at: new Date().toISOString(),
    });
  }, [payload, speed]);

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
        outroFired.current = false;
        audio.src = convertFileSrc(filePath);
        configureHiFiElement(audio, nextSpeed);
        audio.currentTime = Math.max(0, startSec);
        await audio.play();
        setPlaying(true);
        setStatus(`Spelar: ${next.episode.title}`);
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
    [loadEpisodes, refreshLibrary, selectedId],
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
          <button className={view === "queue" ? "active" : ""} onClick={() => setView("queue")}>
            Kö ({queue.length})
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
            podcasts={podcasts}
            selected={selected}
            episodes={episodes}
            busy={busy}
            onSelect={(id) => setSelectedId(id)}
            onAddFeed={async () => {
              setBusy(true);
              try {
                const podcast = await api.addFeed(feedUrl.trim());
                setFeedUrl("");
                setSelectedId(podcast.id);
                await refreshLibrary();
                setStatus(`Prenumererar på ${podcast.title}`);
              } catch (err) {
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
                setStatus(`Importerade ${imported.length} flöden från OPML.`);
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
                setStatus(`Importerade ${imported.length} flöden.`);
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
                await api.refreshFeed(selected.id);
                await loadEpisodes(selected.id);
                await refreshLibrary();
                setStatus("Flödet uppdaterades.");
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
              setStatus("Tillagd i kön.");
            }}
            onSaveSettings={async () => {
              if (!selected) return;
              await api.updatePodcastSettings(selected.id, {
                auto_download: selected.auto_download,
                speed: selected.speed,
                intro_skip_sec: selected.intro_skip_sec,
                outro_skip_sec: selected.outro_skip_sec,
                skip_silence: selected.skip_silence,
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

        {view === "queue" && (
          <QueueView
            queue={queue}
            onPlay={(id) => void playEpisode(id, false)}
            onRemove={async (id) => setQueue(await api.dequeueItem(id))}
            onMove={async (id, dir) => setQueue(await api.moveQueueItem(id, dir))}
          />
        )}

        {view === "settings" && (
          <SettingsView
            paths={paths}
            globalSkipSilence={globalSkipSilence}
            setGlobalSkipSilence={setGlobalSkipSilence}
            onExport={async () => {
              const path = await save({
                defaultPath: "hifi-podcast.opml",
                filters: [{ name: "OPML", extensions: ["opml"] }],
              });
              if (!path) return;
              await api.exportOpmlFile(path);
              setStatus("OPML exporterad.");
            }}
          />
        )}
      </main>

      <footer className="player">
        <audio
          ref={audioRef}
          onTimeUpdate={(e) => {
            const el = e.currentTarget;
            setPosition(el.currentTime);
            setDuration(el.duration || 0);
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
          }}
          onEnded={() => void onEnded()}
          onLoadedMetadata={(e) => setDuration(e.currentTarget.duration || 0)}
        />
        <div className="now">
          <strong>{payload?.episode.title ?? "Inget avsnitt"}</strong>
          <span>{payload?.podcast.title ?? "Välj ett avsnitt att spela"}</span>
        </div>
        <div className="transport">
          <button onClick={() => skip(audioRef.current, -30)} title="Bakåt 30 s">
            −30
          </button>
          <button onClick={() => skip(audioRef.current, -15)} title="Bakåt 15 s">
            −15
          </button>
          <button
            className="play"
            disabled={!payload}
            onClick={() => {
              const audio = audioRef.current;
              if (!audio) return;
              if (audio.paused) void audio.play();
              else audio.pause();
            }}
          >
            {playing ? "Paus" : "Spela"}
          </button>
          <button onClick={() => skip(audioRef.current, 15)} title="Framåt 15 s">
            +15
          </button>
          <button onClick={() => skip(audioRef.current, 30)} title="Framåt 30 s">
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
              if (audioRef.current) audioRef.current.currentTime = value;
              setPosition(value);
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
              if (audioRef.current) configureHiFiElement(audioRef.current, next);
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
        <div className="status">{status}</div>
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
  onSaveSettings: () => void;
  onPatchSelected: (patch: Partial<Podcast>) => void;
}) {
  const selected = props.selected;
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
                <button onClick={props.onSaveSettings}>Spara inställningar</button>
              </div>
              <ul className="episodes">
                {props.episodes.map((episode) => (
                  <li key={episode.id}>
                    <div>
                      <strong>{episode.title}</strong>
                      <span>
                        {formatDate(episode.publish_at)} · {episode.status}
                      </span>
                    </div>
                    <div className="ep-actions">
                      <button onClick={() => props.onDownload(episode.id)}>Ladda ner</button>
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
  globalSkipSilence: boolean;
  setGlobalSkipSilence: (v: boolean) => void;
  onExport: () => void;
}) {
  return (
    <section className="settings">
      <h1>Inställningar</h1>
      <p>
        HIFI podcast är lokal-first. Ingen molnsynk, inga konton och ingen
        omkodning av nedladdade original.
      </p>
      <label className="check">
        <input
          type="checkbox"
          checked={props.globalSkipSilence}
          onChange={(e) => props.setGlobalSkipSilence(e.target.checked)}
        />
        Hoppa tystnad globalt (av som standard — påverkar inte dynamik)
      </label>
      <button onClick={props.onExport}>Exportera OPML…</button>
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
