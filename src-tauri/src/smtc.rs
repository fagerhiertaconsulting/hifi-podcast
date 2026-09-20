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
