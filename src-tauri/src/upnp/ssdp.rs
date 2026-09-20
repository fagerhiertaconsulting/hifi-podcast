//! SSDP (HTTPU) announce and M-SEARCH replies.
//!
//! Packets are built as pure functions so Windows/Onkyo quirks can be tested
//! without binding multicast 239.255.255.250:1900.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tokio::time::{self, Duration};

pub const SSDP_HOST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const SSDP_PORT: u16 = 1900;
pub const SERVER_HEADER: &str = "Windows/10 UPnP/1.0 HIFI-podcast/0.1";
pub const MAX_AGE_SECS: u32 = 1800;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MSearch {
    pub st: String,
}

pub fn notify_packet(nts: &str, nt: &str, usn: &str, location: &str) -> String {
    format!(
        "NOTIFY * HTTP/1.1\r\n\
HOST: 239.255.255.250:1900\r\n\
CACHE-CONTROL: max-age={MAX_AGE_SECS}\r\n\
LOCATION: {location}\r\n\
NT: {nt}\r\n\
NTS: {nts}\r\n\
SERVER: {SERVER_HEADER}\r\n\
USN: {usn}\r\n\
\r\n"
    )
}

pub fn search_response(st: &str, usn: &str, location: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n\
CACHE-CONTROL: max-age={MAX_AGE_SECS}\r\n\
EXT:\r\n\
LOCATION: {location}\r\n\
SERVER: {SERVER_HEADER}\r\n\
ST: {st}\r\n\
USN: {usn}\r\n\
\r\n"
    )
}

pub fn parse_msearch(buf: &[u8]) -> Option<MSearch> {
    let text = std::str::from_utf8(buf).ok()?;
    let first = text.lines().next()?.to_ascii_uppercase();
    if !first.starts_with("M-SEARCH") {
        return None;
    }
    let mut st = None;
    let mut man_ok = false;
    for line in text.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let key = name.trim().to_ascii_uppercase();
        let val = value.trim();
        if key == "ST" {
            st = Some(val.to_string());
        } else if key == "MAN" && val.trim_matches('"').eq_ignore_ascii_case("ssdp:discover") {
            man_ok = true;
        }
    }
    let st = st?;
    let _ = man_ok;
    Some(MSearch { st })
}

/// True when this MediaServer should answer `st`.
pub fn matches_search(st: &str, uuid: &str) -> bool {
    let st = st.trim();
    st.eq_ignore_ascii_case("ssdp:all")
        || st.eq_ignore_ascii_case("upnp:rootdevice")
        || st.eq_ignore_ascii_case(&format!("uuid:{uuid}"))
        || st.eq_ignore_ascii_case("urn:schemas-upnp-org:device:MediaServer:1")
        || st.eq_ignore_ascii_case("urn:schemas-upnp-org:service:ContentDirectory:1")
        || st.eq_ignore_ascii_case("urn:schemas-upnp-org:service:ConnectionManager:1")
}

pub fn advertisement_triples(uuid: &str) -> Vec<(String, String)> {
    let udn = format!("uuid:{uuid}");
    vec![
        ("upnp:rootdevice".into(), format!("{udn}::upnp:rootdevice")),
        (udn.clone(), udn.clone()),
        (
            "urn:schemas-upnp-org:device:MediaServer:1".into(),
            format!("{udn}::urn:schemas-upnp-org:device:MediaServer:1"),
        ),
        (
            "urn:schemas-upnp-org:service:ContentDirectory:1".into(),
            format!("{udn}::urn:schemas-upnp-org:service:ContentDirectory:1"),
        ),
        (
            "urn:schemas-upnp-org:service:ConnectionManager:1".into(),
            format!("{udn}::urn:schemas-upnp-org:service:ConnectionManager:1"),
        ),
    ]
}

pub fn responses_for_search(st: &str, uuid: &str) -> Vec<(String, String)> {
    let all = advertisement_triples(uuid);
    if st.eq_ignore_ascii_case("ssdp:all") {
        return all;
    }
    all.into_iter()
        .filter(|(nt, usn)| nt.eq_ignore_ascii_case(st) || usn.eq_ignore_ascii_case(st))
        .collect()
}

