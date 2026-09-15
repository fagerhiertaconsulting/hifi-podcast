# HIFI podcast — produkt- & arkitekturskiss

**Produkt:** HIFI podcast  
**Repo:** https://github.com/fagerhiertaconsulting/hifi-podcast  
**För:** Nicklas Fagerhierta  
**Datum:** 2026-09-15  
**Typ:** Lokal-first Windows desktop-app (PowerHouse)  
**Status:** Anpassad skiss för HIFI podcast (MVP-skal i repo; ASR/UPnP/musik är senare faser)

---

## 1. Produktvision & unika säljargument

### Vision
**HIFI podcast** är en **lokal-first** podcastspelare för Windows som matchar avancerade funktioner hos Pocket Casts, Podcast Addict och Castro — men där *all* intelligent bearbetning (transkript, kapitel, reklamdetektering) sker på maskinen som standard. Inga konton, ingen molnsynk som krav, ingen uppladdning av ljud utan uttryckligt medgivande. Hi-fi-policy: originalfiler, ingen dynamikkrossning som default, opt-in skip-silence.

### Målgrupp (primär)
Power-användare på Windows som:
- Laddar ner mycket och lyssnar offline
- Vill ha hastighet, kö, filter, kapitel och skip silence
- Bryr sig om sekretess och lokal kontroll
- Vill kunna *söka i* avsnitt via transkript och hoppa till rätt ställe

### Unika säljargument (differentiators)
1. **Lokal transkription av varje nedladdat avsnitt** — whisper.cpp / faster-whisper på maskinen; BYOK-cloud som valfri snabbväg.
2. **Lokalt genererade kapitel** från transkript + tystnad/energi-cues (även när feeden saknar chapter-markers).
3. **Reklamdetektering med skip *eller* cut** — heuristik först, ML senare; användaren väljer om reklam hoppas över i realtid eller klipps bort till en “clean” lokal fil.

### Namnstatus
**HIFI podcast** (beslutat). Tidigare arbetsnamn i researchskissen ("Lyssna Lokal" / "PodForge" / "ChapterCast") används inte.

---

## 2. Konkurrentverifiering: "Rocket cast"

| Kandidat | Status | Kommentar |
|----------|--------|-----------|
| **Castro** (castro.fm) | ✅ Verklig, aktiv iOS/iPad-podcastapp (2025–2026) | Inbox + Queue-modell, Trim Silence, kapitel, CarPlay, Castro Plus |
| **Rocket.fm** som podcastspelare | ❌ Ingen trovärdig app hittad | Relay FM hade en *podcast* vid namn Rocket (nedlagd 2023) — inte en spelare |
| **Rocket Cast / Rocketcast** | ❌ Inte en spelare | Produktionsbolag / dansk tech-podcast (rocketcast.dk m.fl.) |
| **Rocket Music Player** (JRT Studio) | ⚠️ Musikspelare med podcast-bokmärken | Android-musikapp, inte fullvärdig podcatcher — irrelevant som paritetskälla |

**Slutsats:** Paritet byggs mot **Pocket Casts**, **Podcast Addict** och **Castro**. "Rocket cast" tolkas som sannolik förväxling med **Castro**; Rocket Music Player noteras men används inte som kravbas.

---

## 3. Funktionsmatris (parity vs unik)

Legend: ✅ = must-have parity · ⭐ = unik differentiator · ~ = nice-to-have / senare · — = ej relevant för desktop

