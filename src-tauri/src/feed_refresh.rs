//! Automatic feed refresh at startup and every 60 minutes.
//!
//! Only feed URLs already stored as subscriptions are eligible. The download
//! selection inside `subscribe_url` is left unchanged.

use crate::commands;
use crate::db;
use crate::error::AppResult;
use crate::AppState;
use serde::Serialize;
use std::future::Future;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

/// How often stored subscriptions are refreshed while the app is running.
pub const FEED_REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);

const MAX_WARNINGS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSubscription {
    pub podcast_id: i64,
    pub feed_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedRefreshReport {
    pub refreshed: usize,
    pub skipped: usize,
    pub warnings: Vec<String>,
}

pub fn targets_from_conn(conn: &rusqlite::Connection) -> AppResult<Vec<StoredSubscription>> {
    Ok(db::list_podcasts(conn)?
        .into_iter()
        .map(|podcast| StoredSubscription {
            podcast_id: podcast.id,
            feed_url: podcast.feed_url,
        })
        .collect())
}

/// A candidate may be fetched only when it is exactly a stored subscription
/// and the existing HTTP/SSRF check accepts it.
pub fn decide_fetch(
    candidate: &StoredSubscription,
    stored: &[StoredSubscription],
) -> Result<(), String> {
    let listed = stored
        .iter()
        .any(|item| item.podcast_id == candidate.podcast_id && item.feed_url == candidate.feed_url);
    if !listed || candidate.feed_url.trim().is_empty() {
        return Err("Endast sparade prenumerationer får hämtas.".into());
    }
    crate::net::validate_http_url(&candidate.feed_url).map_err(|err| err.to_string())?;
    Ok(())
}

pub fn partition_targets(stored: &[StoredSubscription]) -> (Vec<StoredSubscription>, Vec<String>) {
    let mut ready = Vec::new();
    let mut warnings = Vec::new();
    for item in stored {
        if item.feed_url.trim().is_empty() {
            push_warning(
                &mut warnings,
                "En prenumeration saknar flödesadress och hoppades över.".into(),
            );
            continue;
        }
        match decide_fetch(item, stored) {
            Ok(()) => ready.push(item.clone()),
            Err(err) => push_warning(&mut warnings, format!("{}: {err}", item.feed_url)),
        }
    }
    (ready, warnings)
}

pub async fn apply_refresh<F, Fut>(
    stored: &[StoredSubscription],
    mut fetch_one: F,
) -> FeedRefreshReport
where
    F: FnMut(&StoredSubscription) -> Fut,
    Fut: Future<Output = Result<Vec<String>, String>>,
{
    let (ready, mut warnings) = partition_targets(stored);
    let skipped = stored.len().saturating_sub(ready.len());
    let mut refreshed = 0usize;
    for target in &ready {
        match fetch_one(target).await {
            Ok(extra) => {
                refreshed += 1;
                for warning in extra {
                    push_warning(&mut warnings, warning);
                }
            }
            Err(err) => push_warning(&mut warnings, format!("{}: {err}", target.feed_url)),
        }
    }
    FeedRefreshReport {
        refreshed,
        skipped,
        warnings,
    }
}

/// Refresh once immediately, then wait [`FEED_REFRESH_INTERVAL`] between refreshes.
pub async fn run_refresh_schedule<R, RF, S, SF, C>(
    mut refresh: R,
    mut sleep: S,
    mut keep_running: C,
) where
    R: FnMut() -> RF,
    RF: Future<Output = ()>,
    S: FnMut(Duration) -> SF,
    SF: Future<Output = ()>,
    C: FnMut() -> bool,
{
    loop {
        if !keep_running() {
            break;
        }
        refresh().await;
        if !keep_running() {
            break;
        }
        sleep(FEED_REFRESH_INTERVAL).await;
    }
}

pub async fn refresh_all_stored(state: &AppState) -> FeedRefreshReport {
    let stored = match {
        let conn = state.db.lock().expect("databas-mutex");
        targets_from_conn(&conn)
    } {
        Ok(stored) => stored,
        Err(err) => {
            return FeedRefreshReport {
                refreshed: 0,
                skipped: 0,
                warnings: vec![err.to_string()],
            };
        }
    };
    apply_refresh(&stored, |target| {
        let podcast_id = target.podcast_id;
        let expected_url = target.feed_url.clone();
        async move {
            let current_url = {
                let conn = state.db.lock().expect("databas-mutex");
                match db::get_podcast(&conn, podcast_id) {
                    Ok(podcast) => podcast.feed_url,
                    Err(err) => return Err(err.to_string()),
                }
            };
            if current_url != expected_url {
                return Err("Flödesadressen matchar inte den sparade prenumerationen.".into());
            }
            let outcome = commands::refresh_one_stored(state, podcast_id)
                .await
                .map_err(|err| err.to_string())?;
            Ok(outcome
                .warnings
                .into_iter()
                .map(|warning| format!("{}: {warning}", outcome.podcast.title))
                .collect())
        }
    })
    .await
}

pub async fn auto_refresh_loop(app: AppHandle) {
    run_refresh_schedule(
        move || {
            let app = app.clone();
            async move {
                let state = app.state::<AppState>();
                let report = refresh_all_stored(&state).await;
                let _ = app.emit("feeds-refreshed", &report);
            }
        },
        |delay| async move {
            tokio::time::sleep(delay).await;
        },
        || true,
    )
    .await;
}

fn push_warning(warnings: &mut Vec<String>, text: String) {
    if warnings.len() >= MAX_WARNINGS {
        return;
    }
    let truncated = text.chars().count() > 240;
    let mut text: String = text.chars().take(240).collect();
    if truncated {
        text.push('…');
    }
    warnings.push(text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn interval_is_60_minutes() {
        assert_eq!(FEED_REFRESH_INTERVAL, Duration::from_secs(60 * 60));
        assert!(FEED_REFRESH_INTERVAL > Duration::from_secs(59 * 60));
    }

    #[tokio::test]
    async fn refreshes_at_startup_then_every_60_minutes() {
        let log = Arc::new(Mutex::new(Vec::<String>::new()));
        let remaining = Arc::new(AtomicUsize::new(3));
        let log_refresh = log.clone();
        let remaining_refresh = remaining.clone();
        let log_sleep = log.clone();
        let remaining_gate = remaining.clone();
        run_refresh_schedule(
            move || {
                let log_refresh = log_refresh.clone();
                let remaining_refresh = remaining_refresh.clone();
                async move {
                    remaining_refresh.fetch_sub(1, Ordering::SeqCst);
                    log_refresh.lock().unwrap().push("refresh".into());
                }
            },
            move |delay| {
                let log_sleep = log_sleep.clone();
                async move {
                    log_sleep
                        .lock()
                        .unwrap()
                        .push(format!("sleep:{}", delay.as_secs()));
                }
            },
            move || remaining_gate.load(Ordering::SeqCst) > 0,
        )
        .await;
        assert_eq!(
            log.lock().unwrap().as_slice(),
            [
                "refresh".to_string(),
                "sleep:3600".to_string(),
                "refresh".to_string(),
                "sleep:3600".to_string(),
                "refresh".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn startup_refresh_fetches_only_stored_subscriptions() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        db::upsert_podcast(&conn, "https://1.1.1.1/a.xml", "A", None, None, None, None).unwrap();
        db::upsert_podcast(&conn, "https://8.8.8.8/b.xml", "B", None, None, None, None).unwrap();
        db::upsert_podcast(
            &conn,
            "http://127.0.0.1/secret.xml",
            "Local",
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let stored = targets_from_conn(&conn).unwrap();
        assert_eq!(stored.len(), 3);

        let fetched = Arc::new(Mutex::new(Vec::<String>::new()));
        let fetched_cb = fetched.clone();
        let report = apply_refresh(&stored, move |target| {
            let fetched_cb = fetched_cb.clone();
            let url = target.feed_url.clone();
            async move {
                fetched_cb.lock().unwrap().push(url);
                Ok(Vec::new())
            }
        })
        .await;

        let mut got = fetched.lock().unwrap().clone();
        got.sort();
        assert_eq!(
            got,
            vec![
                "https://1.1.1.1/a.xml".to_string(),
                "https://8.8.8.8/b.xml".to_string(),
            ]
        );
        assert_eq!(report.refreshed, 2);
        assert_eq!(report.skipped, 1);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("127.0.0.1")));
        assert!(!report
            .warnings
            .iter()
            .any(|warning| warning.contains("1.1.1.1") || warning.contains("8.8.8.8")));

        let outsider = StoredSubscription {
            podcast_id: 99,
            feed_url: "https://9.9.9.9/not-stored.xml".into(),
        };
        let err = decide_fetch(&outsider, &stored).unwrap_err();
        assert!(err.contains("prenumeration"));
        assert!(!got.iter().any(|url| url.contains("9.9.9.9")));
        assert!(!got.iter().any(|url| url.contains("127.0.0.1")));
    }

    #[tokio::test]
    async fn startup_refresh_with_no_subscriptions_fetches_nothing() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::initialize(&conn).unwrap();
        let stored = targets_from_conn(&conn).unwrap();
        assert!(stored.is_empty());
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cb = calls.clone();
        let report = apply_refresh(&stored, move |_| {
            let calls_cb = calls_cb.clone();
            async move {
                calls_cb.fetch_add(1, Ordering::SeqCst);
                Ok(Vec::new())
            }
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(report.refreshed, 0);
        assert_eq!(report.skipped, 0);
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn blank_feed_url_is_not_fetched() {
        let stored = vec![StoredSubscription {
            podcast_id: 1,
            feed_url: "   ".into(),
        }];
        let (ready, warnings) = partition_targets(&stored);
        assert!(ready.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("saknar flödesadress"));
    }
}
