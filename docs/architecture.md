# Arkitektur — HIFI podcast

## Faser

| Fas | Vad | Kod nu |
|-----|-----|--------|
| **MVP** | Lokal podcatcher + hi-fi-policy | Ja (skal) |
| **v1** | Lokal ASR, kapitel, reklam-skip | Roadmap |
| **Musik** | Separat modul, foobar2000-ambition | MVP: `music_*`-tabeller + Musik-flik, delad engine |
| **Native engine** | Symphonia PCM + DSD (.dsf/.dff) → ASIO (FiiO native/DoP) / WASAPI | PR #4 PCM; DSD native i efterföljande PR |
| **UPnP** | Media Server mot Onkyo TX-RZ50 | PR #3: SSDP + CDS + HTTP Range; `Music/` stub |

## Process

```
[React UI, svenska]  ←IPC→  [Rust / Tauri 2]
                              ├─ RSS + OPML
                              ├─ Download manager (en fil i taget, original)
                              ├─ SQLite
                              ├─ Native engine (Symphonia → ASIO / WASAPI)
                              ├─ Playback-state + kö
                              └─ Jobbkön (v1: whisper, kapitel, ads)
```

UI ska inte avkoda feeds eller skriva SQLite direkt.

## Lagring

- DB: app local data `db.sqlite`
- Media-root: `media/podcasts/<id>/…` och `media/music/` (migrering från `media/<id>/`)

**Podcast-library** ligger under `media/podcasts/<podcast_id>/<episode_id>.<ext>`. Musikbiblioteket:

```
media/
  podcasts/<podcast_id>/<episode_id>.<ext>
  music/<album_id>/<track_id>.<ext>
```

Migrera podcast-sökvägar i en explicit version. UPnP-servern ska peka på **media-root**, inte på en hårdkodad “podcasts only”-mapp i Windows-firewall/URL-yta.

Delat: output-enhet, DSP-policy, ev. global volym. **Inte** delat: kö, spellistor, “nu spelas” mellan musik och podcast (två sessioner eller tydlig mode-switch).

## Datamodell MVP

Tabeller: `podcast`, `episode`, `download`, `playback_state`, `queue_item`.

v1 lägger till `transcript`, `transcript_segment`, `chapter`, `ad_segment`, `job`, `bookmark`.

Musik får egna tabeller (`music_track`, `music_album`, `music_folder`, `music_playback_state`) i samma `db.sqlite` med strikt prefix — inga FK mot `episode` / podcast-kö.

## Varför Tauri 2 + Rust (inte Electron/Next)

Lägre RAM när v1 kör whisper.cpp på samma maskin. Native Windows-fönster. Bakgrunds-IO och parsers i Rust. React/Vite för UI-hastighet. Next.js SSR tillför inget i en offline-desktopapp.

## Säkerhet / sekretess

Inget konto, ingen telemetri i MVP. Feeds och media lämnar maskinen bara som användaren begär (RSS-hämtning, nedladdning). BYOK-moln-ASR är v1 och opt-in.
