//! Which episodes to auto-download after a feed fetch.
//!
//! The choice is only about episode ids already stored for this fetch.
//! Audio URLs are not read or fetched here. A queued id is downloaded later
//! through `download_episode_inner`, which keeps the validated HTTP path.

use std::collections::HashSet;

/// An episode that was already stored before the current fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownEpisode {
    pub guid: String,
    /// True when a completed download row exists. False is a missing file.
    pub file_present: bool,
}

/// One episode from the fetch that was just upserted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedEpisode {
    pub episode_id: i64,
    pub guid: String,
    pub title: String,
    pub has_audio_url: bool,
}

/// An episode id that should be downloaded. No URL is carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedAutoDownload {
    pub episode_id: i64,
    pub guid: String,
    pub title: String,
}

/// Queue only episodes that were absent from the previous fetch.
///
/// An older episode is not queued when its file is missing. Turning
/// auto-download off queues nothing.
pub fn select_auto_downloads(
    auto_download: bool,
    known_before_fetch: &[KnownEpisode],
    fetched: &[FetchedEpisode],
) -> Vec<QueuedAutoDownload> {
    if !auto_download {
        return Vec::new();
    }

    let mut known = HashSet::new();
    for episode in known_before_fetch {
        // Membership is enough. `file_present == false` still means "already seen",
        // so an older missing file is not queued.
        known.insert(episode.guid.as_str());
    }

    let mut seen_ids = HashSet::new();
    let mut seen_guids = HashSet::new();
    let mut queued = Vec::new();
    for episode in fetched {
        if episode.guid.is_empty() || !episode.has_audio_url {
            continue;
        }
        // Already stored on a previous fetch. A missing file is not enough.
        if known.contains(episode.guid.as_str()) {
            continue;
        }
        if !seen_guids.insert(episode.guid.clone()) || !seen_ids.insert(episode.episode_id) {
            continue;
        }
        queued.push(QueuedAutoDownload {
            episode_id: episode.episode_id,
            guid: episode.guid.clone(),
            title: episode.title.clone(),
        });
    }
    queued
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::PodcastSettings;

    fn fetched(id: i64, guid: &str, title: &str, has_audio_url: bool) -> FetchedEpisode {
        FetchedEpisode {
            episode_id: id,
            guid: guid.into(),
            title: title.into(),
            has_audio_url,
        }
    }

    fn known(guid: &str, file_present: bool) -> KnownEpisode {
        KnownEpisode {
            guid: guid.into(),
            file_present,
        }
    }

    #[test]
    fn queues_only_episodes_new_since_the_previous_fetch() {
        let known_before = vec![known("old-missing", false), known("old-present", true)];
        let fetched_now = vec![
            fetched(1, "old-missing", "Gammalt utan fil", true),
            fetched(2, "old-present", "Gammalt med fil", true),
            fetched(3, "brand-new", "Nytt avsnitt", true),
            fetched(4, "brand-new", "Nytt avsnitt", true),
            fetched(5, "no-audio", "Utan ljud", false),
        ];

        let queued = select_auto_downloads(true, &known_before, &fetched_now);

        assert_eq!(
            queued,
            vec![QueuedAutoDownload {
                episode_id: 3,
                guid: "brand-new".into(),
                title: "Nytt avsnitt".into(),
            }]
        );
    }

    #[test]
    fn older_missing_file_is_not_queued() {
        let known_before = vec![known("old-missing", false)];
        let fetched_now = vec![fetched(11, "old-missing", "Gammalt utan fil", true)];

        let queued = select_auto_downloads(true, &known_before, &fetched_now);

        assert!(queued.is_empty());
        assert!(!queued.iter().any(|item| item.guid == "old-missing"));
    }

    #[test]
    fn auto_download_off_queues_nothing_even_for_new_episodes() {
        let fetched_now = vec![fetched(3, "brand-new", "Nytt avsnitt", true)];
        assert!(select_auto_downloads(false, &[], &fetched_now).is_empty());
    }

    #[test]
    fn second_fetch_leaves_the_older_missing_file_and_queues_the_new_episode() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        let podcast_id = db::upsert_podcast(
            &conn,
            "https://1.1.1.1/show.xml",
            "Podden",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        db::update_podcast_settings(
            &conn,
            podcast_id,
            &PodcastSettings {
                auto_download: true,
                speed: 1.0,
                intro_skip_sec: 0,
                outro_skip_sec: 0,
                skip_silence: false,
                asr_language: None,
                skip_ads: false,
                auto_transcribe: false,
            },
        )
        .unwrap();
        let old_id = db::upsert_subscribed_episode(
            &conn,
            podcast_id,
            "old-missing",
            "Gammalt utan fil",
            None,
            Some("https://1.1.1.1/old.mp3"),
            Some("2020-01-01T00:00:00Z"),
            None,
        )
        .unwrap();
        assert!(!db::download_is_done(&conn, old_id));

        let known_before = db::episodes_seen_before_fetch(&conn, podcast_id).unwrap();
        assert_eq!(
            known_before,
            vec![db::EpisodeSeenBeforeFetch {
                guid: "old-missing".into(),
                file_present: false,
            }]
        );

        let old_again = db::upsert_subscribed_episode(
            &conn,
            podcast_id,
            "old-missing",
            "Gammalt utan fil",
            None,
            Some("https://1.1.1.1/old.mp3"),
            Some("2020-01-01T00:00:00Z"),
            None,
        )
        .unwrap();
        let new_id = db::upsert_subscribed_episode(
            &conn,
            podcast_id,
            "brand-new",
            "Nytt avsnitt",
            None,
            Some("https://1.1.1.1/new.mp3"),
            Some("2026-09-01T00:00:00Z"),
            None,
        )
        .unwrap();
        assert_eq!(old_again, old_id);

        let known = known_before
            .iter()
            .map(|row| known(&row.guid, row.file_present))
            .collect::<Vec<_>>();
        let fetched_now = vec![
            fetched(old_id, "old-missing", "Gammalt utan fil", true),
            fetched(new_id, "brand-new", "Nytt avsnitt", true),
        ];
        let auto_download = db::get_podcast(&conn, podcast_id).unwrap().auto_download;
        let queued = select_auto_downloads(auto_download, &known, &fetched_now);

        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].episode_id, new_id);
        assert_eq!(queued[0].guid, "brand-new");
        assert!(!queued.iter().any(|item| item.episode_id == old_id));
        assert!(!db::download_is_done(&conn, old_id));
    }
}