| Funktion | Pocket Casts | Podcast Addict | Castro | HIFI podcast | Fas |
|----------|:------------:|:--------------:|:------:|:-------:|:---:|
| OPML import/export | ✅ | ✅ | ✅ | ✅ | MVP |
| Prenumerationer via RSS / katalogsök | ✅ | ✅ | ✅ | ✅ | MVP |
| Nedladdning offline + auto-download | ✅ | ✅ | ✅ | ✅ | MVP |
| Up Next / Queue (omordningsbar) | ✅ | ✅ (spellistor) | ✅ (kärna) | ✅ | MVP |
| Inbox / triage (Castro-stil) | ~ | ~ | ✅ | ✅ | v1 |
| Hastighet 0.5–3× | ✅ | ✅ | ✅ | ✅ | MVP |
| Skip silence / Trim Silence | ✅ | ✅ | ✅ (Plus) | ✅ | MVP |
| Volume boost / enhance voices | ✅ | ✅ | ✅ | ✅ | v1 |
| Sleep timer (tid / slut avsnitt / kapitel) | ✅ | ✅ | ✅ | ✅ | MVP |
| Inbäddade kapitel (MP3/M4A / Podcast Index) | ✅ | ✅ | ✅ | ✅ | MVP |
| AI-/genererade kapitel | ✅ (Plus, cloud) | — | — | ⭐ lokal | v1 |
| Bokmärken | ✅ | ✅ | ✅ | ✅ | v1 |
| Filter / smarta spellistor | ✅ | ✅ | ~ | ✅ | v1 |
| Per-podcast-inställningar (hastighet, intro-skip, auto-dl) | ✅ | ✅ | ✅ | ✅ | MVP |
| Intro/outro-skip (sekunder) | ✅ | ✅ | ✅ | ✅ | MVP |
| Transkriptvisning + sök + hopp till tid | ~ (Smart Bookmarks) | ✅ (SRT/VTT) | — | ⭐ lokal ASR | v1 |
| Auto-skip / cut reklam | — | — | — | ⭐ | v1–v2 |
| Car mode / stora knappar | CarPlay | Android Auto + Car Mode | CarPlay | Desktop “bil/kök”-läge | v2 |
| Chromecast / Sonos | ✅ | ✅ | — | ~ | v2 |
| Cloud-synk | ✅ | Google Drive | iCloud (Plus) | Valfri export; lokal default | v2 |
| Teman (mörkt/OLED) | ✅ | ✅ | ✅ | ✅ | MVP |
| Statistik (lyssnad tid) | ✅ | ✅ | ~ | ✅ | v1 |
| Sideloading (importera ljudfiler) | ~ | ✅ | ✅ (Plus) | ✅ | v1 |
| Videoavsnitt | ✅ | ✅ | — | ~ | v2 |
| Bakgrundsjobb (ASR, kapitel, ads) | — (cloud) | — | — | ⭐ | v1 |

### Must-have advanced (sammanfattning för parity)
OPML, RSS-prenumerationer, nedladdningar, kö/queue, hastighet, skip silence, sleep timer, kapitel (inbäddade), per-podcast-inställningar, intro-skip, filter/spellistor, bokmärken, mörkt tema, offline-first.

### Unikt
Lokal ASR-transkript, lokala kapitel, reklam skip/cut, sökbar episodhistorik via transkript — allt utan moln som default.

---

## 4. Faser: MVP → v1 → v2

### MVP (ca 4–6 veckor; “spelbar podcatcher”)
- Prenumerera via RSS-URL + OPML-import
- Lista podcasts/avsnitt, hämta metadata (titel, bild, beskrivning)
- Ladda ner avsnitt till lokal disk; spela upp med hastighet + seek
- Up Next-kö, play/pause, ±15/30 s skip
- Sleep timer (tid / slut avsnitt)
- Per-podcast: hastighet, auto-download, intro-skip (sekunder)
- Mörkt tema, grundläggande Windows-mediekontroller (SMTC)
- Lokal SQLite-databas för state

**Utanför MVP:** ASR, genererade kapitel, reklam, filter-UI, Car Mode.

### v1 (ca 2–3 månader efter MVP)
- Bakgrundskö: transkribera nedladdade avsnitt (whisper.cpp)
- Transkript-UI: scroll-sync, sök, klick → hoppa
- Kapitel: läs inbäddade + generera från transkript/silence
- Reklamheuristik v1: frasmatchning + silence/music-bed cues → **skip-läge**
- Inbox/triage (Castro-inspirerat)
- Filter / smarta spellistor
- Bokmärken, statistik, sideloading
- BYOK-option för moln-ASR (OpenAI/Groq m.fl.) — avstängt som default

