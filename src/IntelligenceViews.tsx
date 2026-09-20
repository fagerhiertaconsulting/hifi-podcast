import { useEffect, useMemo, useState } from "react";
import { api } from "./api";
import type {
  AsrStatus,
  Bookmark,
  Chapter,
  InboxItem,
  JobRow,
  LibraryStats,
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
  jobs?: JobRow[];
}) {
  const [segments, setSegments] = useState<TranscriptSegment[]>([]);
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<TranscriptSegment[] | null>(null);

  const job = props.jobs?.find(
    (j) => j.episode_id === props.episodeId && j.job_type === "transcribe",
  );

  useEffect(() => {
    if (props.episodeId == null) {
      setSegments([]);
      setChapters([]);
      return;
    }
    void Promise.all([
      api.listSegments(props.episodeId),
      api.listChapters(props.episodeId),
    ]).then(([s, c]) => {
      setSegments(s);
      setChapters(c);
    });
  }, [props.episodeId, job?.id, job?.state]);

  const emptyHint = (() => {
    if (props.episodeId == null) return "Spela ett avsnitt.";
    if (job?.state === "running") {
      return job.error_msg
        ? `Transkriberar (lokal Whisper)… ${job.error_msg}`
        : "Transkriberar med lokal Whisper. Ett 40-minutersavsnitt kan ta 15–60 minuter på CPU.";
    }
    if (job?.state === "queued") {
      return "Transkribering väntar i kön. Ett jobb i taget — se Statistik → Jobbkö.";
    }
    if (job?.state === "error") {
      return `Senaste Whisper-försöket misslyckades: ${job.error_msg ?? "okänt fel"}`;
    }
    return "Inget transkript ännu. Ladda ner avsnittet och tryck Transkribera.";
  })();

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
          placeholder="Sök i transkript"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button
          disabled={!query.trim()}
          onClick={() => {
            void api.searchTranscripts(query.trim()).then(setHits);
          }}
        >
          Sök
        </button>
      </div>
      {hits && (
        <ul className="hits">
          {hits.length === 0 && <li className="empty">Inga träffar.</li>}
          {hits.map((h) => (
            <li key={h.id}>
              <button onClick={() => props.onSeekMs(h.start_ms)}>
                {formatClock(h.start_ms)} · {h.text}
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
              <li key={ch.id}>
                <button onClick={() => props.onSeekMs(ch.start_ms)}>
                  {formatClock(ch.start_ms)} · {ch.title}
                  <span> ({ch.source})</span>
                </button>
              </li>
            ))}
          </ul>
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
      <p className="empty">Nya avsnitt hamnar här tills du köar eller arkiverar dem.</p>
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
        Ett Whisper-jobb i taget. <code>running</code> betyder att Whisper
        arbetar (GPU om CUDA är installerad, annars CPU). Äldre rader med error
        är tidigare försök.
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
