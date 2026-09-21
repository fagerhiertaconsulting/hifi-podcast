//! Windows ASIO host: registry discovery + IASIO COM (no Steinberg SDK).
//!
//! Drivers such as **FiiO ASIO Driver** register under `HKLM\SOFTWARE\ASIO`.
//! Hosts (Qobuz, foobar2000) CoCreate the CLSID and talk IASIO. Callbacks are
//! `cdecl`; COM methods are `stdcall`.

use super::byte_ring::ByteRing;
#[allow(unused_imports)]
use super::dsd::{self, DSD_MUTE};
use super::ring::FrameRing;
use super::EngineError;
use serde::Serialize;
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize)]
pub struct AsioDriverInfo {
    pub name: String,
    pub clsid: String,
    pub preferred: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleType {
    F32Le,
    I16Le,
    I24Le,
    I32Le,
    I32Le24,
    DsdLsb,
    DsdMsb,
    DsdNer8,
    Other(i32),
}

impl SampleType {
    fn from_asio(v: i32) -> Self {
        match v {
            16 => Self::I16Le,
            17 => Self::I24Le,
            18 => Self::I32Le,
            19 => Self::F32Le,
            27 => Self::I32Le24,
            32 => Self::DsdLsb,
            33 => Self::DsdMsb,
            40 => Self::DsdNer8,
            other => Self::Other(other),
        }
    }

    fn is_dsd(self) -> bool {
        matches!(self, Self::DsdLsb | Self::DsdMsb | Self::DsdNer8)
    }

    #[allow(dead_code)]
    fn dsd_lsb_first(self) -> bool {
        matches!(self, Self::DsdLsb)
    }

    #[allow(dead_code)]
    fn dop_ok(self) -> bool {
        matches!(self, Self::I24Le | Self::I32Le | Self::I32Le24)
    }

    fn is_pcm(self) -> bool {
        matches!(
            self,
            Self::F32Le | Self::I16Le | Self::I24Le | Self::I32Le | Self::I32Le24
        )
    }

    fn bytes_per_sample(self) -> Option<usize> {
        match self {
            Self::I16Le => Some(2),
            Self::I24Le => Some(3),
            Self::F32Le | Self::I32Le | Self::I32Le24 => Some(4),
            Self::DsdLsb | Self::DsdMsb | Self::DsdNer8 => Some(1),
            Self::Other(_) => None,
        }
    }
}

pub fn asio_available() -> bool {
    cfg!(windows)
}

pub fn is_preferred_hi_fi_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("fiio") || n.contains("fii-o") || n.contains("fii o")
}

pub fn rank_asio_name(name: &str) -> u8 {
    if is_preferred_hi_fi_name(name) {
        0
    } else {
        1
    }
}

/// Parse `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` (with or without braces).
pub fn parse_clsid(s: &str) -> Result<[u8; 16], EngineError> {
    let t = s.trim().trim_matches('{').trim_end_matches('}').trim();
    let hex: String = t.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(EngineError::msg(format!("Ogiltig ASIO CLSID: {s}")));
    }
    let mut bytes = [0u8; 16];
    for i in 0..16 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| EngineError::msg("Ogiltig ASIO CLSID."))?;
    }
    // Windows GUID is mixed endian in the first three fields.
    Ok(bytes)
}

pub fn list_asio_drivers() -> Vec<AsioDriverInfo> {
    #[cfg(windows)]
    {
        let mut list = windows_impl::list_from_registry();
        list.sort_by_key(|d| rank_asio_name(&d.name));
        list
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
enum RtFillMode {
    Pcm,
    NativeDsd { reverse: bool },
    Dop,
}

pub struct AsioStream {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub driver_name: String,
    /// `pcm`, `native_dsd`, or `dop` — never claim native DSD for DoP.
    pub io_path: String,
    /// Raw kAsioCanDo/Set/GetIoFormat, rates, and channel sample_type (QA).
    pub dsd_diag: String,
}

impl Drop for AsioStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
        uninstall_rt_host();
    }
}

