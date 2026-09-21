# Ljudstack — HIFI podcast

HIFI podcast ska låta som källan, inte som en “podcast-app” som normaliserar, komprimerar och packar om. Det här dokumentet är källan för DSP-policy och backend-val.

## Principer (gäller alla faser)

1. **Bevara original.** Nedladdning är en byte-kopia av enclosure. Ingen FFmpeg-transkod, ingen container-ompackning, ingen loudness-fil som default.
2. **Opt-in för skip-silence.** Av som standard. När det slås på hoppar spelaren i tidslinjen; den skriver inte en ny fil.
3. **Ingen dynamikkrossning som default.** Ingen compressor, limiter, “voice enhance” eller ReplayGain-tvång förrän användaren ber om det (v1+).
4. **Tonartsstabil hastighet** när plattformen tillåter det (`preservesPitch` / time-stretch, inte naiv resampling som default).
5. **Flyttalskedja och headroom.** Intern process i `f32` där vi styr DSP. Undvik att mixa i 16-bit med gain > 1 utan limiter. Unity-gain är default.
6. **En motor, två bibliotek.** Podcast-UI och (senare) musik-UI delar engine + output-enhet, inte kö eller datamodell.

## Native hi-fi engine (den här branchen / PR #4)

Målet är att **FLAC och vanliga poddformat ska spelas i appen** via Rust, med **ASIO som primär DAC-väg** (Nicklas: FiiO ASIO Driver, samma som Qobuz *Preferred Audio Output*). WASAPI/cpal ensamt räcker inte för den dagliga kedjan.

### Decode

| Format | Native (Symphonia → f32) | Kommentar |
|--------|--------------------------|-----------|
| FLAC, WAV/PCM, AIFF | Ja | Hi-fi-kärnan |
| MP3, AAC/M4A/M4B, ALAC, OGG/Vorbis/Opus | Ja | Befintliga podcast-enclosure |
| DSD (.dsf/.dff) | Ja (rå DSD-bytes) | Primärt **native DSD via ASIO** (FiiO). DoP om DAC kräver det — UI säger då **DoP**, aldrig “native DSD”. WASAPI exclusive DoP är dokumenterad fallback. Delat WASAPI och WebView2 **vägras** (ingen tyst PCM-stub). SACD-ISO rippning ingår inte. |
| WMA | Nej | WebView2-omkopplare kan användas; ingen tyst omkodning |

### Output — ASIO vs WASAPI (ärligt)

| API | Vad som faktiskt finns i koden | Bit-perfect? |
|-----|--------------------------------|--------------|
| **ASIO** | **Ja:** enhetslista från `HKLM\SOFTWARE\ASIO` (och WOW6432Node) + `CoCreateInstance(clsid, …, clsid)` + IASIO `createBuffers`/`start`/`bufferSwitch`. Det är en riktig öppning, inte en etikett. | Närmast Qobuz-kedjan: källans samplingsfrekvens om drivrutinen tar den; intern kedja är **f32** (inte integer-bitkopia från FLAC). Ingen tyst omsampling i ASIO — om FiiO nekar Hz visas fel. |
| **WASAPI exclusive** | Ja (Windows, `wasapi`-låda). Fallback. | Exclusive låser DAC:n; fortfarande f32-kedja. |
| **WASAPI shared (cpal)** | Ja. Default-fallback om ASIO saknas/nekas i läge **Auto**. | Inte bit-perfect: systemets mixer kan resample:a. |
| WebView2 `HTMLMediaElement` | Kvar som omkopplare | Chromium-mixer, inte hi-fi-DAC-läge |

**Auto:** prova ASIO (FiiO först om namnet matchar), sedan WASAPI exclusive, sedan shared. Explicit **ASIO** i inställningar får inte tyst falla tillbaka — feltext på svenska.

**Krav på maskinen:** FiiO ASIO Driver måste vara installerad (samma som Qobuz). Utan registerpost finns ingen ASIO-rad; UI ljuger inte om det.

**DSD via ASIO:** `IASIO::future(kAsioSetIoFormat, DSD)` + sampletyp 32/33/40. FiiO ASIO Driver är primär. Misslyckad native-växling → ASIO DoP (24-bit, markörer 0x05/0xFA) märkt `dop`. WASAPI exclusive DoP bara om ASIO saknas/nekas i **Auto**. Explicit ASIO tyst-faller **inte** till WASAPI. DSD-tystnad är `0x69`, aldrig `0x00`.

### DSP i native-motorn

- Unity-gain, ingen compressor.
- Hastighet ≠ 1×: WSOLA (tonartsstabil). **ASIO + hastighet ≠ 1 är inte bit-perfect.**
- Hoppa tystnad: opt-in, hopp i sample-klockan.
- Intern `f32`. Vid ASIO skrivs till drivrutinens format (typiskt i32/i24/f32 LSB) utan dither-policy utöver clamp.

## MVP (WebView2, fortfarande valbar)

| Del | Val | Varför |
|-----|-----|--------|
| Transport | HTTP → fil, SHA-256 | Bit-identisk källa; checksumma för integritet |
| Decode/playback | WebView2 `HTMLMediaElement` **direkt till systemets mixer** | Chromium avkodar MP3/M4A utan extra fil. **Inte** `createMediaElementSource` på `asset.localhost` — det ger tyst utgång (CORS nollar bufferten). |
| Hastighet | `playbackRate` + `preservesPitch = true` | WebView2 är Chromium: time-stretch med bevarad pitch som default |
| Skip silence | Opt-in: same-origin `blob:` + `AnalyserNode` | Default är av. Web Audio kopplas bara efter blob-URL, aldrig mot asset-protokollet. |
| Output | Systemets delade läge (WASAPI shared via Chromium) | Inte bit-perfect |

## v1 (podcast-intelligens, samma filer)

- FFmpeg **endast som analys-sidecar** (silence/energy, kapitelkandidater) — inte som default transcoder.
- Inbäddade kapitel (ID3/MP4 / Podcasting 2.0) läses; genererade kapitel skriver metadata, inte ny ljudfil.
- Volume boost / voice enhance: **off** tills användaren slår på; implementation som valbar gain/EQ, inte en tyst “enhancer”.

### Varför inte bara FFmpeg-filtergraph till DAC?

FFmpeg är utmärkt för analys och *fallback-transkod till UPnP-renderer*. En alltid-på FFmpeg-kedja bjuder in resampling, dither och loudnorm. Originalfilen ska kunna strömmas orörd till Onkyo TX-RZ50.

## UPnP/DLNA

Streama **samma media-root** som spelaren (`media/podcasts/…`). MIME från SQLite (`download.mime`). HTTP Range för seek. Transkod bara om receivern nekar formatet — **inte i v1 av servern**. Se [upnp-mediaserver.md](upnp-mediaserver.md).

## SMTC (Windows)

Mediaknappar går till densamma kö-logiken oavsett native/WebView2. Native-motorn äger DAC-sessionen (ASIO/WASAPI); SMTC är transport, inte mixern.
