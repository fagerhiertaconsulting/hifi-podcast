# HIFI podcast — musikmodul (senare)

**Status:** efter podcast-MVP och v1. Inte implementerad i den här branchen.

## Ambition

Lokal uppspelning i foobar2000-klass:

- **FLAC**, **DSD**, **SACD** (och andra hi-res-/bit-perfect-format där OS och DAC tillåter)
- Minimal signalväg, `f32` internt tills bit-perfect exclusive output krävs
- Plugin-vänlig motor (decode, DSP, output) — inte ett andra podcast-UI

## Separation

| | Podcast | Musik |
|---|---|---|
| UI | Bibliotek, kö, avsnitt | Album/artist/taggar |
| Data | `podcast` / `episode` / … | Egna `music_*`-tabeller eller separat DB |
| Kö | Up Next för avsnitt | Egen spellista |
| Motor | Delad hi-fi engine | Delad hi-fi engine |
| Output | Samma DAC / exclusive-läge | Samma |

Blanda aldrig podcast-kö med musikbibliotek. Gemensamt är motor + globala output-inställningar (enhet, buffer, exclusive WASAPI).

## Implikation för dagens stack

MVP använder WebView2-playback eftersom det är rätt för *podcastfiler* (MP3/M4A) och snabbt att få rätt. Native-motorn (Symphonia + WASAPI exclusive) ska kunna ersätta den ytan utan att skriva om RSS/nedladdning. Se [audio.md](audio.md).
