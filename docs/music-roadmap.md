# HIFI podcast — musikmodul

**Status:** MVP i appen (PR #8). SACD-ISO, foobar-skinn och molnstreaming är **inte** med.

## Ambition

Lokal uppspelning i foobar2000-klass:

- **FLAC**, **DSD** (.dsf/.dff) nu; **SACD-ISO** rippning senare
- Minimal signalväg, delad native-motor (ASIO FiiO, WASAPI, Original→Utgång)
- Inte ett andra podcast-UI

## Separation

| | Podcast | Musik |
|---|---|---|
| UI | Bibliotek, kö, avsnitt | **Musik**: album/spår, import mapp |
| Data | `podcast` / `episode` / … | `music_album`, `music_track`, `music_folder`, `music_playback_state` |
| Kö | Up Next för avsnitt | Nästa spår i **samma album** (ingen blandad kö) |
| Motor | Delad hi-fi engine | Delad hi-fi engine |
| Output | Samma DAC / exclusive-läge | Samma |
| Disk | `media/podcasts/<id>/` | `media/music/<album_id>/<track_id>.<ext>` |

Blanda aldrig podcast-kö med musikbibliotek. Gemensamt är motor + globala output-inställningar (enhet, buffer, exclusive WASAPI).

## MVP (den här PR:n)

1. Importera mapp från disk (rekursivt FLAC/WAV/AIFF/DSD/MP3/…). Original **kopieras** till `media/music/` (ingen omkodning).
2. Metadata: taggar (ID3, FLAC Vorbis) + mappstruktur `Artist/Album/01 Titel.flac`.
3. Bläddra album → spår, spela via `engine_open` (samma väg som lokal podd/DSD).
4. Hoppa tystnad och reklam används **inte** för musik.

UPnP-trädets `Music/` är fortfarande en tom stub (Onkyo-podcastträdet oförändrat).
