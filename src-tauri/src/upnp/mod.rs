mod cds;
mod http;
mod ssdp;

use crate::error::{AppError, AppResult};
use crate::models::{UpnpSettings, UpnpStatus};
use rusqlite::Connection;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::watch;

pub struct Handle {
    inner: tokio::sync::Mutex<Inner>,
}

struct Inner {
    live: Option<Live>,
    last_error: Option<String>,
}

struct Live {
    shutdown: watch::Sender<bool>,
    status: UpnpStatus,
}

impl Handle {
    pub fn new() -> Self {
        Self {
            inner: tokio::sync::Mutex::new(Inner {
                live: None,
                last_error: None,
            }),
        }
    }

    pub async fn status(&self, settings: UpnpSettings, uuid: String) -> UpnpStatus {
        let lan = default_lan_ipv4();
        let guard = self.inner.lock().await;
        if let Some(live) = &guard.live {
            return live.status.clone();
        }
        UpnpStatus {
            running: false,
            bind: settings.bind,
            port: settings.port,
            location: None,
            lan_ip: lan.map(|ip| ip.to_string()),
            uuid,
            error: guard.last_error.clone(),
        }
    }

    pub async fn stop(&self) -> AppResult<bool> {
        let mut guard = self.inner.lock().await;
        if let Some(live) = guard.live.take() {
            let _ = live.shutdown.send(true);
            return Ok(true);
        }
        Ok(false)
    }

    pub async fn start(
        &self,
        db_path: PathBuf,
        media_root: PathBuf,
        settings: UpnpSettings,
        uuid: String,
    ) -> AppResult<UpnpStatus> {
        let mut guard = self.inner.lock().await;
        if guard.live.is_some() {
            return Ok(guard.live.as_ref().unwrap().status.clone());
        }

        let lan = resolve_lan_ip(&settings.bind)?;
        let listen_ip: IpAddr = if settings.bind.trim().is_empty() {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            settings
                .bind
                .parse()
                .map_err(|_| AppError::msg("Ogiltig bindningsadress."))?
        };
        let addr = SocketAddr::new(listen_ip, settings.port);
        let public_base = format!("http://{lan}:{}", settings.port);
        let location = format!("{public_base}/upnp/device.xml");

        let upnp_conn = Connection::open(&db_path)
            .map_err(|e| AppError::msg(format!("UPnP kunde inte öppna databasen: {e}")))?;
        let _ = upnp_conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;");
        let state = http::HttpState {
            db: Arc::new(Mutex::new(upnp_conn)),
            media_root,
            uuid: uuid.clone(),
            public_base: public_base.clone(),
        };

        let listener = TcpListener::bind(addr).await.map_err(|err| {
            AppError::msg(format!(
                "Kunde inte binda {addr}: {err}. Prova en annan port (standard 8200)."
            ))
        })?;
        let (tx, rx_http) = watch::channel(false);
        let rx_ssdp = tx.subscribe();
        let http_state = state.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(err) = http::serve(listener, http_state, rx_http).await {
                eprintln!("UPnP HTTP-server avslutades: {err}");
            }
        });
        let loc = location.clone();
        let uuid_ssdp = uuid.clone();
        tauri::async_runtime::spawn(async move {
            ssdp::run_ssdp(loc, uuid_ssdp, lan, rx_ssdp).await;
        });

        let status = UpnpStatus {
            running: true,
            bind: if settings.bind.trim().is_empty() {
                "0.0.0.0".into()
            } else {
                settings.bind.clone()
            },
            port: settings.port,
            location: Some(location),
            lan_ip: Some(lan.to_string()),
            uuid,
            error: None,
        };
        guard.last_error = None;
        guard.live = Some(Live {
            shutdown: tx,
            status: status.clone(),
        });
        Ok(status)
    }
}

pub fn default_lan_ipv4() -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("1.1.1.1:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() && !ip.is_link_local() => {
            Some(ip)
        }
        _ => None,
    }
}

fn resolve_lan_ip(bind: &str) -> AppResult<Ipv4Addr> {
    let trimmed = bind.trim();
    if trimmed.is_empty() || trimmed == "0.0.0.0" {
        return default_lan_ipv4().ok_or_else(|| {
            AppError::msg(
                "Kunde inte avgöra LAN-adress. Ange bindningsadress manuellt (PC:ns IPv4).",
            )
        });
    }
    match trimmed.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => Ok(ip),
        Ok(IpAddr::V6(_)) => Err(AppError::msg("Endast IPv4 stöds i den här versionen.")),
        Err(_) => Err(AppError::msg("Ogiltig bindningsadress.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_ipv6_bind() {
        let err = resolve_lan_ip("::1").unwrap_err();
        assert!(err.to_string().contains("IPv4"));
    }
}
