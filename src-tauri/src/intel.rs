//! Lokal intelligens: Whisper-parse, kapitel, reklamheuristik. Ingen moln-ASR.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterDraft {
    pub start_ms: i64,
    pub end_ms: Option<i64>,
    pub title: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdDraft {
    pub start_ms: i64,
    pub end_ms: i64,
    pub confidence: f64,
    pub method: String,
}

pub fn parse_srt(srt: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let blocks = srt.replace('\r', "");
    for block in blocks.split("\n\n") {
        let lines: Vec<&str> = block
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() < 2 {
            continue;
        }
        let time_line = if lines[0].contains("-->") {
            lines[0]
        } else if lines.len() >= 2 && lines[1].contains("-->") {
            lines[1]
        } else {
            continue;
        };
        let Some((start, end)) = parse_srt_range(time_line) else {
            continue;
        };
        let text_start = if lines[0].contains("-->") { 1 } else { 2 };
        let text = lines[text_start..].join(" ").trim().to_string();
        if !text.is_empty() {
            out.push(Segment {
                start_ms: start,
                end_ms: end,
                text,
            });
        }
    }
    out
}

fn parse_srt_range(line: &str) -> Option<(i64, i64)> {
    let mut parts = line.split("-->");
    let a = parts.next()?.trim();
    let b = parts.next()?.trim().split(' ').next()?.trim();
    Some((parse_srt_clock(a)?, parse_srt_clock(b)?))
}

fn parse_srt_clock(raw: &str) -> Option<i64> {
    let raw = raw.replace(',', ".");
    let (hms, frac) = raw.split_once('.').unwrap_or((raw.as_str(), "0"));
    let mut it = hms.split(':');
    let h: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let s: i64 = it.next()?.parse().ok()?;
    let ms_str: String = frac.chars().take(3).collect();
    let ms: i64 = format!("{:0<3}", ms_str).parse().ok()?;
    Some(((h * 3600 + m * 60 + s) * 1000) + ms)
}

/// Whisper.cpp JSON (`-oj`) plus enklare `{segments:[{start,end,text}]}` (sekunder).
pub fn parse_whisper_json(json: &str) -> Result<Vec<Segment>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("ogiltig whisper-json: {e}"))?;
    if let Some(arr) = value.get("transcription").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for item in arr {
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let ts = item.get("timestamps");
            let start = ts
                .and_then(|t| t.get("from"))
                .and_then(|v| v.as_str())
                .and_then(parse_srt_clock)
                .unwrap_or(0);
            let end = ts
                .and_then(|t| t.get("to"))
                .and_then(|v| v.as_str())
                .and_then(parse_srt_clock)
                .unwrap_or(start);
            if !text.is_empty() {
                out.push(Segment {
                    start_ms: start,
                    end_ms: end.max(start),
                    text,
                });
            }
        }
        return Ok(out);
    }
    if let Some(arr) = value.get("segments").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for item in arr {
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let start = json_seconds(item.get("start"));
            let end = json_seconds(item.get("end"));
            if !text.is_empty() {
                out.push(Segment {
                    start_ms: start,
                    end_ms: end.max(start),
                    text,
                });
            }
        }
        return Ok(out);
    }
    Err("json saknar transcription/segments".into())
}

fn json_seconds(v: Option<&serde_json::Value>) -> i64 {
    match v {
        Some(serde_json::Value::Number(n)) => (n.as_f64().unwrap_or(0.0) * 1000.0).round() as i64,
        Some(serde_json::Value::String(s)) => parse_srt_clock(s).unwrap_or(0),
        _ => 0,
    }
}

