use crate::media_mime;
use crate::models::UpnpEpisode;
use rusqlite::Connection;
use std::path::Path;

pub const ROOT: &str = "0";
pub const PODCASTS: &str = "podcasts";
pub const MUSIC: &str = "music";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectId {
    Root,
    Podcasts,
    Music,
    Show(i64),
    Episode(i64),
}

pub fn parse_object_id(id: &str) -> Option<ObjectId> {
    let id = id.trim();
    if id.is_empty() || id == ROOT {
        return Some(ObjectId::Root);
    }
    if id.eq_ignore_ascii_case(PODCASTS) {
        return Some(ObjectId::Podcasts);
    }
    if id.eq_ignore_ascii_case(MUSIC) {
        return Some(ObjectId::Music);
    }
    if let Some(rest) = id.strip_prefix("show:") {
        return rest.parse().ok().map(ObjectId::Show);
    }
    if let Some(rest) = id.strip_prefix("ep:") {
        return rest.parse().ok().map(ObjectId::Episode);
    }
    None
}

pub fn show_id(podcast_id: i64) -> String {
    format!("show:{podcast_id}")
}

pub fn episode_id(id: i64) -> String {
    format!("ep:{id}")
}

pub fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() && c != '\t' && c != '\n' && c != '\r' => {}
            c => out.push(c),
        }
    }
    out
}

pub fn upnp_duration(ms: i64) -> String {
    let ms = ms.max(0);
    let h = ms / 3_600_000;
    let rem = ms % 3_600_000;
    let m = rem / 60_000;
    let rem = rem % 60_000;
    let s = rem / 1000;
    let millis = rem % 1000;
    format!("{h}:{m:02}:{s:02}.{millis:03}")
}

fn didl_wrap(body: &str) -> String {
    format!(
        r#"<DIDL-Lite xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/">{body}</DIDL-Lite>"#
    )
}

fn container_xml(id: &str, parent: &str, title: &str, child_count: i64, class: &str) -> String {
    format!(
        r#"<container id="{id}" parentID="{parent}" restricted="1" childCount="{child_count}"><dc:title>{}</dc:title><upnp:class>{class}</upnp:class></container>"#,
        xml_escape(title)
    )
}

pub fn episode_mime(ep: &UpnpEpisode) -> String {
    media_mime::for_serving(ep.mime.as_deref(), Path::new(&ep.file_path))
}

fn item_xml(ep: &UpnpEpisode, public_base: &str) -> String {
    let id = episode_id(ep.episode_id);
    let parent = show_id(ep.podcast_id);
    let mime = episode_mime(ep);
    let protocol = media_mime::protocol_info(&mime);
    let format_note = if media_mime::is_known_renderer_audio(&mime) {
        String::new()
    } else {
        "<desc id=\"format\" nameSpace=\"urn:hifi-podcast:meta\">original-no-transcode</desc>"
            .into()
    };
    let url = format!("{public_base}/media/episode/{}", ep.episode_id);
    let mut extra = String::new();
    if let Some(bytes) = ep.bytes {
        extra.push_str(&format!(" size=\"{bytes}\""));
    }
    if let Some(ms) = ep.duration_ms {
        extra.push_str(&format!(" duration=\"{}\"", upnp_duration(ms)));
    }
    let artist = ep
        .author
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&ep.podcast_title);
    let date = ep.publish_at.as_deref().unwrap_or("");
    let date_el = if date.is_empty() {
        String::new()
    } else {
        format!("<dc:date>{}</dc:date>", xml_escape(date))
    };
    format!(
        r#"<item id="{id}" parentID="{parent}" restricted="1"><dc:title>{}</dc:title><dc:creator>{}</dc:creator><upnp:artist>{}</upnp:artist><upnp:album>{}</upnp:album><upnp:class>object.item.audioItem.musicTrack</upnp:class>{date_el}{format_note}<res protocolInfo="{}"{extra}>{url}</res></item>"#,
        xml_escape(&ep.episode_title),
        xml_escape(artist),
        xml_escape(artist),
        xml_escape(&ep.podcast_title),
        xml_escape(&protocol),
    )
}

pub struct BrowseResult {
    pub didl: String,
    pub number_returned: u32,
    pub total_matches: u32,
}

fn slice<T>(items: &[T], start: u32, count: u32) -> &[T] {
    let start = start as usize;
    if start >= items.len() {
        return &[];
    }
    let end = if count == 0 {
        items.len()
    } else {
        (start + count as usize).min(items.len())
    };
    &items[start..end]
}

