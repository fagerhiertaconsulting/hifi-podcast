# Arkitektur — HIFI podcast

## Faser

| Fas | Vad | Kod nu |
|-----|-----|--------|
| **MVP** | Lokal podcatcher + hi-fi-policy | Ja (skal) |
| **v1** | Lokal ASR, kapitel, reklam-skip | Roadmap |
| **Musik** | Separat modul, foobar2000-ambition | Roadmap |
| **UPnP** | Media Server mot Onkyo TX-RZ50 | Roadmap; media-root redan isolerad |

## Process

```
[React UI, svenska]  ←IPC→  [Rust / Tauri 2]
                              ├─ RSS + OPML
                              ├─ Download manager (en fil i taget, original)
                              ├─ SQLite
                              ├─ Playback-state + kö
                              └─ Jobbkön (v1: whisper, kapitel, ads)
```

UI ska inte avkoda feeds eller skriva SQLite direkt.

## Lagring

- DB: app local data `db.sqlite`
- Media-root: `media/<library>/<id>/…`

**Podcast-library** använder `media/<podcast_id>/<episode_id>.<ext>` i MVP. När musikmodulen kommer:

```
media/
  podcasts/<podcast_id>/<episode_id>.<ext>
  music/<album_hash>/...
```

Migrera podcast-sökvägar i en explicit version. UPnP-servern ska peka på **media-root**, inte på en hårdkodad “podcasts only”-mapp i Windows-firewall/URL-yta.

Delat: output-enhet, DSP-policy, ev. global volym. **Inte** delat: kö, spellistor, “nu spelas” mellan musik och podcast (två sessioner eller tydlig mode-switch).

## Datamodell MVP

Tabeller: `podcast`, `episode`, `download`, `playback_state`, `queue_item`.

v1 lägger till `transcript`, `transcript_segment`, `chapter`, `ad_segment`, `job`, `bookmark` utan att blanda in `music_*`.

Musik får egna tabeller (`music_track`, `music_album`, …) eller en separat SQLite-fil under samma data-dir. Preferens: **samma fil, strikt prefix**, så backup är en fil — men inga FK mellan podcast-kö och spår.

## Varför Tauri 2 + Rust (inte Electron/Next)

Lägre RAM när v1 kör whisper.cpp på samma maskin. Native Windows-fönster. Bakgrunds-IO och parsers i Rust. React/Vite för UI-hastighet. Next.js SSR tillför inget i en offline-desktopapp.

## Säkerhet / sekretess

Inget konto, ingen telemetri i MVP. Feeds och media lämnar maskinen bara som användaren begär (RSS-hämtning, nedladdning). BYOK-moln-ASR är v1 och opt-in.
