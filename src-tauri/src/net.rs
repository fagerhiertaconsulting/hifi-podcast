//! HTTP-klient med timeouts, storleksgräns, redirect-tak och SSRF-skydd.
use crate::error::{AppError, AppResult};
use futures_util::StreamExt;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use url::Url;

pub const USER_AGENT: &str =
    "HIFI-podcast/0.1 (+https://github.com/fagerhiertaconsulting/hifi-podcast)";
pub const MAX_REDIRECTS: usize = 5;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_FEED_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_MEDIA_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_MODEL_BYTES: u64 = 900 * 1024 * 1024;

#[derive(Debug)]
struct RedirectDenied(String);

impl std::fmt::Display for RedirectDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RedirectDenied {}

pub fn http_client() -> AppResult<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error(RedirectDenied(
                    "För många HTTP-ompekningar (max 5).".into(),
                ));
            }
            match validate_http_url(attempt.url().as_str()) {
                Ok(()) => attempt.follow(),
                Err(err) => attempt.error(RedirectDenied(err.to_string())),
            }
        }))
        .build()?)
}

pub fn validate_http_url(raw: &str) -> AppResult<()> {
    let url = Url::parse(raw).map_err(|_| {
        AppError::msg("Ogiltig adress. Använd en http- eller https-URL.")
    })?;
    let scheme = url.scheme();
    if scheme != "https" && scheme != "http" {
        return Err(AppError::msg(
            "Endast http och https tillåts (https rekommenderas).",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppError::msg("Adressen saknar värdnamn."))?;
    if host_name_blocked(host) {
        return Err(AppError::msg(format!(
            "Adressen pekar på en blockerad värd ({host})."
        )));
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip_is_blocked(ip) {
            return Err(AppError::msg(format!(
                "Adressen pekar på ett privat eller reserverat IP ({ip})."
            )));
        }
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(if scheme == "https" {
        443
    } else {
        80
    });
    let lookup = format!("{host}:{port}");
    let addrs = lookup
        .to_socket_addrs()
        .map_err(|_| AppError::msg(format!("Kunde inte slå upp värdnamnet {host}.")))?;
    let mut saw_any = false;
    for addr in addrs {
        saw_any = true;
        if ip_is_blocked(addr.ip()) {
            return Err(AppError::msg(format!(
                "Värdnamnet {host} pekar på ett privat eller reserverat IP ({}).",
                addr.ip()
            )));
        }
    }
    if !saw_any {
        return Err(AppError::msg(format!(
            "Värdnamnet {host} gav inga IP-adresser."
        )));
    }
    Ok(())
}

fn host_name_blocked(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    matches!(
        h.as_str(),
        "localhost"
            | "localhost.localdomain"
            | "ip6-localhost"
            | "ip6-loopback"
            | "metadata"
            | "metadata.google.internal"
            | "metadata.goog"
            | "instance-data"
            | "instance-data.ec2.internal"
    ) || h.ends_with(".localhost")
        || h.ends_with(".local")
        || h.ends_with(".internal")
        || h.ends_with(".lan")
}

pub fn ip_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_blocked(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ipv4_is_blocked(v4);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || (v6.segments()[0] & 0xffc0) == 0x2001 && v6.segments()[1] == 0x0db8
        }
    }
}

fn ipv4_is_blocked(v4: Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || v4.is_multicast()
        || v4.is_broadcast()
        || v4.is_documentation()
        || o[0] == 0
        || o[0] == 100 && (o[1] & 0xc0) == 64
        || o[0] == 192 && o[1] == 0 && o[2] == 0
        || o[0] == 198 && (o[1] == 18 || o[1] == 19)
}

pub async fn get_limited(
    client: &reqwest::Client,
    url: &str,
    max_bytes: u64,
) -> AppResult<Vec<u8>> {
    validate_http_url(url)?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(AppError::msg(format!(
            "Hämtning misslyckades (HTTP {}).",
            response.status()
        )));
    }
    reject_oversize(response.content_length(), max_bytes)?;
    let mut out = Vec::new();
    let mut bytes: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > max_bytes {
            return Err(AppError::msg(format!(
                "Svaret är större än tillåtet ({max_bytes} byte)."
            )));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

pub async fn download_to_path(
    client: &reqwest::Client,
    url: &str,
    dest: &std::path::Path,
    max_bytes: u64,
    mut hasher: Option<&mut sha2::Sha256>,
) -> AppResult<u64> {
    validate_http_url(url)?;
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(AppError::msg(format!(
            "Nedladdning misslyckades (HTTP {}).",
            response.status()
        )));
    }
    reject_oversize(response.content_length(), max_bytes)?;
    let tmp = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut bytes: u64 = 0;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > max_bytes {
            drop(file);
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(AppError::msg(format!(
                "Filen är större än tillåtet ({max_bytes} byte)."
            )));
        }
        if let Some(h) = hasher.as_mut() {
            use sha2::Digest;
            h.update(&chunk);
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(&tmp, dest).await?;
    Ok(bytes)
}

fn reject_oversize(content_length: Option<u64>, max_bytes: u64) -> AppResult<()> {
    if let Some(len) = content_length {
        if len > max_bytes {
            return Err(AppError::msg(format!(
                "Content-Length {len} överstiger gränsen {max_bytes} byte."
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_loopback_and_metadata() {
        assert!(validate_http_url("http://127.0.0.1/feed").is_err());
        assert!(validate_http_url("http://localhost/feed").is_err());
        assert!(validate_http_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(validate_http_url("http://192.168.1.10/rss").is_err());
        assert!(validate_http_url("http://10.0.0.5/rss").is_err());
        assert!(validate_http_url("http://172.16.4.1/rss").is_err());
        assert!(validate_http_url("file:///etc/passwd").is_err());
        assert!(validate_http_url("ftp://example.com/x").is_err());
    }

    #[test]
    fn allows_public_https_literal() {
        assert!(validate_http_url("https://1.1.1.1/").is_ok());
        assert!(validate_http_url("https://8.8.8.8/feed.xml").is_ok());
    }

    #[test]
    fn ip_helpers() {
        assert!(ip_is_blocked(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(ip_is_blocked(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(!ip_is_blocked(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
    }
}
