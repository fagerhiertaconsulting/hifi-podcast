#[cfg(windows)]
mod windows_smtc {
    use souvlaki::{
        MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig,
    };
    use std::sync::{Arc, Mutex};
    use tauri::{AppHandle, Emitter, Manager};

    #[derive(Clone)]
    pub struct Handle(pub Arc<Mutex<Option<MediaControls>>>);

    pub fn init(app: &tauri::App) -> Handle {
        let handle = Handle(Arc::new(Mutex::new(None)));
        let _ = try_bind(app.handle(), &handle);
        handle
    }

    pub fn try_bind(app: &AppHandle, handle: &Handle) -> bool {
        {
            let Ok(guard) = handle.0.lock() else {
                return false;
            };
            if guard.is_some() {
                return true;
            }
        }
        let Some(window) = app.get_webview_window("main") else {
            eprintln!("SMTC: fönster 'main' saknas ännu.");
            return false;
        };
        let hwnd = match window.hwnd() {
            Ok(h) => h.0 as *mut std::ffi::c_void,
            Err(err) => {
                eprintln!("SMTC: HWND inte redo ({err}).");
                return false;
            }
        };
        let config = PlatformConfig {
            dbus_name: "consulting.fagerhierta.hifi-podcast",
            display_name: "HIFI podcast",
            hwnd: Some(hwnd),
        };
        match MediaControls::new(config) {
            Ok(mut c) => {
                let emit_handle = app.clone();
                let _ = c.attach(move |event| {
                    let name = match event {
                        MediaControlEvent::Toggle => "toggle",
                        MediaControlEvent::Play => "play",
                        MediaControlEvent::Pause => "pause",
                        MediaControlEvent::Next => "next",
                        MediaControlEvent::Previous => "previous",
                        _ => return,
                    };
                    // Låsskärmens mediatangenter når den redan öppnade motorn
                    // även om webview inte hinner köra. Nästa/föregående seekar inte.
                    apply_lock_screen_transport(&emit_handle, name);
                    let _ = emit_handle.emit("smtc", name);
                });
                if let Ok(mut guard) = handle.0.lock() {
                    *guard = Some(c);
                }
                eprintln!("SMTC: bundet till HWND.");
                true
            }
            Err(err) => {
                eprintln!("SMTC: kunde inte skapa kontroller ({err:?}).");
                false
            }
        }
    }

    pub fn set_playing(handle: &Handle, title: &str, artist: &str, playing: bool) {
        let Ok(mut guard) = handle.0.lock() else {
            return;
        };
        let Some(ctrl) = guard.as_mut() else {
            return;
        };
        let _ = ctrl.set_metadata(MediaMetadata {
            title: Some(title),
            artist: Some(artist),
            album: Some("HIFI podcast"),
            ..Default::default()
        });
        let _ = ctrl.set_playback(if playing {
            MediaPlayback::Playing { progress: None }
        } else {
            MediaPlayback::Paused { progress: None }
        });
    }

    pub fn set_playback_flag(handle: &Handle, playing: bool) {
        let Ok(mut guard) = handle.0.lock() else {
            return;
        };
        let Some(ctrl) = guard.as_mut() else {
            return;
        };
        let _ = ctrl.set_playback(if playing {
            MediaPlayback::Playing { progress: None }
        } else {
            MediaPlayback::Paused { progress: None }
        });
    }

    fn apply_lock_screen_transport(app: &AppHandle, name: &str) {
        let Some(action) = super::lock_screen_engine_command(name) else {
            return;
        };
        let Some(state) = app.try_state::<crate::AppState>() else {
            return;
        };
        let was_playing = state.engine.status().playing;
        let next_playing = match action {
            super::EngineTransport::Play => true,
            super::EngineTransport::Pause => false,
            super::EngineTransport::Toggle => !was_playing,
        };
        let ok = if next_playing {
            state.engine.play().is_ok()
        } else {
            state.engine.pause().is_ok()
        };
        if !ok {
            return;
        }
        let handle = state.smtc.clone();
        std::thread::spawn(move || {
            set_playback_flag(&handle, next_playing);
        });
    }
}

#[cfg(windows)]
pub use windows_smtc::*;

#[cfg(not(windows))]
#[derive(Clone)]
pub struct Handle;

#[cfg(not(windows))]
pub fn init(_app: &tauri::App) -> Handle {
    Handle
}

#[cfg(not(windows))]
pub fn try_bind(_app: &tauri::AppHandle, _handle: &Handle) -> bool {
    false
}

#[cfg(not(windows))]
pub fn set_playing(_handle: &Handle, _title: &str, _artist: &str, _playing: bool) {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineTransport {
    Play,
    Pause,
    Toggle,
}

/// Play, paus och växla gäller den redan öppnade motorn.
/// Nästa, föregående och seek gör det inte — de ska inte flytta uppspelningen härifrån.
pub fn lock_screen_engine_command(name: &str) -> Option<EngineTransport> {
    match name {
        "play" => Some(EngineTransport::Play),
        "pause" => Some(EngineTransport::Pause),
        "toggle" => Some(EngineTransport::Toggle),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_screen_transport_does_not_seek() {
        assert_eq!(
            lock_screen_engine_command("play"),
            Some(EngineTransport::Play)
        );
        assert_eq!(
            lock_screen_engine_command("pause"),
            Some(EngineTransport::Pause)
        );
        assert_eq!(
            lock_screen_engine_command("toggle"),
            Some(EngineTransport::Toggle)
        );
        assert_eq!(lock_screen_engine_command("next"), None);
        assert_eq!(lock_screen_engine_command("previous"), None);
        assert_eq!(lock_screen_engine_command("seek"), None);
    }
}
