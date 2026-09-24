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
| `db.sqlite` | Prenumerationer, avsnitt, kö, transkript, jobb, UPnP-inställningar, **musikbibliotek** |
| `media/podcasts/<podcast_id>/<episode_id>.<ext>` | **Original** enclosure-filer |
| musik | Originalfilerna ligger kvar i den valda mappen. Biblioteket sparar sökvägen i `db.sqlite`. Ingen kopia av ljudet under appdata. |
| `media/music/covers/<album_id>.<ext>` | Omslag från Cover Art Archive, bara efter sparat internet-samtycke. Max 2 MiB. |
| `models/ggml-small.bin` | Whisper-modell (nedladdas av användaren) |
| `bin/whisper-cli.exe` | whisper.cpp sidecar (Windows: Inställningar → Ladda ner whisper-cli) |

Äldre layout `media/<podcast_id>/…` migreras automatiskt till `media/podcasts/…`. Samma media-root används av UPnP/DLNA-servern (Onkyo TX-RZ50). Se [docs/upnp-mediaserver.md](docs/upnp-mediaserver.md).

## MVP i den här branchen

- App-skal (mörkt tema, svenska)
- Prenumerera via RSS-URL
- OPML-import (fil eller inklistrad XML) och export
- Nedladdning utan omkodning + SHA-256-checksumma
- Spelare: native Symphonia PCM + DSD (.dsf/.dff) → **ASIO (FiiO native DSD / DoP) / WASAPI exclusive DoP**, med WebView2 som omkopplare för podd (inte DSD)
- Per-podcast: hastighet, auto-nedladdning, intro/outro-skip, opt-in hoppa tystnad
- **Musik:** separat flik, import av mapp (FLAC/DSD), uppspelning i samma native-motor
- **Online metadata:** av tills användaren sparar samtycke. Då bara `musicbrainz.org` och `coverartarchive.org` (https, port 443). Omslag max 2 MiB under `media/music/covers/`

**Inte i koden ännu:** SACD-ISO-rippning, moln-ASR, in-app UPnP-renderer.

v1 (lokal intelligens) finns i appen: se [docs/v1-asr.md](docs/v1-asr.md).

### UPnP/DLNA-mediaserver (Windows + Onkyo)

Inställningar → **UPnP/DLNA-mediaserver** → **Starta mediaserver** (port 8200). Tom bind = upptäckt LAN-IPv4, inte `0.0.0.0`. Ingen autentisering (DLNA); bara för betrott LAN. Receivern ska se **HIFI podcast** och bläddra `Podcasts`. Originalfiler streamas med HTTP Range; ingen omkodning.

**Brandvägg:** tillåt appen på privat nätverk när Windows frågar (inget admin-krav för att starta servern). Om Onkyo inte hittar den: inbound **TCP 8200** (eller vald port) och **UDP 1900** (SSDP) på den privata profilen. Steg-för-steg: [docs/upnp-mediaserver.md](docs/upnp-mediaserver.md).

## Dokumentation

- [docs/audio.md](docs/audio.md) — ljudstack och varför
- [docs/architecture.md](docs/architecture.md) — faser och modulgränser
- [docs/podcast-player-sketch.md](docs/podcast-player-sketch.md) — produkt- och arkitekturskiss
- [docs/music-roadmap.md](docs/music-roadmap.md) — musikmodul (MVP + senare SACD)
- [docs/upnp-mediaserver.md](docs/upnp-mediaserver.md) — UPnP/DLNA mot Onkyo
- [docs/roadmap.md](docs/roadmap.md) — backlog efter MVP

## Utveckling (övriga OS)

`npm run build` kompilerar UI. `cargo test --manifest-path src-tauri/Cargo.toml` kör parser-/databastester. Full `tauri dev` på Linux kräver WebKitGTK enligt Tauri-dokumentationen; **målplattform är Windows**.