pub fn browse(
    conn: &Connection,
    object_id: &str,
    metadata: bool,
    start: u32,
    count: u32,
    public_base: &str,
) -> Result<BrowseResult, i32> {
    let id = parse_object_id(object_id).ok_or(701)?;
    if metadata {
        return browse_metadata(conn, &id, public_base);
    }
    match id {
        ObjectId::Root => {
            let shows = crate::db::list_upnp_podcasts(conn).map_err(|_| 501)?;
            let xmls = vec![
                container_xml(
                    PODCASTS,
                    ROOT,
                    "Podcasts",
                    shows.len() as i64,
                    "object.container.storageFolder",
                ),
                container_xml(MUSIC, ROOT, "Music", 0, "object.container.storageFolder"),
            ];
            let total = xmls.len() as u32;
            let page = slice(&xmls, start, count);
            Ok(BrowseResult {
                didl: didl_wrap(&page.join("")),
                number_returned: page.len() as u32,
                total_matches: total,
            })
        }
        ObjectId::Podcasts => {
            let shows = crate::db::list_upnp_podcasts(conn).map_err(|_| 501)?;
            let xmls: Vec<String> = shows
                .iter()
                .map(|(pid, title, _, n)| {
                    container_xml(
                        &show_id(*pid),
                        PODCASTS,
                        title,
                        *n,
                        "object.container.album.musicAlbum",
                    )
                })
                .collect();
            let total = xmls.len() as u32;
            let page = slice(&xmls, start, count);
            Ok(BrowseResult {
                didl: didl_wrap(&page.join("")),
                number_returned: page.len() as u32,
                total_matches: total,
            })
        }
        ObjectId::Music => Ok(BrowseResult {
            didl: didl_wrap(""),
            number_returned: 0,
            total_matches: 0,
        }),
        ObjectId::Show(pid) => {
            let eps = crate::db::list_upnp_episodes(conn, pid).map_err(|_| 501)?;
            let xmls: Vec<String> = eps.iter().map(|e| item_xml(e, public_base)).collect();
            let total = xmls.len() as u32;
            let page = slice(&xmls, start, count);
            Ok(BrowseResult {
                didl: didl_wrap(&page.join("")),
                number_returned: page.len() as u32,
                total_matches: total,
            })
        }
        ObjectId::Episode(eid) => {
            let ep = crate::db::get_upnp_episode(conn, eid)
                .map_err(|_| 501)?
                .ok_or(701)?;
            Ok(BrowseResult {
                didl: didl_wrap(&item_xml(&ep, public_base)),
                number_returned: 1,
                total_matches: 1,
            })
        }
    }
}

fn browse_metadata(
    conn: &Connection,
    id: &ObjectId,
    public_base: &str,
) -> Result<BrowseResult, i32> {
    let xml = match id {
        ObjectId::Root => container_xml(
            ROOT,
            "-1",
            "HIFI podcast",
            2,
            "object.container.storageFolder",
        ),
        ObjectId::Podcasts => {
            let shows = crate::db::list_upnp_podcasts(conn).map_err(|_| 501)?;
            let n = shows.len() as i64;
            container_xml(
                PODCASTS,
                ROOT,
                "Podcasts",
                n,
                "object.container.storageFolder",
            )
        }
        ObjectId::Music => container_xml(MUSIC, ROOT, "Music", 0, "object.container.storageFolder"),
        ObjectId::Show(pid) => {
            let eps = crate::db::list_upnp_episodes(conn, *pid).map_err(|_| 501)?;
            let title = eps
                .first()
                .map(|e| e.podcast_title.as_str())
                .unwrap_or("Podcast");
            container_xml(
                &show_id(*pid),
                PODCASTS,
                title,
                eps.len() as i64,
                "object.container.album.musicAlbum",
            )
        }
        ObjectId::Episode(eid) => {
            let ep = crate::db::get_upnp_episode(conn, *eid)
                .map_err(|_| 501)?
                .ok_or(701)?;
            item_xml(&ep, public_base)
        }
    };
    Ok(BrowseResult {
        didl: didl_wrap(&xml),
        number_returned: 1,
        total_matches: 1,
    })
}

