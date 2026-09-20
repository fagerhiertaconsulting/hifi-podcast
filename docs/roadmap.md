# Roadmap efter MVP

## v1 — lokal intelligens

Implementerat i appen (se [v1-asr.md](v1-asr.md)):

- Jobbköl + whisper.cpp sidecar, default `ggml-small`, språk auto + override per podcast
- Transkript-UI: sök, klick → tid
- Kapitel: inbäddade ID3 + genererade
- Reklamheuristik → **skip**
- Inkorg/triage, filter, bokmärken, statistik, sideloading
- BYOK cloud-ASR avstängt
- Windows SMTC (play/pause via systemets mediekontroller när souvlaki kan binda HWND)

## Musikmodul (senare, separat)

Se [music-roadmap.md](music-roadmap.md). Inte i podkastabeller. Delad engine enligt [audio.md](audio.md).

## UPnP/DLNA (efter lokal spelare)

Se [upnp-mediaserver.md](upnp-mediaserver.md). Server först (Onkyo TX-RZ50). Renderer/control point är v2+.

## Medvetet utanför den här branchen

Whisper, ad-ML, full UPnP-serverimplementation, musikbibliotek, Car Mode, Chromecast/Sonos.