### v2 (senare)
- Reklam **cut**-läge (skriva clean lokal fil)
- ML-baserad ad-detektering (träna på användarfeedback)
- Car/kök-läge (stora knappar, begränsad UI)
- Valfri krypterad backup/export; ev. synk mellan egna enheter
- Cast/remote, videoavsnitt, rikare katalog/discover

### Första 2 veckorna (konkret MVP-scope) — se även executive summary
Se sektion 12.

---

## 5. Arkitektur (lokal-first)

### Rekommenderad stack: **Tauri 2 + Rust + React/TypeScript**

| Lager | Val | Motivering |
|-------|-----|------------|
| Shell | **Tauri 2** | Liten binär, låg RAM vs Electron; native Windows; bra för bakgrundsprocesser |
| UI | React + TypeScript (+ Vite) | Snabb UI-iteration; WebView2 på Windows |
| Kärna | Rust | Nedladdning, RSS, DB, jobbkön, FFmpeg-bindningar |
| DB | SQLite (via `sqlx` / `rusqlite`) | En fil, offline, enkel backup |
| Media | `cpal` / Windows Media Foundation + ev. `symphonia`; eller inbäddad player via WebView + lokal fil-URL | Stabil playback + SMTC |
| ASR | **whisper.cpp** (sidecar/binär) primärt; alternativt **faster-whisper** via lokal Python-sidecar | Lokal, ingen API-kostnad |
| Audio-analys | FFmpeg (silence detect, loudness) | Kapitel + reklamheuristik |
| Jobbkö | Rust worker-pool + persistent job-tabell | ASR/kapitel/ads utan att frysa UI |

### Varför inte Electron + Next?
- Electron drar mer RAM/CPU — konkurrerar med Whisper på samma maskin.
- Next.js är webbramverk; overhead och SSR är onödigt för en offline-desktopapp.
- Tauri ger mindre yta, enklare att skeppa “en .exe + modellfiler”, och Rust lämpar sig för långa bakgrundsjobb och FFmpeg/whisper-integration.
- **Undantag:** Om Nicklas redan har stark Electron/Next-kompetens och vill maximera UI-hastighet kan Electron vara pragmatiskt — men default för PowerHouse Windows + lokal ASR är **Tauri 2**.

### Högnivåflöde
```
[UI (WebView)]  ←IPC→  [Rust core]
                           ├─ RSS fetcher / OPML
                           ├─ Download manager
                           ├─ Playback controller
                           ├─ SQLite
                           └─ Job queue
                                 ├─ whisper.cpp (ASR)
                                 ├─ chapter generator
                                 └─ ad detector (+ optional cutter)
```

### Offline & lagring
- Media: `%LOCALAPPDATA%/<App>/media/<podcast_id>/<episode_id>.<ext>`
- DB: `%LOCALAPPDATA%/<App>/db.sqlite`
- Modeller: `%LOCALAPPDATA%/<App>/models/ggml-*.bin` (nedladdas engångs, användarstyrt)
- Transkript: JSON/SRT bredvid media eller i DB (BLOB/text) — rekommendation: fil + metadata i DB

### Bakgrundsjobb för ASR
1. Avsnitt markeras `downloaded`
2. Job `transcribe` enqueueas (prioritet: nyligen spelade > nyligen nedladdade)
3. Worker kör whisper.cpp med vald modellstorlek (tiny/base/small/medium)
4. Resultat → `transcripts` + event till UI
5. Följdjobb: `generate_chapters`, `detect_ads`

CPU-budget: max N samtidiga jobb; pausa ASR vid aktiv uppspelning om systemet är under belastning (konfigurerbart).

---

## 6. Transkription

### Strategi: lokal först, BYOK som option

| Läge | Teknik | Privacy | Kostnad | Kvalitet/hastighet |
|------|--------|---------|---------|-------------------|
| **Default: lokal** | whisper.cpp (GGML) | Ljud lämnar aldrig maskinen | El + disk; gratis modeller | Bra med `small`/`medium`; långsammare på CPU |
| **Option: BYOK cloud** | Användarens API-nyckel (OpenAI Whisper, Groq, etc.) | Ljud skickas till vald leverantör | Användaren betalar | Snabbare; kräver nät |