pub fn extract_soap_arg(body: &str, name: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let name_l = name.to_ascii_lowercase();
    let mut search_from = 0;
    while search_from < lower.len() {
        let rel = lower[search_from..]
            .find(&name_l)
            .map(|i| search_from + i)?;
        let tag_start_ok = rel == 0
            || lower.as_bytes().get(rel - 1).copied() == Some(b'<')
            || lower.as_bytes().get(rel - 1).copied() == Some(b':');
        if !tag_start_ok {
            search_from = rel + name_l.len();
            continue;
        }
        if let Some(gt) = body[rel..].find('>') {
            let start = rel + gt + 1;
            if start < body.len() && body[rel..start].contains('/') {
                search_from = start;
                continue;
            }
            if let Some(end_rel) = lower[start..].find("</") {
                return Some(unescape_xml(body[start..start + end_rel].trim()));
            }
        }
        search_from = rel + name_l.len();
    }
    None
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

pub fn soap_ok(action: &str, service: &str, inner: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action}Response xmlns:u="{service}">{inner}</u:{action}Response></s:Body></s:Envelope>"#
    )
}

pub fn soap_fault(code: i32, description: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring><detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>{code}</errorCode><errorDescription>{}</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#,
        xml_escape(description)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use rusqlite::Connection;

    fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        let pid = db::upsert_podcast(
            &conn,
            "https://example.com/feed.xml",
            "Alpha Show",
            Some("Ann"),
            None,
            None,
            None,
        )
        .unwrap();
        let eid = db::upsert_episode(
            &conn,
            pid,
            "g1",
            "Episode & One",
            None,
            Some("https://example.com/a.mp3"),
            Some("2024-01-02T00:00:00Z"),
            Some(65_000),
        )
        .unwrap();
        db::upsert_download_state(
            &conn,
            eid,
            "done",
            Some("/media/podcasts/1/1.mp3"),
            Some(1000),
            None,
            None,
            Some("t"),
            Some("audio/mpeg"),
        )
        .unwrap();
        conn
    }

    #[test]
    fn parses_object_ids() {
        assert_eq!(parse_object_id("0"), Some(ObjectId::Root));
        assert_eq!(parse_object_id("podcasts"), Some(ObjectId::Podcasts));
        assert_eq!(parse_object_id("music"), Some(ObjectId::Music));
        assert_eq!(parse_object_id("show:12"), Some(ObjectId::Show(12)));
        assert_eq!(parse_object_id("ep:9"), Some(ObjectId::Episode(9)));
        assert!(parse_object_id("../etc").is_none());
    }

    #[test]
    fn root_has_podcasts_and_empty_music() {
        let conn = seed();
        let r = browse(&conn, "0", false, 0, 0, "http://10.0.0.5:8200").unwrap();
        assert_eq!(r.total_matches, 2);
        assert!(r.didl.contains(">Podcasts<"));
        assert!(r.didl.contains(">Music<"));
        assert!(r.didl.contains("childCount=\"0\""));
    }

    #[test]
    fn episode_res_uses_sqlite_title_and_mime() {
        let conn = seed();
        let shows = db::list_upnp_podcasts(&conn).unwrap();
        let r = browse(
            &conn,
            &show_id(shows[0].0),
            false,
            0,
            10,
            "http://10.0.0.5:8200",
        )
        .unwrap();
        assert_eq!(r.number_returned, 1);
        assert!(r.didl.contains("Episode &amp; One"));
        assert!(r.didl.contains("audio/mpeg"));
        assert!(r.didl.contains("/media/episode/"));
        assert!(r.didl.contains("duration=\"0:01:05.000\""));
        assert!(!r.didl.contains("1.mp3</dc:title>"));
    }

    #[test]
    fn soap_arg_with_namespace() {
        let body = r#"<s:Body><u:Browse xmlns:u="urn:schemas-upnp-org:service:ContentDirectory:1"><ObjectID>podcasts</ObjectID><BrowseFlag>BrowseDirectChildren</BrowseFlag></u:Browse></s:Body>"#;
        assert_eq!(
            extract_soap_arg(body, "ObjectID").as_deref(),
            Some("podcasts")
        );
        assert_eq!(
            extract_soap_arg(body, "BrowseFlag").as_deref(),
            Some("BrowseDirectChildren")
        );
    }

    #[test]
    fn duration_format() {
        assert_eq!(upnp_duration(0), "0:00:00.000");
        assert_eq!(upnp_duration(3_661_007), "1:01:01.007");
    }
}
