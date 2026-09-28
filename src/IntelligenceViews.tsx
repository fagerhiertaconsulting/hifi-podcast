import { useEffect, useMemo, useState } from "react";
import { api } from "./api";
import type {
  AdSegment,
  AsrStatus,
  Bookmark,
  Chapter,
  FfmpegStatus,
  InboxItem,
  JobRow,
  LibraryStats,
  TranscriptSearchHit,
  TranscriptSegment,
} from "./types";

export function formatClock(ms: number) {
  const s = Math.max(0, Math.floor(ms / 1000));
  const m = Math.floor(s / 60);
  const h = Math.floor(m / 60);
  const sec = (s % 60).toString().padStart(2, "0");
  if (h > 0) return `${h}:${(m % 60).toString().padStart(2, "0")}:${sec}`;
  return `${m}:${sec}`;
}

export function TranscriptPane(props: {
  episodeId: number | null;
  positionMs: number;
  onSeekMs: (ms: number) => void;
  onOpenHit: (hit: TranscriptSearchHit) => void;
  onAdsChanged?: (ads: AdSegment[]) => void;
  onNotice?: (message: string) => void;
  jobs?: JobRow[];
}) {
  const [segments, setSegments] = useState<TranscriptSegment[]>([]);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [chapterDrafts, setChapterDrafts] = useState<Record<number, string>>({});
  const [ads, setAds] = useState<AdSegment[]>([]);
  const [ffmpeg, setFfmpeg] = useState<FfmpegStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<TranscriptSearchHit[] | null>(null);

  const job = props.jobs?.find(
    (j) => j.episode_id === props.episodeId && j.job_type === "transcribe",
  );

  useEffect(() => {
    if (props.episodeId == null) {
      setSegments([]);
      setChapters([]);
      setChapterDrafts({});
      setAds([]);
      return;
    }
    const episodeId = props.episodeId;
    void Promise.all([
      api.listSegments(episodeId),
      api.listChapters(episodeId),
      api.listAds(episodeId),
      api.ffmpegStatus(),
    ]).then(([s, c, a, ff]) => {
      setSegments(s);
      setChapters(c);
      setChapterDrafts(Object.fromEntries(c.map((ch) => [ch.id, ch.title])));
      setAds(a);
      setFfmpeg(ff);
      props.onAdsChanged?.(a);
    });
  }, [props.episodeId, job?.id, job?.state]);

  const emptyHint = (() => {
    if (props.episodeId == null) return "Spela ett avsnitt.";
    if (job?.state === "running") {
      return job.error_msg
        ? `Transkriberar (lokal Whisper)… ${job.error_msg}`
        : "Transkriberar med lokal Whisper. Jobbet pausas medan ljud spelas.";
    }
    if (job?.state === "queued") {
      return "Transkribering väntar i kön. Ett jobb i taget — se Statistik → Jobbkö.";
    }
    if (job?.state === "error") {
      return `Senaste Whisper-försöket misslyckades: ${job.error_msg ?? "okänt fel"}`;
    }
    return "Inget transkript ännu. Transkribering köas när nedladdningen är klar. Knappen Transkribera finns kvar.";
  })();

  function adLabel(ad: AdSegment) {
    const flag =
      ad.user_label === "ad"
        ? "detta var reklam"
        : ad.user_label === "not_ad"
          ? "inte reklam"
          : ad.method === "silence"
            ? "tystnad"
            : ad.method;
    return `${flag}${ad.enabled ? "" : ", hoppas inte över"}`;
  }

  function labelAt(
    episodeId: number,
    positionMs: number,
    label: "ad" | "not_ad",
    adId: number | null,
  ) {
    setBusy(true);
    void api
      .setAdFeedback(episodeId, positionMs, label, adId)
      .then((next) => {
        setAds(next);
        props.onAdsChanged?.(next);
        props.onNotice?.(
          label === "ad" ? "Markerat: detta var reklam." : "Markerat: inte reklam.",
        );
      })
      .catch((err) => props.onNotice?.(String(err)))
      .finally(() => setBusy(false));
  }

  const activeId = useMemo(() => {
    const hit = segments.find(
      (s) => props.positionMs >= s.start_ms && props.positionMs < s.end_ms,
    );
    return hit?.id ?? null;
  }, [segments, props.positionMs]);

  return (
    <aside className="transcript">
      <h2>Transkript</h2>
      <div className="row-actions">
        <input
          placeholder="Sök i hela biblioteket"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button
          disabled={!query.trim()}
          onClick={() => {
            void api.searchTranscripts(query.trim(), props.episodeId).then(setHits);
          }}
        >
          Sök
        </button>
      </div>
      {hits && (
        <ul className="hits">
          {hits.length === 0 && <li className="empty">Inga träffar i biblioteket.</li>}
          {hits.map((h) => (
            <li key={h.id}>
              <button onClick={() => props.onOpenHit(h)}>
                {h.podcast_title} · {h.episode_title} · {formatClock(h.start_ms)} · {h.text}
              </button>
            </li>
          ))}
        </ul>
      )}
      {chapters.length > 0 && (
        <>
          <h3>Kapitel</h3>
          <ul className="hits">
            {chapters.map((ch) => (
              <li key={ch.id} className="chapter-edit">
                <button onClick={() => props.onSeekMs(ch.start_ms)}>
                  {formatClock(ch.start_ms)}
                </button>
                <input
                  aria-label={`Kapiteltitel ${formatClock(ch.start_ms)}`}
                  value={chapterDrafts[ch.id] ?? ch.title}
                  onChange={(e) =>
                    setChapterDrafts((prev) => ({ ...prev, [ch.id]: e.target.value }))
                  }
                />
                <button
                  onClick={() => {
                    const title = (chapterDrafts[ch.id] ?? ch.title).trim();
                    void api
                      .updateChapterTitle(ch.id, title)
                      .then((saved) => {
                        setChapters((prev) =>
                          prev.map((row) => (row.id === saved.id ? saved : row)),
                        );
                        setChapterDrafts((prev) => ({ ...prev, [saved.id]: saved.title }));
                        props.onNotice?.("Kapiteltiteln är sparad.");
                      })
                      .catch((err) => props.onNotice?.(String(err)));
                  }}
                >
                  Spara titel
                </button>
              </li>
            ))}
          </ul>
        </>
      )}
      {props.episodeId != null && (
        <>
          <h3>Reklam</h3>
          <p className="empty">
            Fraslistan i transkriptet och tystnad runt 15, 30 eller 60 sekunder
            nära start, mitten eller slutet blir avsnitt som Hoppa reklam hoppar
            över. Inte reklam hoppas inte över. Originalfilen behålls. Klippet
            är en ny fil för privat bruk och kodas inte om.
          </p>
          <div className="ad-actions">
            <button
              disabled={busy}
              onClick={() => {
                if (props.episodeId == null) return;
                void labelAt(props.episodeId, props.positionMs, "ad", null);
              }}
            >
              detta var reklam
            </button>
            <button
              disabled={busy}
              onClick={() => {
                if (props.episodeId == null) return;
                void labelAt(props.episodeId, props.positionMs, "not_ad", null);
              }}
            >
              inte reklam
            </button>
            <button
              disabled={busy}
              onClick={() => {
                if (props.episodeId == null) return;
                setBusy(true);
                void api
                  .scanSilenceAds(props.episodeId)
                  .then((next) => {
                    setAds(next);
                    props.onAdsChanged?.(next);
                    props.onNotice?.("Tystnadsanalysen är klar.");
                  })
                  .catch((err) => props.onNotice?.(String(err)))
                  .finally(() => setBusy(false));
              }}
            >
              Sök reklam via tystnad
            </button>
          </div>
          <ul className="hits">
            {ads.length === 0 && <li className="empty">Inga reklamblock.</li>}
            {ads.map((ad) => (
              <li key={ad.id} className="ad-row">
                <button onClick={() => props.onSeekMs(ad.start_ms)}>
                  {formatClock(ad.start_ms)}–{formatClock(ad.end_ms)} · {adLabel(ad)}
                </button>
                <div className="ad-actions">
                  <button
                    disabled={busy}
                    onClick={() => {
                      if (props.episodeId == null) return;
                      void labelAt(props.episodeId, ad.start_ms, "ad", ad.id);
                    }}
                  >
                    detta var reklam
                  </button>
                  <button
                    disabled={busy}
                    onClick={() => {
                      if (props.episodeId == null) return;
                      void labelAt(props.episodeId, ad.start_ms, "not_ad", ad.id);
                    }}
                  >
                    inte reklam
                  </button>
                </div>
              </li>
            ))}
          </ul>
          <div className="ad-actions">
            <button
              className="primary"
              disabled={busy || props.episodeId == null}
              onClick={() => {
                if (props.episodeId == null) return;
                setBusy(true);
                void api
                  .cutEpisodeAds(props.episodeId)
                  .then((out) => {
                    props.onNotice?.(
                      `Ny fil utan markerad reklam. Originalet är kvar. ${out.cut_path}`,
                    );
                  })
                  .catch((err) => props.onNotice?.(String(err)))
                  .finally(() => setBusy(false));
              }}
            >
              Klipp bort reklam till ny fil
            </button>
            {ffmpeg && !ffmpeg.present && (
              <button
                disabled={busy}
                onClick={() => {
                  setBusy(true);
                  void api
                    .downloadFfmpeg()
                    .then((next) => {
                      setFfmpeg(next);
                      props.onNotice?.(next.hint);
                    })
                    .catch((err) => props.onNotice?.(String(err)))
                    .finally(() => setBusy(false));
                }}
              >
                Ladda ner ffmpeg
              </button>
            )}
          </div>
        </>
      )}
      <ul className="segments">
        {props.episodeId == null && <li className="empty">Spela ett avsnitt.</li>}
        {props.episodeId != null && segments.length === 0 && (
          <li className="empty">{emptyHint}</li>
        )}
        {segments.map((seg) => (
          <li key={seg.id} className={seg.id === activeId ? "active" : ""}>
            <button onClick={() => props.onSeekMs(seg.start_ms)}>
              <span>{formatClock(seg.start_ms)}</span>
              {seg.text}
            </button>
          </li>
        ))}
      </ul>
    </aside>
  );
}