### Rekommendationer
- Defaultmodell: **`ggml-small`** (balans) med möjlighet att välja `base` (snabbare) eller `medium` (bättre SV/EN).
- Språk: auto-detect + manuell override per podcast (svenska/engelska vanligt för Nicklas).
- Format: lagra segments med start/end/text; exportera SRT/VTT.
- UI: highlight aktuellt segment under playback; fulltextsök över biblioteket.
- Aldrig auto-aktivera cloud; tydlig consent-dialog + nyckel lagras i OS credential store (Windows Credential Manager), inte i plaintext i DB.

### Kostnad/privacy (sammanfattning)
- Lokal: maximal privacy, förutsägbar offline, kräver disk (~1–5 GB modeller) och CPU/GPU-tid.
- BYOK: bekvämlighet för långa köer; risk = uppladdning av episodljud; måste vara opt-in.

---

## 7. Kapitelgenerering

### Källor (prioritetsordning)
1. Inbäddade kapitel (ID3/MP4/chapters, Podcasting 2.0 / Podcast Index)
2. Feed-metadata (om separat chapter-url)
3. **Genererade** (vår pipeline) när 1–2 saknas eller är ofullständiga

### Pipeline (lokal)
1. **Silence/energy cues (FFmpeg):** hitta längre tystnader, musikbäddar, energiskiften → kandidatgränser.
2. **Transkriptsegmentering:** gruppera meningar till tematiska block (rubrik via första meningen / LLM-löst: heuristik med nyckelordsbyte, tidsfönster 2–8 min).
3. **Merge:** kombinera audio-cues + textgränser; filtrera för korta/överlappande kapitel.
4. **Titel:** kort label från första substantiviska frasen eller “Del N — …” tills bättre summarizer finns.
5. Användaren kan redigera/radera/exportera kapitel.

v1 = heuristik utan moln-LLM. v2 = valfri lokal liten LLM för bättre rubriker (t.ex. via llama.cpp) — fortfarande lokal.

---

## 8. Reklamdetektering: skip vs cut

### Heuristik v1 (ingen ML-träning krävs)
- **Fraslexikon (flerspråkigt):** t.ex. “this podcast is sponsored by”, “today’s episode is brought to you”, “reklam”, “i samarbete med”, “kod … för rabatt”, midroll-markörer.
- **Silence före/efter:** typiska 0.5–2 s luckor runt ad-blocks.
- **Music beds:** detektera icke-tal / musikdominant segment (enkel spectral/energy-klassning via FFmpeg).
- **Kända mönster:** återkommande längd (15/30/60 s), position (första 2 min, midroll ~50 %).
- **Användarfeedback:** “Detta var reklam” / “Inte reklam” → förbättrar lokala regler per podcast.

### Lägen
| Läge | Beteende | När |
|------|----------|-----|
| **Skip** | Under playback hoppas markerade `ad_segment` över (som muted chapters) | v1 default |
| **Cut** | Generera lokal “clean” fil utan ad-segment (FFmpeg concat) | v2 |
| **Off** | Visa markörer men hoppa inte | alltid valbart |

### ML later (v2+)
- Klassificerare på audio+text-features, tränad på anonymiserade lokala labels (stannar på enheten) eller opt-in delning.
- Aldrig skicka råa avsnitt till vår server (vi har ingen server i lokal-first-modellen).

### Juridik/etik (notera)
- Skip för personligt lyssnande är praxis i många appar (intro-skip, chapter skip).
- **Cut** som skapar modifierad fil: endast för privat bruk; ingen redistribution. Visa tydlig disclaimer i UI.

---

## 9. Datamodell (översikt)