async fn send_notifies(socket: &UdpSocket, location: &str, uuid: &str, nts: &str) {
    let dest = SocketAddr::V4(SocketAddrV4::new(SSDP_HOST, SSDP_PORT));
    for (nt, usn) in advertisement_triples(uuid) {
        let pkt = notify_packet(nts, &nt, &usn, location);
        let _ = socket.send_to(pkt.as_bytes(), dest).await;
    }
}

pub async fn run_ssdp(
    location: String,
    uuid: String,
    lan_ip: Ipv4Addr,
    mut shutdown: watch::Receiver<bool>,
) {
    let send_sock = match UdpSocket::bind(SocketAddr::from((lan_ip, 0))).await {
        Ok(s) => s,
        Err(err) => {
            eprintln!("UPnP SSDP: kunde inte binda sändarsocket: {err}");
            return;
        }
    };
    let _ = send_sock.set_broadcast(true);
    let _ = send_sock.join_multicast_v4(SSDP_HOST, lan_ip);

    let recv_sock = bind_ssdp_listener(lan_ip).await;
    for _ in 0..3 {
        send_notifies(&send_sock, &location, &uuid, "ssdp:alive").await;
        time::sleep(Duration::from_millis(400)).await;
    }

    let mut alive = time::interval(Duration::from_secs((MAX_AGE_SECS / 3) as u64));
    let mut buf = vec![0u8; 2048];
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    send_notifies(&send_sock, &location, &uuid, "ssdp:byebye").await;
                    break;
                }
            }
            _ = alive.tick() => {
                send_notifies(&send_sock, &location, &uuid, "ssdp:alive").await;
            }
            result = async {
                match recv_sock.as_ref() {
                    Some(sock) => sock.recv_from(&mut buf).await.ok(),
                    None => {
                        std::future::pending::<Option<(usize, SocketAddr)>>().await
                    }
                }
            } => {
                if let Some((n, from)) = result {
                    if let Some(search) = parse_msearch(&buf[..n]) {
                        if matches_search(&search.st, &uuid) {
                            for (st, usn) in responses_for_search(&search.st, &uuid) {
                                let pkt = search_response(&st, &usn, &location);
                                let _ = send_sock.send_to(pkt.as_bytes(), from).await;
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn bind_ssdp_listener(lan_ip: Ipv4Addr) -> Option<UdpSocket> {
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, SSDP_PORT));
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )
    .ok()?;
    let _ = socket.set_reuse_address(true);
    #[cfg(unix)]
    let _ = socket.set_reuse_port(true);
    if socket.bind(&addr.into()).is_err() {
        eprintln!(
            "UPnP SSDP: kunde inte binda UDP {SSDP_PORT}. NOTIFY skickas ändå; M-SEARCH kan missas om en annan server äger porten."
        );
        return None;
    }
    let _ = socket.set_nonblocking(true);
    let socket = match UdpSocket::from_std(socket.into()) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("UPnP SSDP: tokio-socket misslyckades: {err}");
            return None;
        }
    };
    if let Err(err) = socket.join_multicast_v4(SSDP_HOST, lan_ip) {
        eprintln!("UPnP SSDP: join multicast misslyckades: {err}");
    }
    Some(socket)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notify_contains_location_and_nts() {
        let p = notify_packet(
            "ssdp:alive",
            "upnp:rootdevice",
            "uuid:abc::upnp:rootdevice",
            "http://192.168.1.9:8200/upnp/device.xml",
        );
        assert!(p.starts_with("NOTIFY * HTTP/1.1"));
        assert!(p.contains("LOCATION: http://192.168.1.9:8200/upnp/device.xml"));
        assert!(p.contains("NTS: ssdp:alive"));
        assert!(p.contains("\r\n\r\n"));
    }

    #[test]
    fn parses_msearch_ssdp_all() {
        let raw = b"M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 3\r\nST: ssdp:all\r\n\r\n";
        let m = parse_msearch(raw).unwrap();
        assert_eq!(m.st, "ssdp:all");
        assert!(matches_search(&m.st, "abc"));
    }

    #[test]
    fn search_media_server_returns_one_triple() {
        let uuid = "11111111-2222-3333-4444-555555555555";
        let r = responses_for_search("urn:schemas-upnp-org:device:MediaServer:1", uuid);
        assert_eq!(r.len(), 1);
        assert!(r[0].1.contains(uuid));
        assert!(r[0].1.contains("MediaServer:1"));
    }

    #[test]
    fn ignores_non_search() {
        assert!(parse_msearch(b"NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\n\r\n").is_none());
    }
}
