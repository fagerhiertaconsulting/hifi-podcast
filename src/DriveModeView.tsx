import { useState } from "react";
import {
  DRIVE_LABELS,
  DRIVE_SKIP_SECONDS,
  hiddenSurfaces,
  removalStep,
} from "./driveMode";
import type { QueueItem } from "./types";

const SKIP_LABEL: Record<(typeof DRIVE_SKIP_SECONDS)[number], string> = {
  [-30]: DRIVE_LABELS.back30,
  [-15]: DRIVE_LABELS.back15,
  15: DRIVE_LABELS.forward15,
  30: DRIVE_LABELS.forward30,
};

export function DriveModeView(props: {
  playing: boolean;
  title: string;
  subtitle: string;
  positionLabel: string;
  durationLabel: string;
  queue: QueueItem[];
  onPlayPause: () => void;
  onSkip: (seconds: number) => void;
  onPrevious: () => void;
  onNext: () => void;
  onResume: () => void;
  onPlayQueue: (episodeId: number) => void;
  onRemoveQueue: (queueId: number) => void;
  onExit: () => void;
}) {
  const [pendingId, setPendingId] = useState<number | null>(null);
  const hidden = hiddenSurfaces(true).join(" ");

  const requestRemove = (queueId: number) => {
    const step = removalStep(pendingId, queueId, false);
    setPendingId(step.pendingId);
  };

  const confirmRemove = (queueId: number) => {
    const step = removalStep(pendingId, queueId, true);
    setPendingId(step.pendingId);
    if (step.remove) props.onRemoveQueue(queueId);
  };

  return (
    <section className="drive" data-mode="drive" data-hidden={hidden}>
      <header className="drive-top">
        <p className="drive-kicker">{DRIVE_LABELS.enter}</p>
        <button type="button" className="drive-exit" onClick={props.onExit}>
          {DRIVE_LABELS.exit}
        </button>
      </header>

      <div className="drive-now">
        <h1>{props.title}</h1>
        <p>{props.subtitle}</p>
        <p className="drive-time">
          {props.positionLabel}
          <span> / {props.durationLabel}</span>
        </p>
      </div>

      <div className="drive-transport">
        {DRIVE_SKIP_SECONDS.filter((seconds) => seconds < 0).map((seconds) => (
          <button key={seconds} type="button" onClick={() => props.onSkip(seconds)}>
            {SKIP_LABEL[seconds]}
          </button>
        ))}
        <button type="button" className="drive-play" onClick={props.onPlayPause}>
          {props.playing ? DRIVE_LABELS.pause : DRIVE_LABELS.play}
        </button>
        {DRIVE_SKIP_SECONDS.filter((seconds) => seconds > 0).map((seconds) => (
          <button key={seconds} type="button" onClick={() => props.onSkip(seconds)}>
            {SKIP_LABEL[seconds]}
          </button>
        ))}
      </div>

      <div className="drive-transport drive-transport-secondary">
        <button type="button" onClick={props.onPrevious}>
          {DRIVE_LABELS.previous}
        </button>
        <button type="button" onClick={props.onNext}>
          {DRIVE_LABELS.next}
        </button>
      </div>

      <button type="button" className="drive-resume" onClick={props.onResume}>
        {DRIVE_LABELS.resume}
      </button>

      <div className="drive-queue">
        <h2>{DRIVE_LABELS.queue}</h2>
        {props.queue.length === 0 ? <p>{DRIVE_LABELS.emptyQueue}</p> : null}
        <ol>
          {props.queue.map((item) => {
            const armed = pendingId === item.id;
            return (
              <li key={item.id}>
                <div>
                  <strong>{item.episode_title}</strong>
                  <span>{item.podcast_title}</span>
                </div>
                <div className="drive-row">
                  <button type="button" onClick={() => props.onPlayQueue(item.episode_id)}>
                    {DRIVE_LABELS.play}
                  </button>
                  {armed ? (
                    <>
                      <span>{DRIVE_LABELS.confirmRemove}</span>
                      <button type="button" onClick={() => setPendingId(null)}>
                        {DRIVE_LABELS.cancel}
                      </button>
                      <button type="button" className="danger" onClick={() => confirmRemove(item.id)}>
                        {DRIVE_LABELS.confirm}
                      </button>
                    </>
                  ) : (
                    <button type="button" onClick={() => requestRemove(item.id)}>
                      {DRIVE_LABELS.remove}
                    </button>
                  )}
                </div>
              </li>
            );
          })}
        </ol>
      </div>
    </section>
  );
}