pub fn generate_chapters(segments: &[Segment]) -> Vec<ChapterDraft> {
    if segments.is_empty() {
        return Vec::new();
    }
    let mut chapters = Vec::new();
    let mut start = segments[0].start_ms;
    let mut title = title_from(&segments[0].text);
    let mut last_end = segments[0].end_ms;
    let mut acc = 0_i64;
    for pair in segments.windows(2) {
        let a = &pair[0];
        let b = &pair[1];
        let gap = (b.start_ms - a.end_ms).max(0);
        acc += (a.end_ms - a.start_ms).max(0);
        if gap >= 2500 || acc >= 6 * 60 * 1000 {
            chapters.push(ChapterDraft {
                start_ms: start,
                end_ms: Some(a.end_ms),
                title,
                source: "generated".into(),
            });
            start = b.start_ms;
            title = title_from(&b.text);
            acc = 0;
        }
        last_end = b.end_ms;
    }
    chapters.push(ChapterDraft {
        start_ms: start,
        end_ms: Some(last_end),
        title,
        source: "generated".into(),
    });
    chapters
}

fn title_from(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() <= 48 {
        return t.to_string();
    }
    let mut out = String::new();
    for (i, ch) in t.chars().enumerate() {
        if i >= 45 {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

const AD_PHRASES: &[&str] = &[
    "this podcast is sponsored",
    "today's episode is brought to you",
    "brought to you by",
    "use code",
    "promo code",
    "i samarbete med",
    "sponsrad av",
    "sponsored by",
    "our sponsor",
    "midroll",
    "kod för rabatt",
    "reklaminslag",
    "reklampaus",
];

/// RMS under this is silence. Same ballpark as opt-in skip-silence, and it
/// does not need a playback device.
pub const SILENCE_RMS: f32 = 0.012;
pub const SILENCE_GAP_MIN_MS: i64 = 500;
const AD_BLOCK_MIN_MS: i64 = 12_000;
const AD_BLOCK_MAX_MS: i64 = 90_000;
const AD_NEAR_START_MS: i64 = 120_000;
const AD_NEAR_END_MS: i64 = 90_000;

#[derive(Debug, Clone, PartialEq)]
pub struct RmsWindow {
    pub start_ms: i64,
    pub end_ms: i64,
    pub rms: f32,
}

pub fn rms_of(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Window interleaved PCM into RMS blocks. No device and no file.
pub fn rms_windows_from_pcm(
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    window_ms: i64,
) -> Vec<RmsWindow> {
    if sample_rate == 0 || channels == 0 || window_ms <= 0 || samples.is_empty() {
        return Vec::new();
    }
    let ch = channels as usize;
    let frames = ((sample_rate as i64 * window_ms) / 1000).max(1) as usize;
    let window = frames.saturating_mul(ch);
    if window == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    let mut frame_at = 0i64;
    while offset + window <= samples.len() {
        let rms = rms_of(&samples[offset..offset + window]);
        let start_ms = frame_at.saturating_mul(1000) / i64::from(sample_rate);
        frame_at += frames as i64;
        let end_ms = frame_at.saturating_mul(1000) / i64::from(sample_rate);
        out.push(RmsWindow {
            start_ms,
            end_ms,
            rms,
        });
        offset += window;
    }
    out
}

/// Merge windows below `threshold` into gaps of at least [`SILENCE_GAP_MIN_MS`].
pub fn silence_gaps(windows: &[RmsWindow], threshold: f32) -> Vec<(i64, i64)> {
    let mut gaps = Vec::new();
    let mut open: Option<(i64, i64)> = None;
    for w in windows {
        if w.end_ms <= w.start_ms {
            continue;
        }
        if w.rms < threshold {
            open = Some(match open {
                Some((start, _)) => (start, w.end_ms),
                None => (w.start_ms, w.end_ms),
            });
        } else if let Some((start, end)) = open.take() {
            if end - start >= SILENCE_GAP_MIN_MS {
                gaps.push((start, end));
            }
        }
    }
    if let Some((start, end)) = open {
        if end - start >= SILENCE_GAP_MIN_MS {
            gaps.push((start, end));
        }
    }
    gaps
}

/// Ad-sized blocks bounded by silence, near the start, the outro, or mid-roll.
/// Typical sponsor reads are about 15, 30, or 60 seconds.
pub fn detect_silence_ad_blocks(gaps: &[(i64, i64)], duration_ms: i64) -> Vec<AdDraft> {
    if duration_ms <= 0 {
        return Vec::new();
    }
    let gaps = normalized_gaps(gaps, duration_ms);
    let mut regions: Vec<(i64, i64, bool)> = Vec::new();
    let mut cursor = 0i64;
    let mut gap_before = false;
    for (gs, ge) in &gaps {
        if *gs > cursor {
            regions.push((cursor, *gs, true));
        }
        cursor = (*ge).max(cursor);
        gap_before = true;
    }
    if cursor < duration_ms {
        regions.push((cursor, duration_ms, gap_before));
    }
    let mut ads = Vec::new();
    for (start, end, bounded) in regions {
        let len = end - start;
        if !bounded || len < AD_BLOCK_MIN_MS || len > AD_BLOCK_MAX_MS {
            continue;
        }
        if !ad_position(start, end, duration_ms) {
            continue;
        }
        let near_standard = [15_000i64, 30_000, 60_000]
            .iter()
            .any(|n| (len - n).abs() <= 3_000);
        ads.push(AdDraft {
            start_ms: start,
            end_ms: end,
            confidence: if near_standard { 0.7 } else { 0.5 },
            method: "silence".into(),
        });
    }
    ads
}

fn normalized_gaps(gaps: &[(i64, i64)], duration_ms: i64) -> Vec<(i64, i64)> {
    let mut gaps: Vec<(i64, i64)> = gaps
        .iter()
        .map(|(a, b)| ((*a).max(0), (*b).min(duration_ms)))
        .filter(|(a, b)| *b - *a >= SILENCE_GAP_MIN_MS)
        .collect();
    gaps.sort_by_key(|(a, b)| (*a, *b));
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in gaps {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

fn ad_position(start: i64, end: i64, duration_ms: i64) -> bool {
    if start < AD_NEAR_START_MS {
        return true;
    }
    if start >= duration_ms.saturating_sub(AD_NEAR_END_MS) {
        return true;
    }
    let center = start + (end - start) / 2;
    let mid = duration_ms / 2;
    (center - mid).abs() <= (duration_ms * 12) / 100
}

pub fn dedupe_ad_drafts(mut ads: Vec<AdDraft>) -> Vec<AdDraft> {
    ads.sort_by(|a, b| a.start_ms.cmp(&b.start_ms).then(a.end_ms.cmp(&b.end_ms)));
    let mut out: Vec<AdDraft> = Vec::new();
    for ad in ads {
        if ad.end_ms <= ad.start_ms {
            continue;
        }
        if let Some(prev) = out.last_mut() {
            let overlap = prev.end_ms.min(ad.end_ms) - prev.start_ms.max(ad.start_ms);
            let shorter = (prev.end_ms - prev.start_ms).min(ad.end_ms - ad.start_ms);
            if overlap > 0 && shorter > 0 && overlap * 2 >= shorter {
                if ad.confidence > prev.confidence {
                    prev.confidence = ad.confidence;
                    prev.method = ad.method.clone();
                }
                prev.start_ms = prev.start_ms.min(ad.start_ms);
                prev.end_ms = prev.end_ms.max(ad.end_ms);
                continue;
            }
        }
        out.push(ad);
    }
    out
}

pub fn detect_ads(segments: &[Segment]) -> Vec<AdDraft> {
    let mut ads = Vec::new();
    for (i, seg) in segments.iter().enumerate() {
        let hay = seg.text.to_lowercase();
        if AD_PHRASES.iter().any(|p| hay.contains(p)) {
            ads.push(AdDraft {
                start_ms: seg.start_ms,
                end_ms: seg.end_ms.max(seg.start_ms + 1000),
                confidence: 0.82,
                method: "phrase".into(),
            });
            continue;
        }
        if i + 1 < segments.len() {
            let next = &segments[i + 1];
            let gap = next.start_ms - seg.end_ms;
            if gap >= 1500 && gap <= 8000 {
                let around = format!("{} {}", hay, next.text.to_lowercase());
                if AD_PHRASES.iter().any(|p| around.contains(p)) {
                    ads.push(AdDraft {
                        start_ms: seg.end_ms,
                        end_ms: next.start_ms,
                        confidence: 0.55,
                        method: "silence".into(),
                    });
                }
            }
        }
    }
    ads
}

pub fn read_id3_chapters(path: &std::path::Path) -> Vec<ChapterDraft> {
    let Ok(tag) = id3::Tag::read_from_path(path) else {
        return Vec::new();
    };
    tag.chapters()
        .filter_map(|ch| {
            let title = ch
                .frames
                .iter()
                .find_map(|f| f.content().text())
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Kapitel".into());
            Some(ChapterDraft {
                start_ms: i64::from(ch.start_time),
                end_ms: Some(i64::from(ch.end_time)),
                title,
                source: "embedded".into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_and_chapters_and_ads() {
        let srt = "1\n00:00:00,000 --> 00:00:04,000\nVälkommen till showen\n\n2\n00:00:08,000 --> 00:00:12,000\nThis podcast is sponsored by Acme\n\n3\n00:00:20,000 --> 00:00:25,000\nNu kör vi vidare\n";
        let segs = parse_srt(srt);
        assert_eq!(segs.len(), 3);
        let chapters = generate_chapters(&segs);
        assert!(chapters.len() >= 2);
        let ads = detect_ads(&segs);
        assert!(ads.iter().any(|a| a.method == "phrase"));
        let false_pos = vec![Segment {
            start_ms: 0,
            end_ms: 1000,
            text: "Det var ingen reklam i slutet av samtalet".into(),
        }];
        assert!(detect_ads(&false_pos).is_empty());
        let sponsor = vec![Segment {
            start_ms: 0,
            end_ms: 1000,
            text: "Avsnittet är sponsrad av Acme".into(),
        }];
        assert!(!detect_ads(&sponsor).is_empty());
    }

    #[test]
    fn silence_blocks_detected_without_a_device() {
        let duration = 600_000;
        let gaps = vec![(8_000, 8_800), (38_800, 39_600)];
        let ads = detect_silence_ad_blocks(&gaps, duration);
        assert_eq!(ads.len(), 1, "{ads:?}");
        assert_eq!(ads[0].method, "silence");
        assert_eq!(ads[0].start_ms, 8_800);
        assert_eq!(ads[0].end_ms, 38_800);
        assert!(ads[0].confidence >= 0.7);

        let tiny = vec![(8_000, 8_200), (38_200, 38_400)];
        assert!(detect_silence_ad_blocks(&tiny, duration).is_empty());

        let off_position = vec![(180_000, 180_800), (210_800, 211_600)];
        assert!(
            detect_silence_ad_blocks(&off_position, duration).is_empty(),
            "a pause away from start, outro and mid-roll is not an ad"
        );
        assert!(detect_silence_ad_blocks(&[], duration).is_empty());

        let rate = 8_000u32;
        let mut samples = Vec::new();
        for n in 0..rate {
            samples.push((n as f32 * 0.05).sin() * 0.4);
        }
        samples.extend(std::iter::repeat(0.0f32).take((rate as usize * 7) / 10));
        for n in 0..rate {
            samples.push((n as f32 * 0.05).sin() * 0.4);
        }
        let windows = rms_windows_from_pcm(&samples, rate, 1, 100);
        let found = silence_gaps(&windows, SILENCE_RMS);
        assert!(
            found.iter().any(|(start, end)| end - start >= 500),
            "{found:?}"
        );
    }

    #[test]
    fn whisper_json_segments_seconds() {
        let json = r#"{"segments":[{"start":1.5,"end":3.0,"text":" hej "}]}"#;
        let segs = parse_whisper_json(json).unwrap();
        assert_eq!(segs[0].start_ms, 1500);
        assert_eq!(segs[0].text, "hej");
    }
}
