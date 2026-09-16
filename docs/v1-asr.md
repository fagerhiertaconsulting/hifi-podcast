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

På **Windows** laddas `whisper-cli` ner automatiskt:

- Inställningar → **Ladda ner whisper-cli** (eller **Ladda ner allt**)
- Första transkriberingen, om CLI saknas i `bin\` och `PATH`

Zip hämtas från [ggml-org/whisper.cpp releases](https://github.com/ggml-org/whisper.cpp/releases) (`whisper-bin-x64.zip`). Exe och DLL packas ut i `bin\`.

`ggml-small` är stor och hämtas **inte** tyst i bakgrunden — den kräver knappen ovan.

### PowerShell (manuellt, om GitHub är blockerat)

```powershell
$bin = Join-Path $env:LOCALAPPDATA "consulting.fagerhierta.hifi-podcast\bin"
New-Item -ItemType Directory -Force -Path $bin | Out-Null
$zip = Join-Path $bin "whisper-bin-x64.zip"
Invoke-WebRequest -Uri "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip" -OutFile $zip
Expand-Archive -Path $zip -DestinationPath $bin -Force
Get-ChildItem -Path $bin -Recurse -Include whisper-cli.exe,*.dll | ForEach-Object {
  Copy-Item $_.FullName -Destination (Join-Path $bin $_.Name) -Force
}
Remove-Item $zip -ErrorAction SilentlyContinue
Get-ChildItem $bin\whisper-cli.exe
```

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
