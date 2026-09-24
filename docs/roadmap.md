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

## Native hi-fi engine (PR #4)

Symphonia-avkodning + **ASIO som primär utgång** (FiiO ASIO Driver, samma som Qobuz) med WASAPI delat/exclusive som fallback. WebView2 kvar som omkopplare. Musikbibliotek-UI är **inte** med — tomma hyllor väntar på PR #5+. Se [audio.md](audio.md).

## Musikmodul (MVP, separat från podcast)

Se [music-roadmap.md](music-roadmap.md). Egna `music_*`-tabeller. Delad engine enligt [audio.md](audio.md). Inte i podkastabeller.

## UPnP/DLNA (PR #3 — podcastträd)

Se [upnp-mediaserver.md](upnp-mediaserver.md). **Media Server** mot Onkyo TX-RZ50: ContentDirectory `Podcasts/<show>/<episode>` och `Music/<album>/<track>` streamar originalfiler med HTTP Range. Musik ligger kvar där den indexerades. Renderer/control point *i appen* är v2+. Transkod-fallback är inte med.

## Medvetet utanför

## Medvetet utanför

foobar-skinn, SACD-ISO-rippning, moln-ASR, reklam-cut ML, Chromecast/Sonos-specifikt, in-app UPnP-renderer.