pub fn start_asio(
    driver_name: Option<&str>,
    source_rate: u32,
    hwnd: Option<usize>,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<AsioStream, EngineError> {
    #[cfg(windows)]
    {
        windows_impl::start(driver_name, source_rate, hwnd, ring, playing)
    }
    #[cfg(not(windows))]
    {
        let _ = (driver_name, source_rate, hwnd, ring, playing);
        Err(EngineError::msg(
            "ASIO finns bara på Windows. Installera FiiO ASIO Driver på PC:n och välj den i Inställningar.",
        ))
    }
}

pub fn start_asio_native_dsd(
    driver_name: Option<&str>,
    dsd_hz: u32,
    hwnd: Option<usize>,
    ring: Arc<ByteRing>,
    playing: Arc<AtomicBool>,
    file_lsb_first: bool,
) -> Result<AsioStream, EngineError> {
    #[cfg(windows)]
    {
        windows_impl::start_dsd(
            driver_name,
            dsd_hz,
            hwnd,
            ring,
            playing,
            file_lsb_first,
            true,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (driver_name, dsd_hz, hwnd, ring, playing, file_lsb_first);
        Err(EngineError::msg(
            "Native DSD via ASIO finns bara på Windows (FiiO ASIO Driver).",
        ))
    }
}

pub fn start_asio_dop(
    driver_name: Option<&str>,
    dsd_hz: u32,
    hwnd: Option<usize>,
    ring: Arc<ByteRing>,
    playing: Arc<AtomicBool>,
    file_lsb_first: bool,
) -> Result<AsioStream, EngineError> {
    #[cfg(windows)]
    {
        windows_impl::start_dsd(
            driver_name,
            dsd_hz,
            hwnd,
            ring,
            playing,
            file_lsb_first,
            false,
        )
    }
    #[cfg(not(windows))]
    {
        let _ = (driver_name, dsd_hz, hwnd, ring, playing, file_lsb_first);
        Err(EngineError::msg("DoP via ASIO finns bara på Windows."))
    }
}

/// Realtime host published to ASIO `bufferSwitch` via atomic pointer (no Mutex in the callback).
/// Fields are read from the Windows ASIO callback; Linux tests only install/uninstall the pointer.
#[allow(dead_code)]
struct RtHost {
    pcm_ring: Arc<FrameRing>,
    dsd_ring: Option<Arc<ByteRing>>,
    fill: RtFillMode,
    playing: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
    stop: Arc<AtomicBool>,
    scratch: UnsafeCell<Vec<f32>>,
    scratch_u8: UnsafeCell<Vec<u8>>,
    scratch_busy: AtomicBool,
    dop_marker_a: AtomicBool,
    /// True when file bit order must be reversed for this output (DSF→DoP/MSB, etc.).
    dsd_reverse: bool,
    reentries: AtomicU64,
    /// Max bytes we may write per channel half-buffer (never past ASIO allocation).
    buffer_bytes: usize,
}

unsafe impl Send for RtHost {}
unsafe impl Sync for RtHost {}

static RT_HOST: AtomicPtr<RtHost> = AtomicPtr::new(ptr::null_mut());
static RT_IN_FLIGHT: AtomicU32 = AtomicU32::new(0);
static ASIO_RESET: AtomicBool = AtomicBool::new(false);

fn install_rt_host(host: Box<RtHost>) {
    uninstall_rt_host();
    ASIO_REENTRIES.store(0, Ordering::Relaxed);
    RT_HOST.store(Box::into_raw(host), Ordering::Release);
}

fn uninstall_rt_host() {
    let ptr = RT_HOST.swap(ptr::null_mut(), Ordering::AcqRel);
    if ptr.is_null() {
        return;
    }
    let started = std::time::Instant::now();
    while RT_IN_FLIGHT.load(Ordering::Acquire) != 0 {
        std::thread::yield_now();
        if started.elapsed() > std::time::Duration::from_millis(500) {
            // Prefer a leak over dropping a host a callback may still touch.
            eprintln!(
                "ASIO: rt-host timeout while callbacks in flight — leaking to avoid use-after-free"
            );
            return;
        }
    }
    unsafe {
        drop(Box::from_raw(ptr));
    }
}

/// Steinberg `ASIOError` success (`asio.h`). IASIO methods other than `init` use this.
/// `init` is `ASIOBool` (`ASIOTrue` = 1, `ASIOFalse` = 0) — not an error code.
#[allow(dead_code)] // used by the Windows host path and unit tests
const ASE_OK: i32 = 0;
/// `asio.h` `ASE_SUCCESS` — **required** success code for `IASIO::future` / `ASIOFuture`.
/// Distinct from `ASE_OK`. SDK DSD sample: `if (ASE_SUCCESS != ASIOFuture(kAsioSetIoFormat, …))`.
#[allow(dead_code)]
const ASE_SUCCESS: i32 = 0x3f48_47a0;
/// `asio.h` `ASE_NotPresent` — selector/method not implemented.
#[allow(dead_code)]
const ASE_NOT_PRESENT: i32 = -1000;
/// `asio.h` `ASIOTrue` — some drivers return this from CanDo instead of ASE_SUCCESS.
#[allow(dead_code)]
const ASIO_TRUE: i32 = 1;
/// `kASIOIoFormatInvalid` — driver overwrites the requested type when it refuses.
#[allow(dead_code)]
const ASIO_IO_INVALID: i32 = -1;
/// `kASIODSDFormat`
#[allow(dead_code)]
const ASIO_IO_DSD: i32 = 1;

/// Steinberg `IASIO::canSampleRate` / `ASIOCanSampleRate`.
///
/// `true` iff the driver reports the inquired rate as available.
/// SDK: **ASE_OK (0) = supported**; ASE_NoClock = unsupported; ASE_NotPresent = no I/O.
/// JUCE (`juce_win32_ASIO.cpp`): `canSampleRate(rate) == 0` then the rate is listed.
/// foobar2000 / common hosts: same `ASE_OK` test. Non-zero is *not* “supported”.
#[allow(dead_code)] // used by the Windows host path and unit tests
fn asio_sample_rate_supported(can_sample_rate: i32) -> bool {
    can_sample_rate == ASE_OK
}

/// Result of `kAsioCanDoIoFormat` + `kAsioSetIoFormat`. Native DSD is only `Applied`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum IoFormatSwitch {
    /// Driver accepted the requested PCM/DSD IO format.
    Applied,
    /// `future` not implemented — PCM/DoP may continue; native DSD must not.
    NotImplemented,
    /// Driver refused the format.
    Denied,
}

/// Steinberg: `ASE_OK` for ordinary IASIO methods; `ASE_SUCCESS` for `future()`.
#[allow(dead_code)]
fn asio_future_succeeded(code: i32) -> bool {
    code == ASE_OK || code == ASE_SUCCESS
}

/// Snapshot of kAsioCanDoIoFormat / Set / Get used for gating and QA diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
struct IoFormatProbe {
    can: i32,
    set: i32,
    set_format_type: i32,
    get: Option<i32>,
    get_format_type: Option<i32>,
    requested: i32,
}

/// Native DSD requires a successful Set (ASE_OK **or** ASE_SUCCESS) that left the
/// requested type unchanged, or a successful GetIoFormat read-back of that type.
/// CanDo is not a hard gate: SDK sample only checks Set; some drivers return
/// ASIOTrue(1) from CanDo. Both NotPresent → extension missing (PCM/DoP may continue).
#[allow(dead_code)]
fn classify_io_format_probe(p: &IoFormatProbe) -> IoFormatSwitch {
    if p.can == ASE_NOT_PRESENT && p.set == ASE_NOT_PRESENT {
        return IoFormatSwitch::NotImplemented;
    }
    let set_format_ok = p.set_format_type == p.requested && p.set_format_type != ASIO_IO_INVALID;
    let set_applied = asio_future_succeeded(p.set) && set_format_ok;
    let get_applied = set_format_ok
        && p.get.map(asio_future_succeeded).unwrap_or(false)
        && p.get_format_type == Some(p.requested);
    if set_applied || get_applied {
        IoFormatSwitch::Applied
    } else {
        IoFormatSwitch::Denied
    }
}

/// Convenience for tests that only vary CanDo/Set (format assumed kept as DSD).
#[allow(dead_code)]
fn classify_io_format_switch(can: i32, set: i32) -> IoFormatSwitch {
    classify_io_format_probe(&IoFormatProbe {
        can,
        set,
        set_format_type: ASIO_IO_DSD,
        get: None,
        get_format_type: None,
        requested: ASIO_IO_DSD,
    })
}

#[allow(dead_code)]
fn format_asio_code(code: i32) -> String {
    match code {
        ASE_OK => "ASE_OK(0)".into(),
        ASE_SUCCESS => "ASE_SUCCESS(0x3f4847a0)".into(),
        ASE_NOT_PRESENT => "ASE_NotPresent(-1000)".into(),
        ASIO_TRUE => "ASIOTrue(1)".into(),
        other => format!("{other}"),
    }
}

/// Pick a legal ASIO `createBuffers` length.
/// `granularity == -1` means **only power-of-two sizes** (FiiO PCM QA: gran-1 / pref 64).
#[allow(dead_code)]
fn pick_buffer_frames(min: i32, max: i32, preferred: i32, gran: i32) -> usize {
    const CAP: i32 = 65_536;
    let min = min.max(0);
    let max = max.max(0);
    let mut n = if preferred > 0 {
        preferred
    } else if min > 0 {
        min
    } else {
        64
    };
    if max > 0 && n > max {
        n = max;
    }
    if min > 0 && n < min {
        n = min;
    }
    if gran > 1 {
        let base = min;
        if n >= base {
            n = base + ((n - base) / gran) * gran;
            if min > 0 && n < min {
                n = min;
            }
            if n <= 0 {
                n = gran;
            }
        }
    } else if gran < 0 {
        if !is_pow2_i32(n) {
            n = floor_pow2_i32(n);
        }
        if min > 0 && n < min {
            n = if is_pow2_i32(min) {
                min
            } else {
                let up = ceil_pow2_i32(min);
                if max > 0 && up > max {
                    min
                } else {
                    up
                }
            };
        }
        if max > 0 && n > max {
            n = floor_pow2_i32(max);
        }
    }
    if n > CAP && !(min > CAP) {
        n = if max > 0 { max.min(CAP) } else { CAP };
        if gran < 0 && !is_pow2_i32(n) {
            n = floor_pow2_i32(n);
        }
        if min > 0 && n < min {
            n = min;
        }
    }
    n.max(1) as usize
}

#[allow(dead_code)]
fn is_pow2_i32(n: i32) -> bool {
    n > 0 && (n & (n - 1)) == 0
}

#[allow(dead_code)]
fn floor_pow2_i32(n: i32) -> i32 {
    if n <= 0 {
        return 1;
    }
    if is_pow2_i32(n) {
        return n;
    }
    let z = (n as u32).leading_zeros();
    if z >= 31 {
        1
    } else {
        1 << (31 - z)
    }
}

#[allow(dead_code)]
fn ceil_pow2_i32(n: i32) -> i32 {
    if n <= 1 {
        return 1;
    }
    if is_pow2_i32(n) {
        return n;
    }
    let f = floor_pow2_i32(n);
    f.saturating_mul(2).max(1)
}

/// Native DSD must not `createBuffers` until the channel type is already a DSD
/// type. createBuffers in DSD IoFormat with a PCM sample_type is what armed
/// the FiiO ACCESS_VIOLATION (in-proc driver + first bufferSwitch).
#[allow(dead_code)]
fn native_dsd_may_create_buffers(sample_type: SampleType) -> bool {
    sample_type.is_dsd()
}

#[allow(dead_code)]
fn format_io_format_diag(p: &IoFormatProbe, sw: IoFormatSwitch) -> String {
    let get = p.get.map(format_asio_code).unwrap_or_else(|| "n/a".into());
    let get_ty = p
        .get_format_type
        .map(|t| t.to_string())
        .unwrap_or_else(|| "n/a".into());
    format!(
        "can={} set={} set_type={} get={} get_type={} switch={sw:?}",
        format_asio_code(p.can),
        format_asio_code(p.set),
        p.set_format_type,
        get,
        get_ty
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum CallbackBusyAction {
    /// Stream stopping: mute this buffer, do not count underrun or re-entry.
    StopMute,
    /// Nested bufferSwitch while fill is in progress: mute this callback only, count re-entry (not underrun).
    ReentryMute,
    Fill,
}

#[allow(dead_code)]
fn callback_busy_action(stopped: bool, already_busy: bool) -> CallbackBusyAction {
    if stopped {
        CallbackBusyAction::StopMute
    } else if already_busy {
        CallbackBusyAction::ReentryMute
    } else {
        CallbackBusyAction::Fill
    }
}

static ASIO_REENTRIES: AtomicU64 = AtomicU64::new(0);

pub type AsioDiagHook = Arc<dyn Fn(&str) + Send + Sync>;
static DIAG_HOOK: Mutex<Option<AsioDiagHook>> = Mutex::new(None);

/// UI/QA hook invoked from the ASIO thread **before** `start()` so a later AV
/// can still have shown `dsd_diag`.
pub fn set_asio_diag_hook(hook: Option<AsioDiagHook>) {
    if let Ok(mut g) = DIAG_HOOK.lock() {
        *g = hook;
    }
}

#[allow(dead_code)]
fn publish_dsd_diag(s: &str) {
    eprintln!("hifi-asio: {s}");
    if let Ok(g) = DIAG_HOOK.lock() {
        if let Some(h) = g.as_ref() {
            h(s);
        }
    }
}

/// Nested `bufferSwitch` count (not underruns). Reset when an ASIO stream is installed.
#[allow(dead_code)]
pub fn asio_callback_reentries() -> u64 {
    ASIO_REENTRIES.load(Ordering::Relaxed)
}

fn new_rt_host(
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
) -> Box<RtHost> {
    new_rt_host_ex(
        ring,
        None,
        playing,
        stop,
        buffers,
        frames,
        sample_type,
        channels,
        RtFillMode::Pcm,
    )
}

fn new_rt_host_ex(
    pcm_ring: Arc<FrameRing>,
    dsd_ring: Option<Arc<ByteRing>>,
    playing: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
    fill: RtFillMode,
) -> Box<RtHost> {
    let n = frames.saturating_mul(channels.max(1));
    let u8_n = match fill {
        RtFillMode::Dop => n.saturating_mul(2),
        _ => n,
    };
    let bps = match fill {
        RtFillMode::NativeDsd { .. } => 1,
        RtFillMode::Dop => match sample_type {
            SampleType::I24Le => 3,
            _ => 4,
        },
        RtFillMode::Pcm => sample_type.bytes_per_sample().unwrap_or(4),
    };
    let buffer_bytes = frames.saturating_mul(bps).max(1);
    Box::new(RtHost {
        pcm_ring,
        dsd_ring,
        fill,
        playing,
        buffers,
        frames,
        sample_type,
        channels,
        stop,
        scratch: UnsafeCell::new(vec![0.0f32; n]),
        scratch_u8: UnsafeCell::new(vec![DSD_MUTE; u8_n.max(1)]),
        scratch_busy: AtomicBool::new(false),
        dop_marker_a: AtomicBool::new(true),
        dsd_reverse: match fill {
            RtFillMode::NativeDsd { reverse } => reverse,
            _ => false,
        },
        reentries: AtomicU64::new(0),
        buffer_bytes,
    })
}

#[allow(dead_code)]
fn new_rt_host_dsd(
    dsd_ring: Arc<ByteRing>,
    playing: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
    fill: RtFillMode,
    reverse: bool,
) -> Box<RtHost> {
    let mut host = new_rt_host_ex(
        Arc::new(FrameRing::new(channels.max(1))),
        Some(dsd_ring),
        playing,
        stop,
        buffers,
        frames,
        sample_type,
        channels,
        fill,
    );
    host.dsd_reverse = reverse;
    host
}

#[allow(dead_code)]
fn install_session_host(
    pcm_ring: Option<Arc<FrameRing>>,
    dsd_ring: Option<Arc<ByteRing>>,
    playing: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
    fill: RtFillMode,
    reverse: bool,
) {
    match fill {
        RtFillMode::Pcm => {
            let ring = pcm_ring.unwrap_or_else(|| Arc::new(FrameRing::new(channels.max(1))));
            install_rt_host(new_rt_host(
                ring,
                playing,
                stop,
                buffers,
                frames,
                sample_type,
                channels,
            ));
        }
        _ => {
            let ring = dsd_ring.unwrap_or_else(|| Arc::new(ByteRing::new(channels.max(1))));
            install_rt_host(new_rt_host_dsd(
                ring,
                playing,
                stop,
                buffers,
                frames,
                sample_type,
                channels,
                fill,
                reverse,
            ));
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::ffi::{c_long, c_void};
    use std::ptr;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    pub fn list_from_registry() -> Vec<AsioDriverInfo> {
        let mut out = Vec::new();
        for path in ["SOFTWARE\\ASIO", "SOFTWARE\\WOW6432Node\\ASIO"] {
            let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
            let Ok(root) = hklm.open_subkey(path) else {
                continue;
            };
            let Ok(keys) = root.enum_keys().collect::<Result<Vec<_>, _>>() else {
                continue;
            };
            for name in keys {
                let Ok(sub) = root.open_subkey(&name) else {
                    continue;
                };
                let clsid: String = sub
                    .get_value("CLSID")
                    .or_else(|_| sub.get_value("clsid"))
                    .unwrap_or_default();
                if clsid.trim().is_empty() {
                    continue;
                }
                let desc: String = sub
                    .get_value("Description")
                    .or_else(|_| sub.get_value("description"))
                    .unwrap_or_else(|_| name.clone());
                let display = if desc.trim().is_empty() { name } else { desc };
                if out
                    .iter()
                    .any(|d: &AsioDriverInfo| d.clsid.eq_ignore_ascii_case(&clsid))
                {
                    continue;
                }
                out.push(AsioDriverInfo {
                    preferred: is_preferred_hi_fi_name(&display),
                    name: display,
                    clsid,
                });
            }
        }
        out
    }

    fn guid_from_clsid(s: &str) -> Result<GUID, EngineError> {
        let b = parse_clsid(s)?;
        Ok(GUID {
            data1: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            data2: u16::from_be_bytes([b[4], b[5]]),
            data3: u16::from_be_bytes([b[6], b[7]]),
            data4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        })
    }

    #[repr(C)]
    struct Iasio {
        vtbl: *const IasioVtbl,
    }

    #[repr(C)]
    struct IasioVtbl {
        query_interface:
            unsafe extern "system" fn(*mut Iasio, *const GUID, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut Iasio) -> u32,
        release: unsafe extern "system" fn(*mut Iasio) -> u32,
        init: unsafe extern "system" fn(*mut Iasio, *mut c_void) -> i32,
        get_driver_name: unsafe extern "system" fn(*mut Iasio, *mut u8),
        get_driver_version: unsafe extern "system" fn(*mut Iasio) -> c_long,
        get_error_message: unsafe extern "system" fn(*mut Iasio, *mut u8),
        start: unsafe extern "system" fn(*mut Iasio) -> c_long,
        stop: unsafe extern "system" fn(*mut Iasio) -> c_long,
        get_channels: unsafe extern "system" fn(*mut Iasio, *mut c_long, *mut c_long) -> c_long,
        get_latencies: unsafe extern "system" fn(*mut Iasio, *mut c_long, *mut c_long) -> c_long,
        get_buffer_size: unsafe extern "system" fn(
            *mut Iasio,
            *mut c_long,
            *mut c_long,
            *mut c_long,
            *mut c_long,
        ) -> c_long,
        can_sample_rate: unsafe extern "system" fn(*mut Iasio, f64) -> c_long,
        get_sample_rate: unsafe extern "system" fn(*mut Iasio, *mut f64) -> c_long,
        set_sample_rate: unsafe extern "system" fn(*mut Iasio, f64) -> c_long,
        get_clock_sources:
            unsafe extern "system" fn(*mut Iasio, *mut c_void, *mut c_long) -> c_long,
        set_clock_source: unsafe extern "system" fn(*mut Iasio, c_long) -> c_long,
        get_sample_position:
            unsafe extern "system" fn(*mut Iasio, *mut c_void, *mut c_void) -> c_long,
        get_channel_info: unsafe extern "system" fn(*mut Iasio, *mut ChannelInfo) -> c_long,
        create_buffers: unsafe extern "system" fn(
            *mut Iasio,
            *mut BufferInfo,
            c_long,
            c_long,
            *mut Callbacks,
        ) -> c_long,
        dispose_buffers: unsafe extern "system" fn(*mut Iasio) -> c_long,
        control_panel: unsafe extern "system" fn(*mut Iasio) -> c_long,
        future: unsafe extern "system" fn(*mut Iasio, c_long, *mut c_void) -> c_long,
        output_ready: unsafe extern "system" fn(*mut Iasio) -> c_long,
    }

    #[repr(C)]
    struct BufferInfo {
        is_input: c_long,
        channel_num: c_long,
        buffers: [*mut c_void; 2],
    }

    #[repr(C)]
    struct ChannelInfo {
        channel: c_long,
        is_input: c_long,
        is_active: c_long,
        channel_group: c_long,
        sample_type: c_long,
        name: [u8; 32],
    }

    #[repr(C)]
    struct Callbacks {
        buffer_switch: unsafe extern "C" fn(c_long, c_long),
        sample_rate_did_change: unsafe extern "C" fn(f64),
        asio_message: unsafe extern "C" fn(c_long, c_long, *mut c_void, *mut f64) -> c_long,
        buffer_switch_time_info: unsafe extern "C" fn(*mut c_void, c_long, c_long) -> *mut c_void,
    }

    /// Heap-backed: some DSD drivers write the callback table (rodata AV on `static`).
    fn make_callbacks() -> Box<Callbacks> {
        Box::new(Callbacks {
            buffer_switch,
            sample_rate_did_change,
            asio_message,
            buffer_switch_time_info,
        })
    }

    static SUPPORT_TIME_INFO: AtomicBool = AtomicBool::new(true);

    unsafe extern "C" {
        fn hifi_asio_seh_invoke(cb: unsafe extern "C" fn(i32), index: i32) -> i32;
    }

    unsafe extern "C" fn buffer_switch(index: c_long, _direct: c_long) {
        fill_buffers(index);
    }

    unsafe extern "C" fn sample_rate_did_change(_rate: f64) {}

    unsafe extern "C" fn asio_message(
        selector: c_long,
        value: c_long,
        _message: *mut c_void,
        _opt: *mut f64,
    ) -> c_long {
        // kAsioSelectorSupported = 1, kAsioEngineVersion = 2, kAsioResetRequest = 3,
        // kAsioSupportsTimeInfo = 7
        match selector {
            1 => match value {
                1 | 2 | 3 | 7 => 1,
                _ => 0,
            },
            2 => 2,
            3 => {
                ASIO_RESET.store(true, Ordering::Release);
                1
            }
            7 => {
                if SUPPORT_TIME_INFO.load(Ordering::Relaxed) {
                    1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    unsafe extern "C" fn buffer_switch_time_info(
        params: *mut c_void,
        index: c_long,
        _direct: c_long,
    ) -> *mut c_void {
        fill_buffers(index);
        params
    }

    unsafe extern "C" fn fill_buffers_c(index: i32) {
        fill_buffers_inner(index as c_long);
    }

    fn fill_buffers(index: c_long) {
        RT_IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
        // ACCESS_VIOLATION is SEH, not a Rust panic — catch_unwind cannot save us.
        let code = unsafe { hifi_asio_seh_invoke(fill_buffers_c, index as i32) };
        if code != 0 {
            eprintln!("hifi-asio: bufferSwitch SEH exception {code:#x} (process kept alive)");
        }
        RT_IN_FLIGHT.fetch_sub(1, Ordering::Release);
    }

    fn fill_buffers_inner(index: c_long) {
        let host = RT_HOST.load(Ordering::Acquire);
        if host.is_null() {
            return;
        }
        let st = unsafe { &*host };
        let idx = if index == 0 { 0 } else { 1 };
        let frames = st.frames;
        let ch = st.channels.max(1);

        let already_busy = st.scratch_busy.swap(true, Ordering::AcqRel);
        match callback_busy_action(st.stop.load(Ordering::Relaxed), already_busy) {
            CallbackBusyAction::StopMute => {
                unsafe {
                    write_mute(st, idx, frames, ch);
                }
                return;
            }
            CallbackBusyAction::ReentryMute => {
                st.reentries.fetch_add(1, Ordering::Relaxed);
                ASIO_REENTRIES.fetch_add(1, Ordering::Relaxed);
                unsafe {
                    write_mute(st, idx, frames, ch);
                }
                return;
            }
            CallbackBusyAction::Fill => {}
        }

        match st.fill {
            RtFillMode::Pcm => fill_pcm(st, idx, frames, ch),
            RtFillMode::NativeDsd { reverse } => fill_native_dsd(st, idx, frames, ch, reverse),
            RtFillMode::Dop => fill_dop(st, idx, frames, ch),
        }
        st.scratch_busy.store(false, Ordering::Release);
    }

    fn fill_pcm(st: &RtHost, idx: usize, frames: usize, ch: usize) {
        let scratch = unsafe { &mut *st.scratch.get() };
        if scratch.len() < frames * ch {
            scratch.fill(0.0);
            st.pcm_ring.note_underrun();
            for c in 0..ch {
                let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
                if p.is_null() {
                    continue;
                }
                unsafe {
                    write_channel(p, st.sample_type, frames, st.buffer_bytes, |_| 0.0);
                }
            }
            return;
        }
        let dest = &mut scratch[..frames * ch];
        if st.playing.load(Ordering::Relaxed) {
            let _ = st.pcm_ring.pop_interleaved(dest);
        } else {
            dest.fill(0.0);
        }
        for c in 0..ch {
            let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
            if p.is_null() {
                continue;
            }
            unsafe {
                write_channel(p, st.sample_type, frames, st.buffer_bytes, |i| {
                    dest[i * ch + c]
                });
            }
        }
    }

    fn fill_native_dsd(st: &RtHost, idx: usize, frames: usize, ch: usize, reverse: bool) {
        let Some(ring) = st.dsd_ring.as_ref() else {
            unsafe { write_mute(st, idx, frames, ch) };
            return;
        };
        let scratch = unsafe { &mut *st.scratch_u8.get() };
        if scratch.len() < frames * ch {
            ring.note_underrun();
            unsafe { write_mute(st, idx, frames, ch) };
            return;
        }
        let dest = &mut scratch[..frames * ch];
        if st.playing.load(Ordering::Relaxed) {
            let _ = ring.pop_interleaved(dest, DSD_MUTE);
        } else {
            dest.fill(DSD_MUTE);
        }
        let nbytes = frames.min(st.buffer_bytes);
        for c in 0..ch {
            let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
            if p.is_null() {
                continue;
            }
            unsafe {
                for i in 0..nbytes {
                    let mut b = dest[i * ch + c];
                    if reverse {
                        b = dsd::reverse_bits_byte(b);
                    }
                    ptr::write(p.add(i), b);
                }
            }
        }
    }

    fn fill_dop(st: &RtHost, idx: usize, frames: usize, ch: usize) {
        let Some(ring) = st.dsd_ring.as_ref() else {
            unsafe { write_mute(st, idx, frames, ch) };
            return;
        };
        let need = frames * ch * 2;
        let scratch = unsafe { &mut *st.scratch_u8.get() };
        if scratch.len() < need {
            ring.note_underrun();
            unsafe { write_mute(st, idx, frames, ch) };
            return;
        }
        let dest = &mut scratch[..need];
        if st.playing.load(Ordering::Relaxed) {
            let _ = ring.pop_interleaved(dest, DSD_MUTE);
        } else {
            dest.fill(DSD_MUTE);
        }
        let reverse = st.dsd_reverse;
        let mut marker_a = st.dop_marker_a.load(Ordering::Relaxed);
        let bps = st.sample_type.bytes_per_sample().unwrap_or(4).max(1);
        let max_frames = (st.buffer_bytes / bps).min(frames);
        for i in 0..max_frames {
            let marker = if marker_a {
                dsd::DOP_MARKER_A
            } else {
                dsd::DOP_MARKER_B
            };
            marker_a = !marker_a;
            for c in 0..ch {
                let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
                if p.is_null() {
                    continue;
                }
                let mut b0 = dest[i * 2 * ch + c];
                let mut b1 = dest[i * 2 * ch + ch + c];
                if reverse {
                    b0 = dsd::reverse_bits_byte(b0);
                    b1 = dsd::reverse_bits_byte(b1);
                }
                unsafe {
                    write_dop_sample(p, st.sample_type, i, b0, b1, marker);
                }
            }
        }
        st.dop_marker_a.store(marker_a, Ordering::Relaxed);
    }

    unsafe fn write_mute(st: &RtHost, idx: usize, frames: usize, ch: usize) {
        match st.fill {
            RtFillMode::Pcm => {
                for c in 0..ch {
                    let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
                    if p.is_null() {
                        continue;
                    }
                    write_channel(p, st.sample_type, frames, st.buffer_bytes, |_| 0.0);
                }
            }
            RtFillMode::NativeDsd { .. } => {
                let nbytes = frames.min(st.buffer_bytes);
                for c in 0..ch {
                    let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
                    if p.is_null() {
                        continue;
                    }
                    for i in 0..nbytes {
                        ptr::write(p.add(i), DSD_MUTE);
                    }
                }
            }
            RtFillMode::Dop => {
                let mut marker_a = st.dop_marker_a.load(Ordering::Relaxed);
                let bps = st.sample_type.bytes_per_sample().unwrap_or(4).max(1);
                let max_frames = (st.buffer_bytes / bps).min(frames);
                for i in 0..max_frames {
                    let marker = if marker_a {
                        dsd::DOP_MARKER_A
                    } else {
                        dsd::DOP_MARKER_B
                    };
                    marker_a = !marker_a;
                    for c in 0..ch {
                        let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
                        if p.is_null() {
                            continue;
                        }
                        write_dop_sample(p, st.sample_type, i, DSD_MUTE, DSD_MUTE, marker);
                    }
                }
                st.dop_marker_a.store(marker_a, Ordering::Relaxed);
            }
        }
    }

    unsafe fn write_dop_sample(
        ptr: *mut u8,
        ty: SampleType,
        i: usize,
        first: u8,
        second: u8,
        marker: u8,
    ) {
        match ty {
            SampleType::I24Le => {
                let base = ptr.add(i * 3);
                ptr::write(base, second);
                ptr::write(base.add(1), first);
                ptr::write(base.add(2), marker);
            }
            SampleType::I32Le | SampleType::I32Le24 => {
                let base = ptr.add(i * 4);
                ptr::write(base, 0);
                ptr::write(base.add(1), second);
                ptr::write(base.add(2), first);
                ptr::write(base.add(3), marker);
            }
            _ => {
                ptr::write(ptr.add(i), DSD_MUTE);
            }
        }
    }

    unsafe fn write_channel(
        ptr: *mut u8,
        ty: SampleType,
        frames: usize,
        max_bytes: usize,
        sample: impl Fn(usize) -> f32,
    ) {
        let Some(bps) = ty.bytes_per_sample() else {
            return;
        };
        let frames = frames.min(max_bytes / bps.max(1));
        match ty {
            SampleType::F32Le => {
                for i in 0..frames {
                    let v = sample(i);
                    ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), ptr.add(i * 4), 4);
                }
            }
            SampleType::I16Le => {
                for i in 0..frames {
                    let v = (sample(i).clamp(-1.0, 1.0) * 32767.0) as i16;
                    ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), ptr.add(i * 2), 2);
                }
            }
            SampleType::I24Le => {
                for i in 0..frames {
                    let v = (sample(i).clamp(-1.0, 1.0) * 8_388_607.0) as i32;
                    let b = v.to_le_bytes();
                    ptr::copy_nonoverlapping(b.as_ptr(), ptr.add(i * 3), 3);
                }
            }
            SampleType::I32Le => {
                for i in 0..frames {
                    let v = (sample(i).clamp(-1.0, 1.0) * 2_147_483_647.0) as i32;
                    ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), ptr.add(i * 4), 4);
                }
            }
            SampleType::I32Le24 => {
                for i in 0..frames {
                    let v = ((sample(i).clamp(-1.0, 1.0) * 8_388_607.0) as i32) << 8;
                    ptr::copy_nonoverlapping(v.to_le_bytes().as_ptr(), ptr.add(i * 4), 4);
                }
            }
            SampleType::DsdLsb
            | SampleType::DsdMsb
            | SampleType::DsdNer8
            | SampleType::Other(_) => {
                for i in 0..frames {
                    ptr::write(ptr.add(i), DSD_MUTE);
                }
            }
        }
    }

    fn pick_driver(name: Option<&str>) -> Result<AsioDriverInfo, EngineError> {
        let mut list = list_from_registry();
        if list.is_empty() {
            return Err(EngineError::msg(
                "Inga ASIO-drivrutiner i registret (HKLM\\SOFTWARE\\ASIO). Installera FiiO ASIO Driver (samma som i Qobuz) och starta om appen.",
            ));
        }
        list.sort_by_key(|d| rank_asio_name(&d.name));
        if let Some(want) = name.map(str::trim).filter(|s| !s.is_empty()) {
            let want_l = want.to_ascii_lowercase();
            if let Some(d) = list.iter().find(|d| {
                d.name.eq_ignore_ascii_case(want)
                    || d.clsid.eq_ignore_ascii_case(want)
                    || d.name.to_ascii_lowercase().contains(&want_l)
            }) {
                return Ok(d.clone());
            }
            return Err(EngineError::msg(format!(
                "ASIO-enheten \"{want}\" saknas. Tillgängliga: {}.",
                list.iter()
                    .map(|d| d.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(list.into_iter().next().expect("non-empty"))
    }

    fn err_of(driver: *mut Iasio, fallback: &str) -> EngineError {
        let mut buf = [0u8; 128];
        unsafe {
            if !driver.is_null() {
                let v = &*(*driver).vtbl;
                (v.get_error_message)(driver, buf.as_mut_ptr());
            }
        }
        let msg = buf
            .iter()
            .take_while(|b| **b != 0)
            .map(|b| *b as char)
            .collect::<String>();
        if msg.trim().is_empty() {
            EngineError::msg(fallback.to_string())
        } else {
            EngineError::msg(format!("{fallback} ({msg})"))
        }
    }

    const K_ASIO_SET_IO_FORMAT: c_long = 0x2311_1961;
    const K_ASIO_GET_IO_FORMAT: c_long = 0x2311_1983;
    const K_ASIO_CAN_DO_IO_FORMAT: c_long = 0x2311_2004;
    const ASIO_IO_PCM: i32 = 0;

    #[repr(C)]
    struct AsioIoFormat {
        format_type: i32,
        reserved: [u8; 508],
    }

    enum StreamKind {
        Pcm {
            ring: Arc<FrameRing>,
            rate: u32,
        },
        NativeDsd {
            ring: Arc<ByteRing>,
            dsd_hz: u32,
            file_lsb: bool,
        },
        Dop {
            ring: Arc<ByteRing>,
            dsd_hz: u32,
            file_lsb: bool,
        },
    }

    struct Opened {
        rate: u32,
        channels: u16,
        name: String,
        io_path: String,
        dsd_diag: String,
    }

    pub fn start(
        driver_name: Option<&str>,
        source_rate: u32,
        hwnd: Option<usize>,
        ring: Arc<FrameRing>,
        playing: Arc<AtomicBool>,
    ) -> Result<AsioStream, EngineError> {
        start_kind(
            driver_name,
            hwnd,
            playing,
            StreamKind::Pcm {
                ring,
                rate: source_rate,
            },
        )
    }

    pub fn start_dsd(
        driver_name: Option<&str>,
        dsd_hz: u32,
        hwnd: Option<usize>,
        ring: Arc<ByteRing>,
        playing: Arc<AtomicBool>,
        file_lsb: bool,
        native: bool,
    ) -> Result<AsioStream, EngineError> {
        let kind = if native {
            StreamKind::NativeDsd {
                ring,
                dsd_hz,
                file_lsb,
            }
        } else {
            StreamKind::Dop {
                ring,
                dsd_hz,
                file_lsb,
            }
        };
        start_kind(driver_name, hwnd, playing, kind)
    }

    fn start_kind(
        driver_name: Option<&str>,
        hwnd: Option<usize>,
        playing: Arc<AtomicBool>,
        kind: StreamKind,
    ) -> Result<AsioStream, EngineError> {
        let meta = pick_driver(driver_name)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<Result<Opened, EngineError>>();
        let stop_t = stop.clone();
        let name_for_thread = meta.name.clone();
        let clsid = meta.clsid.clone();

        let thread = std::thread::Builder::new()
            .name("hifi-asio".into())
            .spawn(move || {
                let hr = unsafe { CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32) };
                if hr < 0 && hr != 1 {
                    let _ = tx.send(Err(EngineError::msg(format!(
                        "Kunde inte initiera COM för ASIO (HRESULT {hr:#x})."
                    ))));
                    return;
                }
                let result = run_driver(&clsid, hwnd, playing, stop_t, &name_for_thread, kind, &tx);
                if let Err(err) = result {
                    let _ = tx.send(Err(err));
                }
                unsafe { CoUninitialize() };
            })
            .map_err(|e| EngineError::msg(format!("ASIO-tråd: {e}")))?;

        match rx.recv() {
            Ok(Ok(opened)) => Ok(AsioStream {
                stop,
                thread: Some(thread),
                sample_rate: opened.rate,
                channels: opened.channels,
                driver_name: opened.name,
                io_path: opened.io_path,
                dsd_diag: opened.dsd_diag,
            }),
            Ok(Err(err)) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                Err(EngineError::msg("ASIO-tråden dog innan strömmen öppnades."))
            }
        }
    }

    fn probe_io_format(driver: *mut Iasio, vtbl: &IasioVtbl, dsd: bool) -> IoFormatProbe {
        let requested = if dsd { ASIO_IO_DSD } else { ASIO_IO_PCM };
        let mut fmt = AsioIoFormat {
            format_type: requested,
            reserved: [0; 508],
        };
        let can = unsafe {
            (vtbl.future)(
                driver,
                K_ASIO_CAN_DO_IO_FORMAT,
                &mut fmt as *mut AsioIoFormat as *mut c_void,
            )
        };
        fmt.format_type = requested;
        let set = unsafe {
            (vtbl.future)(
                driver,
                K_ASIO_SET_IO_FORMAT,
                &mut fmt as *mut AsioIoFormat as *mut c_void,
            )
        };
        let set_format_type = fmt.format_type;
        // Seed PCM (0), not Invalid(-1): some drivers treat Get's input as a command.
        // A no-op Get leaving 0 cannot confirm DSD (requested=1).
        let mut got = AsioIoFormat {
            format_type: ASIO_IO_PCM,
            reserved: [0; 508],
        };
        let get = unsafe {
            (vtbl.future)(
                driver,
                K_ASIO_GET_IO_FORMAT,
                &mut got as *mut AsioIoFormat as *mut c_void,
            )
        };
        IoFormatProbe {
            can,
            set,
            set_format_type,
            get: Some(get),
            get_format_type: Some(got.format_type),
            requested,
        }
    }

    fn read_channel_sample_type(driver: *mut Iasio, vtbl: &IasioVtbl) -> Result<i32, EngineError> {
        let mut info = ChannelInfo {
            channel: 0,
            is_input: 0,
            is_active: 0,
            channel_group: 0,
            sample_type: 0,
            name: [0; 32],
        };
        if unsafe { (vtbl.get_channel_info)(driver, &mut info) } != 0 {
            return Err(err_of(driver, "getChannelInfo misslyckades."));
        }
        Ok(info.sample_type)
    }

    fn buffers_complete(infos: &[BufferInfo]) -> bool {
        !infos.is_empty()
            && infos
                .iter()
                .all(|b| !b.buffers[0].is_null() && !b.buffers[1].is_null())
    }

    fn teardown_asio(
        driver: *mut Iasio,
        vtbl: &IasioVtbl,
        created_buffers: bool,
        restore_pcm: bool,
    ) {
        SUPPORT_TIME_INFO.store(true, Ordering::Release);
        let _ = unsafe { (vtbl.stop)(driver) };
        uninstall_rt_host();
        if created_buffers {
            let _ = unsafe { (vtbl.dispose_buffers)(driver) };
        }
        // IoFormat switch is only legal outside the prepared state (after dispose).
        if restore_pcm {
            let _ = probe_io_format(driver, vtbl, false);
        }
        unsafe {
            (vtbl.release)(driver);
        }
    }

    fn try_set_rate(
        driver: *mut Iasio,
        vtbl: &IasioVtbl,
        display_name: &str,
        want: u32,
    ) -> Result<u32, EngineError> {
        if !asio_sample_rate_supported(unsafe { (vtbl.can_sample_rate)(driver, want as f64) }) {
            return Err(EngineError::msg(format!(
                "{display_name} stöder inte {want} Hz (canSampleRate)."
            )));
        }
        if unsafe { (vtbl.set_sample_rate)(driver, want as f64) } != 0 {
            return Err(EngineError::msg(format!(
                "{display_name} tog inte {want} Hz (setSampleRate)."
            )));
        }
        let mut actual = 0.0f64;
        let _ = unsafe { (vtbl.get_sample_rate)(driver, &mut actual) };
        Ok(if actual > 1.0 {
            actual.round() as u32
        } else {
            want
        })
    }

    fn run_driver(
        clsid: &str,
        hwnd: Option<usize>,
        playing: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        display_name: &str,
        kind: StreamKind,
        tx: &std::sync::mpsc::Sender<Result<Opened, EngineError>>,
    ) -> Result<(), EngineError> {
        let guid = guid_from_clsid(clsid)?;
        let mut raw: *mut c_void = ptr::null_mut();
        let hr = unsafe {
            CoCreateInstance(
                &guid,
                ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &guid,
                &mut raw,
            )
        };
        if hr < 0 || raw.is_null() {
            return Err(EngineError::msg(format!(
                "CoCreateInstance misslyckades för {display_name} (HRESULT {hr:#x}). Är FiiO ASIO Driver installerad (64-bit)?"
            )));
        }
        let driver = raw as *mut Iasio;
        let vtbl = unsafe { &*(*driver).vtbl };

        let sys = hwnd.map(|h| h as *mut c_void).unwrap_or(ptr::null_mut());
        // IASIO::init returns ASIOBool, not ASIOError: 0 = ASIOFalse (denied).
        let ok = unsafe { (vtbl.init)(driver, sys) };
        if ok == 0 {
            let err = err_of(
                driver,
                &format!("{display_name} init() nekade. Stäng Qobuz/annan ASIO-värd som kan ha låst DAC:n."),
            );
            unsafe { (vtbl.release)(driver) };
            return Err(err);
        }

        let mut ins = 0i32;
        let mut outs = 0i32;
        if unsafe { (vtbl.get_channels)(driver, &mut ins, &mut outs) } != 0 || outs < 1 {
            unsafe {
                (vtbl.release)(driver);
            }
            return Err(EngineError::msg(format!(
                "{display_name} har inga utgångskanaler."
            )));
        }
        let channels = outs.min(2) as usize;

        let (want_dsd_io, rate_candidates, native_dsd, dop, file_lsb, pcm_ring, dsd_ring) =
            match &kind {
                StreamKind::Pcm { ring, rate } => (
                    false,
                    vec![*rate],
                    false,
                    false,
                    false,
                    Some(ring.clone()),
                    None,
                ),
                StreamKind::NativeDsd {
                    ring,
                    dsd_hz,
                    file_lsb,
                } => (
                    true,
                    vec![*dsd_hz, dsd::native_dsd_byte_rate(*dsd_hz)],
                    true,
                    false,
                    *file_lsb,
                    None,
                    Some(ring.clone()),
                ),
                StreamKind::Dop {
                    ring,
                    dsd_hz,
                    file_lsb,
                } => (
                    false,
                    vec![dsd::dop_pcm_rate(*dsd_hz)],
                    false,
                    true,
                    *file_lsb,
                    None,
                    Some(ring.clone()),
                ),
            };

        let io_probe = probe_io_format(driver, vtbl, want_dsd_io);
        let io_sw = classify_io_format_probe(&io_probe);
        let mut dsd_diag = format_io_format_diag(&io_probe, io_sw);
        publish_dsd_diag(&dsd_diag);
        // Native always calls Set(DSD); restore PCM before release if we abort.
        let restore_pcm = native_dsd;
        if native_dsd && io_sw != IoFormatSwitch::Applied {
            teardown_asio(driver, vtbl, false, true);
            return Err(EngineError::msg(format!(
                "{display_name} bekräftade inte native DSD ({dsd_diag}). Native DSD avbryts — prova DoP. Filen rörs inte."
            )));
        }

        let mut rate_err = String::new();
        let mut rate = 0u32;
        let mut got_rate = false;
        let mut rejected_rates: Vec<String> = Vec::new();
        for cand in &rate_candidates {
            if *cand == 0 {
                continue;
            }
            match try_set_rate(driver, vtbl, display_name, *cand) {
                Ok(r) => {
                    rate = r;
                    got_rate = true;
                    dsd_diag.push_str(&format!(" rate_ok={r} (asked {cand})"));
                    break;
                }
                Err(e) => {
                    rejected_rates.push(format!("{cand}:{e}"));
                    rate_err = e.to_string();
                }
            }
        }
        if !rejected_rates.is_empty() && !got_rate {
            dsd_diag.push_str(&format!(" rate_fail=[{}]", rejected_rates.join("; ")));
        } else if !rejected_rates.is_empty() {
            dsd_diag.push_str(&format!(" rate_skipped=[{}]", rejected_rates.join("; ")));
        }
        if !got_rate {
            teardown_asio(driver, vtbl, false, restore_pcm);
            return Err(EngineError::msg(if native_dsd {
                format!(
                    "{display_name} tog inte native DSD-hastighet (bit/byte). {dsd_diag} {rate_err} Ingen tyst PCM-omkodning."
                )
            } else if dop {
                format!(
                    "{display_name} tog inte DoP-hastighet ({}/16 Hz). {dsd_diag} {rate_err} Delat WASAPI används inte för DSD.",
                    match &kind {
                        StreamKind::Dop { dsd_hz, .. } => *dsd_hz,
                        _ => 0,
                    }
                )
            } else {
                format!(
                    "{display_name} stöder inte källans Hz. Ingen tyst omsampling i ASIO — byt filhastighet eller använd WASAPI. {rate_err}"
                )
            }));
        }

        let sample_type_pre = match read_channel_sample_type(driver, vtbl) {
            Ok(v) => v,
            Err(err) => {
                teardown_asio(driver, vtbl, false, restore_pcm);
                return Err(EngineError::msg(format!("{err} {dsd_diag}")));
            }
        };
        dsd_diag.push_str(&format!(" sample_type_pre={sample_type_pre}"));
        publish_dsd_diag(&dsd_diag);
        let sample_type = SampleType::from_asio(sample_type_pre);
        // FiiO AV: do not createBuffers/start in DSD IoFormat unless type is already DSD.
        if native_dsd && !native_dsd_may_create_buffers(sample_type) {
            dsd_diag.push_str(" refuse_createBuffers=sample_type_not_dsd_pre");
            publish_dsd_diag(&dsd_diag);
            teardown_asio(driver, vtbl, false, true);
            return Err(EngineError::msg(format!(
                "{display_name} native DSD: sampletyp {sample_type_pre} före createBuffers (inte DSD 32/33/40). Hoppar createBuffers/start (FiiO AV). {dsd_diag} Native DSD avbryts — prova DoP. Filen rörs inte."
            )));
        }
        if dop {
            if !sample_type.dop_ok() {
                teardown_asio(driver, vtbl, false, false);
                return Err(EngineError::msg(format!(
                    "{display_name} DoP kräver 24/32-bit PCM (inte f32/i16/DSD). Sampletyp {sample_type_pre}. {dsd_diag} Filen rörs inte.",
                )));
            }
        } else if !native_dsd {
            if sample_type.is_dsd() {
                teardown_asio(driver, vtbl, false, false);
                return Err(EngineError::msg(
                    "ASIO-kanalen är DSD medan filen är PCM. Växla DAC:n till PCM eller spela en DSD-fil. Filen rörs inte.",
                ));
            }
            if !sample_type.is_pcm() {
                teardown_asio(driver, vtbl, false, false);
                return Err(EngineError::msg(format!(
                    "ASIO-kanalen har okänd sampletyp ({sample_type_pre}). Native-motorn vägrar tyst uppspelning — använd PCM/FLAC eller en drivrutin med i16/i24/i32/f32. Filen rörs inte."
                )));
            }
        }

        let mut min = 0i32;
        let mut max = 0i32;
        let mut preferred = 0i32;
        let mut gran = 0i32;
        if unsafe { (vtbl.get_buffer_size)(driver, &mut min, &mut max, &mut preferred, &mut gran) }
            != 0
        {
            let err = err_of(driver, "getBufferSize misslyckades.");
            teardown_asio(driver, vtbl, false, restore_pcm);
            return Err(EngineError::msg(format!("{err} {dsd_diag}")));
        }
        let frames = pick_buffer_frames(min, max, preferred, gran);
        dsd_diag.push_str(&format!(
            " buf=min{min}/max{max}/pref{preferred}/gran{gran}/used{frames}"
        ));
        publish_dsd_diag(&dsd_diag);

        SUPPORT_TIME_INFO.store(!native_dsd, Ordering::Release);

        let mut callbacks = make_callbacks();
        let mut buf_infos: Vec<BufferInfo> = (0..channels)
            .map(|ch| BufferInfo {
                is_input: 0,
                channel_num: ch as c_long,
                buffers: [ptr::null_mut(); 2],
            })
            .collect();

        let created = unsafe {
            (vtbl.create_buffers)(
                driver,
                buf_infos.as_mut_ptr(),
                channels as c_long,
                frames as c_long,
                &mut *callbacks as *mut Callbacks,
            )
        };
        if created != 0 {
            let err = err_of(
                driver,
                &format!("{display_name} createBuffers misslyckades."),
            );
            teardown_asio(driver, vtbl, false, restore_pcm);
            return Err(EngineError::msg(format!("{err} {dsd_diag}")));
        }
        if !buffers_complete(&buf_infos) {
            teardown_asio(driver, vtbl, true, restore_pcm);
            return Err(EngineError::msg(format!(
                "{display_name} createBuffers lämnade null-pekare. {dsd_diag} Filen rörs inte."
            )));
        }

        let sample_type_post = match read_channel_sample_type(driver, vtbl) {
            Ok(v) => v,
            Err(err) => {
                teardown_asio(driver, vtbl, true, restore_pcm);
                return Err(EngineError::msg(format!("{err} {dsd_diag}")));
            }
        };
        dsd_diag.push_str(&format!(" sample_type_post={sample_type_post}"));
        publish_dsd_diag(&dsd_diag);
        let sample_type = SampleType::from_asio(sample_type_post);
        if native_dsd && !sample_type.is_dsd() {
            teardown_asio(driver, vtbl, true, true);
            return Err(EngineError::msg(format!(
                "{display_name} växlar inte till DSD-sampletyp efter createBuffers (fick {sample_type_post}). {dsd_diag} Native DSD avbryts — prova DoP om DAC:n stöder det. Filen rörs inte."
            )));
        }

        let buffers: Vec<[*mut u8; 2]> = buf_infos
            .iter()
            .map(|b| [b.buffers[0] as *mut u8, b.buffers[1] as *mut u8])
            .collect();

        ASIO_RESET.store(false, Ordering::Relaxed);
        let reverse = if native_dsd {
            file_lsb != sample_type.dsd_lsb_first()
        } else if dop {
            file_lsb // DoP is MSB-first
        } else {
            false
        };
        let fill = if native_dsd {
            RtFillMode::NativeDsd { reverse }
        } else if dop {
            RtFillMode::Dop
        } else {
            RtFillMode::Pcm
        };
        let io_path = if native_dsd {
            "native_dsd"
        } else if dop {
            "dop"
        } else {
            "pcm"
        };
        install_session_host(
            pcm_ring.clone(),
            dsd_ring.clone(),
            playing.clone(),
            stop.clone(),
            buffers,
            frames,
            sample_type,
            channels,
            fill,
            reverse,
        );

        dsd_diag.push_str(" pre_start_ok");
        publish_dsd_diag(&dsd_diag);

        if unsafe { (vtbl.start)(driver) } != 0 {
            let err = err_of(driver, &format!("{display_name} start() misslyckades."));
            teardown_asio(driver, vtbl, true, restore_pcm);
            return Err(EngineError::msg(format!("{err} {dsd_diag}")));
        }

        let _ = tx.send(Ok(Opened {
            rate,
            channels: channels as u16,
            name: display_name.to_string(),
            io_path: io_path.to_string(),
            dsd_diag: dsd_diag.clone(),
        }));

        while !stop.load(Ordering::Relaxed) {
            if ASIO_RESET.swap(false, Ordering::AcqRel) {
                let _ = unsafe { (vtbl.stop)(driver) };
                uninstall_rt_host();
                let _ = unsafe { (vtbl.dispose_buffers)(driver) };
                for info in buf_infos.iter_mut() {
                    info.buffers = [ptr::null_mut(); 2];
                }
                let created = unsafe {
                    (vtbl.create_buffers)(
                        driver,
                        buf_infos.as_mut_ptr(),
                        channels as c_long,
                        frames as c_long,
                        &mut *callbacks as *mut Callbacks,
                    )
                };
                if created != 0 || !buffers_complete(&buf_infos) {
                    let err = err_of(
                        driver,
                        &format!("{display_name} createBuffers misslyckades efter ASIO reset."),
                    );
                    teardown_asio(driver, vtbl, created == 0, restore_pcm);
                    return Err(EngineError::msg(format!("{err} {dsd_diag}")));
                }
                let buffers: Vec<[*mut u8; 2]> = buf_infos
                    .iter()
                    .map(|b| [b.buffers[0] as *mut u8, b.buffers[1] as *mut u8])
                    .collect();
                install_session_host(
                    pcm_ring.clone(),
                    dsd_ring.clone(),
                    playing.clone(),
                    stop.clone(),
                    buffers,
                    frames,
                    sample_type,
                    channels,
                    fill,
                    reverse,
                );
                if unsafe { (vtbl.start)(driver) } != 0 {
                    let err = err_of(
                        driver,
                        &format!("{display_name} start() misslyckades efter ASIO reset."),
                    );
                    teardown_asio(driver, vtbl, true, restore_pcm);
                    return Err(EngineError::msg(format!("{err} {dsd_diag}")));
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        teardown_asio(driver, vtbl, true, restore_pcm);
        let _ = callbacks;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_io_format_switch_native_requires_applied() {
        assert_eq!(classify_io_format_switch(0, 0), IoFormatSwitch::Applied);
        assert_eq!(
            classify_io_format_switch(ASE_NOT_PRESENT, 0),
            IoFormatSwitch::Applied
        );
        assert_eq!(
            classify_io_format_switch(ASE_NOT_PRESENT, ASE_NOT_PRESENT),
            IoFormatSwitch::NotImplemented
        );
        assert_eq!(classify_io_format_switch(0, -1), IoFormatSwitch::Denied);
        // Set ASE_OK is the gate; CanDo=ASIOTrue(1) must not deny (FiiO-style).
        assert_eq!(classify_io_format_switch(1, 0), IoFormatSwitch::Applied);
        assert_ne!(IoFormatSwitch::NotImplemented, IoFormatSwitch::Applied);
        assert_ne!(IoFormatSwitch::Denied, IoFormatSwitch::Applied);
    }

    #[test]
    fn classify_io_format_accepts_ase_success_future_codes() {
        assert!(asio_future_succeeded(ASE_SUCCESS));
        assert!(asio_future_succeeded(ASE_OK));
        assert!(!asio_future_succeeded(ASIO_TRUE));
        assert!(!asio_future_succeeded(ASE_NOT_PRESENT));
        // Steinberg asio.h: ASIOFuture DSD switch succeeds with ASE_SUCCESS, not ASE_OK.
        assert_eq!(
            classify_io_format_switch(ASE_SUCCESS, ASE_SUCCESS),
            IoFormatSwitch::Applied
        );
        assert_eq!(
            classify_io_format_switch(ASE_OK, ASE_SUCCESS),
            IoFormatSwitch::Applied
        );
        assert_eq!(
            classify_io_format_switch(ASE_SUCCESS, ASE_OK),
            IoFormatSwitch::Applied
        );
        assert_eq!(
            classify_io_format_switch(ASIO_TRUE, ASE_SUCCESS),
            IoFormatSwitch::Applied
        );
        assert_eq!(
            classify_io_format_switch(ASIO_TRUE, ASE_OK),
            IoFormatSwitch::Applied
        );
        // Set not implemented and CanDo not implemented: PCM-only driver.
        assert_eq!(
            classify_io_format_switch(ASE_NOT_PRESENT, ASE_NOT_PRESENT),
            IoFormatSwitch::NotImplemented
        );
        // Hard refuse.
        assert_eq!(
            classify_io_format_switch(ASE_OK, ASE_NOT_PRESENT),
            IoFormatSwitch::Denied
        );
    }

    #[test]
    fn classify_io_format_invalidated_type_is_denied() {
        let denied = IoFormatProbe {
            can: ASE_OK,
            set: ASE_OK,
            set_format_type: ASIO_IO_INVALID,
            get: Some(ASE_OK),
            get_format_type: Some(ASIO_IO_DSD),
            requested: ASIO_IO_DSD,
        };
        assert_eq!(classify_io_format_probe(&denied), IoFormatSwitch::Denied);
        let kept_but_set_failed = IoFormatProbe {
            can: ASE_OK,
            set: -998, // ASE_InvalidParameter-ish
            set_format_type: ASIO_IO_DSD,
            get: None,
            get_format_type: None,
            requested: ASIO_IO_DSD,
        };
        assert_eq!(
            classify_io_format_probe(&kept_but_set_failed),
            IoFormatSwitch::Denied
        );
    }

    #[test]
    fn classify_io_format_get_readback_can_confirm() {
        // Set return odd but format kept and GetIoFormat reports DSD with ASE_SUCCESS.
        let p = IoFormatProbe {
            can: ASIO_TRUE,
            set: ASIO_TRUE,
            set_format_type: ASIO_IO_DSD,
            get: Some(ASE_SUCCESS),
            get_format_type: Some(ASIO_IO_DSD),
            requested: ASIO_IO_DSD,
        };
        assert_eq!(classify_io_format_probe(&p), IoFormatSwitch::Applied);
        // Get no-op leaving Invalid must not confirm.
        let stub_get = IoFormatProbe {
            can: ASE_OK,
            set: -1,
            set_format_type: ASIO_IO_DSD,
            get: Some(ASE_OK),
            get_format_type: Some(ASIO_IO_INVALID),
            requested: ASIO_IO_DSD,
        };
        assert_eq!(classify_io_format_probe(&stub_get), IoFormatSwitch::Denied);
    }

    #[test]
    fn io_format_diag_includes_raw_codes() {
        let p = IoFormatProbe {
            can: ASE_SUCCESS,
            set: ASE_SUCCESS,
            set_format_type: ASIO_IO_DSD,
            get: Some(ASE_OK),
            get_format_type: Some(ASIO_IO_DSD),
            requested: ASIO_IO_DSD,
        };
        let s = format_io_format_diag(&p, IoFormatSwitch::Applied);
        assert!(s.contains("ASE_SUCCESS(0x3f4847a0)"), "{s}");
        assert!(s.contains("get_type=1"), "{s}");
        assert!(s.contains("switch=Applied"), "{s}");
        assert_eq!(format_asio_code(1), "ASIOTrue(1)");
        assert_eq!(format_asio_code(-1000), "ASE_NotPresent(-1000)");
    }

    #[test]
    fn pick_buffer_frames_clamps_insane_preferred() {
        assert_eq!(pick_buffer_frames(64, 2048, 512, 8), 512);
        assert_eq!(pick_buffer_frames(64, 2048, 0, 0), 64);
        assert_eq!(pick_buffer_frames(0, 0, 0, 0), 64);
        // DSD bit-rate leaked as preferredSize must not become the write length.
        assert_eq!(pick_buffer_frames(64, 2048, 11_289_600, 0), 2048);
        assert_eq!(pick_buffer_frames(0, 0, 11_289_600, 0), 65_536);
        assert_eq!(pick_buffer_frames(64, 2048, 100, 64), 64);
        // FiiO PCM QA: gran-1 = power-of-two sizes only.
        assert_eq!(pick_buffer_frames(8, 2048, 64, -1), 64);
        assert_eq!(pick_buffer_frames(8, 2048, 100, -1), 64);
        assert_eq!(pick_buffer_frames(8, 2048, 63, -1), 32);
    }

    #[test]
    fn native_dsd_does_not_create_buffers_on_pcm_sample_type() {
        assert!(native_dsd_may_create_buffers(SampleType::DsdLsb));
        assert!(native_dsd_may_create_buffers(SampleType::DsdMsb));
        assert!(native_dsd_may_create_buffers(SampleType::DsdNer8));
        assert!(!native_dsd_may_create_buffers(SampleType::I32Le));
        assert!(!native_dsd_may_create_buffers(SampleType::F32Le));
        assert!(!native_dsd_may_create_buffers(SampleType::I24Le));
        assert_eq!(SampleType::I32Le.bytes_per_sample(), Some(4));
        assert_eq!(SampleType::DsdLsb.bytes_per_sample(), Some(1));
    }

    #[test]
    fn reentry_is_not_classified_as_underrun() {
        assert_eq!(
            callback_busy_action(false, true),
            CallbackBusyAction::ReentryMute
        );
        assert_eq!(callback_busy_action(false, false), CallbackBusyAction::Fill);
        assert_eq!(
            callback_busy_action(true, true),
            CallbackBusyAction::StopMute
        );
        assert_eq!(
            callback_busy_action(true, false),
            CallbackBusyAction::StopMute
        );
    }

    #[test]
    fn can_sample_rate_ase_ok_means_supported() {
        // Steinberg asio.h: ASE_OK = 0 success; ASE_NoClock = rate not supported.
        const ASE_NO_CLOCK: i32 = -995;
        const ASE_NOT_PRESENT: i32 = -1000;
        assert!(asio_sample_rate_supported(0));
        assert!(asio_sample_rate_supported(ASE_OK));
        assert!(!asio_sample_rate_supported(ASE_NO_CLOCK));
        assert!(!asio_sample_rate_supported(ASE_NOT_PRESENT));
        assert!(!asio_sample_rate_supported(1));
    }

    #[test]
    fn fiio_is_preferred() {
        assert!(is_preferred_hi_fi_name("FiiO ASIO Driver"));
        assert!(is_preferred_hi_fi_name("fiio usb dac"));
        assert_eq!(rank_asio_name("FiiO ASIO Driver"), 0);
        assert_eq!(rank_asio_name("Generic ASIO"), 1);
        assert!(!is_preferred_hi_fi_name("Realtek ASIO"));
    }

    #[test]
    fn parses_clsid() {
        let g = parse_clsid("{AABBCCDD-EEFF-1122-3344-556677889900}").unwrap();
        assert_eq!(g[0], 0xaa);
        assert_eq!(g[15], 0x00);
        assert!(parse_clsid("not-a-guid").is_err());
    }

    #[test]
    fn dsd_sample_types_detected() {
        assert!(SampleType::from_asio(32).is_dsd());
        assert!(SampleType::from_asio(33).is_dsd());
        assert!(SampleType::from_asio(40).is_dsd());
        assert!(SampleType::from_asio(32).dsd_lsb_first());
        assert!(!SampleType::from_asio(33).dsd_lsb_first());
        assert!(!SampleType::from_asio(18).is_dsd());
        assert!(!SampleType::from_asio(19).is_dsd());
        assert!(SampleType::from_asio(18).is_pcm());
        assert!(SampleType::from_asio(19).is_pcm());
        assert!(!SampleType::from_asio(32).is_pcm());
    }

    #[test]
    fn unknown_sample_type_is_not_played_as_pcm() {
        let other = SampleType::from_asio(99);
        assert!(!other.is_pcm());
        assert!(!other.is_dsd());
        assert_eq!(other, SampleType::Other(99));
    }

    #[test]
    fn reset_request_flag_can_be_acked_without_host() {
        ASIO_RESET.store(true, Ordering::Relaxed);
        assert!(ASIO_RESET.swap(false, Ordering::AcqRel));
        assert!(!ASIO_RESET.load(Ordering::Relaxed));
    }

    #[test]
    fn non_windows_list_is_empty() {
        if !cfg!(windows) {
            assert!(list_asio_drivers().is_empty());
            assert!(!asio_available());
        }
    }

    #[test]
    fn rt_host_install_then_uninstall_clears_pointer() {
        let ring = Arc::new(FrameRing::new(2));
        install_rt_host(new_rt_host(
            ring,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            vec![[ptr::null_mut(), ptr::null_mut()]],
            64,
            SampleType::F32Le,
            2,
        ));
        assert!(!RT_HOST.load(Ordering::Acquire).is_null());
        uninstall_rt_host();
        assert!(RT_HOST.load(Ordering::Acquire).is_null());
        uninstall_rt_host();
        assert!(RT_HOST.load(Ordering::Acquire).is_null());
    }

    #[test]
    fn rt_host_fast_reinstall_replaces_previous() {
        let ring = Arc::new(FrameRing::new(2));
        for _ in 0..3 {
            install_rt_host(new_rt_host(
                ring.clone(),
                Arc::new(AtomicBool::new(true)),
                Arc::new(AtomicBool::new(false)),
                vec![[ptr::null_mut(), ptr::null_mut()]],
                32,
                SampleType::I32Le,
                2,
            ));
        }
        uninstall_rt_host();
        assert!(RT_HOST.load(Ordering::Acquire).is_null());
    }
}
