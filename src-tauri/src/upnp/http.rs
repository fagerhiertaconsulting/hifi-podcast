use super::cds;
use crate::db;
use crate::media_mime;
use axum::body::Body;
use axum::extract::{Path as AxPath, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get};
use axum::Router;
use rusqlite::Connection;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::watch;
use tokio_util::io::ReaderStream;

#[derive(Clone)]
pub struct HttpState {
    pub db: Arc<Mutex<Connection>>,
    pub media_root: PathBuf,
    pub uuid: String,
    pub public_base: String,
}

pub fn router(state: HttpState) -> Router {
    Router::new()
        .route("/", get(status_page))
        .route("/upnp/device.xml", get(device_desc))
        .route("/upnp/cds.xml", get(cds_scpd))
        .route("/upnp/cm.xml", get(cm_scpd))
        .route("/upnp/control/cds", any(cds_control))
        .route("/upnp/control/cm", any(cm_control))
        .route("/upnp/event/cds", any(event_stub))
        .route("/upnp/event/cm", any(event_stub))
        .route(
            "/media/episode/{id}",
            get(serve_episode).head(serve_episode),
        )
        .route("/media/track/{id}", get(serve_track).head(serve_track))
        .with_state(state)
}

pub async fn serve(
    listener: tokio::net::TcpListener,
    state: HttpState,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move {
            let _ = shutdown.wait_for(|stop| *stop).await;
        })
        .await
}

async fn status_page(State(state): State<HttpState>) -> impl IntoResponse {
    let body = format!(
        "<html><body><h1>HIFI podcast UPnP</h1><p>Enhet: <a href=\"/upnp/device.xml\">device.xml</a></p><p>Poddar och indexerad musik. Originalfiler, ingen omkodning.</p><p>{}</p></body></html>",
        cds::xml_escape(&state.public_base)
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body)
}

async fn device_desc(State(state): State<HttpState>) -> impl IntoResponse {
    let xml = device_xml(&state.uuid, &state.public_base);
    xml_response(xml)
}

async fn cds_scpd() -> impl IntoResponse {
    xml_response(CDS_SCPD.into())
}

async fn cm_scpd() -> impl IntoResponse {
    xml_response(CM_SCPD.into())
}

fn xml_response(xml: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/xml; charset=\"utf-8\""),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        xml,
    )
        .into_response()
}

async fn event_stub() -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::HeaderName::from_static("sid"),
        HeaderValue::from_static("uuid:00000000-0000-0000-0000-000000000001"),
    );
    headers.insert(
        header::HeaderName::from_static("timeout"),
        HeaderValue::from_static("Second-1800"),
    );
    (StatusCode::OK, headers).into_response()
}

