# HIFI podcast — UPnP/DLNA Media Server

**Målenhet:** Onkyo TX-RZ50  
**Roll:** UPnP AV Media Server (DLNA-kompatibel)  
**Fas:** efter lokal player-MVP. Inte implementerad i den här branchen.

## Innehåll

- Podcast-nedladdningar (original i media-root)
- Senare: musikbibliotek

**Separata browse-träd** i ContentDirectory:

```
HIFI podcast
 ├─ Podcasts
 │   └─ <show>
 │       └─ <episode originalfil>
 └─ Music
     └─ … (senare)
```

Receivern ska kunna bläddra mappar och spela. HIFI podcast är servern; Onkyo är renderer/control point. Valfri UPnP-renderer *inifrån* appen är v2+.

## Stream-policy

1. Skicka **originalfilen** (FLAC / m4a / mp3 / …) med korrekt MIME.
2. Transkod **bara** om TX-RZ50 (eller annan renderer) nekar formatet.
3. Transkod är en tillfällig ström, inte en ersättningsfil i media-root.

## Media-root (måste inte målas in i ett hörn)

MVP sparar redan under appens local data:

`%LOCALAPPDATA%\consulting.fagerhierta.hifi-podcast\media\`

Regler så serverläge förblir möjligt:

- En **rot** för allt stream-bart innehåll, med underskataloger per library.
- Metadata (titel, MIME, duration, bitrate) i SQLite — servern ska inte behöva gissa från filnamn.
- Inga WebView-only URL:er som enda åtkomstväg; Rust ska kunna öppna filen via absolut sökväg.
- Firewall/bind-adress och en intern HTTP(S)-stream för UPnP `res` ska kunna sitta i samma process som download manager.

När kataloglayouten delas upp i `media/podcasts` och `media/music` görs det med migrering, inte med en andra “hemlig” mapp som bara UI känner till.

## Relaterat

Delad motor dokumenteras i [audio.md](audio.md). Musikträdet i [music-roadmap.md](music-roadmap.md).
