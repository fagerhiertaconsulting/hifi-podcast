//! Online albummetadata och omslag.
//!
//! Tillåtna värdar, exakt match och inga underdomäner:
//! - `musicbrainz.org`
//! - `coverartarchive.org`
//!
//! Ingen DNS-uppslagning och ingen HTTP-förfrågan görs förrän
//! internet-samtycket är exakt `1`. Omdirigeringar bort från de här värdarna
//! avvisas, liksom privata och länklokala adresser. Omslag begränsas till
//! 2 MiB och sparas bara som JPEG, PNG, GIF eller WebP under
//! `media/music/covers/<album-id>.<ext>`.

use crate::db;
use crate::error::{AppError, AppResult};
use crate::models::{MusicAlbum, OnlineMetadataOutcome};
use crate::net::{ip_is_blocked, CONNECT_TIMEOUT, READ_TIMEOUT};
use futures_util::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use rusqlite::Connection;
use serde::Deserialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use url::Url;

pub(crate) const ALLOWED_METADATA_HOSTS: &[&str] = &["musicbrainz.org", "coverartarchive.org"];
pub(crate) const MAX_COVER_BYTES: u64 = 2 * 1024 * 1024;
pub(crate) const MAX_METADATA_BYTES: u64 = 1024 * 1024;
const MAX_METADATA_REDIRECTS: usize = 3;
const MAX_QUERY_CHARS: usize = 200;
const MIN_IMAGE_BYTES: usize = 32;
const METADATA_USER_AGENT: &str =
    "HIFI-podcast/0.1 ( https://github.com/fagerhiertaconsulting/hifi-podcast )";

#[derive(Debug)]
pub(crate) struct Fetched {
    pub mbid: String,
    pub release_date: Option<String>,
    pub cover: Option<Vec<u8>>,
    pub warnings: Vec<String>,
}

pub(crate) struct HttpBody {
    status: u16,
    bytes: Vec<u8>,
}

pub(crate) trait MetadataHttp {
    fn get(
        &self,
        url: &str,
        max_bytes: u64,
    ) -> impl std::future::Future<Output = AppResult<HttpBody>> + Send;
}

pub(crate) struct LiveHttp {
    client: reqwest::Client,
}

impl LiveHttp {
    pub(crate) fn connect() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .user_agent(METADATA_USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .no_proxy()
            .dns_resolver(Arc::new(AllowlistResolver))
            .build()
            .map_err(|_| {
                AppError::msg(
                    "Kunde inte förbereda hämtning mot MusicBrainz eller Cover Art Archive.",
                )
            })?;
        Ok(Self { client })
    }
}

impl MetadataHttp for LiveHttp {
    async fn get(&self, url: &str, max_bytes: u64) -> AppResult<HttpBody> {
        let mut current = inspect_metadata_url(url)?.to_string();
        for hop in 0..=MAX_METADATA_REDIRECTS {
            validate_metadata_url(&current)?;
            pace_metadata_request().await;
            let response = self.client.get(&current).send().await.map_err(http_failed)?;
            let status = response.status().as_u16();
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            match classify_hop(&current, status, location.as_deref(), hop)? {
                Hop::Follow(next) => {
                    drop(response);
                    current = next;
                }
                Hop::Response => {
                    if status == 404 || status == 400 {
                        return Ok(HttpBody {
                            status,
                            bytes: Vec::new(),
                        });
                    }
                    if !(200..300).contains(&status) {
                        return Err(AppError::msg(format!(
                            "Hämtningen misslyckades (HTTP {status})."
                        )));
                    }
                    let bytes = read_capped(response, max_bytes).await?;
                    return Ok(HttpBody { status, bytes });
                }
            }
        }
        Err(AppError::msg("För många omdirigeringar."))
    }
}

/// Consent is checked before the factory runs. The factory is what opens a client.
pub(crate) async fn lookup_with_client<C, F>(
    consent: bool,
    artist: &str,
    title: &str,
    client_factory: F,
) -> AppResult<Fetched>
where
    F: FnOnce() -> AppResult<C>,
    C: MetadataHttp,
{
    if !consent {
        return Err(AppError::msg(
            "Internet-samtycke saknas. Ingen förfrågan skickades.",
        ));
    }
    let artist = normalize_query(artist)?;
    let title = normalize_query(title)?;
    let client = client_factory()?;
    lookup_consented(&client, &artist, &title).await
}

