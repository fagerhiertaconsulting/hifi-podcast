# HIFI podcast — UPnP/DLNA Media Server

**Målenhet:** Onkyo TX-RZ50  
**Roll:** UPnP AV Media Server (DLNA-kompatibel DMS)  
**Fas:** PR #3. Appen är servern; Onkyo är renderer/control point. Ingen in-app UPnP-renderer.

## Stackval (kvalitet först)

Det finns ingen välunderhållen Rust-crate som är en komplett DLNA *Media Server* (DMS). `rupnp` är en **control point**-klient, inte en server. MiniDLNA/ReadyMedia är C.

PR #3 implementerar därför en **minimal, testad** stack i-process:

| Del | Implementation |
|-----|----------------|
| SSDP | Egen HTTPU: `NOTIFY` alive/byebye + svar på `M-SEARCH` (`ssdp.rs`) |
| HTTP | `axum` 0.8 på konfigurerbar bind/port (standard **8200**, tom bind = upptäckt LAN-IPv4) |
| Device | `urn:schemas-upnp-org:device:MediaServer:1` + DLNA DMS-1.50 |
| Tjänster | ContentDirectory:1 + ConnectionManager:1 |
| `res` | Originalfil under media-root, **HTTP Range** (`DLNA.ORG_OP=01`, `CI=0`) |

Ingen FFmpeg-transkod i v1. Om TX-RZ50 nekar ett format (t.ex. udda container som `application/octet-stream`) streamas filen ändå orörd; dokumentera/byt feed-format. Transkod som tillfällig ström är en senare fallback, aldrig en ersättningsfil i media-root.

## Innehållsträd (ContentDirectory)

```
HIFI podcast          (id 0)
 ├─ Podcasts          (id podcasts)
 │   └─ <show>        (id show:<podcast_id>)
 │       └─ <episode> (id ep:<episode_id>, originalfil)
 └─ Music             (id music, tom stub — ingen musikimplementation)
```

Titlar, duration och MIME kommer från **SQLite** (`episode.title`, `episode.duration_ms`, `download.mime`), inte från filnamn. `res`-URL: `http://<lan-ip>:<port>/media/episode/<id>`.

## Media-root

En rot, app local data:

`%LOCALAPPDATA%\consulting.fagerhierta.hifi-podcast\media\`

Layout efter PR #3 (med migrering från MVP):

```
media/
  podcasts/<podcast_id>/<episode_id>.<ext>
  music/          (tom katalog, för senare)
```

Befintliga `media/<podcast_id>/…` flyttas till `media/podcasts/<podcast_id>/…` och `download.file_path` uppdateras **bara om filen faktiskt flyttades**. Om målet redan finns lämnas källfil och DB-sökväg orörda.

## Exponering (LAN opt-in, ingen autentisering)

DLNA/UPnP AV har **ingen inloggning** på SOAP (ContentDirectory), ConnectionManager eller media-GET. Det är normalt för receivrar men **riskabelt** om tjänsten nås utanför hemmanätet.

- Servern startar **inte** automatiskt. Du måste trycka **Starta mediaserver**.
- Tom/standard bindningsadress lyssnar **inte** på `0.0.0.0`. HTTP och SSDP UDP 1900 binds till **upptäckt LAN-IPv4**.
- `0.0.0.0` (alla gränssnitt) tillåts bara om du skriver in det själv. Inställningarna varnar på svenska att biblioteket då kan nås utanför LAN.
- Ingen query-token eller auth-header på media-URL:er. Onkyo (och de flesta DLNA-renderers) hämtar `res` från DIDL som HTTP GET utan extra headers; en token skulle kunna bryta uppspelning. Skyddet är **LAN-bind + explicit start + varning + brandvägg**.
- Använd bara på **betrott LAN**. Stoppa servern när den inte behövs.

## Starta på Windows och testa mot Onkyo TX-RZ50

1. Ladda ner avsnitt i HIFI podcast (original, ingen omkodning).
2. **Inställningar → UPnP/DLNA-mediaserver**
   - Port: `8200` (eller annan ledig TCP-port).
   - Bindningsadress: **lämna tom** för upptäckt LAN-IPv4 (rekommenderat). Fyll i PC:ns IPv4 om auto-detektering misslyckas. Skriv `0.0.0.0` bara om du medvetet vill lyssna på alla gränssnitt (varning i UI).
   - **Starta mediaserver**.
3. Windows kan visa en brandväggsdialog — tillåt appen på **privat** nätverk. Inget admin-konto krävs för att starta servern; appen lägger inte till brandväggsregler själv.
4. Om receivern inte hittar servern, öppna manuellt:
   - **TCP** den valda porten (in, privat profil) — HTTP `device.xml` + media.
   - **UDP 1900** — SSDP på samma bind som HTTP. Om en annan DMS redan äger 1900 skickas ändå `NOTIFY`; M-SEARCH kan då missas.
5. På TX-RZ50: Network / NET / DLNA (menyn varierar med firmware) → välj **HIFI podcast** → **Podcasts** → show → avsnitt. Play ska seeka (Range).
6. Stoppa servern från samma inställningssida när den inte behövs.

Sanity-check från en annan dator på LAN: öppna `http://<pc-ip>:8200/upnp/device.xml`.

## Relaterat

Delad motor: [audio.md](audio.md). Musikträd: [music-roadmap.md](music-roadmap.md).
