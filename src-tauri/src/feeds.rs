use crate::error::{AppError, AppResult};
use rss::Channel;

#[derive(Debug, Clone)]
pub struct ParsedFeed {
    pub title: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub guid: Option<String>,
    pub episodes: Vec<ParsedEpisode>,
}

#[derive(Debug, Clone)]
pub struct ParsedEpisode {
    pub guid: String,
    pub title: String,
    pub description: Option<String>,
    pub audio_url: Option<String>,
    pub publish_at: Option<String>,
    pub duration_ms: Option<i64>,
}

pub fn parse_feed(xml: &str, feed_url: &str) -> AppResult<ParsedFeed> {
    let channel = Channel::read_from(xml.as_bytes())?;
    let title = if channel.title.trim().is_empty() {
        feed_url.to_string()
    } else {
        channel.title.clone()
    };
    let author = channel
        .itunes_ext
        .as_ref()
        .and_then(|ext| ext.author.clone())
        .or(channel.managing_editor.clone());
    let language = channel.language.clone();
    let description = if channel.description.trim().is_empty() {
        None
    } else {
        Some(channel.description.clone())
    };

    let mut episodes = Vec::new();
    for (index, item) in channel.items.iter().enumerate() {
        let title = item
            .title
            .clone()
            .unwrap_or_else(|| format!("Avsnitt {}", index + 1));
        let guid = item
            .guid
            .as_ref()
            .map(|g| g.value.clone())
            .or_else(|| item.link.clone())
            .or_else(|| {
                item.enclosure
                    .as_ref()
                    .map(|enc| format!("{}::{}", feed_url, enc.url))
            })
            .unwrap_or_else(|| format!("{}::{}", feed_url, title));
        let audio_url = item.enclosure.as_ref().map(|enc| enc.url.clone());
        let publish_at = item.pub_date.as_ref().and_then(|raw| {
            chrono::DateTime::parse_from_rfc2822(raw)
                .ok()
                .map(|dt| dt.with_timezone(&chrono::Utc).to_rfc3339())
                .or_else(|| Some(raw.clone()))
        });
        let duration_ms = item
            .itunes_ext
            .as_ref()
            .and_then(|ext| ext.duration.as_ref())
            .and_then(|d| parse_itunes_duration(d));
        episodes.push(ParsedEpisode {
            guid,
            title,
            description: item.description.clone(),
            audio_url,
            publish_at,
            duration_ms,
        });
    }

    Ok(ParsedFeed {
        title,
        author,
        description,
        language,
        guid: Some(feed_url.to_string()),
        episodes,
    })
}

fn parse_itunes_duration(raw: &str) -> Option<i64> {
    let parts: Vec<&str> = raw.split(':').collect();
    let nums: Option<Vec<i64>> = parts.iter().map(|p| p.parse().ok()).collect();
    let nums = nums?;
    let seconds = match nums.as_slice() {
        [s] => *s,
        [m, s] => m * 60 + s,
        [h, m, s] => h * 3600 + m * 60 + s,
        _ => return None,
    };
    Some(seconds.saturating_mul(1000))
}

pub async fn fetch_text(url: &str) -> AppResult<String> {
    let client = reqwest::Client::builder()
        .user_agent("HIFI-podcast/0.1 (+https://github.com/fagerhiertaconsulting/hifi-podcast)")
        .build()?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(AppError::msg(format!(
            "Kunde inte hämta feed (HTTP {}).",
            response.status()
        )));
    }
    Ok(response.text().await?)
}

pub fn import_opml_urls(xml: &str) -> AppResult<Vec<String>> {
    let document = opml::OPML::from_str(xml)?;
    let mut urls = Vec::new();
    collect_xml_urls(&document.body.outlines, &mut urls);
    urls.sort();
    urls.dedup();
    if urls.is_empty() {
        return Err(AppError::msg("OPML-filen innehåller inga RSS-adresser."));
    }
    Ok(urls)
}

fn collect_xml_urls(outlines: &[opml::Outline], urls: &mut Vec<String>) {
    for outline in outlines {
        if let Some(url) = outline.xml_url.clone() {
            if !url.trim().is_empty() {
                urls.push(url.trim().to_string());
            }
        }
        collect_xml_urls(&outline.outlines, urls);
    }
}

pub fn export_opml(feeds: &[(String, String)]) -> AppResult<String> {
    let outlines = feeds
        .iter()
        .map(|(title, url)| {
            let mut outline = opml::Outline::default();
            outline.text = title.clone();
            outline.title = Some(title.clone());
            outline.xml_url = Some(url.clone());
            outline
        })
        .collect();
    let document = opml::OPML {
        version: "2.0".into(),
        head: Some(opml::Head {
            title: Some("HIFI podcast".into()),
            ..opml::Head::default()
        }),
        body: opml::Body { outlines },
    };
    Ok(document.to_string()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rss_enclosure_and_guid() {
        let xml = r#"<?xml version="1.0"?>
        <rss version="2.0">
          <channel>
            <title>HiFi Test</title>
            <description>Desc</description>
            <item>
              <title>Ep 1</title>
              <guid>abc</guid>
              <pubDate>Mon, 01 Jan 2024 00:00:00 GMT</pubDate>
              <enclosure url="https://cdn.example.com/ep1.mp3" length="10" type="audio/mpeg"/>
            </item>
          </channel>
        </rss>"#;
        let feed = parse_feed(xml, "https://example.com/feed.xml").unwrap();
        assert_eq!(feed.title, "HiFi Test");
        assert_eq!(feed.episodes.len(), 1);
        assert_eq!(feed.episodes[0].audio_url.as_deref(), Some("https://cdn.example.com/ep1.mp3"));
    }

    #[test]
    fn parses_nested_opml() {
        let xml = r#"<?xml version="1.0"?>
        <opml version="2.0">
          <head><title>t</title></head>
          <body>
            <outline text="folder">
              <outline text="A" type="rss" xmlUrl="https://a.example/feed.xml"/>
            </outline>
            <outline text="B" xmlUrl="https://b.example/feed.xml"/>
          </body>
        </opml>"#;
        let urls = import_opml_urls(xml).unwrap();
        assert_eq!(urls.len(), 2);
    }
}