async fn lookup_consented<C: MetadataHttp>(
    client: &C,
    artist: &str,
    title: &str,
) -> AppResult<Fetched> {
    let search_url = musicbrainz_search_url(artist, title)?;
    let search = client.get(&search_url, MAX_METADATA_BYTES).await?;
    if search.status != 200 {
        return Err(AppError::msg(format!(
            "MusicBrainz svarade inte med metadata (HTTP {}).",
            search.status
        )));
    }
    let (mbid, release_date, title_differs) = parse_release_search(&search.bytes, artist, title)?;
    let mut warnings = Vec::new();
    if title_differs {
        push_warning(
            &mut warnings,
            "MusicBrainz-träffen har en annan titel. Lokala taggar ändras inte.".into(),
        );
    }
    let cover_url = cover_art_url(&mbid)?;
    let cover = match client.get(&cover_url, MAX_COVER_BYTES).await {
        Ok(body) if body.status == 200 => match accept_cover_bytes(&body.bytes) {
            Ok(_) => Some(body.bytes),
            Err(err) => {
                push_warning(&mut warnings, format!("Omslaget sparades inte: {err}"));
                None
            }
        },
        Ok(body) if body.status == 404 || body.status == 400 => {
            push_warning(
                &mut warnings,
                "Inget omslag fanns på Cover Art Archive.".into(),
            );
            None
        }
        Ok(body) => {
            push_warning(
                &mut warnings,
                format!("Omslaget hämtades inte (HTTP {}).", body.status),
            );
            None
        }
        Err(err) => {
            push_warning(&mut warnings, format!("Omslaget hämtades inte: {err}"));
            None
        }
    };
    Ok(Fetched {
        mbid,
        release_date,
        cover,
        warnings,
    })
}

pub(crate) fn persist(
    conn: &Connection,
    media_root: &Path,
    album_id: i64,
    fetched: Fetched,
) -> AppResult<OnlineMetadataOutcome> {
    let cover_ext = if let Some(bytes) = &fetched.cover {
        Some(write_album_cover(media_root, album_id, bytes)?)
    } else {
        None
    };
    let cover_saved = cover_ext.is_some();
    db::set_music_album_online(
        conn,
        album_id,
        &fetched.mbid,
        fetched.release_date.as_deref(),
        cover_ext.as_deref(),
    )?;
    Ok(OnlineMetadataOutcome {
        album_id,
        mbid: fetched.mbid,
        release_date: fetched.release_date,
        cover_saved,
        warnings: fetched.warnings,
    })
}

pub(crate) fn attach_cover_path(media_root: &Path, album: &mut MusicAlbum) {
    album.cover_path = safe_cover_path(media_root, album.id, album.cover_ext.as_deref());
}

pub(crate) fn metadata_host_allowed(host: &str) -> bool {
    let normalized = normalized_host(host);
    ALLOWED_METADATA_HOSTS
        .iter()
        .any(|allowed| *allowed == normalized)
}

pub(crate) fn inspect_metadata_url(raw: &str) -> AppResult<Url> {
    let url = Url::parse(raw).map_err(|_| AppError::msg("Ogiltig metadata-adress."))?;
    if url.scheme() != "https" {
        return Err(AppError::msg(
            "Metadata får bara hämtas med https från musicbrainz.org och coverartarchive.org.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AppError::msg(
            "Metadata-adressen får inte innehålla användarnamn eller lösenord.",
        ));
    }
    if url.port().is_some_and(|port| port != 443) {
        return Err(AppError::msg(
            "Metadata får bara hämtas på port 443.",
        ));
    }
    let host = url
        .host_str()
        .ok_or_else(|| AppError::msg("Metadata-adressen saknar värdnamn."))?;
    if host.parse::<IpAddr>().is_ok() || host.starts_with('[') {
        return Err(AppError::msg(
            "IP-adresser är inte tillåtna för metadata.",
        ));
    }
    if !metadata_host_allowed(host) {
        return Err(AppError::msg(
            "Värden är inte tillåten. Bara musicbrainz.org och coverartarchive.org får användas.",
        ));
    }
    Ok(url)
}

pub(crate) fn validate_metadata_url(raw: &str) -> AppResult<()> {
    let url = inspect_metadata_url(raw)?;
    let host = url
        .host_str()
        .ok_or_else(|| AppError::msg("Metadata-adressen saknar värdnamn."))?;
    let ips = blocking_lookup(host)?;
    screen_metadata_resolution(host, &ips)
}

pub(crate) fn screen_metadata_resolution(host: &str, ips: &[IpAddr]) -> AppResult<()> {
    if !metadata_host_allowed(host) {
        return Err(AppError::msg(
            "Värden är inte tillåten. Bara musicbrainz.org och coverartarchive.org får användas.",
        ));
    }
    if ips.is_empty() {
        return Err(AppError::msg("Värdnamnet gav inga IP-adresser."));
    }
    if ips.iter().copied().any(ip_is_blocked) {
        return Err(AppError::msg(
            "Värden pekar på en privat, lokal eller länklokal adress.",
        ));
    }
    Ok(())
}

pub(crate) fn next_metadata_url(current: &str, location: &str) -> AppResult<String> {
    let location = location.trim();
    if location.is_empty() || location.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
        return Err(AppError::msg("Omdirigeringen har en ogiltig adress."));
    }
    let base = inspect_metadata_url(current)?;
    let next = base
        .join(location)
        .map_err(|_| AppError::msg("Omdirigeringen har en ogiltig adress."))?;
    inspect_metadata_url(next.as_str())?;
    Ok(next.to_string())
}

enum Hop {
    Follow(String),
    Response,
}

