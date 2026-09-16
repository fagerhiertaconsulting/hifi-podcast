# Ljudstack — HIFI podcast

HIFI podcast ska låta som källan, inte som en “podcast-app” som normaliserar, komprimerar och packar om. Det här dokumentet är källan för DSP-policy och backend-val.

## Principer (gäller alla faser)

1. **Bevara original.** Nedladdning är en byte-kopia av enclosure. Ingen FFmpeg-transkod, ingen container-ompackning, ingen loudness-fil som default.
2. **Opt-in för skip-silence.** Av som standard. När det slås på hoppar spelaren i tidslinjen; den skriver inte en ny fil.
3. **Ingen dynamikkrossning som default.** Ingen compressor, limiter, “voice enhance” eller ReplayGain-tvång förrän användaren ber om det (v1+).
4. **Tonartsstabil hastighet** när plattformen tillåter det (`preservesPitch` / time-stretch, inte naiv resampling som default).
5. **Flyttalskedja och headroom.** Intern process i `f32` där vi styr DSP. Undvik att mixa i 16-bit med gain > 1 utan limiter. Unity-gain är default.
6. **En motor, två bibliotek.** Podcast-UI och (senare) musik-UI delar engine + output-enhet, inte kö eller datamodell.

## MVP (den här branchen)

| Del | Val | Varför |
|-----|-----|--------|
| Transport | HTTP → fil, SHA-256 | Bit-identisk källa; checksumma för integritet |
| Decode/playback | WebView2 `HTMLMediaElement` **direkt till systemets mixer** | Chromium avkodar MP3/M4A utan extra fil. **Inte** `createMediaElementSource` på `asset.localhost` — det ger tyst utgång (CORS nollar bufferten). |
| Hastighet | `playbackRate` + `preservesPitch = true` | WebView2 är Chromium: time-stretch med bevarad pitch som default |
| Skip silence | Opt-in: same-origin `blob:` + `AnalyserNode` | Default är av. Web Audio kopplas bara efter blob-URL, aldrig mot asset-protokollet. |
| Output | Systemets delade läge (WASAPI shared via Chromium) | Tillräckligt för podcast-MVP; inte bit-perfect |

**Begränsning att vara ärlig om:** Chromium-playern är inte foobar2000. Den gör intern decode till float (bra) men går via webbläsarens mixer, inte exclusive WASAPI. Gapless, DSD och bit-perfect är **inte** MVP.

## v1 (podcast-intelligens, samma filer)

- FFmpeg **endast som analys-sidecar** (silence/energy, kapitelkandidater) — inte som default transcoder.
- Inbäddade kapitel (ID3/MP4 / Podcasting 2.0) läses; genererade kapitel skriver metadata, inte ny ljudfil.
- Volume boost / voice enhance: **off** tills användaren slår på; implementation som valbar gain/EQ, inte en tyst “enhancer”.

## Native hi-fi engine (musikfas + podcast-ersättning)

Målbild, i prioriterad ordning:

1. **Symphonia** (decode av PCM/FLAC/MP3/AAC/OGG i Rust, `f32` frames).
2. **cpal** för portabel output; på Windows en **WASAPI exclusive**-backend när användaren valt bit-perfect (musik).
3. **DSD/SACD:** DoP eller native DSD via dedikerad Windows-backend (ASIO/WASAPI) — plugin-yta, inte inbakat i podcast-UI.
4. Time-stretch för podcast: **Signalsmith Stretch**, **Rubber Band** eller WebView tills native-playern är paritet; aldrig pitch-shift som bieffekt om det går att undvika.
5. Skip silence: hoppa i sample-klockan; skriv inte “trimmat” original. Cut-läge för reklam (v2) är en **explicit** ny fil med disclaimer.

### Varför inte bara FFmpeg-filtergraph till DAC?

FFmpeg är utmärkt för analys och *fallback-transkod till UPnP-renderer*. En alltid-på FFmpeg-kedja bjuder in resampling, dither och loudnorm. Originalfilen ska kunna strömmas orörd till Onkyo TX-RZ50.

## UPnP/DLNA

Streama **samma media-root** som spelaren. MIME från original. Transkod bara om receivern nekar formatet. Se [upnp-mediaserver.md](upnp-mediaserver.md).

## SMTC (Windows)

Mediaknappar / System Media Transport Controls är backlog strax efter MVP-skalet (sketch §12). Native engine ska äga SMTC så podcast och musik inte slåss om sessionen.
