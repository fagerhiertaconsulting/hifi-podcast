//! Windows ASIO host: registry discovery + IASIO COM (no Steinberg SDK).
//!
//! Drivers such as **FiiO ASIO Driver** register under `HKLM\SOFTWARE\ASIO`.
//! Hosts (Qobuz, foobar2000) CoCreate the CLSID and talk IASIO. Callbacks are
//! `cdecl`; COM methods are `stdcall`.

use super::ring::FrameRing;
use super::EngineError;
use serde::Serialize;
use std::cell::UnsafeCell;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::Arc;

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
    Dsd,
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
            32 | 33 | 40 => Self::Dsd,
            other => Self::Other(other),
        }
    }

    fn is_dsd(self) -> bool {
        matches!(self, Self::Dsd)
    }

    fn is_pcm(self) -> bool {
        matches!(
            self,
            Self::F32Le | Self::I16Le | Self::I24Le | Self::I32Le | Self::I32Le24
        )
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

pub struct AsioStream {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub driver_name: String,
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

/// Realtime host published to ASIO `bufferSwitch` via atomic pointer (no Mutex in the callback).
/// Fields are read from the Windows ASIO callback; Linux tests only install/uninstall the pointer.
#[allow(dead_code)]
struct RtHost {
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
    stop: Arc<AtomicBool>,
    scratch: UnsafeCell<Vec<f32>>,
    scratch_busy: AtomicBool,
}

unsafe impl Send for RtHost {}
unsafe impl Sync for RtHost {}

static RT_HOST: AtomicPtr<RtHost> = AtomicPtr::new(ptr::null_mut());
static RT_IN_FLIGHT: AtomicU32 = AtomicU32::new(0);
static ASIO_RESET: AtomicBool = AtomicBool::new(false);

fn install_rt_host(host: Box<RtHost>) {
    uninstall_rt_host();
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

fn new_rt_host(
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
) -> Box<RtHost> {
    let n = frames.saturating_mul(channels.max(1));
    Box::new(RtHost {
        ring,
        playing,
        buffers,
        frames,
        sample_type,
        channels,
        stop,
        scratch: UnsafeCell::new(vec![0.0f32; n]),
        scratch_busy: AtomicBool::new(false),
    })
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

    static CALLBACKS: Callbacks = Callbacks {
        buffer_switch,
        sample_rate_did_change,
        asio_message,
        buffer_switch_time_info,
    };

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
            7 => 1,
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

    fn fill_buffers(index: c_long) {
        RT_IN_FLIGHT.fetch_add(1, Ordering::AcqRel);
        let host = RT_HOST.load(Ordering::Acquire);
        if host.is_null() {
            RT_IN_FLIGHT.fetch_sub(1, Ordering::Release);
            return;
        }
        let st = unsafe { &*host };
        let idx = if index == 0 { 0 } else { 1 };
        let frames = st.frames;
        let ch = st.channels.max(1);

        let write_silence =
            st.stop.load(Ordering::Relaxed) || st.scratch_busy.swap(true, Ordering::AcqRel);
        if write_silence {
            if !st.stop.load(Ordering::Relaxed) {
                st.ring.note_underrun();
            }
            for c in 0..ch {
                let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
                if p.is_null() {
                    continue;
                }
                unsafe {
                    write_channel(p, st.sample_type, frames, |_| 0.0);
                }
            }
            RT_IN_FLIGHT.fetch_sub(1, Ordering::Release);
            return;
        }

        let scratch = unsafe { &mut *st.scratch.get() };
        if scratch.len() < frames * ch {
            // Preallocated at open; never grow on the realtime thread.
            scratch.fill(0.0);
            st.ring.note_underrun();
            st.scratch_busy.store(false, Ordering::Release);
            for c in 0..ch {
                let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
                if p.is_null() {
                    continue;
                }
                unsafe {
                    write_channel(p, st.sample_type, frames, |_| 0.0);
                }
            }
            RT_IN_FLIGHT.fetch_sub(1, Ordering::Release);
            return;
        }
        let dest = &mut scratch[..frames * ch];
        if st.playing.load(Ordering::Relaxed) {
            let _ = st.ring.pop_interleaved(dest);
        } else {
            dest.fill(0.0);
        }
        for c in 0..ch {
            let p = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
            if p.is_null() {
                continue;
            }
            unsafe {
                write_channel(p, st.sample_type, frames, |i| dest[i * ch + c]);
            }
        }
        st.scratch_busy.store(false, Ordering::Release);
        RT_IN_FLIGHT.fetch_sub(1, Ordering::Release);
    }

    unsafe fn write_channel(
        ptr: *mut u8,
        ty: SampleType,
        frames: usize,
        sample: impl Fn(usize) -> f32,
    ) {
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
            SampleType::Dsd | SampleType::Other(_) => {
                // Open refuses these types; zeros are a last-resort RT fill only.
                for i in 0..frames {
                    ptr::write(ptr.add(i), 0);
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

    pub fn start(
        driver_name: Option<&str>,
        source_rate: u32,
        hwnd: Option<usize>,
        ring: Arc<FrameRing>,
        playing: Arc<AtomicBool>,
    ) -> Result<AsioStream, EngineError> {
        let meta = pick_driver(driver_name)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<Result<(u32, u16, String), EngineError>>();
        let stop_t = stop.clone();
        let name_for_thread = meta.name.clone();
        let clsid = meta.clsid.clone();

        let thread = std::thread::Builder::new()
            .name("hifi-asio".into())
            .spawn(move || {
                let hr = unsafe { CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32) };
                if hr < 0 && hr != 1 {
                    // 1 = S_FALSE already initialized
                    let _ = tx.send(Err(EngineError::msg(format!(
                        "Kunde inte initiera COM för ASIO (HRESULT {hr:#x})."
                    ))));
                    return;
                }
                let result = run_driver(
                    &clsid,
                    source_rate,
                    hwnd,
                    ring,
                    playing,
                    stop_t,
                    &name_for_thread,
                    &tx,
                );
                if let Err(err) = result {
                    let _ = tx.send(Err(err));
                }
                unsafe { CoUninitialize() };
            })
            .map_err(|e| EngineError::msg(format!("ASIO-tråd: {e}")))?;

        match rx.recv() {
            Ok(Ok((rate, ch, name))) => Ok(AsioStream {
                stop,
                thread: Some(thread),
                sample_rate: rate,
                channels: ch,
                driver_name: name,
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

    fn run_driver(
        clsid: &str,
        source_rate: u32,
        hwnd: Option<usize>,
        ring: Arc<FrameRing>,
        playing: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        display_name: &str,
        tx: &std::sync::mpsc::Sender<Result<(u32, u16, String), EngineError>>,
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

        let want = source_rate as f64;
        // canSampleRate is ASIOError: ASE_OK (0) means the rate *is* supported (see
        // `asio_sample_rate_supported`). Do not treat this like init()'s ASIOBool.
        if asio_sample_rate_supported(unsafe { (vtbl.can_sample_rate)(driver, want) }) {
            if unsafe { (vtbl.set_sample_rate)(driver, want) } != 0 {
                unsafe { (vtbl.release)(driver) };
                return Err(EngineError::msg(format!(
                    "{display_name} tog inte {source_rate} Hz. Välj en fil med en samplingsfrekvens DAC:n stöder, eller använd WASAPI-delat som fallback."
                )));
            }
        } else {
            unsafe { (vtbl.release)(driver) };
            return Err(EngineError::msg(format!(
                "{display_name} stöder inte {source_rate} Hz (canSampleRate). Ingen tyst omsampling i ASIO — byt filhastighet eller använd WASAPI."
            )));
        }

        let mut actual = 0.0f64;
        let _ = unsafe { (vtbl.get_sample_rate)(driver, &mut actual) };
        let rate = if actual > 1.0 {
            actual.round() as u32
        } else {
            source_rate
        };

        let mut info = ChannelInfo {
            channel: 0,
            is_input: 0,
            is_active: 0,
            channel_group: 0,
            sample_type: 0,
            name: [0; 32],
        };
        if unsafe { (vtbl.get_channel_info)(driver, &mut info) } != 0 {
            let err = err_of(driver, "getChannelInfo misslyckades.");
            unsafe { (vtbl.release)(driver) };
            return Err(err);
        }
        let sample_type = SampleType::from_asio(info.sample_type);
        if sample_type.is_dsd() {
            unsafe { (vtbl.release)(driver) };
            return Err(EngineError::msg(
                "ASIO-kanalen är DSD. Native DSD via ASIO är inte i den här PR:n — spela FLAC/PCM. Filen rörs inte.",
            ));
        }
        if !sample_type.is_pcm() {
            let code = match sample_type {
                SampleType::Other(v) => v,
                _ => info.sample_type,
            };
            unsafe { (vtbl.release)(driver) };
            return Err(EngineError::msg(format!(
                "ASIO-kanalen har okänd sampletyp ({code}). Native-motorn vägrar tyst uppspelning — använd PCM/FLAC eller en drivrutin med i16/i24/i32/f32. Filen rörs inte."
            )));
        }

        let mut min = 0i32;
        let mut max = 0i32;
        let mut preferred = 0i32;
        let mut gran = 0i32;
        if unsafe { (vtbl.get_buffer_size)(driver, &mut min, &mut max, &mut preferred, &mut gran) }
            != 0
        {
            let err = err_of(driver, "getBufferSize misslyckades.");
            unsafe { (vtbl.release)(driver) };
            return Err(err);
        }
        let frames = if preferred > 0 {
            preferred
        } else {
            min.max(64)
        } as usize;

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
                &CALLBACKS as *const Callbacks as *mut Callbacks,
            )
        };
        if created != 0 {
            let err = err_of(
                driver,
                &format!("{display_name} createBuffers misslyckades."),
            );
            unsafe { (vtbl.release)(driver) };
            return Err(err);
        }

        let buffers: Vec<[*mut u8; 2]> = buf_infos
            .iter()
            .map(|b| [b.buffers[0] as *mut u8, b.buffers[1] as *mut u8])
            .collect();

        ASIO_RESET.store(false, Ordering::Relaxed);
        install_rt_host(new_rt_host(
            ring.clone(),
            playing.clone(),
            stop.clone(),
            buffers,
            frames,
            sample_type,
            channels,
        ));

        if unsafe { (vtbl.start)(driver) } != 0 {
            let err = err_of(driver, &format!("{display_name} start() misslyckades."));
            uninstall_rt_host();
            let _ = unsafe { (vtbl.dispose_buffers)(driver) };
            unsafe { (vtbl.release)(driver) };
            return Err(err);
        }

        let _ = tx.send(Ok((rate, channels as u16, display_name.to_string())));

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
                        &CALLBACKS as *const Callbacks as *mut Callbacks,
                    )
                };
                if created != 0 {
                    let err = err_of(
                        driver,
                        &format!("{display_name} createBuffers misslyckades efter ASIO reset."),
                    );
                    unsafe { (vtbl.release)(driver) };
                    return Err(err);
                }
                let buffers: Vec<[*mut u8; 2]> = buf_infos
                    .iter()
                    .map(|b| [b.buffers[0] as *mut u8, b.buffers[1] as *mut u8])
                    .collect();
                install_rt_host(new_rt_host(
                    ring.clone(),
                    playing.clone(),
                    stop.clone(),
                    buffers,
                    frames,
                    sample_type,
                    channels,
                ));
                if unsafe { (vtbl.start)(driver) } != 0 {
                    let err = err_of(
                        driver,
                        &format!("{display_name} start() misslyckades efter ASIO reset."),
                    );
                    uninstall_rt_host();
                    let _ = unsafe { (vtbl.dispose_buffers)(driver) };
                    unsafe { (vtbl.release)(driver) };
                    return Err(err);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let _ = unsafe { (vtbl.stop)(driver) };
        uninstall_rt_host();
        let _ = unsafe { (vtbl.dispose_buffers)(driver) };
        unsafe { (vtbl.release)(driver) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