export function InboxView(props: {
  items: InboxItem[];
  onPlay: (id: number) => void;
  onQueue: (id: number) => void;
  onArchive: (id: number) => void;
}) {
  return (
    <section className="queue">
      <h1>Inkorg</h1>
      <p className="empty">
        Bara avsnitt publicerade efter prenumerationen hamnar här. De ligger kvar
        tills du köar eller arkiverar dem.
      </p>
      {props.items.length === 0 && <p className="empty">Inkorgen är tom.</p>}
      <ol>
        {props.items.map((item) => (
          <li key={item.episode.id}>
            <div>
              <strong>{item.episode.title}</strong>
              <span>
                {item.podcast_title}
                {item.download_state ? ` · ${item.download_state}` : ""}
              </span>
            </div>
            <div className="ep-actions">
              <button onClick={() => props.onQueue(item.episode.id)}>Köa</button>
              <button className="primary" onClick={() => props.onPlay(item.episode.id)}>
                Spela
              </button>
              <button onClick={() => props.onArchive(item.episode.id)}>Arkivera</button>
            </div>
          </li>
        ))}
      </ol>
    </section>
  );
}

export function StatsView(props: {
  stats: LibraryStats | null;
  bookmarks: Bookmark[];
  jobs: JobRow[];
  onSeekBookmark: (episodeId: number, ms: number) => void;
  onDeleteBookmark: (id: number) => void;
}) {
  const hours = ((props.stats?.listened_ms ?? 0) / 3600000).toFixed(1);
  return (
    <section className="settings">
      <h1>Statistik & bokmärken</h1>
      {props.stats && (
        <dl className="paths">
          <dt>Podcasts</dt>
          <dd>{props.stats.podcast_count}</dd>
          <dt>Avsnitt</dt>
          <dd>{props.stats.episode_count}</dd>
          <dt>Nedladdade</dt>
          <dd>{props.stats.downloaded_count}</dd>
          <dt>Färdigspelade</dt>
          <dd>{props.stats.completed_count}</dd>
          <dt>Inkorg</dt>
          <dd>{props.stats.inbox_count}</dd>
          <dt>Lyssnad tid (ca)</dt>
          <dd>{hours} h</dd>
        </dl>
      )}
      <h2>Bokmärken</h2>
      <ul className="hits">
        {props.bookmarks.length === 0 && <li className="empty">Inga bokmärken.</li>}
        {props.bookmarks.map((b) => (
          <li key={b.id}>
            <button onClick={() => props.onSeekBookmark(b.episode_id, b.position_ms)}>
              {b.episode_title} · {formatClock(b.position_ms)}
              {b.note ? ` — ${b.note}` : ""}
            </button>
            <button onClick={() => props.onDeleteBookmark(b.id)}>Ta bort</button>
          </li>
        ))}
      </ul>
      <h2>Jobbkö</h2>
      <p className="empty">
        Ett Whisper-jobb i taget. Det pausas medan ljud spelas och fortsätter
        när uppspelningen stannar. <code>running</code> betyder att Whisper
        arbetar (NVIDIA GPU om CUDA är installerad, annars CPU). Äldre rader
        med error är tidigare försök.
      </p>
      <ul className="hits">
        {props.jobs.slice(0, 12).map((j) => (
          <li key={j.id}>
            {j.job_type} · avsnitt {j.episode_id} · {j.state}
            {j.error_msg ? ` — ${j.error_msg}` : ""}
          </li>
        ))}
      </ul>
    </section>
  );
}

