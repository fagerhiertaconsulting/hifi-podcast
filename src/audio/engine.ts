/**
 * MVP-uppspelning via WebView2/Chromium.
 *
 * Default (alltid): HTMLMediaElement direkt till systemets mixer.
 * createMediaElementSource() på asset.localhost ger tyst utgång
 * ("MediaElementAudioSource outputs zeroes due to CORS").
 *
 * Hoppa tystnad (opt-in): same-origin blob: URL + Web Audio.
 * crossOrigin måste sättas INNAN src.
 */

import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export type SkipSilenceHandle = {
  stop: () => void;
};

export function configureHiFiElement(audio: HTMLAudioElement, speed: number) {
  audio.crossOrigin = "anonymous";
  audio.preservesPitch = true;
  if ("mozPreservesPitch" in audio) {
    (audio as HTMLAudioElement & { mozPreservesPitch?: boolean }).mozPreservesPitch =
      true;
  }
  audio.playbackRate = speed;
  audio.volume = 1;
}

export function mimeFromPath(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  switch (ext) {
    case "mp3":
      return "audio/mpeg";
    case "m4a":
    case "m4b":
    case "mp4":
    case "aac":
      return "audio/mp4";
    case "ogg":
    case "opus":
      return "audio/ogg";
    case "flac":
      return "audio/flac";
    case "dsf":
    case "dff":
    case "dsd":
      return "audio/x-dsd";
    default:
      return "application/octet-stream";
  }
}

export async function blobUrlForLocalFile(filePath: string): Promise<string> {
  const bytes = await invoke<number[] | Uint8Array>("read_media_file", {
    path: filePath,
  });
  const data = bytes instanceof Uint8Array ? bytes : Uint8Array.from(bytes);
  const copy = new Uint8Array(data.byteLength);
  copy.set(data);
  const blob = new Blob([copy], { type: mimeFromPath(filePath) });
  return URL.createObjectURL(blob);
}

export function assetUrlForLocalFile(filePath: string): string {
  return convertFileSrc(filePath);
}

const graphs = new WeakMap<
  HTMLAudioElement,
  { ctx: AudioContext; analyser: AnalyserNode }
>();

/**
 * Bind Web Audio only after a same-origin (blob:) src is assigned.
 * Never call this for asset.localhost playback.
 */
export function attachSkipSilence(
  audio: HTMLAudioElement,
  isEnabled: () => boolean,
): SkipSilenceHandle {
  let graph = graphs.get(audio);
  if (!graph) {
    const ctx = new AudioContext();
    const source = ctx.createMediaElementSource(audio);
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 2048;
    analyser.smoothingTimeConstant = 0.4;
    const gain = ctx.createGain();
    gain.gain.value = 1;
    source.connect(analyser);
    analyser.connect(gain);
    gain.connect(ctx.destination);
    audio.addEventListener("play", () => {
      void ctx.resume();
    });
    graph = { ctx, analyser };
    graphs.set(audio, graph);
  }

  const { analyser } = graph;
  const data = new Float32Array(analyser.fftSize);
  let silentMs = 0;
  let last = performance.now();
  let raf = 0;

  const tick = () => {
    const now = performance.now();
    const dt = now - last;
    last = now;
    if (isEnabled() && !audio.paused && !audio.ended) {
      analyser.getFloatTimeDomainData(data);
      let sum = 0;
      for (const sample of data) {
        sum += sample * sample;
      }
      const rms = Math.sqrt(sum / data.length);
      if (rms < 0.012) {
        silentMs += dt;
        if (silentMs > 450) {
          audio.currentTime = Math.min(
            audio.duration || audio.currentTime,
            audio.currentTime + 0.4,
          );
          silentMs = 200;
        }
      } else {
        silentMs = 0;
      }
    } else {
      silentMs = 0;
    }
    raf = requestAnimationFrame(tick);
  };
  raf = requestAnimationFrame(tick);

  return {
    stop: () => {
      cancelAnimationFrame(raf);
    },
  };
}