fn classify_hop(
    current: &str,
    status: u16,
    location: Option<&str>,
    hop: usize,
) -> AppResult<Hop> {
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return Ok(Hop::Response);
    }
    if hop >= MAX_METADATA_REDIRECTS {
        return Err(AppError::msg("För många omdirigeringar."));
    }
    let location = location.ok_or_else(|| AppError::msg("Omdirigeringen saknar adress."))?;
    Ok(Hop::Follow(next_metadata_url(current, location)?))
}

fn normalized_host(host: &str) -> String {
    host.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn blocking_lookup(host: &str) -> AppResult<Vec<IpAddr>> {
    let host = normalized_host(host);
    if !metadata_host_allowed(&host) {
        return Err(AppError::msg(
            "Värden är inte tillåten. Bara musicbrainz.org och coverartarchive.org får användas.",
        ));
    }
    let lookup = format!("{host}:443");
    let addrs = lookup.to_socket_addrs_metadata()?;
    if addrs.is_empty() {
        return Err(AppError::msg("Värdnamnet gav inga IP-adresser."));
    }
    Ok(addrs)
}

trait MetadataLookup {
    fn to_socket_addrs_metadata(&self) -> AppResult<Vec<IpAddr>>;
}

impl MetadataLookup for String {
    fn to_socket_addrs_metadata(&self) -> AppResult<Vec<IpAddr>> {
        let found = self
            .to_socket_addrs()
            .map_err(|_| AppError::msg("Kunde inte slå upp värdnamnet."))?;
        let mut ips = Vec::new();
        for addr in found {
            ips.push(addr.ip());
        }
        Ok(ips)
    }
}

use std::net::ToSocketAddrs;

struct AllowlistResolver;

impl Resolve for AllowlistResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let resolved = tokio::task::spawn_blocking(move || resolve_allowlisted(&host))
                .await
                .map_err(|err| -> Box<dyn std::error::Error + Send + Sync> {
                    err.to_string().into()
                })?;
            let addrs = resolved.map_err(|err| -> Box<dyn std::error::Error + Send + Sync> {
                err.to_string().into()
            })?;
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

fn resolve_allowlisted(host: &str) -> AppResult<Vec<SocketAddr>> {
    let ips = blocking_lookup(host)?;
    screen_metadata_resolution(host, &ips)?;
    Ok(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect())
}

fn pace_lock() -> &'static tokio::sync::Mutex<Option<Instant>> {
    static LOCK: OnceLock<tokio::sync::Mutex<Option<Instant>>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(None))
}

async fn pace_metadata_request() {
    let mut guard = pace_lock().lock().await;
    let now = Instant::now();
    if let Some(earliest) = *guard {
        if earliest > now {
            let wait = earliest - now;
            drop(guard);
            tokio::time::sleep(wait).await;
            *pace_lock().lock().await = Some(Instant::now() + Duration::from_millis(1100));
            return;
        }
    }
    *guard = Some(now + Duration::from_millis(1100));
}

fn http_failed(err: reqwest::Error) -> AppError {
    if err.is_timeout() {
        return AppError::msg(
            "Tidsgränsen gick ut mot MusicBrainz eller Cover Art Archive.",
        );
    }
    if err.is_connect() {
        return AppError::msg(
            "Kunde inte ansluta till MusicBrainz eller Cover Art Archive.",
        );
    }
    AppError::msg("Hämtningen från MusicBrainz eller Cover Art Archive misslyckades.")
}

async fn read_capped(response: reqwest::Response, max_bytes: u64) -> AppResult<Vec<u8>> {
    reject_declared_length(response.content_length(), max_bytes)?;
    let mut buf = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(http_failed)?;
        append_capped(&mut buf, &chunk, max_bytes)?;
    }
    Ok(buf)
}

fn reject_declared_length(len: Option<u64>, max_bytes: u64) -> AppResult<()> {
    if let Some(len) = len {
        if len > max_bytes {
            return Err(AppError::msg(format!(
                "Svaret är större än tillåtet ({max_bytes} byte)."
            )));
        }
    }
    Ok(())
}

fn append_capped(buf: &mut Vec<u8>, chunk: &[u8], max_bytes: u64) -> AppResult<()> {
    let next = (buf.len() as u64).saturating_add(chunk.len() as u64);
    if next > max_bytes {
        return Err(AppError::msg(format!(
            "Svaret är större än tillåtet ({max_bytes} byte)."
        )));
    }
    buf.extend_from_slice(chunk);
    Ok(())
}

fn normalize_query(raw: &str) -> AppResult<String> {
    let cleaned: String = raw.chars().filter(|ch| !ch.is_control()).collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return Err(AppError::msg(
            "Artist och albumtitel behövs för att söka.",
        ));
    }
    if cleaned.chars().count() > MAX_QUERY_CHARS {
        return Err(AppError::msg("Artist eller albumtitel är för lång."));
    }
    Ok(cleaned.to_string())
}

