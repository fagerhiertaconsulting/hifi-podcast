//! Shared-mode output via cpal (WASAPI shared on Windows).

use super::ring::FrameRing;
use super::EngineError;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize)]
pub struct OutputDeviceInfo {
    pub name: String,
    pub id: String,
    pub api: String,
    pub is_default: bool,
    pub preferred: bool,
    pub sample_rate: u32,
    pub channels: u16,
    pub exclusive_capable: bool,
}

pub fn list_output_devices() -> Vec<OutputDeviceInfo> {
    let mut out: Vec<OutputDeviceInfo> = crate::engine::asio_host::list_asio_drivers()
        .into_iter()
        .map(|d| OutputDeviceInfo {
            id: format!("asio:{}", d.clsid),
            api: "asio".into(),
            is_default: d.preferred,
            preferred: d.preferred,
            name: d.name,
            sample_rate: 0,
            channels: 2,
            exclusive_capable: true,
        })
        .collect();

    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    if let Ok(devices) = host.output_devices() {
        for d in devices {
            let Ok(name) = d.name() else { continue };
            let cfg = d.default_output_config().ok();
            out.push(OutputDeviceInfo {
                id: format!("wasapi:{name}"),
                api: "wasapi".into(),
                name: name.clone(),
                is_default: name == default_name && out.iter().all(|x| x.api != "asio"),
                preferred: false,
                sample_rate: cfg.as_ref().map(|c| c.sample_rate().0).unwrap_or(0),
                channels: cfg.as_ref().map(|c| c.channels()).unwrap_or(0),
                exclusive_capable: cfg!(windows),
            });
        }
    }
    out
}

pub fn exclusive_available() -> bool {
    cfg!(windows)
}

pub struct SharedStream {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub sample_rate: u32,
    pub channels: u16,
    pub output_bits: u32,
    pub output_sample: String,
}

impl Drop for SharedStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start_shared(
    device_name: Option<&str>,
    source_rate: u32,
    source_channels: u16,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<SharedStream, EngineError> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_t = stop.clone();
    let device_owned = device_name.map(str::to_string);
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("hifi-cpal".into())
        .spawn(move || {
            match open_cpal_stream(
                device_owned.as_deref(),
                source_rate,
                source_channels,
                ring,
                playing,
            ) {
                Ok(stream) => {
                    let meta = (stream.sample_rate, stream.channels);
                    let _ = tx.send(Ok(meta));
                    while !stop_t.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    drop(stream);
                }
                Err(err) => {
                    let _ = tx.send(Err(err));
                }
            }
        })
        .map_err(|e| EngineError::msg(format!("cpal-tråd: {e}")))?;
    match rx.recv() {
        Ok(Ok((sample_rate, channels))) => Ok(SharedStream {
            stop,
            thread: Some(thread),
            sample_rate,
            channels,
            output_bits: 32,
            output_sample: "f32".into(),
        }),
        Ok(Err(err)) => {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
            Err(err)
        }
        Err(_) => {
            stop.store(true, Ordering::Relaxed);
            let _ = thread.join();
            Err(EngineError::msg("cpal-tråden dog innan utgången öppnades."))
        }
    }
}

struct CpalStream {
    _stream: Stream,
    sample_rate: u32,
    channels: u16,
}

fn open_cpal_stream(
    device_name: Option<&str>,
    source_rate: u32,
    source_channels: u16,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<CpalStream, EngineError> {
    let host = cpal::default_host();
    let device = pick_device(&host, device_name)?;
    let default_cfg = device
        .default_output_config()
        .map_err(|e| EngineError::msg(format!("Ingen utgångskonfiguration: {e}")))?;

    let mut channels = source_channels.max(1);
    let mut sample_rate = source_rate;
    if let Ok(mut cfgs) = device.supported_output_configs() {
        let wanted = cfgs.find(|c| {
            c.sample_format() == SampleFormat::F32
                && c.channels() == channels
                && c.min_sample_rate().0 <= source_rate
                && c.max_sample_rate().0 >= source_rate
        });
        if wanted.is_none() {
            channels = default_cfg.channels();
            sample_rate = default_cfg.sample_rate().0;
        }
    } else {
        channels = default_cfg.channels();
        sample_rate = default_cfg.sample_rate().0;
    }

    let config = StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let err_fn = |err| eprintln!("cpal utgång: {err}");
    let playing_cb = playing.clone();
    let ring_cb = ring.clone();
    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                if !playing_cb.load(Ordering::Relaxed) {
                    data.fill(0.0);
                    return;
                }
                let _ = ring_cb.pop_interleaved(data);
            },
            err_fn,
            None,
        )
        .map_err(|e| EngineError::msg(format!("Kunde inte öppna delad utgång: {e}")))?;
    stream
        .play()
        .map_err(|e| EngineError::msg(format!("Kunde inte starta utgång: {e}")))?;
    Ok(CpalStream {
        _stream: stream,
        sample_rate,
        channels,
    })
}

fn pick_device(
    host: &cpal::Host,
    name: Option<&str>,
) -> Result<cpal::Device, EngineError> {
    if let Some(name) = name.map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(mut devices) = host.output_devices() {
            if let Some(d) = devices.find(|d| d.name().ok().as_deref() == Some(name)) {
                return Ok(d);
            }
        }
        return Err(EngineError::msg(format!(
            "Hittade inte utgångsenheten \"{name}\"."
        )));
    }
    host.default_output_device()
        .ok_or_else(|| EngineError::msg("Ingen standardutgångsenhet. Anslut en DAC eller högtalare."))
}

/// True if we must resample into `stream_rate` before the ring.
pub fn needs_resample(source_rate: u32, stream_rate: u32) -> bool {
    source_rate != stream_rate && source_rate > 0 && stream_rate > 0
}
