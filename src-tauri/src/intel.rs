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
    "i samarbete med",
    "reklam",
    "sponsrad av",
    "sponsor",
    "midroll",
    "kod för rabatt",
    "annons",
];

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
    }

    #[test]
    fn whisper_json_segments_seconds() {
        let json = r#"{"segments":[{"start":1.5,"end":3.0,"text":" hej "}]}"#;
        let segs = parse_whisper_json(json).unwrap();
        assert_eq!(segs[0].start_ms, 1500);
        assert_eq!(segs[0].text, "hej");
    }
}