fn lucene_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '+' | '-'
                | '&'
                | '|'
                | '!'
                | '('
                | ')'
                | '{'
                | '}'
                | '['
                | ']'
                | '^'
                | '"'
                | '~'
                | '*'
                | '?'
                | ':'
                | '\\'
                | '/'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn musicbrainz_search_url(artist: &str, title: &str) -> AppResult<String> {
    let mut url = Url::parse("https://musicbrainz.org/ws/2/release/")
        .map_err(|_| AppError::msg("Ogiltig metadata-adress."))?;
    let query = format!(
        "release:\"{}\" AND artist:\"{}\"",
        lucene_escape(title),
        lucene_escape(artist)
    );
    url.query_pairs_mut()
        .append_pair("fmt", "json")
        .append_pair("limit", "5")
        .append_pair("query", &query);
    let text = url.to_string();
    if text.len() > 2000 {
        return Err(AppError::msg("Sökningen är för lång."));
    }
    inspect_metadata_url(&text)?;
    Ok(text)
}

fn cover_art_url(mbid: &str) -> AppResult<String> {
    let mbid = canonical_mbid(mbid)?;
    let url = format!("https://coverartarchive.org/release/{mbid}/front-250");
    inspect_metadata_url(&url)?;
    Ok(url)
}

fn canonical_mbid(raw: &str) -> AppResult<String> {
    let id = raw.trim().to_ascii_lowercase();
    let bytes = id.as_bytes();
    let ok = bytes.len() == 36
        && bytes.iter().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        });
    if !ok {
        return Err(AppError::msg("MusicBrainz returnerade ett ogiltigt id."));
    }
    Ok(id)
}

fn parse_release_search(
    bytes: &[u8],
    artist: &str,
    title: &str,
) -> AppResult<(String, Option<String>, bool)> {
    let parsed: MbSearch = serde_json::from_slice(bytes)
        .map_err(|_| AppError::msg("MusicBrainz-svaret kunde inte läsas som metadata."))?;
    let mut ranked: Vec<(i32, usize)> = parsed
        .releases
        .iter()
        .take(5)
        .enumerate()
        .map(|(index, release)| (score_release(release, artist, title), index))
        .collect();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(&right.1)));
    for (_, index) in ranked {
        let release = &parsed.releases[index];
        if let Ok(mbid) = canonical_mbid(&release.id) {
            let date = sanitize_release_date(release.date.as_deref());
            let title_differs = !release.title.trim().eq_ignore_ascii_case(title);
            return Ok((mbid, date, title_differs));
        }
    }
    Err(AppError::msg(
        "Ingen träff på MusicBrainz för det här albumet.",
    ))
}

fn score_release(release: &MbRelease, artist: &str, title: &str) -> i32 {
    let mut score = 0;
    if release
        .status
        .as_deref()
        .unwrap_or("")
        .eq_ignore_ascii_case("Official")
    {
        score += 5;
    }
    if release.title.trim().eq_ignore_ascii_case(title) {
        score += 4;
    }
    if artist_credit_name(release)
        .trim()
        .eq_ignore_ascii_case(artist)
    {
        score += 3;
    }
    score
}

fn artist_credit_name(release: &MbRelease) -> String {
    let mut out = String::new();
    for credit in &release.artist_credit {
        let name = credit.name.clone().or_else(|| {
            credit
                .artist
                .as_ref()
                .and_then(|artist| artist.name.clone())
        });
        if let Some(name) = name {
            out.push_str(&name);
        }
        if let Some(join) = &credit.joinphrase {
            out.push_str(join);
        }
    }
    out
}

fn sanitize_release_date(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if release_date_ok(raw) {
        Some(raw.to_string())
    } else {
        None
    }
}

fn release_date_ok(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    match bytes.len() {
        4 => bytes.iter().all(|byte| byte.is_ascii_digit()),
        7 => bytes[4] == b'-' && digits(bytes, 0, 4) && digits(bytes, 5, 7),
        10 => {
            bytes[4] == b'-'
                && bytes[7] == b'-'
                && digits(bytes, 0, 4)
                && digits(bytes, 5, 7)
                && digits(bytes, 8, 10)
        }
        _ => false,
    }
}

fn digits(bytes: &[u8], start: usize, end: usize) -> bool {
    bytes
        .get(start..end)
        .is_some_and(|slice| slice.iter().all(|byte| byte.is_ascii_digit()))
}

fn push_warning(warnings: &mut Vec<String>, text: String) {
    if warnings.len() >= 8 {
        return;
    }
    warnings.push(text.chars().take(240).collect());
}

fn accept_cover_bytes(bytes: &[u8]) -> AppResult<&'static str> {
    if bytes.len() < MIN_IMAGE_BYTES || bytes.len() as u64 > MAX_COVER_BYTES {
        return Err(AppError::msg(format!(
            "Omslaget är större än tillåtet ({MAX_COVER_BYTES} byte) eller tomt."
        )));
    }
    image_extension(bytes).ok_or_else(|| {
        AppError::msg("Svaret är inte en bild (JPEG, PNG, GIF eller WebP).")
    })
}

fn image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("png")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("gif")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

