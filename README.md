# HIFI podcast

Lokal-first Windows-desktopapp för podcasts med hi-fi-ambition: originalfiler, ingen onödig DSP, och en motor som senare kan bära både podcast och musik.

Inga konton. Ingen molnsynk som krav. Svenskt gränssnitt.

## Stack

- **Tauri 2** + **Rust** + **React/TypeScript** (Vite)
- **SQLite** (`rusqlite`, bundled) för podcasts, avsnitt, nedladdningar, uppspelning och kö
- **RSS** + **OPML** i Rust-kärnan
- Nedladdning: HTTP-byte kopia till disk — **ingen omkodning**

## Windows — köra från källkod

Krav:

- Windows 10/11
- [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) (finns på de flesta Win11-maskiner)
- [Node.js 20+](https://nodejs.org/)
- [Rust stable](https://rustup.rs/) (1.85+, rekommenderat: senaste stable)
- Visual Studio Build Tools med “Desktop development with C++”
- [Tauri 2-förutsättningar](https://v2.tauri.app/start/prerequisites/)

```powershell
git clone https://github.com/fagerhiertaconsulting/hifi-podcast.git
cd hifi-podcast
npm install
npm run tauri dev
```

Bygg installer:

```powershell
npm run tauri build
```

NSIS/MSI hamnar under `src-tauri/target/release/bundle/`.

## Data på disk (Windows)

Tauri lägger appdata under `%LOCALAPPDATA%\consulting.fagerhierta.hifi-podcast\`:

| Sökväg | Innehåll |
|--------|----------|
| `db.sqlite` | Prenumerationer, avsnitt, kö, lyssningsposition |
| `media/<podcast_id>/<episode_id>.<ext>` | **Original** enclosure-filer |

Samma `media/`-root är avsiktligt delbar med en framtida UPnP/DLNA-server (Onkyo TX-RZ50). Se [docs/upnp-mediaserver.md](docs/upnp-mediaserver.md).

## MVP i den här branchen

- App-skal (mörkt tema, svenska)
- Prenumerera via RSS-URL
- OPML-import (fil eller inklistrad XML) och export
- Nedladdning utan omkodning + SHA-256-checksumma
- Spelare: play/paus, seek, hastighet, ±15/±30 s, kö, sovtimer, sparad position
- Per-podcast: hastighet, auto-nedladdning, intro/outro-skip, opt-in hoppa tystnad

**Inte i koden ännu** (endast roadmap): Whisper/ASR, reklam-ML, full UPnP-server, musikbibliotek.

## Dokumentation

- [docs/audio.md](docs/audio.md) — ljudstack och varför
- [docs/architecture.md](docs/architecture.md) — faser och modulgränser
- [docs/podcast-player-sketch.md](docs/podcast-player-sketch.md) — produkt- och arkitekturskiss
- [docs/music-roadmap.md](docs/music-roadmap.md) — musikmodul (senare)
- [docs/upnp-mediaserver.md](docs/upnp-mediaserver.md) — UPnP/DLNA mot Onkyo
- [docs/roadmap.md](docs/roadmap.md) — backlog efter MVP

## Utveckling (övriga OS)

`npm run build` kompilerar UI. `cargo test --manifest-path src-tauri/Cargo.toml` kör parser-/databastester. Full `tauri dev` på Linux kräver WebKitGTK enligt Tauri-dokumentationen; **målplattform är Windows**.
