#[cfg(windows)]
mod windows_smtc {
    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
    use std::sync::Mutex;
    use tauri::{AppHandle, Emitter, Manager};

    pub struct Handle(pub Mutex<Option<MediaControls>>);

    pub fn init(app: &tauri::App) -> Handle {
        let mut controls = None;
        if let Some(window) = app.get_webview_window("main") {
            if let Ok(hwnd) = window.hwnd() {
                let hwnd = hwnd.0 as *mut std::ffi::c_void;
                let config = PlatformConfig {
                    dbus_name: "consulting.fagerhierta.hifi-podcast",
                    display_name: "HIFI podcast",
                    hwnd: Some(hwnd),
                };
                if let Ok(mut c) = MediaControls::new(config) {
                    let handle = app.handle().clone();
                    let _ = c.attach(move |event| {
                        let name = match event {
                            MediaControlEvent::Toggle => "toggle",
                            MediaControlEvent::Play => "play",
                            MediaControlEvent::Pause => "pause",
                            MediaControlEvent::Next => "next",
                            MediaControlEvent::Previous => "previous",
                            _ => return,
                        };
                        let _ = handle.emit("smtc", name);
                    });
                    controls = Some(c);
                }
            }
        }
        Handle(Mutex::new(controls))
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
pub struct Handle;

#[cfg(not(windows))]
pub fn init(_app: &tauri::App) -> Handle {
    Handle
}

#[cfg(not(windows))]
pub fn set_playing(_handle: &Handle, _title: &str, _artist: &str, _playing: bool) {}