fn write_album_cover(media_root: &Path, album_id: i64, bytes: &[u8]) -> AppResult<String> {
    let ext = accept_cover_bytes(bytes)?;
    if album_id <= 0 {
        return Err(AppError::msg("Albumet finns inte."));
    }
    let dir = ensure_cover_dir(media_root)?;
    let name = format!("{album_id}.{ext}");
    let dest = dir.join(&name);
    if dest.file_name().and_then(|value| value.to_str()) != Some(name.as_str()) {
        return Err(AppError::msg("Ogiltigt omslagsnamn."));
    }
    refuse_symlink_file(&dest)?;
    for other in ["jpg", "png", "gif", "webp"] {
        if other == ext {
            continue;
        }
        refuse_symlink_file(&dir.join(format!("{album_id}.{other}")))?;
    }
    let part = dir.join(format!("{name}.part"));
    match std::fs::symlink_metadata(&part) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
            std::fs::remove_file(&part)?;
        }
        Ok(_) => return Err(AppError::msg("Omslagsfilen får inte vara en symlänk.")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    let mut guard = RegularFileGuard {
        path: &part,
        armed: true,
    };
    let written = std::fs::symlink_metadata(&part)?;
    if !written.is_file()
        || written.file_type().is_symlink()
        || written.len() != bytes.len() as u64
        || written.len() > MAX_COVER_BYTES
    {
        return Err(AppError::msg("Omslaget kunde inte sparas."));
    }
    match std::fs::rename(&part, &dest) {
        Ok(()) => {}
        Err(_) => {
            match std::fs::symlink_metadata(&dest) {
                Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => {
                    std::fs::remove_file(&dest)?;
                    std::fs::rename(&part, &dest)?;
                }
                Ok(_) => {
                    return Err(AppError::msg("Omslagsfilen får inte vara en symlänk."));
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::rename(&part, &dest)?;
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
    guard.armed = false;
    let saved = std::fs::symlink_metadata(&dest)?;
    if !saved.is_file() || saved.file_type().is_symlink() {
        return Err(AppError::msg("Omslagsfilen får inte vara en symlänk."));
    }
    for other in ["jpg", "png", "gif", "webp"] {
        if other == ext {
            continue;
        }
        let old = dir.join(format!("{album_id}.{other}"));
        if let Ok(meta) = std::fs::symlink_metadata(&old) {
            if meta.file_type().is_symlink() || !meta.is_file() {
                return Err(AppError::msg("Omslagsfilen får inte vara en symlänk."));
            }
            std::fs::remove_file(&old)?;
        }
    }
    Ok(ext.to_string())
}

fn refuse_symlink_file(path: &Path) -> AppResult<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::msg("Omslagsfilen får inte vara en symlänk.")),
        Err(err) => Err(err.into()),
    }
}

struct RegularFileGuard<'a> {
    path: &'a Path,
    armed: bool,
}

impl Drop for RegularFileGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            remove_regular_file(self.path);
        }
    }
}

fn remove_regular_file(path: &Path) {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.is_file() && !meta.file_type().is_symlink() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn ensure_cover_dir(media_root: &Path) -> AppResult<PathBuf> {
    let music = ensure_real_child(media_root, "music")?;
    ensure_real_child(&music, "covers")
}

fn ensure_real_child(parent: &Path, name: &str) -> AppResult<PathBuf> {
    let parent_meta = std::fs::symlink_metadata(parent)?;
    if parent_meta.file_type().is_symlink() || !parent_meta.is_dir() {
        return Err(AppError::msg(
            "Omslagsmappen måste ligga i en vanlig katalog.",
        ));
    }
    let dir = parent.join(name);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(AppError::msg("Omslagsmappen får inte vara en symlänk."));
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&dir)?;
        }
        Err(err) => return Err(err.into()),
    }
    let parent_canon = std::fs::canonicalize(parent)?;
    let dir_canon = std::fs::canonicalize(&dir)?;
    if !dir_canon.starts_with(&parent_canon) {
        return Err(AppError::msg(
            "Omslagsmappen ligger utanför mediamappen.",
        ));
    }
    Ok(dir)
}

fn safe_cover_path(media_root: &Path, album_id: i64, cover_ext: Option<&str>) -> Option<String> {
    let ext = cover_ext?;
    if album_id <= 0 || !matches!(ext, "jpg" | "png" | "gif" | "webp") {
        return None;
    }
    let music = real_child(media_root, "music")?;
    let dir = real_child(&music, "covers")?;
    let path = dir.join(format!("{album_id}.{ext}"));
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    if meta.len() == 0 || meta.len() > MAX_COVER_BYTES {
        return None;
    }
    let dir_canon = std::fs::canonicalize(&dir).ok()?;
    let file_canon = std::fs::canonicalize(&path).ok()?;
    if file_canon.file_name()?.to_str()? != format!("{album_id}.{ext}") {
        return None;
    }
    if !file_canon.starts_with(&dir_canon) {
        return None;
    }
    let again = std::fs::symlink_metadata(&path).ok()?;
    if !again.is_file() || again.file_type().is_symlink() {
        return None;
    }
    Some(file_canon.display().to_string())
}

