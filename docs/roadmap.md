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

## UPnP/DLNA (PR #3 — podcastträd)

Se [upnp-mediaserver.md](upnp-mediaserver.md). **Media Server** mot Onkyo TX-RZ50: ContentDirectory `Podcasts/<show>/<episode>` streamar originalfiler med HTTP Range. `Music/` är en tom stub. Renderer/control point *i appen* är v2+. Transkod-fallback är inte med i v1 av servern.

## Medvetet utanför

Musikbibliotek / FLAC-DSD-UI, native Symphonia/WASAPI exclusive, moln-ASR, reklam-cut ML, Chromecast/Sonos-specifikt, in-app UPnP-renderer.