async fn cds_control(State(state): State<HttpState>, req: Request) -> Response {
    if req.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let action = soap_action(req.headers());
    let bytes = match axum::body::to_bytes(req.into_body(), 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    let body = String::from_utf8_lossy(&bytes);
    let conn = match state.db.lock() {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let service = "urn:schemas-upnp-org:service:ContentDirectory:1";
    match action.as_deref() {
        Some("Browse") => {
            let object_id = cds::extract_soap_arg(&body, "ObjectID").unwrap_or_else(|| "0".into());
            let flag = cds::extract_soap_arg(&body, "BrowseFlag").unwrap_or_default();
            let start: u32 = cds::extract_soap_arg(&body, "StartingIndex")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let count: u32 = cds::extract_soap_arg(&body, "RequestedCount")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let metadata = flag.to_ascii_lowercase().contains("metadata");
            match cds::browse(
                &conn,
                &object_id,
                metadata,
                start,
                count,
                &state.public_base,
            ) {
                Ok(result) => {
                    let inner = format!(
                        "<Result>{}</Result><NumberReturned>{}</NumberReturned><TotalMatches>{}</TotalMatches><UpdateID>1</UpdateID>",
                        cds::xml_escape(&result.didl),
                        result.number_returned,
                        result.total_matches
                    );
                    xml_response(cds::soap_ok("Browse", service, &inner))
                }
                Err(code) => {
                    let mut resp = xml_response(cds::soap_fault(code, "No such object"));
                    *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                    resp
                }
            }
        }
        Some("GetSearchCapabilities") => xml_response(cds::soap_ok(
            "GetSearchCapabilities",
            service,
            "<SearchCaps></SearchCaps>",
        )),
        Some("GetSortCapabilities") => xml_response(cds::soap_ok(
            "GetSortCapabilities",
            service,
            "<SortCaps>dc:title</SortCaps>",
        )),
        Some("GetSystemUpdateID") => {
            xml_response(cds::soap_ok("GetSystemUpdateID", service, "<Id>1</Id>"))
        }
        _ => {
            let mut resp = xml_response(cds::soap_fault(401, "Invalid Action"));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            resp
        }
    }
}

async fn cm_control(req: Request) -> Response {
    if req.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let action = soap_action(req.headers());
    let service = "urn:schemas-upnp-org:service:ConnectionManager:1";
    match action.as_deref() {
        Some("GetProtocolInfo") => xml_response(cds::soap_ok(
            "GetProtocolInfo",
            service,
            "<Source>http-get:*:audio/mpeg:*,http-get:*:audio/mp4:*,http-get:*:audio/flac:*,http-get:*:audio/ogg:*,http-get:*:audio/wav:*,http-get:*:audio/aiff:*,http-get:*:audio/x-dsd:*</Source><Sink></Sink>",
        )),
        Some("GetCurrentConnectionIDs") => xml_response(cds::soap_ok(
            "GetCurrentConnectionIDs",
            service,
            "<ConnectionIDs>0</ConnectionIDs>",
        )),
        Some("GetCurrentConnectionInfo") => xml_response(cds::soap_ok(
            "GetCurrentConnectionInfo",
            service,
            "<RcsID>-1</RcsID><AVTransportID>-1</AVTransportID><ProtocolInfo></ProtocolInfo><PeerConnectionManager></PeerConnectionManager><PeerConnectionID>-1</PeerConnectionID><Direction>Output</Direction><Status>OK</Status>",
        )),
        _ => {
            let mut resp = xml_response(cds::soap_fault(401, "Invalid Action"));
            *resp.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            resp
        }
    }
}

fn soap_action(headers: &HeaderMap) -> Option<String> {
    let raw = headers
        .get("soapaction")
        .or_else(|| headers.get("SOAPAction"))
        .or_else(|| headers.get("SOAPACTION"))?;
    let s = raw.to_str().ok()?.trim().trim_matches('"');
    s.rsplit('#').next().map(|a| a.to_string())
}

/// Parse RFC 7233 `bytes=` single range into inclusive start..=end.
pub fn parse_byte_range(header: &str, file_len: u64) -> Result<Option<(u64, u64)>, ()> {
    if file_len == 0 {
        return Ok(None);
    }
    let header = header.trim();
    let Some(spec) = header.strip_prefix("bytes=") else {
        return Err(());
    };
    if spec.contains(',') {
        return Err(());
    }
    let (start_s, end_s) = spec.split_once('-').ok_or(())?;
    if start_s.is_empty() {
        let suffix: u64 = end_s.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let start = file_len.saturating_sub(suffix);
        return Ok(Some((start, file_len - 1)));
    }
    let start: u64 = start_s.parse().map_err(|_| ())?;
    if start >= file_len {
        return Err(());
    }
    let end = if end_s.is_empty() {
        file_len - 1
    } else {
        let e: u64 = end_s.parse().map_err(|_| ())?;
        e.min(file_len - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

async fn serve_episode(
    State(state): State<HttpState>,
    AxPath(id): AxPath<i64>,
    headers: HeaderMap,
    method: Method,
) -> Response {
    let episode = {
        let conn = match state.db.lock() {
            Ok(c) => c,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        match db::get_upnp_episode(&conn, id) {
            Ok(Some(ep)) => ep,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    };
    let path = PathBuf::from(&episode.file_path);
    if !file_is_under_root(&path, &state.media_root) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let meta = match tokio::fs::metadata(&path).await {
        Ok(m) if m.is_file() => m,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let file_len = meta.len();
    let mime = media_mime::for_serving(episode.mime.as_deref(), &path);
    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    respond_with_file(file, file_len, &mime, &headers, &method).await
}

#[derive(Debug, PartialEq, Eq)]
pub enum MusicHttpDeny {
    NotFound,
    Forbidden,
}

/// Opens the file indexed for `track_id` after the same checks as local playback.
/// `request_path` is never used as the file to open. A supplied path is accepted
/// only when it is that indexed file; any other path is rejected.
pub fn authorize_indexed_music_file(
    conn: &Connection,
    media_root: &Path,
    track_id: i64,
    request_path: Option<&Path>,
) -> Result<PathBuf, MusicHttpDeny> {
    let track = db::get_music_track(conn, track_id).map_err(|_| MusicHttpDeny::NotFound)?;
    let indexed =
        crate::music::require_playable_track(conn, media_root, &track).map_err(|err| {
            if err.to_string().contains("saknar lokal fil") {
                MusicHttpDeny::NotFound
            } else {
                MusicHttpDeny::Forbidden
            }
        })?;
    if let Some(requested) = request_path {
        if !crate::music::request_path_is_indexed_file(requested, &indexed) {
            return Err(MusicHttpDeny::Forbidden);
        }
    }
    Ok(indexed)
}

/// The URL may name a track id. It may not name a file.
pub fn foreign_music_request(path: &str, query: Option<&str>, track_id: i64) -> bool {
    if path != format!("/media/track/{track_id}") {
        return true;
    }
    query.is_some_and(query_names_a_file)
}

fn query_names_a_file(query: &str) -> bool {
    decode_percent(query)
        .to_ascii_lowercase()
        .split('&')
        .any(|pair| {
            let key = pair.split('=').next().unwrap_or("").trim();
            matches!(
                key,
                "path" | "file" | "url" | "src" | "filename" | "filepath"
            )
        })
}

fn decode_percent(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3]) {
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn serve_track(
    State(state): State<HttpState>,
    AxPath(id): AxPath<i64>,
    req: Request,
) -> Response {
    let method = req.method().clone();
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    if foreign_music_request(req.uri().path(), req.uri().query(), id) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let headers = req.headers().clone();
    let path = {
        let conn = match state.db.lock() {
            Ok(c) => c,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
        match authorize_indexed_music_file(&conn, &state.media_root, id, None) {
            Ok(path) => path,
            Err(MusicHttpDeny::NotFound) => return StatusCode::NOT_FOUND.into_response(),
            Err(MusicHttpDeny::Forbidden) => return StatusCode::FORBIDDEN.into_response(),
        }
    };
    let mime = media_mime::for_serving(None, &path);
    let file = match open_indexed_music_file(&path) {
        Ok(file) => file,
        Err(status) => return status.into_response(),
    };
    let file_len = match file.metadata().await {
        Ok(meta) if meta.len() <= crate::net::MAX_MEDIA_BYTES => meta.len(),
        Ok(_) => return StatusCode::FORBIDDEN.into_response(),
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    respond_with_file(file, file_len, &mime, &headers, &method).await
}

fn open_indexed_music_file(path: &Path) -> Result<tokio::fs::File, StatusCode> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => return Err(StatusCode::NOT_FOUND),
    };
    if crate::music::metadata_is_link(&meta) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !meta.is_file() || meta.len() > crate::net::MAX_MEDIA_BYTES {
        return Err(if meta.is_file() {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::NOT_FOUND
        });
    }
    let std_file = open_nofollow(path)?;
    Ok(tokio::fs::File::from_std(std_file))
}

fn open_nofollow(path: &Path) -> Result<std::fs::File, StatusCode> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        const O_NOFOLLOW: i32 = 0x20000;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(O_NOFOLLOW)
            .open(path)
            .map_err(|err| {
                if err.raw_os_error() == Some(40) {
                    StatusCode::FORBIDDEN
                } else {
                    StatusCode::NOT_FOUND
                }
            })
    }
    #[cfg(not(target_os = "linux"))]
    {
        std::fs::File::open(path).map_err(|_| StatusCode::NOT_FOUND)
    }
}

async fn respond_with_file(
    mut file: tokio::fs::File,
    file_len: u64,
    mime: &str,
    headers: &HeaderMap,
    method: &Method,
) -> Response {
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let range = match range_header.as_deref() {
        Some(h) => match parse_byte_range(h, file_len) {
            Ok(r) => r,
            Err(()) => {
                let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
                resp.headers_mut().insert(
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes */{file_len}"))
                        .unwrap_or(HeaderValue::from_static("bytes */0")),
                );
                return resp;
            }
        },
        None => None,
    };

    let (start, end, status) = match range {
        Some((s, e)) => (s, e, StatusCode::PARTIAL_CONTENT),
        None => (0, file_len.saturating_sub(1), StatusCode::OK),
    };
    let content_len = if file_len == 0 { 0 } else { end - start + 1 };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, content_len)
        .header(header::CACHE_CONTROL, "public, max-age=3600")
        .header("contentFeatures.dlna.org", "DLNA.ORG_OP=01;DLNA.ORG_CI=0")
        .header("transferMode.dlna.org", "Streaming");
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{file_len}"),
        );
    }

    if *method == Method::HEAD {
        return builder
            .body(Body::empty())
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
    }

    if start > 0 && file.seek(SeekFrom::Start(start)).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let stream = ReaderStream::new(file.take(content_len));
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

pub fn file_is_under_root(path: &Path, root: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        return false;
    };
    let Ok(file) = std::fs::canonicalize(path) else {
        return false;
    };
    file.starts_with(&root)
}

pub fn device_xml(uuid: &str, public_base: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<root xmlns="urn:schemas-upnp-org:device-1-0" xmlns:dlna="urn:schemas-dlna-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
    <friendlyName>HIFI podcast</friendlyName>
    <manufacturer>Fagerhierta Consulting</manufacturer>
    <manufacturerURL>https://github.com/fagerhiertaconsulting/hifi-podcast</manufacturerURL>
    <modelName>HIFI podcast</modelName>
    <modelNumber>0.1</modelNumber>
    <modelDescription>UPnP AV Media Server for podcasts and the local music library</modelDescription>
    <UDN>uuid:{uuid}</UDN>
    <dlna:X_DLNADOC>DMS-1.50</dlna:X_DLNADOC>
    <serviceList>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ContentDirectory</serviceId>
        <SCPDURL>/upnp/cds.xml</SCPDURL>
        <controlURL>/upnp/control/cds</controlURL>
        <eventSubURL>/upnp/event/cds</eventSubURL>
      </service>
      <service>
        <serviceType>urn:schemas-upnp-org:service:ConnectionManager:1</serviceType>
        <serviceId>urn:upnp-org:serviceId:ConnectionManager</serviceId>
        <SCPDURL>/upnp/cm.xml</SCPDURL>
        <controlURL>/upnp/control/cm</controlURL>
        <eventSubURL>/upnp/event/cm</eventSubURL>
      </service>
    </serviceList>
    <presentationURL>{public_base}/</presentationURL>
  </device>
</root>
"#
    )
}

const CDS_SCPD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action><name>Browse</name><argumentList>
      <argument><name>ObjectID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ObjectID</relatedStateVariable></argument>
      <argument><name>BrowseFlag</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_BrowseFlag</relatedStateVariable></argument>
      <argument><name>Filter</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Filter</relatedStateVariable></argument>
      <argument><name>StartingIndex</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Index</relatedStateVariable></argument>
      <argument><name>RequestedCount</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
      <argument><name>SortCriteria</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_SortCriteria</relatedStateVariable></argument>
      <argument><name>Result</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Result</relatedStateVariable></argument>
      <argument><name>NumberReturned</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
      <argument><name>TotalMatches</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Count</relatedStateVariable></argument>
      <argument><name>UpdateID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_UpdateID</relatedStateVariable></argument>
    </argumentList></action>
    <action><name>GetSearchCapabilities</name><argumentList>
      <argument><name>SearchCaps</name><direction>out</direction><relatedStateVariable>SearchCapabilities</relatedStateVariable></argument>
    </argumentList></action>
    <action><name>GetSortCapabilities</name><argumentList>
      <argument><name>SortCaps</name><direction>out</direction><relatedStateVariable>SortCapabilities</relatedStateVariable></argument>
    </argumentList></action>
    <action><name>GetSystemUpdateID</name><argumentList>
      <argument><name>Id</name><direction>out</direction><relatedStateVariable>SystemUpdateID</relatedStateVariable></argument>
    </argumentList></action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ObjectID</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_BrowseFlag</name><dataType>string</dataType><allowedValueList><allowedValue>BrowseMetadata</allowedValue><allowedValue>BrowseDirectChildren</allowedValue></allowedValueList></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Filter</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Index</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Count</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_SortCriteria</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Result</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_UpdateID</name><dataType>ui4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SearchCapabilities</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>SortCapabilities</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>SystemUpdateID</name><dataType>ui4</dataType></stateVariable>
  </serviceStateTable>
</scpd>
"#;

const CM_SCPD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<scpd xmlns="urn:schemas-upnp-org:service-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <actionList>
    <action><name>GetProtocolInfo</name><argumentList>
      <argument><name>Source</name><direction>out</direction><relatedStateVariable>SourceProtocolInfo</relatedStateVariable></argument>
      <argument><name>Sink</name><direction>out</direction><relatedStateVariable>SinkProtocolInfo</relatedStateVariable></argument>
    </argumentList></action>
    <action><name>GetCurrentConnectionIDs</name><argumentList>
      <argument><name>ConnectionIDs</name><direction>out</direction><relatedStateVariable>CurrentConnectionIDs</relatedStateVariable></argument>
    </argumentList></action>
    <action><name>GetCurrentConnectionInfo</name><argumentList>
      <argument><name>ConnectionID</name><direction>in</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
      <argument><name>RcsID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_RcsID</relatedStateVariable></argument>
      <argument><name>AVTransportID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_AVTransportID</relatedStateVariable></argument>
      <argument><name>ProtocolInfo</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ProtocolInfo</relatedStateVariable></argument>
      <argument><name>PeerConnectionManager</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionManager</relatedStateVariable></argument>
      <argument><name>PeerConnectionID</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionID</relatedStateVariable></argument>
      <argument><name>Direction</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_Direction</relatedStateVariable></argument>
      <argument><name>Status</name><direction>out</direction><relatedStateVariable>A_ARG_TYPE_ConnectionStatus</relatedStateVariable></argument>
    </argumentList></action>
  </actionList>
  <serviceStateTable>
    <stateVariable sendEvents="yes"><name>SourceProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>SinkProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="yes"><name>CurrentConnectionIDs</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionID</name><dataType>i4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_RcsID</name><dataType>i4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_AVTransportID</name><dataType>i4</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ProtocolInfo</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionManager</name><dataType>string</dataType></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_Direction</name><dataType>string</dataType><allowedValueList><allowedValue>Input</allowedValue><allowedValue>Output</allowedValue></allowedValueList></stateVariable>
    <stateVariable sendEvents="no"><name>A_ARG_TYPE_ConnectionStatus</name><dataType>string</dataType></stateVariable>
  </serviceStateTable>
</scpd>
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::path::Path;

    #[test]
    fn parses_open_and_closed_ranges() {
        assert_eq!(
            parse_byte_range("bytes=0-499", 1000).unwrap(),
            Some((0, 499))
        );
        assert_eq!(
            parse_byte_range("bytes=500-", 1000).unwrap(),
            Some((500, 999))
        );
        assert_eq!(
            parse_byte_range("bytes=-100", 1000).unwrap(),
            Some((900, 999))
        );
        assert!(parse_byte_range("bytes=1000-2000", 1000).is_err());
    }

    #[test]
    fn device_xml_is_mediaserver() {
        let xml = device_xml("abc", "http://192.168.1.4:8200");
        assert!(xml.contains("MediaServer:1"));
        assert!(xml.contains("uuid:abc"));
        assert!(xml.contains("ContentDirectory"));
        assert!(xml.contains("DMS-1.50"));
    }

    #[tokio::test]
    async fn range_get_returns_partial_content() {
        use crate::db;
        use rusqlite::Connection;
        use std::sync::{Arc, Mutex};
        use tokio::net::TcpListener;
        use tokio::sync::watch;

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        let show = media.join("podcasts").join("1");
        std::fs::create_dir_all(&show).unwrap();
        let file = show.join("1.mp3");
        std::fs::write(&file, b"0123456789ABCDEF").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        let pid = db::upsert_podcast(&conn, "https://e/f", "S", None, None, None, None).unwrap();
        let eid =
            db::upsert_episode(&conn, pid, "g", "Titled", None, None, None, Some(1000)).unwrap();
        db::upsert_download_state(
            &conn,
            eid,
            "done",
            Some(&file.display().to_string()),
            Some(16),
            None,
            None,
            Some("t"),
            Some("audio/mpeg"),
        )
        .unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = watch::channel(false);
        let state = HttpState {
            db: Arc::new(Mutex::new(conn)),
            media_root: media,
            uuid: "test-uuid".into(),
            public_base: format!("http://{addr}"),
        };
        tokio::spawn(async move {
            let _ = serve(listener, state, rx).await;
        });

        let client = reqwest::Client::new();
        let desc = client
            .get(format!("http://{addr}/upnp/device.xml"))
            .send()
            .await
            .unwrap();
        assert!(desc.status().is_success());
        let xml = desc.text().await.unwrap();
        assert!(xml.contains("MediaServer:1"));

        let soap = r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:Browse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1"><ObjectID>0</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>0</RequestedCount><SortCriteria></SortCriteria></u:Browse></s:Body></s:Envelope>"#;
        let browse = client
            .post(format!("http://{addr}/upnp/control/cds"))
            .header(
                "SOAPACTION",
                "\"urn:schemas-upnp-org:service:ContentDirectory:1#Browse\"",
            )
            .header("Content-Type", "text/xml")
            .body(soap)
            .send()
            .await
            .unwrap();
        assert!(browse.status().is_success());
        let body = browse.text().await.unwrap();
        assert!(body.contains("Podcasts"));
        assert!(body.contains("Music"));

        let ranged = client
            .get(format!("http://{addr}/media/episode/{eid}"))
            .header("Range", "bytes=0-3")
            .send()
            .await
            .unwrap();
        assert_eq!(ranged.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(ranged.headers()["content-type"], "audio/mpeg");
        assert_eq!(ranged.headers()["accept-ranges"], "bytes");
        let bytes = ranged.bytes().await.unwrap();
        assert_eq!(&bytes[..], b"0123");
        let _ = tx.send(true);
    }

    #[test]
    fn rejects_request_path_that_is_not_the_indexed_file() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let folder = dir.path().join("library");
        std::fs::create_dir_all(&folder).unwrap();
        let indexed = folder.join("tune.wav");
        let other = folder.join("other.wav");
        crate::engine::test_wav::write_sine_wav(&indexed, 44100, 1, 440.0, 0.05, 0.2);
        crate::engine::test_wav::write_sine_wav(&other, 44100, 1, 880.0, 0.05, 0.2);
        let conn = Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        crate::music::import_folder(&conn, &folder).unwrap();
        let tracks = crate::music::list_tracks(&conn, None).unwrap();
        let tune = tracks
            .iter()
            .find(|t| t.file_path.ends_with("tune.wav"))
            .unwrap();
        let other_track = tracks
            .iter()
            .find(|t| t.file_path.ends_with("other.wav"))
            .unwrap();

        let opened = authorize_indexed_music_file(&conn, &media, tune.id, None).unwrap();
        assert!(opened.ends_with("tune.wav"));
        assert!(!opened.starts_with(&media));
        assert_eq!(
            authorize_indexed_music_file(&conn, &media, tune.id, Some(&indexed)).unwrap(),
            opened
        );
        assert_eq!(
            authorize_indexed_music_file(&conn, &media, tune.id, Some(&other)).unwrap_err(),
            MusicHttpDeny::Forbidden
        );
        assert_eq!(
            authorize_indexed_music_file(
                &conn,
                &media,
                tune.id,
                Some(Path::new(&other_track.file_path))
            )
            .unwrap_err(),
            MusicHttpDeny::Forbidden
        );
        let outside = dir.path().join("secret.wav");
        crate::engine::test_wav::write_sine_wav(&outside, 44100, 1, 100.0, 0.05, 0.2);
        assert_eq!(
            authorize_indexed_music_file(&conn, &media, tune.id, Some(&outside)).unwrap_err(),
            MusicHttpDeny::Forbidden
        );
        let dotted = folder.join("nope").join("..").join("..").join("secret.wav");
        assert_eq!(
            authorize_indexed_music_file(&conn, &media, tune.id, Some(&dotted)).unwrap_err(),
            MusicHttpDeny::Forbidden
        );
        assert!(foreign_music_request(
            "/media/track/1",
            Some("path=/tmp/secret.wav"),
            1
        ));
        assert!(foreign_music_request(
            "/tmp/library/tune.wav",
            None,
            tune.id
        ));
        assert!(!foreign_music_request(
            &format!("/media/track/{}", tune.id),
            None,
            tune.id
        ));

        let grown = std::fs::OpenOptions::new()
            .write(true)
            .open(&other)
            .unwrap();
        grown.set_len(crate::net::MAX_MEDIA_BYTES + 1).unwrap();
        drop(grown);
        assert_eq!(
            authorize_indexed_music_file(&conn, &media, other_track.id, None).unwrap_err(),
            MusicHttpDeny::Forbidden
        );

        #[cfg(unix)]
        {
            std::fs::remove_file(&indexed).unwrap();
            std::os::unix::fs::symlink(&outside, &indexed).unwrap();
            assert_eq!(
                authorize_indexed_music_file(&conn, &media, tune.id, None).unwrap_err(),
                MusicHttpDeny::Forbidden
            );
        }
        let music_dir = media.join("music");
        let copied = std::fs::read_dir(&music_dir).unwrap().count();
        assert_eq!(copied, 0);
    }

    #[tokio::test]
    async fn music_browse_and_get_use_the_indexed_file_only() {
        use tokio::net::TcpListener;
        use tokio::sync::watch;

        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        crate::download::ensure_media_layout(&media).unwrap();
        let folder = dir.path().join("library");
        let album_dir = folder.join("Miles Davis").join("Kind of Blue");
        std::fs::create_dir_all(&album_dir).unwrap();
        let indexed = album_dir.join("01 So What.wav");
        crate::engine::test_wav::write_sine_wav(&indexed, 44100, 1, 440.0, 0.05, 0.2);
        let original = std::fs::read(&indexed).unwrap();
        let secret = dir.path().join("secret.bin");
        std::fs::write(&secret, b"SECRET-NOT-INDEXED-BYTES").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        crate::music::import_folder(&conn, &folder).unwrap();
        let track = crate::music::list_tracks(&conn, None).unwrap().remove(0);
        assert!(!track.file_path.starts_with(&media.display().to_string()));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = watch::channel(false);
        let state = HttpState {
            db: Arc::new(Mutex::new(conn)),
            media_root: media,
            uuid: "music-test".into(),
            public_base: format!("http://{addr}"),
        };
        tokio::spawn(async move {
            let _ = serve(listener, state, rx).await;
        });

        let client = reqwest::Client::new();
        let browse = |oid: &str| {
            let body = format!(
                r#"<?xml version="1.0"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:Browse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1"><ObjectID>{oid}</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag><Filter>*</Filter><StartingIndex>0</StartingIndex><RequestedCount>0</RequestedCount><SortCriteria></SortCriteria></u:Browse></s:Body></s:Envelope>"#
            );
            let url = format!("http://{addr}/upnp/control/cds");
            let client = client.clone();
            async move {
                client
                    .post(url)
                    .header(
                        "SOAPACTION",
                        "\"urn:schemas-upnp-org:service:ContentDirectory:1#Browse\"",
                    )
                    .header("Content-Type", "text/xml")
                    .body(body)
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap()
            }
        };
        let root = browse("0").await;
        assert!(root.contains("Podcasts"));
        assert!(root.contains("id=&quot;music&quot;"));
        assert!(root.contains("childCount=&quot;1&quot;"));
        let music = browse("music").await;
        assert!(music.contains("Kind of Blue"));
        assert!(music.contains("album:"));
        assert!(!music.contains("secret.bin"));
        assert!(!music.contains(&indexed.display().to_string()));
        let album_id = music
            .split("album:")
            .nth(1)
            .unwrap()
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap();
        let tracks = browse(&format!("album:{album_id}")).await;
        assert!(tracks.contains("So What"));
        assert!(tracks.contains(&format!("/media/track/{}", track.id)));
        assert!(tracks.contains("audio/wav"));
        assert!(tracks.contains("DLNA.ORG_CI=0"));
        assert!(!tracks.contains("secret.bin"));
        assert!(!tracks.contains(&indexed.display().to_string()));

        let played = client
            .get(format!("http://{addr}/media/track/{}", track.id))
            .send()
            .await
            .unwrap();
        assert_eq!(played.status(), reqwest::StatusCode::OK);
        assert_eq!(played.headers()["content-type"], "audio/wav");
        assert_eq!(
            played.headers()["contentfeatures.dlna.org"],
            "DLNA.ORG_OP=01;DLNA.ORG_CI=0"
        );
        let bytes = played.bytes().await.unwrap();
        assert_eq!(&bytes[..], original.as_slice());

        let ranged = client
            .get(format!("http://{addr}/media/track/{}", track.id))
            .header("Range", "bytes=0-3")
            .send()
            .await
            .unwrap();
        assert_eq!(ranged.status(), reqwest::StatusCode::PARTIAL_CONTENT);
        assert_eq!(&ranged.bytes().await.unwrap()[..], &original[..4]);

        let foreign = client
            .get(format!(
                "http://{addr}/media/track/{}?path={}",
                track.id,
                secret.display()
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(foreign.status(), reqwest::StatusCode::FORBIDDEN);
        let foreign_body = foreign.bytes().await.unwrap();
        assert!(!contains_bytes(&foreign_body, b"SECRET-NOT-INDEXED-BYTES"));

        let by_path = client
            .get(format!("http://{addr}{}", secret.display()))
            .send()
            .await
            .unwrap();
        assert_ne!(by_path.status(), reqwest::StatusCode::OK);
        let by_path_body = by_path.bytes().await.unwrap();
        assert!(!contains_bytes(&by_path_body, b"SECRET-NOT-INDEXED-BYTES"));

        #[cfg(unix)]
        {
            std::fs::remove_file(&indexed).unwrap();
            std::os::unix::fs::symlink(&secret, &indexed).unwrap();
            let escaped = client
                .get(format!("http://{addr}/media/track/{}", track.id))
                .send()
                .await
                .unwrap();
            assert_eq!(escaped.status(), reqwest::StatusCode::FORBIDDEN);
            let escaped_body = escaped.bytes().await.unwrap();
            assert!(!contains_bytes(&escaped_body, b"SECRET-NOT-INDEXED-BYTES"));
        }
        let _ = tx.send(true);
    }

    fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }
}