fn real_child(parent: &Path, name: &str) -> Option<PathBuf> {
    let parent_meta = std::fs::symlink_metadata(parent).ok()?;
    if parent_meta.file_type().is_symlink() || !parent_meta.is_dir() {
        return None;
    }
    let dir = parent.join(name);
    let meta = std::fs::symlink_metadata(&dir).ok()?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return None;
    }
    let parent_canon = std::fs::canonicalize(parent).ok()?;
    let dir_canon = std::fs::canonicalize(&dir).ok()?;
    if !dir_canon.starts_with(&parent_canon) {
        return None;
    }
    Some(dir)
}

#[derive(Deserialize)]
struct MbSearch {
    #[serde(default)]
    releases: Vec<MbRelease>,
}

#[derive(Deserialize)]
struct MbRelease {
    id: String,
    #[serde(default)]
    title: String,
    date: Option<String>,
    status: Option<String>,
    #[serde(default, rename = "artist-credit")]
    artist_credit: Vec<MbCredit>,
}

#[derive(Deserialize)]
struct MbCredit {
    name: Option<String>,
    joinphrase: Option<String>,
    artist: Option<MbArtistName>,
}

#[derive(Deserialize)]
struct MbArtistName {
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[test]
    fn online_metadata_consent_defaults_off() {
        let conn = Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        assert!(!db::online_metadata_consent(&conn).unwrap());
        db::set_setting(&conn, "online_metadata_consent", "true").unwrap();
        assert!(!db::online_metadata_consent(&conn).unwrap());
        db::set_setting(&conn, "online_metadata_consent", "yes").unwrap();
        assert!(!db::online_metadata_consent(&conn).unwrap());
        db::set_setting(&conn, "online_metadata_consent", "on").unwrap();
        assert!(!db::online_metadata_consent(&conn).unwrap());
        db::set_setting(&conn, "online_metadata_consent", "").unwrap();
        assert!(!db::online_metadata_consent(&conn).unwrap());
        db::set_online_metadata_consent(&conn, true).unwrap();
        assert!(db::online_metadata_consent(&conn).unwrap());
        assert_eq!(
            db::get_setting(&conn, "online_metadata_consent")
                .unwrap()
                .as_deref(),
            Some("1")
        );
        db::set_online_metadata_consent(&conn, false).unwrap();
        assert!(!db::online_metadata_consent(&conn).unwrap());
    }

    struct PanicHttp;

    impl MetadataHttp for PanicHttp {
        async fn get(&self, _url: &str, _max_bytes: u64) -> AppResult<HttpBody> {
            panic!("nätet anropades utan samtycke");
        }
    }

