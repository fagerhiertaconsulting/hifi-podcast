//! WASAPI exclusive output. Windows only; other OS get a clear error.

use super::ring::FrameRing;
use super::EngineError;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub struct ExclusiveStream {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl Drop for ExclusiveStream {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(not(windows))]
pub fn start_exclusive(
    _device_name: Option<&str>,
    _source_rate: u32,
    _source_channels: u16,
    _ring: Arc<FrameRing>,
    _playing: Arc<AtomicBool>,
) -> Result<ExclusiveStream, EngineError> {
    Err(EngineError::msg(
        "Exklusivt WASAPI finns bara på Windows. Delat läge (cpal) används på den här plattformen.",
    ))
}

#[cfg(windows)]
pub fn start_exclusive(
    device_name: Option<&str>,
    source_rate: u32,
    source_channels: u16,
    ring: Arc<FrameRing>,
    playing: Arc<AtomicBool>,
) -> Result<ExclusiveStream, EngineError> {
    windows_impl::start(device_name, source_rate, source_channels, ring, playing)
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::Sender;
    use wasapi::*;

    /// WASAPI `AudioClient` / `AudioRenderClient` wrap COM pointers that are not `Send`.
    /// Create, use, and drop them on one dedicated MTA thread for the stream lifetime.
    pub fn start(
        device_name: Option<&str>,
        source_rate: u32,
        source_channels: u16,
        ring: Arc<FrameRing>,
        playing: Arc<AtomicBool>,
    ) -> Result<ExclusiveStream, EngineError> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let device_owned = device_name.map(str::to_string);
        let (tx, rx) = std::sync::mpsc::channel();

        let thread = std::thread::Builder::new()
            .name("hifi-wasapi-ex".into())
            .spawn(move || {
                if let Err(err) = run_exclusive(
                    device_owned.as_deref(),
                    source_rate,
                    source_channels,
                    ring,
                    playing,
                    stop_t,
                    &tx,
                ) {
                    let _ = tx.send(Err(err));
                }
            })
            .map_err(|e| EngineError::msg(format!("WASAPI-tråd: {e}")))?;

        match rx.recv() {
            Ok(Ok((sample_rate, channels))) => Ok(ExclusiveStream {
                stop,
                thread: Some(thread),
                sample_rate,
                channels,
            }),
            Ok(Err(err)) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                Err(err)
            }
            Err(_) => {
                stop.store(true, Ordering::Relaxed);
                let _ = thread.join();
                Err(EngineError::msg(
                    "WASAPI exclusive-tråden dog innan strömmen öppnades.",
                ))
            }
        }
    }

    fn run_exclusive(
        device_name: Option<&str>,
        source_rate: u32,
        source_channels: u16,
        ring: Arc<FrameRing>,
        playing: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        tx: &Sender<Result<(u32, u16), EngineError>>,
    ) -> Result<(), EngineError> {
        let _ = initialize_mta();
        let device = pick(device_name)?;
        let mut client = device
            .get_iaudioclient()
            .map_err(|e| EngineError::msg(format!("IAudioClient: {e}")))?;

        let channels = source_channels.max(1) as usize;
        let rate = source_rate as usize;
        let candidates = [
            WaveFormat::new(32, 32, &SampleType::Float, rate, channels, None),
            WaveFormat::new(24, 24, &SampleType::Int, rate, channels, None),
            WaveFormat::new(16, 16, &SampleType::Int, rate, channels, None),
        ];

        let mut chosen: Option<WaveFormat> = None;
        for fmt in candidates {
            if let Ok(supported) = client.is_supported_exclusive_with_quirks(&fmt) {
                chosen = Some(supported);
                break;
            }
        }
        let format = chosen.ok_or_else(|| {
            EngineError::msg(format!(
                "DAC:n accepterar inte exclusive {rate} Hz / {channels} kanaler. Avmarkera exklusivt läge (delat WASAPI) eller välj en enhet som tar källans format. Ingen tyst omsampling i exclusive."
            ))
        })?;

        let (_def_period, min_period) = client
            .get_device_period()
            .map_err(|e| EngineError::msg(format!("WASAPI-period: {e}")))?;
        let period = client
            .calculate_aligned_period_near(3 * min_period / 2, Some(128), &format)
            .unwrap_or(min_period);
        let mode = StreamMode::EventsExclusive {
            period_hns: period,
        };
        client
            .initialize_client(&format, &Direction::Render, &mode)
            .map_err(|e| {
                EngineError::msg(format!(
                    "Kunde inte öppna WASAPI exclusive: {e}. Delat läge är standard — slå av exklusivt läge."
                ))
            })?;

        let h_event = client
            .set_get_eventhandle()
            .map_err(|e| EngineError::msg(format!("WASAPI-event: {e}")))?;
        let render = client
            .get_audiorenderclient()
            .map_err(|e| EngineError::msg(format!("WASAPI render: {e}")))?;
        client
            .start_stream()
            .map_err(|e| EngineError::msg(format!("WASAPI start: {e}")))?;

        let is_float = matches!(format.get_subformat(), Ok(SampleType::Float));
        let bits = format.get_validbitspersample() as usize;
        let block = format.get_blockalign() as usize;
        let bytes_per_sample = if channels == 0 { 4 } else { block / channels };

        tx.send(Ok((source_rate, source_channels)))
            .map_err(|_| EngineError::msg("WASAPI exclusive-start kunde inte rapporteras."))?;

        let mut f32_buf = vec![0.0f32; 8192 * channels];
        while !stop.load(Ordering::Relaxed) {
            if h_event.wait_for_event(200).is_err() {
                continue;
            }
            let Ok(frames) = client.get_available_space_in_frames() else {
                break;
            };
            if frames == 0 {
                continue;
            }
            let need = frames as usize * channels;
            if f32_buf.len() < need {
                f32_buf.resize(need, 0.0);
            }
            let dest = &mut f32_buf[..need];
            if playing.load(Ordering::Relaxed) {
                let _ = ring.pop_interleaved(dest);
            } else {
                dest.fill(0.0);
            }
            let bytes = f32_to_device_bytes(dest, bits, is_float, bytes_per_sample);
            let _ = render.write_to_device(frames as usize, &bytes, None);
        }
        let _ = client.stop_stream();
        Ok(())
    }

    fn pick(name: Option<&str>) -> Result<Device, EngineError> {
        if let Some(name) = name.map(str::trim).filter(|s| !s.is_empty()) {
            let col = DeviceCollection::new(&Direction::Render)
                .map_err(|e| EngineError::msg(format!("WASAPI-lista: {e}")))?;
            return col.get_device_with_name(name).map_err(|e| {
                EngineError::msg(format!("Hittade inte utgångsenheten \"{name}\" för exclusive ({e})."))
            });
        }
        get_default_device(&Direction::Render)
            .map_err(|e| EngineError::msg(format!("Ingen standard-DAC: {e}")))
    }

    fn f32_to_device_bytes(samples: &[f32], bits: usize, float: bool, bytes_per_sample: usize) -> Vec<u8> {
        if float || (bits >= 32 && bytes_per_sample == 4) {
            let mut out = Vec::with_capacity(samples.len() * 4);
            for s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
            return out;
        }
        if bits <= 16 {
            let mut out = Vec::with_capacity(samples.len() * 2);
            for s in samples {
                let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
            return out;
        }
        let mut out = Vec::with_capacity(samples.len() * bytes_per_sample.max(3));
        for s in samples {
            let v = (s.clamp(-1.0, 1.0) * 8388607.0) as i32;
            let b = v.to_le_bytes();
            if bytes_per_sample >= 4 {
                out.extend_from_slice(&[b[0], b[1], b[2], 0]);
            } else {
                out.extend_from_slice(&[b[0], b[1], b[2]]);
            }
        }
        out
    }
}
