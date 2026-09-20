//! Windows ASIO host: registry discovery + IASIO COM (no Steinberg SDK).
//!
//! Drivers such as **FiiO ASIO Driver** register under `HKLM\SOFTWARE\ASIO`.
//! Hosts (Qobuz, foobar2000) CoCreate the CLSID and talk IASIO. Callbacks are
//! `cdecl`; COM methods are `stdcall`.

use super::ring::FrameRing;
use super::EngineError;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

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
        clear_callback_state();
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

struct CallbackState {
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
    buffers: Vec<[*mut u8; 2]>,
    frames: usize,
    sample_type: SampleType,
    channels: usize,
    stop: Arc<AtomicBool>,
}

unsafe impl Send for CallbackState {}
unsafe impl Sync for CallbackState {}

static CALLBACK: OnceLock<Mutex<Option<CallbackState>>> = OnceLock::new();

fn callback_slot() -> &'static Mutex<Option<CallbackState>> {
    CALLBACK.get_or_init(|| Mutex::new(None))
}

fn clear_callback_state() {
    if let Ok(mut g) = callback_slot().lock() {
        *g = None;
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::ffi::{c_long, c_void};
    use std::ptr;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    use windows_sys::core::GUID;
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };

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
                let clsid: String = sub.get_value("CLSID").or_else(|_| sub.get_value("clsid")).unwrap_or_default();
                if clsid.trim().is_empty() {
                    continue;
                }
                let desc: String = sub
                    .get_value("Description")
                    .or_else(|_| sub.get_value("description"))
                    .unwrap_or_else(|_| name.clone());
                let display = if desc.trim().is_empty() { name } else { desc };
                if out.iter().any(|d: &AsioDriverInfo| d.clsid.eq_ignore_ascii_case(&clsid)) {
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
        query_interface: unsafe extern "system" fn(*mut Iasio, *const GUID, *mut *mut c_void) -> i32,
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
        get_clock_sources: unsafe extern "system" fn(*mut Iasio, *mut c_void, *mut c_long) -> c_long,
        set_clock_source: unsafe extern "system" fn(*mut Iasio, c_long) -> c_long,
        get_sample_position: unsafe extern "system" fn(*mut Iasio, *mut c_void, *mut c_void) -> c_long,
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
        _value: c_long,
        _message: *mut c_void,
        _opt: *mut f64,
    ) -> c_long {
        // kAsioSelectorSupported = 1, kAsioEngineVersion = 2, kAsioResetRequest = 3
        match selector {
            1 => 1,
            2 => 2,
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
        let Ok(g) = callback_slot().lock() else {
            return;
        };
        let Some(st) = g.as_ref() else {
            return;
        };
        if st.stop.load(Ordering::Relaxed) {
            return;
        }
        let idx = if index == 0 { 0 } else { 1 };
        let frames = st.frames;
        let ch = st.channels.max(1);
        let mut interleaved = vec![0.0f32; frames * ch];
        if st.playing.load(Ordering::Relaxed) {
            let _ = st.ring.pop_interleaved(&mut interleaved);
        }
        for c in 0..ch {
            let ptr = st.buffers.get(c).map(|b| b[idx]).unwrap_or(ptr::null_mut());
            if ptr.is_null() {
                continue;
            }
            unsafe {
                write_channel(ptr, st.sample_type, frames, |i| interleaved[i * ch + c]);
            }
        }
    }

    unsafe fn write_channel(ptr: *mut u8, ty: SampleType, frames: usize, sample: impl Fn(usize) -> f32) {
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
                list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")
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
                let result = run_driver(&clsid, source_rate, hwnd, ring, playing, stop_t, &name_for_thread, &tx);
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
        if unsafe { (vtbl.can_sample_rate)(driver, want) } == 0 {
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
        let rate = if actual > 1.0 { actual.round() as u32 } else { source_rate };

        let mut info = ChannelInfo {
            channel: 0,
            is_input: 0,
            is_active: 0,
            channel_group: 0,
            sample_type: 0,
            name: [0; 32],
        };
        if unsafe { (vtbl.get_channel_info)(driver, &mut info) } != 0 {
            unsafe { (vtbl.release)(driver) };
            return Err(err_of(driver, "getChannelInfo misslyckades."));
        }
        let sample_type = SampleType::from_asio(info.sample_type);
        if sample_type.is_dsd() {
            unsafe { (vtbl.release)(driver) };
            return Err(EngineError::msg(
                "ASIO-kanalen är DSD. Native DSD via ASIO är inte i den här PR:n — spela FLAC/PCM. Filen rörs inte.",
            ));
        }

        let mut min = 0i32;
        let mut max = 0i32;
        let mut preferred = 0i32;
        let mut gran = 0i32;
        if unsafe { (vtbl.get_buffer_size)(driver, &mut min, &mut max, &mut preferred, &mut gran) } != 0
        {
            unsafe { (vtbl.release)(driver) };
            return Err(err_of(driver, "getBufferSize misslyckades."));
        }
        let frames = if preferred > 0 { preferred } else { min.max(64) } as usize;

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
            unsafe { (vtbl.release)(driver) };
            return Err(err_of(
                driver,
                &format!("{display_name} createBuffers misslyckades."),
            ));
        }

        let buffers: Vec<[*mut u8; 2]> = buf_infos
            .iter()
            .map(|b| [b.buffers[0] as *mut u8, b.buffers[1] as *mut u8])
            .collect();

        {
            let mut slot = callback_slot().lock().expect("asio callback");
            *slot = Some(CallbackState {
                ring,
                playing,
                buffers,
                frames,
                sample_type,
                channels,
                stop: stop.clone(),
            });
        }

        if unsafe { (vtbl.start)(driver) } != 0 {
            let _ = unsafe { (vtbl.dispose_buffers)(driver) };
            clear_callback_state();
            unsafe { (vtbl.release)(driver) };
            return Err(err_of(driver, &format!("{display_name} start() misslyckades.")));
        }

        let _ = tx.send(Ok((rate, channels as u16, display_name.to_string())));

        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let _ = unsafe { (vtbl.stop)(driver) };
        let _ = unsafe { (vtbl.dispose_buffers)(driver) };
        clear_callback_state();
        unsafe { (vtbl.release)(driver) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    }

    #[test]
    fn non_windows_list_is_empty() {
        if !cfg!(windows) {
            assert!(list_asio_drivers().is_empty());
            assert!(!asio_available());
        }
    }
}