    #[tokio::test]
    async fn no_network_when_consent_is_off() {
        let calls = AtomicUsize::new(0);
        let err = lookup_with_client(false, "Miles Davis", "Kind of Blue", || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(PanicHttp)
        })
        .await
        .unwrap_err();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let text = err.to_string();
        assert!(text.contains("samtycke"));
        assert!(text.contains("Ingen förfrågan"));
    }

    #[test]
    fn host_allowlist_is_only_musicbrainz_and_cover_art_archive() {
        assert_eq!(
            ALLOWED_METADATA_HOSTS,
            ["musicbrainz.org", "coverartarchive.org"]
        );
        assert!(inspect_metadata_url("https://musicbrainz.org/ws/2/release/").is_ok());
        assert!(inspect_metadata_url(
            "https://coverartarchive.org/release/12345678-1234-1234-1234-123456789abc/front-250"
        )
        .is_ok());
        assert!(inspect_metadata_url("https://musicbrainz.org.:443/ws/2/").is_ok());
        for blocked in [
            "https://example.com/",
            "https://archive.org/download/x",
            "https://ia6001.us.archive.org/x",
            "https://www.musicbrainz.org/",
            "https://musicbrainz.org.evil.com/",
            "https://evilmusicbrainz.org/",
            "https://notcoverartarchive.org/",
            "http://musicbrainz.org/ws/2/",
            "http://coverartarchive.org/release/x",
            "https://musicbrainz.org:444/ws/2/",
            "https://user@musicbrainz.org/",
            "https://musicbrainz.org@evil.com/",
            "https://127.0.0.1/",
            "https://169.254.169.254/latest/meta-data",
            "https://192.168.1.1/",
            "https://10.1.1.1/",
            "https://172.16.0.1/",
            "https://[fe80::1]/",
            "https://[::1]/",
            "https://[fd00::1]/",
            "file:///etc/passwd",
            "ftp://musicbrainz.org/x",
        ] {
            assert!(
                inspect_metadata_url(blocked).is_err(),
                "ska avvisas: {blocked}"
            );
        }
    }

    #[test]
    fn redirects_off_allowlist_and_private_targets_are_rejected() {
        let current =
            "https://coverartarchive.org/release/12345678-1234-1234-1234-123456789abc/front-250";
        let archive = next_metadata_url(current, "https://archive.org/download/mbid/file.jpg");
        let archive_text = archive.unwrap_err().to_string();
        assert!(archive_text.contains("inte tillåten"));
        assert!(!archive_text.contains("download"));
        assert!(next_metadata_url(current, "https://169.254.169.254/latest").is_err());
        assert!(next_metadata_url(current, "https://[fe80::1]/").is_err());
        assert!(next_metadata_url(current, "https://192.168.0.5/cover.jpg").is_err());
        assert!(next_metadata_url(current, "http://coverartarchive.org/release/x").is_err());
        assert!(next_metadata_url(current, "//archive.org/download/x").is_err());
        assert!(next_metadata_url(current, "https://coverartarchive.org.evil.com/x").is_err());
        let followed = next_metadata_url(current, "/release/12345678-1234-1234-1234-123456789abc/")
            .unwrap();
        assert!(followed.starts_with("https://coverartarchive.org/"));
        let other = next_metadata_url(current, "https://musicbrainz.org/ws/2/release/").unwrap();
        assert!(other.starts_with("https://musicbrainz.org/"));
        assert!(matches!(
            classify_hop(current, 307, Some("https://archive.org/download/x"), 0),
            Err(_)
        ));
        assert!(matches!(
            classify_hop(current, 200, None, 0),
            Ok(Hop::Response)
        ));
        assert!(classify_hop(
            current,
            302,
            Some("https://coverartarchive.org/release/12345678-1234-1234-1234-123456789abc/"),
            MAX_METADATA_REDIRECTS
        )
        .is_err());
    }

    #[test]
    fn allowed_host_with_private_or_link_local_address_is_rejected() {
        let public = [IpAddr::from([1, 1, 1, 1])];
        assert!(screen_metadata_resolution("musicbrainz.org", &public).is_ok());
        assert!(screen_metadata_resolution("coverartarchive.org.", &public).is_ok());
        assert!(screen_metadata_resolution("MusicBrainz.org", &public).is_ok());
        for blocked in [
            IpAddr::from([127, 0, 0, 1]),
            IpAddr::from([10, 0, 0, 1]),
            IpAddr::from([172, 16, 0, 1]),
            IpAddr::from([192, 168, 1, 1]),
            IpAddr::from([169, 254, 169, 254]),
            IpAddr::from([169, 254, 1, 1]),
            IpAddr::from([100, 64, 0, 1]),
            IpAddr::from([0, 0, 0, 0]),
            IpAddr::from([255, 255, 255, 255]),
            IpAddr::from([192, 0, 2, 1]),
            "fe80::1".parse().unwrap(),
            "::1".parse().unwrap(),
            "fd00::1".parse().unwrap(),
            "::ffff:169.254.169.254".parse().unwrap(),
            "::ffff:127.0.0.1".parse().unwrap(),
        ] {
            assert!(
                screen_metadata_resolution("musicbrainz.org", &[blocked]).is_err(),
                "ska avvisas: {blocked}"
            );
        }
        let mixed = [IpAddr::from([1, 1, 1, 1]), IpAddr::from([10, 1, 1, 1])];
        assert!(screen_metadata_resolution("coverartarchive.org", &mixed).is_err());
        assert!(screen_metadata_resolution("archive.org", &public).is_err());
        assert!(screen_metadata_resolution("musicbrainz.org", &[]).is_err());
    }

    #[test]
    fn cover_download_size_limit_and_image_type() {
        assert_eq!(MAX_COVER_BYTES, 2 * 1024 * 1024);
        assert!(reject_declared_length(Some(MAX_COVER_BYTES + 1), MAX_COVER_BYTES).is_err());
        assert!(reject_declared_length(Some(MAX_COVER_BYTES), MAX_COVER_BYTES).is_ok());
        let mut buf = Vec::new();
        assert!(append_capped(&mut buf, &vec![0; MAX_COVER_BYTES as usize + 1], MAX_COVER_BYTES).is_err());
        assert!(buf.is_empty());
        let mut near = vec![0; MAX_COVER_BYTES as usize - 1];
        assert!(append_capped(&mut near, &[1, 2], MAX_COVER_BYTES).is_err());
        assert_eq!(near.len(), MAX_COVER_BYTES as usize - 1);
        assert_eq!(accept_cover_bytes(&sample_jpeg()).unwrap(), "jpg");
        assert_eq!(accept_cover_bytes(&sample_with_prefix(b"\x89PNG\r\n\x1a\n")).unwrap(), "png");
        assert!(accept_cover_bytes(b"<html>not an image</html>").is_err());
        let mut over = sample_jpeg();
        over.resize(MAX_COVER_BYTES as usize + 1, 0);
        assert!(accept_cover_bytes(&over).is_err());
    }

    #[test]
    fn search_url_stays_on_musicbrainz_and_bad_mbid_has_no_cover_url() {
        let url = musicbrainz_search_url("Miles", "https://evil.com\" OR artist:x").unwrap();
        let parsed = Url::parse(&url).unwrap();
        assert_eq!(parsed.host_str(), Some("musicbrainz.org"));
        assert_eq!(parsed.scheme(), "https");
        assert!(cover_art_url("../evil").is_err());
        assert!(cover_art_url("https://archive.org/x").is_err());
        let cover = cover_art_url("12345678-1234-1234-1234-123456789ABC").unwrap();
        assert_eq!(
            cover,
            "https://coverartarchive.org/release/12345678-1234-1234-1234-123456789abc/front-250"
        );
    }

    struct ScriptedHttp {
        hits: Arc<AtomicUsize>,
        urls: Arc<Mutex<Vec<String>>>,
        json: Vec<u8>,
        image_status: u16,
        image: Vec<u8>,
    }

    impl MetadataHttp for ScriptedHttp {
        async fn get(&self, url: &str, max_bytes: u64) -> AppResult<HttpBody> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            let parsed = Url::parse(url).unwrap();
            let host = parsed.host_str().unwrap();
            assert!(
                host == "musicbrainz.org" || host == "coverartarchive.org",
                "{url}"
            );
            assert_eq!(parsed.scheme(), "https");
            self.urls.lock().unwrap().push(url.to_string());
            if host == "musicbrainz.org" {
                assert_eq!(max_bytes, MAX_METADATA_BYTES);
                return Ok(HttpBody {
                    status: 200,
                    bytes: self.json.clone(),
                });
            }
            assert_eq!(max_bytes, MAX_COVER_BYTES);
            Ok(HttpBody {
                status: self.image_status,
                bytes: self.image.clone(),
            })
        }
    }

    fn sample_release_json(id: &str) -> Vec<u8> {
        format!(
            r#"{{"releases":[{{"id":"{id}","title":"Kind of Blue","status":"Official","date":"1959-08-17","artist-credit":[{{"name":"Miles Davis","joinphrase":""}}]}}]}}"#
        )
        .into_bytes()
    }

    #[tokio::test]
    async fn consent_on_stores_metadata_without_real_network() {
        let hits = Arc::new(AtomicUsize::new(0));
        let urls = Arc::new(Mutex::new(Vec::new()));
        let http = ScriptedHttp {
            hits: hits.clone(),
            urls: urls.clone(),
            json: sample_release_json("12345678-1234-1234-1234-123456789abc"),
            image_status: 200,
            image: sample_jpeg(),
        };
        let calls = AtomicUsize::new(0);
        let fetched = lookup_with_client(true, "Miles Davis", "Kind of Blue", || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(http)
        })
        .await
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        let seen = urls.lock().unwrap().clone();
        assert!(seen[0].starts_with("https://musicbrainz.org/"));
        assert!(seen[1].starts_with("https://coverartarchive.org/release/12345678-1234-1234-1234-123456789abc/"));
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        let conn = Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        let album_id = db::upsert_music_album(&conn, "Kind of Blue", "Miles Davis").unwrap();
        let outcome = persist(&conn, &media, album_id, fetched).unwrap();
        assert!(outcome.cover_saved);
        assert_eq!(outcome.mbid, "12345678-1234-1234-1234-123456789abc");
        assert_eq!(outcome.release_date.as_deref(), Some("1959-08-17"));
        let album = db::get_music_album(&conn, album_id).unwrap();
        assert_eq!(album.title, "Kind of Blue");
        assert_eq!(album.artist, "Miles Davis");
        assert_eq!(album.cover_ext.as_deref(), Some("jpg"));
        let mut album = album;
        attach_cover_path(&media, &mut album);
        let cover = album.cover_path.expect("cover path");
        assert!(!cover.contains("://"));
        let bytes = std::fs::read(&cover).unwrap();
        assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]));
    }

    #[tokio::test]
    async fn invalid_mbid_does_not_request_another_host() {
        let hits = Arc::new(AtomicUsize::new(0));
        let urls = Arc::new(Mutex::new(Vec::new()));
        let err = lookup_with_client(
            true,
            "Miles Davis",
            "Kind of Blue",
            || {
                Ok(ScriptedHttp {
                    hits: hits.clone(),
                    urls: urls.clone(),
                    json: sample_release_json("https://evil.example/x"),
                    image_status: 200,
                    image: sample_jpeg(),
                })
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("ogiltigt") || err.to_string().contains("Ingen träff"));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let seen = urls.lock().unwrap().clone();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].starts_with("https://musicbrainz.org/"));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_cover_file_and_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let media = tmp.path().join("media");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(media.join("music")).unwrap();
        std::os::unix::fs::symlink(&outside, media.join("music").join("covers")).unwrap();
        let err = write_album_cover(&media, 1, &sample_jpeg()).unwrap_err();
        assert!(err.to_string().contains("symlänk"));
        assert!(outside.read_dir().unwrap().next().is_none());

        std::fs::remove_file(media.join("music").join("covers")).unwrap();
        std::fs::create_dir_all(media.join("music").join("covers")).unwrap();
        let secret = tmp.path().join("secret");
        std::fs::write(&secret, b"secret").unwrap();
        std::os::unix::fs::symlink(&secret, media.join("music").join("covers").join("1.jpg")).unwrap();
        let err = write_album_cover(&media, 1, &sample_jpeg()).unwrap_err();
        assert!(err.to_string().contains("symlänk"));
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret");
    }

    fn sample_jpeg() -> Vec<u8> {
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xD9];
        bytes.resize(32, 0);
        bytes
    }

    fn sample_with_prefix(prefix: &[u8]) -> Vec<u8> {
        let mut bytes = prefix.to_vec();
        bytes.resize(32, 0);
        bytes
    }
}
