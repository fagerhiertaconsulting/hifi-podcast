# Roadmap efter MVP

## v1 — lokal intelligens

- whisper.cpp (sidecar), default `ggml-small`, språk auto + override per podcast
- Transkript-UI: sök, klick → tid
- Kapitel: inbäddade + genererade (tystnad/energi + transkript)
- Reklamheuristik → **skip** (cut är v2 + privatbruks-disclaimer)
- Inbox/triage, filter, bokmärken, statistik, sideloading
- BYOK cloud-ASR avstängt som default
- Windows SMTC om det inte hunnits i MVP-plus

## Musikmodul (senare, separat)

Se [music-roadmap.md](music-roadmap.md). Inte i podkastabeller. Delad engine enligt [audio.md](audio.md).

## UPnP/DLNA (efter lokal spelare)

Se [upnp-mediaserver.md](upnp-mediaserver.md). Server först (Onkyo TX-RZ50). Renderer/control point är v2+.

## Medvetet utanför den här branchen

Whisper, ad-ML, full UPnP-serverimplementation, musikbibliotek, Car Mode, Chromecast/Sonos.
