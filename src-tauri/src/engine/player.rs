//! Decode → buffer → output. f32 pipeline, unity gain, opt-in skip-silence and stretch.

use super::byte_ring::ByteRing;
use super::decode::{to_stereo, Decoder};
use super::dsd::{self, BitOrder, DsdReader};
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
    pub underruns: u64,
    pub error: Option<String>,
    /// `pcm` or `dsd`
    pub format: String,
    /// e.g. `DSD256`; empty for PCM
    pub dsd_rate: String,
    /// `pcm`, `native_dsd`, or `dop` — never `native_dsd` for converted/DoP
    pub output_path: String,
    pub output_path_sv: String,
    /// Nested ASIO `bufferSwitch` (not underruns).
    pub callback_reentries: u64,
    /// Raw native-DSD negotiation (CanDo/Set/Get, rates, sample_type). Kept when DoP wins.
    pub dsd_diag: String,
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
            underruns: 0,
            error: None,
            format: "pcm".into(),
            dsd_rate: String::new(),
            output_path: "pcm".into(),
            output_path_sv: "PCM".into(),
            callback_reentries: 0,
            dsd_diag: String::new(),
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
            dsd_status_sv: "DSD (.dsf/.dff) spelas native: ASIO native DSD först (FiiO ASIO Driver). Om DAC:n kräver DoP märks utgången DoP — aldrig \"native DSD\". WASAPI exclusive DoP är dokumenterad fallback. Delat WASAPI och WebView2 används inte för DSD. SACD-ISO rippning ingår inte.",
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
            if !support.native_decode {
                return Ok(ProbeInfo {
                    path: path.display().to_string(),
                    support,
                    sample_rate: None,
                    channels: None,
                    duration_ms: None,
                    codec: Some("DSD".into()),
                });
            }
            let r = DsdReader::open(path)?;
            let spec = r.spec().clone();
            return Ok(ProbeInfo {
                path: path.display().to_string(),
                support,
                sample_rate: Some(spec.sample_rate),
                channels: Some(spec.channels),
                duration_ms: Some(spec.duration_ms),
                codec: Some(spec.codec),
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
            return self.open_dsd(path, params.output_api, params.device_name, probe);
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
                underruns: 0,
                error: None,
                format: "pcm".into(),
                dsd_rate: String::new(),
                output_path: if opened.api == "asio" {
                    opened.io_path.clone()
                } else {
                    "pcm".into()
                },
                output_path_sv: "PCM (f32-kedja)".into(),
                callback_reentries: 0,
                dsd_diag: opened.dsd_diag.clone(),
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

    fn open_dsd(
        &self,
        path: PathBuf,
        output_api: String,
        device_name: String,
        probe: ProbeInfo,
    ) -> Result<EngineStatus, EngineError> {
        if !probe.support.native_decode {
            return Err(EngineError::msg(probe.support.note_sv));
        }
        let reader = DsdReader::open(&path)?;
        let spec = reader.spec().clone();
        let file_lsb = spec.bit_order == BitOrder::LsbFirst;
        let playing = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let ring = Arc::new(ByteRing::new(2));
        let hwnd = *self.hwnd.lock().expect("engine hwnd");

        let opened = open_dsd_output(
            &output_api,
            &device_name,
            spec.sample_rate,
            hwnd,
            ring.clone(),
            playing.clone(),
            file_lsb,
        )?;

        let path_sv = match opened.io_path.as_str() {
            "native_dsd" => format!(
                "Native DSD via ASIO ({}) — bit-perfect DSD-bytes, inte PCM-omkodning.",
                opened.device_name
            ),
            "dop" => {
                let mut s = format!(
                    "DoP (DSD over PCM) via {} — inte native DSD. Markörer 0x05/0xFA, 24-bit.",
                    opened.api
                );
                if opened.dsd_diag.contains("native_attempt_failed")
                    || opened.dsd_diag.contains("asio_failed")
                {
                    s.push_str(" Native ASIO misslyckades (dsd_diag).");
                }
                s
            }
            other => format!("Okänd DSD-väg ({other})."),
        };

        {
            let mut st = self.status.lock().expect("engine status");
            *st = EngineStatus {
                loaded: true,
                playing: false,
                ended: false,
                position_ms: 0,
                duration_ms: spec.duration_ms,
                sample_rate: spec.sample_rate,
                channels: spec.channels,
                codec: spec.codec.clone(),
                path: path.display().to_string(),
                exclusive: opened.exclusive,
                output_api: opened.api.clone(),
                device_name: opened.device_name.clone(),
                output_rate: opened.out_rate,
                resampled: false,
                speed: 1.0,
                skip_silence: false,
                underruns: 0,
                error: None,
                format: "dsd".into(),
                dsd_rate: spec.rate_label.clone(),
                output_path: opened.io_path.clone(),
                output_path_sv: path_sv,
                callback_reentries: 0,
                dsd_diag: opened.dsd_diag.clone(),
            };
        }

        let (tx, rx) = mpsc::channel();
        let status = self.status.clone();
        let app = self.app.clone();
        let stop_w = stop.clone();
        let playing_w = playing.clone();
        let source_rate = spec.sample_rate;
        let worker = std::thread::Builder::new()
            .name("hifi-dsd".into())
            .spawn(move || {
                dsd_loop(
                    reader,
                    rx,
                    ring,
                    status,
                    app,
                    stop_w,
                    playing_w,
                    source_rate,
                );
            })
            .map_err(|e| EngineError::msg(format!("DSD-tråd: {e}")))?;

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
    io_path: String,
    dsd_diag: String,
}

fn wasapi_device_name(name: &str) -> Option<&str> {
    let n = name.trim();
    if n.is_empty() {
        return None;
    }
    if let Some(rest) = n.strip_prefix("wasapi:") {
        return Some(rest);
    }
    if n.starts_with("asio:") {
        return None;
    }
    Some(n)
}

/// Auto-fallback to WASAPI must not look up an ASIO driver name as a WASAPI device
/// (that fails open). Explicit WASAPI names are kept. ASIO-only names use system default.
fn wasapi_name_for_auto_fallback(name: &str) -> Option<&str> {
    let n = wasapi_device_name(name)?;
    if super::asio_host::is_preferred_hi_fi_name(n) {
        return None;
    }
    if super::asio_host::list_asio_drivers()
        .iter()
        .any(|d| d.name.eq_ignore_ascii_case(n) || d.clsid.eq_ignore_ascii_case(n))
    {
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
        _ => {
            let wasapi_name = wasapi_name_for_auto_fallback(device_name).unwrap_or("");
            match open_asio(
                device_name,
                sample_rate,
                hwnd,
                ring.clone(),
                playing.clone(),
            ) {
                Ok(s) => Ok(s),
                Err(asio_err) => match open_exclusive(
                    wasapi_name,
                    sample_rate,
                    channels,
                    ring.clone(),
                    playing.clone(),
                ) {
                    Ok(s) => Ok(s),
                    Err(_) => open_shared(wasapi_name, sample_rate, channels, ring, playing)
                        .map_err(|wasapi_err| {
                            EngineError::msg(format!("ASIO: {asio_err} WASAPI: {wasapi_err}"))
                        }),
                },
            }
        }
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
    let io_path = stream.io_path.clone();
    let dsd_diag = stream.dsd_diag.clone();
    Ok(OpenedOutput {
        shared: None,
        exclusive_stream: None,
        asio: Some(stream),
        out_rate,
        exclusive: true,
        api: "asio".into(),
        device_name,
        io_path,
        dsd_diag,
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
        io_path: "pcm".into(),
        dsd_diag: String::new(),
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
        io_path: "pcm".into(),
        dsd_diag: String::new(),
    })
}

fn open_dsd_output(
    api: &str,
    device_name: &str,
    dsd_hz: u32,
    hwnd: Option<usize>,
    ring: Arc<ByteRing>,
    playing: Arc<AtomicBool>,
    file_lsb: bool,
) -> Result<OpenedOutput, EngineError> {
    let api = api.trim().to_ascii_lowercase();
    match api.as_str() {
        "wasapi_shared" | "wasapi" | "shared" => Err(EngineError::msg(
            "DSD spelas inte via WASAPI delat (systemets mixer). Välj ASIO (FiiO) för native DSD, eller WASAPI exclusive för DoP. Filen rörs inte.",
        )),
        "wasapi_exclusive" | "exclusive" => open_dsd_wasapi_dop(device_name, dsd_hz, ring, playing, file_lsb),
        "asio" => open_dsd_asio(device_name, dsd_hz, hwnd, ring, playing, file_lsb, true),
        _ => {
            match open_dsd_asio(
                device_name,
                dsd_hz,
                hwnd,
                ring.clone(),
                playing.clone(),
                file_lsb,
                true,
            ) {
                Ok(s) => Ok(s),
                Err(asio_err) => match open_dsd_wasapi_dop(
                    device_name,
                    dsd_hz,
                    ring,
                    playing,
                    file_lsb,
                ) {
                    Ok(mut s) => {
                        s.dsd_diag = format!("asio_failed: {asio_err} | {}", s.dsd_diag);
                        Ok(s)
                    }
                    Err(wasapi_err) => Err(EngineError::msg(format!(
                        "DSD kunde inte öppnas. ASIO native/DoP: {asio_err} WASAPI exclusive DoP: {wasapi_err} Ingen tyst PCM-omkodning. Delat WASAPI används inte."
                    ))),
                },
            }
        }
    }
}

fn merge_native_probe_into_dop(native_err: &EngineError, dop_diag: &str) -> String {
    format!("native_attempt_failed: {native_err} | dop: {dop_diag}")
}

fn combine_dsd_open_errors(native: &EngineError, dop: &EngineError) -> EngineError {
    EngineError::msg(format!(
        "Native DSD: {native} DoP: {dop} (båda vägarna misslyckades; ingen tyst PCM-omkodning)."
    ))
}

fn open_dsd_asio(
    device_name: &str,
    dsd_hz: u32,
    hwnd: Option<usize>,
    ring: Arc<ByteRing>,
    playing: Arc<AtomicBool>,
    file_lsb: bool,
    try_native_then_dop: bool,
) -> Result<OpenedOutput, EngineError> {
    let name = asio_driver_name(device_name);
    if try_native_then_dop {
        match super::asio_host::start_asio_native_dsd(
            name,
            dsd_hz,
            hwnd,
            ring.clone(),
            playing.clone(),
            file_lsb,
        ) {
            Ok(stream) => return wrap_asio_dsd(stream),
            Err(native_err) => {
                return match super::asio_host::start_asio_dop(
                    name, dsd_hz, hwnd, ring, playing, file_lsb,
                ) {
                    Ok(stream) => {
                        let mut out = wrap_asio_dsd(stream)?;
                        out.dsd_diag = merge_native_probe_into_dop(&native_err, &out.dsd_diag);
                        Ok(out)
                    }
                    Err(dop_err) => Err(combine_dsd_open_errors(&native_err, &dop_err)),
                };
            }
        }
    }
    let stream =
        super::asio_host::start_asio_native_dsd(name, dsd_hz, hwnd, ring, playing, file_lsb)?;
    wrap_asio_dsd(stream)
}

fn wrap_asio_dsd(stream: super::asio_host::AsioStream) -> Result<OpenedOutput, EngineError> {
    if stream.io_path == "native_dsd" || stream.io_path == "dop" {
        let out_rate = stream.sample_rate;
        let device_name = stream.driver_name.clone();
        let io_path = stream.io_path.clone();
        let dsd_diag = stream.dsd_diag.clone();
        return Ok(OpenedOutput {
            shared: None,
            exclusive_stream: None,
            asio: Some(stream),
            out_rate,
            exclusive: true,
            api: "asio".into(),
            device_name,
            io_path,
            dsd_diag,
        });
    }
    Err(EngineError::msg(
        "ASIO öppnade DSD men rapporterade PCM-väg. Vägrar låtsas-native. Filen rörs inte.",
    ))
}

fn open_dsd_wasapi_dop(
    device_name: &str,
    dsd_hz: u32,
    ring: Arc<ByteRing>,
    playing: Arc<AtomicBool>,
    file_lsb: bool,
) -> Result<OpenedOutput, EngineError> {
    let name =
        wasapi_name_for_auto_fallback(device_name).or_else(|| wasapi_device_name(device_name));
    let dop_rate = dsd::dop_pcm_rate(dsd_hz);
    let stream = super::exclusive::start_exclusive_dop(name, dop_rate, 2, ring, playing, file_lsb)?;
    Ok(OpenedOutput {
        shared: None,
        exclusive_stream: Some(stream),
        asio: None,
        out_rate: dop_rate,
        exclusive: true,
        api: "wasapi_exclusive".into(),
        device_name: name.unwrap_or("").to_string(),
        io_path: "dop".into(),
        dsd_diag: format!("wasapi_exclusive_dop rate={dop_rate}"),
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
    let mut last_underruns = 0u64;

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
            match decoder
                .read_interleaved((source_rate as usize / 10) * decoder.spec().channels as usize)
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
                        let skip_frames = ((extra_ms / 1000.0) * source_rate as f32) as u64;
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
            let underruns = ring.underrun_count();
            let reentries = super::asio_host::asio_callback_reentries();
            if underruns > last_underruns {
                eprintln!(
                    "native audio underrun: empty ring or try_lock contended (count {underruns}); ASIO re-entry {reentries}"
                );
                last_underruns = underruns;
            }
            if let Ok(mut st) = status.lock() {
                st.position_ms = pos_ms;
                st.playing = playing.load(Ordering::Relaxed);
                st.ended = ended;
                st.speed = speed;
                st.underruns = underruns;
                st.callback_reentries = reentries;
            }
            emit_tick(&app, &status);
        }

        std::thread::sleep(Duration::from_millis(4));
    }
}

fn dsd_loop(
    mut reader: DsdReader,
    rx: Receiver<Cmd>,
    ring: Arc<ByteRing>,
    status: Arc<Mutex<EngineStatus>>,
    app: Arc<Mutex<Option<AppHandle>>>,
    stop: Arc<AtomicBool>,
    playing: Arc<AtomicBool>,
    source_rate: u32,
) {
    let mut source_consumed_bytes: u64 = 0;
    let mut ended = false;
    let mut last_emit = std::time::Instant::now();
    let mut last_underruns = 0u64;

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
                Cmd::Speed(_) => {
                    if let Ok(mut st) = status.lock() {
                        st.error = Some(
                            "DSD spelas bara i 1× (bit-perfect). Hastighet ≠ 1 vägras.".into(),
                        );
                    }
                }
                Cmd::SkipSilence(_) => {
                    if let Ok(mut st) = status.lock() {
                        st.skip_silence = false;
                    }
                }
                Cmd::Seek(ms) => {
                    ring.clear();
                    if let Ok(actual) = reader.seek_ms(ms) {
                        let ch = 2u64;
                        source_consumed_bytes = actual as u64 * source_rate as u64 / 1000 / 8 * ch;
                    }
                    ended = false;
                }
            }
        }

        let high_water = (source_rate as usize / 8) * 2 / 5; // ~400 ms of stereo bytes
        if playing.load(Ordering::Relaxed) && ring.len_frames() < high_water && !ended {
            match reader.read_interleaved_bytes((source_rate as usize / 8 / 10) * 2) {
                Ok(raw) if raw.is_empty() => {
                    ended = reader.is_eof();
                    if ended {
                        playing.store(false, Ordering::Relaxed);
                        emit_ended(&app, &status);
                    }
                }
                Ok(raw) => {
                    let stereo = dsd::to_stereo_bytes(&raw, reader.spec().channels);
                    source_consumed_bytes += stereo.len() as u64;
                    if !stereo.is_empty() {
                        ring.push_interleaved(&stereo);
                    }
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
            let ring_bytes = ring.len_frames() as u64 * 2;
            let audible = source_consumed_bytes.saturating_sub(ring_bytes);
            let bits = audible / 2 * 8;
            let pos_ms = bits * 1000 / source_rate.max(1) as u64;
            let underruns = ring.underrun_count();
            let reentries = super::asio_host::asio_callback_reentries();
            if underruns > last_underruns {
                eprintln!("DSD underrun (count {underruns}); ASIO re-entry {reentries}");
                last_underruns = underruns;
            }
            if let Ok(mut st) = status.lock() {
                st.position_ms = pos_ms;
                st.playing = playing.load(Ordering::Relaxed);
                st.ended = ended;
                st.underruns = underruns;
                st.callback_reentries = reentries;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_fallback_keeps_wasapi_device_name() {
        assert_eq!(
            wasapi_name_for_auto_fallback("Speakers (Realtek)"),
            Some("Speakers (Realtek)")
        );
        assert_eq!(wasapi_name_for_auto_fallback("wasapi:HDMI"), Some("HDMI"));
    }

    #[test]
    fn auto_fallback_does_not_pass_asio_names_to_wasapi() {
        assert_eq!(wasapi_name_for_auto_fallback("asio:{aabb}"), None);
        assert_eq!(wasapi_name_for_auto_fallback("FiiO ASIO Driver"), None);
        assert_eq!(wasapi_name_for_auto_fallback(""), None);
    }

    #[test]
    fn native_and_dop_failures_are_both_in_the_error() {
        let native = EngineError::msg("kan inte sätta DSD-format");
        let dop = EngineError::msg("ingen 24-bit DoP-hastighet");
        let combined = combine_dsd_open_errors(&native, &dop).to_string();
        assert!(combined.contains("Native DSD"), "{combined}");
        assert!(combined.contains("kan inte sätta DSD-format"), "{combined}");
        assert!(combined.contains("DoP"), "{combined}");
        assert!(
            combined.contains("ingen 24-bit DoP-hastighet"),
            "{combined}"
        );
    }

    #[test]
    fn dop_success_keeps_native_failure_diagnostics() {
        let native = EngineError::msg("can=ASIOTrue(1) set=ASE_SUCCESS(0x3f4847a0) switch=Denied");
        let merged = merge_native_probe_into_dop(&native, "rate_ok=705600 sample_type_post=18");
        assert!(merged.contains("native_attempt_failed"), "{merged}");
        assert!(merged.contains("0x3f4847a0"), "{merged}");
        assert!(merged.contains("dop:"), "{merged}");
        assert!(merged.contains("705600"), "{merged}");
        assert!(!merged.contains("native_dsd"), "{merged}");
    }
}