```sql
-- Förenklad logisk modell (SQLite)

podcast (
  id PK, guid, title, author, feed_url UNIQUE,
  artwork_path, description,
  auto_download BOOL, speed REAL, intro_skip_sec INT, outro_skip_sec INT,
  language TEXT, created_at, updated_at
)

episode (
  id PK, podcast_id FK, guid UNIQUE, title, description,
  audio_url, publish_at, duration_ms, episode_image_path,
  status ENUM('new','downloaded','played','archived'),
  created_at, updated_at
)

download (
  id PK, episode_id FK UNIQUE, file_path, bytes, checksum,
  state ENUM('queued','downloading','done','error'),
  error_msg, completed_at
)

transcript (
  id PK, episode_id FK UNIQUE, source ENUM('local_whisper','byok','feed'),
  model TEXT, language TEXT, srt_path, json_path,
  status ENUM('pending','running','ready','error'),
  created_at
)

transcript_segment (
  id PK, transcript_id FK, start_ms, end_ms, text, speaker NULL
)

chapter (
  id PK, episode_id FK, start_ms, end_ms NULL, title,
  source ENUM('embedded','feed','generated','user'),
  artwork_path NULL
)

ad_segment (
  id PK, episode_id FK, start_ms, end_ms,
  confidence REAL, method TEXT,  -- 'phrase','silence','music','ml','user'
  user_label ENUM('ad','not_ad','unset'),
  enabled BOOL  -- ingår i skip/cut
)

playback_state (
  episode_id PK FK, position_ms, speed REAL, completed BOOL,
  last_played_at
)

queue_item (
  id PK, episode_id FK, position INT UNIQUE, added_at
)

job (
  id PK, type TEXT, episode_id FK, priority INT,
  state ENUM('queued','running','done','error'),
  attempts INT, payload_json, error_msg, created_at, updated_at
)

bookmark (
  id PK, episode_id FK, position_ms, note, created_at
)
```

Indexera: `episode(podcast_id, publish_at)`, FTS på `transcript_segment(text)`, `job(state, priority)`.

---

## 10. Sekretess

- **Allt lokalt som default:** feeds hämtas, media sparas, ASR körs on-device.
- Inget konto, ingen telemetri utan opt-in.
- BYOK: explicit opt-in per jobb eller per podcast; nyckel i Credential Manager.
- Inga tredjeparts trackers i UI.
- OPML/export och DB-backup = användarstyrda filer.
- Modellfiler laddas från officiella källor med checksum-verifiering.
- Loggar: lokala, roterande; inga episodtexter i crash-reports utan samtycke.

---

## 11. Risker & öppna frågor

### Risker
| Risk | Mitigering |
|------|------------|
| ASR för långsamt på CPU | Mindre modell, GPU (Vulkan/CUDA) om tillgängligt, kö nattetid, BYOK-option |
| Diskväxt (media + transkript) | Retention-policy, komprimering, “radera spelade efter N dagar” |
| Falska reklampositiver | Confidence-tröskel, per-podcast lexikon, enkel undo (“spela reklam”) |
| Feed-fragmentering / trasiga RSS | Robust parser, fallback enclosure, manuell URL |
| Tauri/WebView2-kantfall på Windows | CI på Win10/11; native dialogs för filer |
| Scope creep (parity med tre appar) | Faserna ovan; MVP = podcatcher först |
| Juridik runt ad-cut | Skip först; cut med privatbruks-disclaimer |

### Öppna frågor till Nicklas
1. **Plattform:** Endast Windows 10/11, eller även macOS/Linux senare?
2. **Namn & tonalitet:** Beslutat: **HIFI podcast**.
3. **Stack-accept:** Beslutat: Tauri 2 + Rust + React/TS + SQLite.
4. **Hårdvara:** Har PowerHouse-maskinen NVIDIA/AMD GPU att räkna med för Whisper, eller CPU-only?
5. **Språkfokus:** Primärt svenska feeds, engelska, eller blandat (påverkar modellval)?
6. **Reklampolicy:** Skip räcker i v1, eller är cut ett hårt krav tidigt?

---

## 12. Föreslagen scope — första 2 veckorna

**Mål:** Skelett som kan prenumerera, ladda ner och spela — ingen ASR ännu.

