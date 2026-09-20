# HIFI podcast — musikmodul (senare)

**Status:** efter native-motorn (PR #4). Inte implementerad i den här branchen.

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

MVP:s WebView2-yta är fallback. Native-motorn (Symphonia + **ASIO / FiiO ASIO Driver**, WASAPI som fallback) ersätter den för lokala filer utan att skriva om RSS/nedladdning. Se [audio.md](audio.md).
