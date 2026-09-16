# v1 — lokal ASR (whisper.cpp)

HIFI podcast transkriberar **på maskinen**. Moln-ASR (BYOK) finns som kodkrok men är **avstängt**.

## 1. Modell (`ggml-small`)

Inställningar → **Ladda ner ggml-small** (~466 MB).

Fil: `%LOCALAPPDATA%\consulting.fagerhierta.hifi-podcast\models\ggml-small.bin`

Källa: Hugging Face `ggerganov/whisper.cpp`.

## 2. Sidecar `whisper-cli`

Appen letar i denna ordning:

1. `%LOCALAPPDATA%\consulting.fagerhierta.hifi-podcast\bin\whisper-cli.exe`
2. `whisper-cli` / `whisper-cli.exe` i `PATH`

Lägg en Windows-build från [whisper.cpp releases](https://github.com/ggerganov/whisper.cpp/releases) i `bin\`-mappen.

Körs ungefär:

```
whisper-cli -m ggml-small.bin -f avsnitt.mp3 -l auto -oj -osrt -of …
```

`-l` är `auto`, `sv` eller `en` från podcastinställningen.

## 3. Flöde

1. Avsnitt laddas ner (originalfil).
2. Om **Auto-transkribera** är på köas jobbet `transcribe`.
3. Efter lyckad ASR köas `generate_chapters` och `detect_ads`.
4. UI: transkriptpanel (sök, klick → tid), kapitel, reklamskip under uppspelning om **Hoppa reklam** är på.

## 4. Kapitel

Först inbäddade ID3-kapitel. Annars genererade från transkriptluckor / ~6 min block.

## 5. Reklam

Frasheuristik + tystnadsluckor. **Skip** under playback. Cut-läge är v2.
