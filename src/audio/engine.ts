/**
 * MVP-uppspelning via WebView2/Chromium.
 * - Originalfilen spelas som den är (ingen omkodning).
 * - preservesPitch = true (hastighet utan tonartsändring när motorn stöder det).
 * - Ingen dynamikkompression.
 * - Hoppa tystnad är opt-in och görs genom seek, inte genom att skriva ny fil.
 */

export type SkipSilenceHandle = {
  stop: () => void;
};

export function configureHiFiElement(audio: HTMLAudioElement, speed: number) {
  audio.preservesPitch = true;
  if ("mozPreservesPitch" in audio) {
    (audio as HTMLAudioElement & { mozPreservesPitch?: boolean }).mozPreservesPitch = true;
  }
  audio.playbackRate = speed;
  audio.volume = 1;
}

const graphs = new WeakMap<
  HTMLAudioElement,
  { ctx: AudioContext; analyser: AnalyserNode }
>();

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
    const resume = () => {
      void ctx.resume();
    };
    audio.addEventListener("play", resume);
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
