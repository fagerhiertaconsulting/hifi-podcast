//! Decode → buffer → output. f32 pipeline, unity gain, opt-in skip-silence and stretch.

use super::decode::{to_stereo, Decoder};
use super::exclusive::ExclusiveStream;
use super::formats::{self, FormatKind, FormatSupport};
use super::output::{self, OutputDeviceInfo, SharedStream};
use super::resample::RateConvert;
use super::ring::FrameRing;
use super::skip::SkipSilence;
use super::stretch::TimeStretch;
use super::EngineError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
pub struct ProbeInfo {
    pub path: String,
    pub support: FormatSupport,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub duration_ms: Option<u64>,
    pub codec: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineStatus {
    pub loaded: bool,
    pub playing: bool,
    pub ended: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: String,
    pub path: String,
    pub exclusive: bool,
    pub output_api: String,
    pub device_name: String,
    pub output_rate: u32,
    pub resampled: bool,
    pub speed: f64,
    pub skip_silence: bool,
    pub error: Option<String>,
}

impl Default for EngineStatus {
    fn default() -> Self {
        Self {
            loaded: false,
            playing: false,
            ended: false,
            position_ms: 0,
            duration_ms: 0,
            sample_rate: 0,
            channels: 0,
            codec: String::new(),
            path: String::new(),
            exclusive: false,
            output_api: "wasapi_shared".into(),
            device_name: String::new(),
            output_rate: 0,
            resampled: false,
            speed: 1.0,
            skip_silence: false,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineCapabilities {
    pub backend_default: &'static str,
    pub pcm_mimes: Vec<String>,
    pub dsd_status_sv: &'static str,
    pub asio_available: bool,
    pub exclusive_available: bool,
    pub asio_note_sv: &'static str,
    pub exclusive_note_sv: &'static str,
    pub stretch_note_sv: &'static str,
}

#[derive(Debug, Clone)]
pub struct OpenParams {
    pub path: PathBuf,
    pub speed: f64,
    pub skip_silence: bool,
    pub output_api: String,
    pub device_name: String,
}

enum Cmd {
    Play,
    Pause,
    Seek(u64),
    Speed(f64),
    SkipSilence(bool),
    Stop,
}

struct Session {
    cmd: Sender<Cmd>,
    playing: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    _shared: Option<SharedStream>,
    _exclusive: Option<ExclusiveStream>,
    _asio: Option<super::asio_host::AsioStream>,
}

#[derive(Clone)]
pub struct Handle {
    session: Arc<Mutex<Option<Session>>>,
    status: Arc<Mutex<EngineStatus>>,
    app: Arc<Mutex<Option<AppHandle>>>,
    hwnd: Arc<Mutex<Option<usize>>>,
}

impl Handle {
    pub fn new() -> Self {
        Self {
            session: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(EngineStatus::default())),
            app: Arc::new(Mutex::new(None)),
            hwnd: Arc::new(Mutex::new(None)),
        }
    }

    pub fn bind_app(&self, app: AppHandle) {
        *self.app.lock().expect("engine app") = Some(app);
    }

    pub fn set_hwnd(&self, hwnd: usize) {
        *self.hwnd.lock().expect("engine hwnd") = Some(hwnd);
    }

    pub fn capabilities() -> EngineCapabilities {
        EngineCapabilities {
            backend_default: "native",
            pcm_mimes: formats::native_pcm_mimes()
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            dsd_status_sv: "DSD/SACD (.dsf/.dff) känns igen men avkodas inte. Native DSD via ASIO (DoP eller DSD-sampletyp) är uppskjutet. FLAC/PCM går till ASIO som PCM. Originalfilen rörs inte.",
            asio_available: super::asio_host::asio_available(),
            exclusive_available: output::exclusive_available(),
            asio_note_sv: "Primär hi-fi-utgång är ASIO (FiiO ASIO Driver, samma som i Qobuz). Listan läses från HKLM\\SOFTWARE\\ASIO och strömmen öppnas via IASIO. WASAPI delat/exclusive är fallback om ASIO saknas eller nekas.",
            exclusive_note_sv: if output::exclusive_available() {
                "Exklusivt WASAPI är fallback när ASIO inte kan öppnas. Intern kedja är f32. Misslyckad öppning blir ett fel — ingen tyst omsampling i exclusive."
            } else {
                "Exklusivt WASAPI är Windows-only."
            },
            stretch_note_sv: "Hastighet ≠ 1× använder WSOLA (tonartsstabil). ASIO + hastighet ≠ 1 är inte bit-perfect.",
        }
    }

    pub fn list_devices() -> Vec<OutputDeviceInfo> {
        output::list_output_devices()
    }

    pub fn status(&self) -> EngineStatus {
        self.status.lock().expect("engine status").clone()
    }

    pub fn probe(path: &Path) -> Result<ProbeInfo, EngineError> {
        let support = formats::support_for(path, None);
        if support.kind == FormatKind::Dsd {
            return Ok(ProbeInfo {
                path: path.display().to_string(),
                support,
                sample_rate: None,
                channels: None,
                duration_ms: None,
                codec: Some("DSD".into()),
            });
        }
        if !support.native_decode {
            return Ok(ProbeInfo {
                path: path.display().to_string(),
                support,
                sample_rate: None,
                channels: None,
                duration_ms: None,
                codec: None,
            });
        }
        let dec = Decoder::open(path)?;
        let spec = dec.spec().clone();
        Ok(ProbeInfo {
            path: path.display().to_string(),
            support,
            sample_rate: Some(spec.sample_rate),
            channels: Some(spec.channels),
            duration_ms: spec.duration_ms,
            codec: Some(spec.codec),
        })
    }

    pub fn open(&self, params: OpenParams) -> Result<EngineStatus, EngineError> {
        self.stop();
        let path = params.path;
        let speed = params.speed;
        let skip_silence = params.skip_silence;
        let probe = Self::probe(&path)?;
        if probe.support.kind == FormatKind::Dsd {
            return Err(EngineError::dsd());
        }
        if !probe.support.native_decode {
            return Err(EngineError::msg(probe.support.note_sv.clone()));
        }

        let decoder = Decoder::open(&path)?;
        let spec = decoder.spec().clone();
        let channels_stereo: u16 = 2;
        let playing = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let ring = Arc::new(FrameRing::new(2));
        let hwnd = *self.hwnd.lock().expect("engine hwnd");

        let opened = open_output(
            &params.output_api,
            &params.device_name,
            spec.sample_rate,
            channels_stereo,
            hwnd,
            ring.clone(),
            playing.clone(),
        )?;

        let resampled = output::needs_resample(spec.sample_rate, opened.out_rate);
        {
            let mut st = self.status.lock().expect("engine status");
            *st = EngineStatus {
                loaded: true,
                playing: false,
                ended: false,
                position_ms: 0,
                duration_ms: spec.duration_ms.unwrap_or(0),
                sample_rate: spec.sample_rate,
                channels: spec.channels,
                codec: spec.codec.clone(),
                path: path.display().to_string(),
                exclusive: opened.exclusive,
                output_api: opened.api.clone(),
                device_name: opened.device_name.clone(),
                output_rate: opened.out_rate,
                resampled,
                speed,
                skip_silence,
                error: None,
            };
        }

        let (tx, rx) = mpsc::channel();
        let status = self.status.clone();
        let app = self.app.clone();
        let stop_w = stop.clone();
        let playing_w = playing.clone();
        let worker = std::thread::Builder::new()
            .name("hifi-decode".into())
            .spawn(move || {
                decode_loop(
                    decoder,
                    rx,
                    ring,
                    status,
                    app,
                    stop_w,
                    playing_w,
                    speed,
                    skip_silence,
                    spec.sample_rate,
                    opened.out_rate,
                    resampled,
                );
            })
            .map_err(|e| EngineError::msg(format!("Avkodningstråd: {e}")))?;

        *self.session.lock().expect("engine session") = Some(Session {
            cmd: tx,
            playing,
            stop,
            worker: Some(worker),
            _shared: opened.shared,
            _exclusive: opened.exclusive_stream,
            _asio: opened.asio,
        });
        Ok(self.status())
    }

    pub fn play(&self) -> Result<(), EngineError> {
        self.send(Cmd::Play)
    }

    pub fn pause(&self) -> Result<(), EngineError> {
        self.send(Cmd::Pause)
    }

    pub fn seek_ms(&self, ms: u64) -> Result<(), EngineError> {
        self.send(Cmd::Seek(ms))
    }

    pub fn set_speed(&self, speed: f64) -> Result<(), EngineError> {
        self.send(Cmd::Speed(speed))
    }

    pub fn set_skip_silence(&self, enabled: bool) -> Result<(), EngineError> {
        self.send(Cmd::SkipSilence(enabled))
    }

    pub fn stop(&self) {
        if let Ok(mut g) = self.session.lock() {
            if let Some(mut s) = g.take() {
                s.stop.store(true, Ordering::Relaxed);
                s.playing.store(false, Ordering::Relaxed);
                let _ = s.cmd.send(Cmd::Stop);
                if let Some(h) = s.worker.take() {
                    let _ = h.join();
                }
            }
        }
        if let Ok(mut st) = self.status.lock() {
            st.playing = false;
            st.loaded = false;
        }
    }

    fn send(&self, cmd: Cmd) -> Result<(), EngineError> {
        let g = self.session.lock().expect("engine session");
        let Some(s) = g.as_ref() else {
            return Err(EngineError::msg("Ingen native-uppspelning är laddad."));
        };
        s.cmd
            .send(cmd)
            .map_err(|_| EngineError::msg("Motorn svarar inte."))
    }
}

struct OpenedOutput {
    shared: Option<SharedStream>,
    exclusive_stream: Option<ExclusiveStream>,
    asio: Option<super::asio_host::AsioStream>,
    out_rate: u32,
    exclusive: bool,
    api: String,
    device_name: String,
}

fn wasapi_device_name(name: &str) -> Option<&str> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    if let Some(rest) = n.strip_prefix("wasapi:") {
        return Some(rest);
    }
    if n.starts_with("asio:") || super::asio_host::is_preferred_hi_fi_name(n) {
        return None;
    }
    Some(n)
}

fn asio_driver_name(name: &str) -> Option<&str> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    if let Some(rest) = n.strip_prefix("asio:") {
        return Some(rest);
    }
    if n.starts_with("wasapi:") {
        return None;
    }
    Some(n)
}

fn open_output(
    api: &str,
    device_name: &str,
    sample_rate: u32,
    channels: u16,
    hwnd: Option<usize>,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<OpenedOutput, EngineError> {
    let api = api.trim().to_ascii_lowercase();
    match api.as_str() {
        "asio" => open_asio(device_name, sample_rate, hwnd, ring, playing),
        "wasapi_exclusive" | "exclusive" => {
            open_exclusive(device_name, sample_rate, channels, ring, playing)
        }
        "wasapi_shared" | "wasapi" | "shared" => {
            open_shared(device_name, sample_rate, channels, ring, playing)
        }
        _ => match open_asio(device_name, sample_rate, hwnd, ring.clone(), playing.clone()) {
            Ok(s) => Ok(s),
            Err(asio_err) => match open_exclusive(
                device_name,
                sample_rate,
                channels,
                ring.clone(),
                playing.clone(),
            ) {
                Ok(s) => Ok(s),
                Err(_) => open_shared(device_name, sample_rate, channels, ring, playing).map_err(
                    |wasapi_err| EngineError::msg(format!("ASIO: {asio_err} WASAPI: {wasapi_err}")),
                ),
            },
        },
    }
}

fn open_asio(
    device_name: &str,
    sample_rate: u32,
    hwnd: Option<usize>,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<OpenedOutput, EngineError> {
    let name = asio_driver_name(device_name);
    let stream = super::asio_host::start_asio(name, sample_rate, hwnd, ring, playing)?;
    let out_rate = stream.sample_rate;
    let device_name = stream.driver_name.clone();
    Ok(OpenedOutput {
        shared: None,
        exclusive_stream: None,
        asio: Some(stream),
        out_rate,
        exclusive: true,
        api: "asio".into(),
        device_name,
    })
}

fn open_exclusive(
    device_name: &str,
    sample_rate: u32,
    channels: u16,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<OpenedOutput, EngineError> {
    let name = wasapi_device_name(device_name);
    let stream = super::exclusive::start_exclusive(name, sample_rate, channels, ring, playing)?;
    let out_rate = stream.sample_rate;
    Ok(OpenedOutput {
        shared: None,
        exclusive_stream: Some(stream),
        asio: None,
        out_rate,
        exclusive: true,
        api: "wasapi_exclusive".into(),
        device_name: name.unwrap_or("").to_string(),
    })
}

fn open_shared(
    device_name: &str,
    sample_rate: u32,
    channels: u16,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<OpenedOutput, EngineError> {
    let name = wasapi_device_name(device_name);
    let stream = output::start_shared(name, sample_rate, channels, ring, playing)?;
    let out_rate = stream.sample_rate;
    Ok(OpenedOutput {
        exclusive_stream: None,
        asio: None,
        out_rate,
        exclusive: false,
        api: "wasapi_shared".into(),
        device_name: name.unwrap_or("").to_string(),
        shared: Some(stream),
    })
}

fn decode_loop(
    mut decoder: Decoder,
    rx: Receiver<Cmd>,
    ring: Arc<FrameRing>,
    status: Arc<Mutex<EngineStatus>>,
    app: Arc<Mutex<Option<AppHandle>>>,
    stop: Arc<AtomicBool>,
    playing: Arc<AtomicBool>,
    mut speed: f64,
    skip_on: bool,
    source_rate: u32,
    out_rate: u32,
    resampled: bool,
) {
    let mut stretch = TimeStretch::new(2);
    stretch.set_speed(speed);
    let mut skip = SkipSilence::default();
    skip.set_enabled(skip_on);
    let mut resampler = if resampled {
        RateConvert::new(source_rate, out_rate, 2).ok()
    } else {
        None
    };
    let mut source_consumed: u64 = 0;
    let mut ended = false;
    let mut last_emit = std::time::Instant::now();

    while !stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Play => {
                    playing.store(true, Ordering::Relaxed);
                    ended = false;
                }
                Cmd::Pause => playing.store(false, Ordering::Relaxed),
                Cmd::Stop => {
                    stop.store(true, Ordering::Relaxed);
                    playing.store(false, Ordering::Relaxed);
                }
                Cmd::Speed(s) => {
                    speed = s.clamp(0.5, 4.0);
                    stretch.set_speed(speed);
                    if let Ok(mut st) = status.lock() {
                        st.speed = speed;
                    }
                }
                Cmd::SkipSilence(on) => {
                    skip.set_enabled(on);
                    if let Ok(mut st) = status.lock() {
                        st.skip_silence = on;
                    }
                }
                Cmd::Seek(ms) => {
                    ring.clear();
                    stretch.reset();
                    skip.reset();
                    if let Some(r) = resampler.as_mut() {
                        r.reset();
                    }
                    if let Ok(actual) = decoder.seek_ms(ms) {
                        source_consumed = actual * source_rate as u64 / 1000;
                    }
                    ended = false;
                }
            }
        }

        let high_water = (out_rate as usize) * 2 / 5; // ~400 ms
        if playing.load(Ordering::Relaxed) && ring.len_frames() < high_water && !ended {
            match decoder.read_interleaved((source_rate as usize / 10) * decoder.spec().channels as usize)
            {
                Ok(raw) if raw.is_empty() => {
                    ended = decoder.is_eof();
                    if ended {
                        playing.store(false, Ordering::Relaxed);
                        emit_ended(&app, &status);
                    }
                }
                Ok(raw) => {
                    let stereo = to_stereo(&raw, decoder.spec().channels);
                    let frames = (stereo.len() / 2) as u64;
                    let block_ms = frames as f32 * 1000.0 / source_rate as f32;
                    let extra_ms = skip.observe(&stereo, block_ms);
                    source_consumed += frames;
                    let pcm = stereo;
                    if extra_ms > 0.0 {
                        let skip_frames =
                            ((extra_ms / 1000.0) * source_rate as f32) as u64;
                        let pos_ms = source_consumed * 1000 / source_rate.max(1) as u64;
                        if decoder.seek_ms(pos_ms + extra_ms as u64).is_ok() {
                            source_consumed += skip_frames;
                        }
                    }
                    let stretched = stretch.push(&pcm);
                    let out = if let Some(r) = resampler.as_mut() {
                        r.push_interleaved(&stretched)
                    } else {
                        stretched
                    };
                    if !out.is_empty() {
                        ring.push_interleaved(&out);
                    }
                    let _ = pcm;
                }
                Err(err) => {
                    if let Ok(mut st) = status.lock() {
                        st.error = Some(err.to_string());
                        st.playing = false;
                    }
                    playing.store(false, Ordering::Relaxed);
                }
            }
        }

        if last_emit.elapsed() >= Duration::from_millis(80) {
            last_emit = std::time::Instant::now();
            let ring_out = ring.len_frames() as f64;
            let buffered_source = if speed > 0.01 {
                ring_out * speed * (source_rate as f64 / out_rate.max(1) as f64)
            } else {
                0.0
            };
            let audible = source_consumed.saturating_sub(buffered_source as u64);
            let pos_ms = audible * 1000 / source_rate.max(1) as u64;
            if let Ok(mut st) = status.lock() {
                st.position_ms = pos_ms;
                st.playing = playing.load(Ordering::Relaxed);
                st.ended = ended;
                st.speed = speed;
            }
            emit_tick(&app, &status);
        }

        std::thread::sleep(Duration::from_millis(4));
    }
}

fn emit_tick(app: &Arc<Mutex<Option<AppHandle>>>, status: &Arc<Mutex<EngineStatus>>) {
    let Ok(st) = status.lock() else {
        return;
    };
    if let Ok(appg) = app.lock() {
        if let Some(app) = appg.as_ref() {
            let _ = app.emit("engine-tick", st.clone());
        }
    }
}

fn emit_ended(app: &Arc<Mutex<Option<AppHandle>>>, status: &Arc<Mutex<EngineStatus>>) {
    if let Ok(mut st) = status.lock() {
        st.ended = true;
        st.playing = false;
        let snap = st.clone();
        drop(st);
        if let Ok(appg) = app.lock() {
            if let Some(app) = appg.as_ref() {
                let _ = app.emit("engine-ended", snap);
            }
        }
    }
}