1. Initiera Tauri 2-projekt (Rust + React/TS), mörkt skal-UI.
2. SQLite-schema v0: `podcast`, `episode`, `download`, `playback_state`, `queue_item`.
3. OPML-import + lägg till feed via URL (RSS-parse → podcasts/episodes).
4. Download manager (en fil i taget) till lokal media-mapp.
5. Player: play/pause, seek, hastighet 1×/1.5×/2×, kö “Up Next”.
6. Persistens av lyssningsposition; grundläggande sleep timer.
7. Windows SMTC (media keys) + app-ikon/tema.
8. README med körmodell, kataloglayout, och backlog för v1 (ASR-jobb).

**Definition of done (2 veckor):** Nicklas kan importera OPML, ladda ner 3 avsnitt, köa dem och lyssna offline med hastighet och sparad position.

---

## 13. Stackbeslut (sammanfattning)

**Default för PowerHouse Windows: Tauri 2 + Rust + React/TypeScript + SQLite + whisper.cpp + FFmpeg.**

Rationale: lägre resursfotavtryck än Electron (viktigt när lokal ASR körs), native känsla på Windows, stark bakgrundsbearbetning i Rust, UI-flexibilitet i React. Electron + Next väljs bort som default p.g.a. högre minnesbruk och onödig webbramverkskomplexitet för en offline-first desktopapp.

---

## 14. Källor (research 2026-09-15)

- Pocket Casts Support: OPML, Sleep Timer, Chapters; App Store-featurelista; blog (Desktop 2.0, Playlists 2025, AI chapters 2026)
- Podcast Addict: podcastaddict.com/app; changelogs 2025.4 / 2026.4 (HLS, transcript view, car mode intent, volume leveler)
- Castro: castro.fm (+ support Auto-Queue, Plus-features, 2025.8 release notes)
- Rocket-disambiguering: ingen Rocket.fm-spelare; Rocket Music Player = musikapp; Rocket Cast = produktion/podcast — ej paritetskälla

---

*Anpassad skiss för HIFI podcast. Ljudstack: [audio.md](audio.md). Faser: [architecture.md](architecture.md).*

---

## Tillägg 2026-09-15 (Nicklas) — HIFI podcast + musik

**Produktnamn:** HIFI podcast  
**Repo:** https://github.com/fagerhiertaconsulting/hifi-podcast

### Musik (senare fas, foobar2000-ambition)
- Lokal uppspelning av högkvalitativa musikfiler: **FLAC**, **DSD**, **SACD** (och andra bit-perfect-/hi-res-format där det är rimligt).
- Ambition: foobar2000-liknande kvalitet (bit-perfect där OS/DAC tillåter, minimal signalväg, plugin-vänlig motor).
- **Inte** MVP. Separat utvecklingssteg efter podcast-MVP/v1.

### Separation
- **Podcast-modulen** och **musikmodulen** ska vara tydligt åtskilda i UI och datamodell (egna libraries, egna vyer, egna köer).
- **Delad hi-fi audio engine** under (samma uppspelningskärna, DSP-policy, device-output).
- Undvik att blanda podcast-kö med musikbibliotek; gemensamt är bara motor + eventuellt globala output-inställningar (DAC, buffer, exclusive mode).

### Implikation för stack
- Välj audio-backend med sikte på både podcast (variable speed, skip silence opt-in) **och** framtida bit-perfect musik — dokumentera begränsningar per fas.

### UPnP / DLNA mediaserver (cast till nätverksreceiver)
- Appen ska kunna fungera som **UPnP AV Media Server** (DLNA-kompatibel) så att t.ex. **Onkyo TX-RZ50** kan spela upp biblioteket från HIFI podcast över LAN.
- Scope: både **podcast-nedladdningar** och (senare) **musikbibliotek**; separata containrar/träd i UPnP-trädet (Podcasts vs Music).
- Kontrollpunkt: discoverbar i Onkyos nätverksspelare; browse folders; stream originalfiler där möjligt (FLAC/m4a/mp3) utan onödig transkod — transkod endast som fallback om receivern kräver det.
- **Fas:** efter lokal player-MVP; tidig arkitektur ska inte låsa ut serverläge (dela media-root, metadata, MIME-map).
- Relaterat (v2+): valfri UPnP **renderer**/control point — men primärt krav är **server** mot Onkyo.