export function AsrSettings(props: {
  status: AsrStatus | null;
  pathsBin?: string;
  onDownloadModel: () => void;
  onDownloadCli: () => void;
  onDownloadAll: () => void;
  busy: boolean;
}) {
  return (
    <div className="asr-box">
      <h2>Lokal ASR (whisper.cpp)</h2>
      <p>
        Standardmodell: <code>ggml-small</code>. BYOK moln-ASR är avstängt. CUDA-zippen
        (~640 MB) och modellen hämtas <strong>bara</strong> när du trycker på knapparna
        nedan — aldrig tyst vid prenumeration eller avsnittsnedladdning.
      </p>
      {props.status && (
        <>
          <p>{props.status.hint}</p>
          <dl className="paths">
            <dt>Modell</dt>
            <dd>
              {props.status.model_present ? "finns" : "saknas"} · {props.status.model_path}
            </dd>
            <dt>CLI</dt>
            <dd>
              {props.status.cli_present ? props.status.cli_path : "saknas"}
            </dd>
            <dt>Backend</dt>
            <dd>
              {props.status.cli_backend}
              {props.status.nvidia_present ? " · NVIDIA GPU hittad" : ""}
            </dd>
            {props.pathsBin && (
              <>
                <dt>Bin-mapp</dt>
                <dd>{props.pathsBin}</dd>
              </>
            )}
          </dl>
        </>
      )}
      <div className="row-actions">
        <button disabled={props.busy} onClick={props.onDownloadCli}>
          {props.status?.nvidia_present
            ? "Ladda ner whisper-cli (CUDA)"
            : "Ladda ner whisper-cli"}
        </button>
        <button disabled={props.busy} onClick={props.onDownloadModel}>
          Ladda ner ggml-small
        </button>
        <button disabled={props.busy} className="primary" onClick={props.onDownloadAll}>
          Ladda ner allt
        </button>
      </div>
    </div>
  );
}
