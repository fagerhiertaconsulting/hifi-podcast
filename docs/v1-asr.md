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

På **Windows** hämtas `whisper-cli` **bara** via Inställningar:

- Inställningar → **Ladda ner whisper-cli** (eller **Ladda ner allt**), efter bekräftelse
- CUDA-zippen (~640 MB) laddas **aldrig** ner i bakgrunden vid prenumeration, avsnittsnedladdning eller transkribering

Om NVIDIA-GPU hittas (`nvcuda.dll`) hämtas **`whisper-cublas-12.4.0-bin-x64.zip`** (~640 MB, CUDA 12.4 + cuBLAS) i stället för CPU-zippen när du trycker på knappen. CPU-build som redan ligger i `bin\` **uppgraderas inte av sig själv** — tryck på knappen igen.

Zip hämtas från [ggml-org/whisper.cpp releases](https://github.com/ggml-org/whisper.cpp/releases). Exe och DLL packas ut i `bin\`.

`ggml-small` (~466 MB) hämtas **inte** tyst. SHA-256 verifieras mot Hugging Face-digest; vid mismatch raderas filen och nedladdningen misslyckas.

**Auto-transkribera** är avstängd som standard för nya podcasts.

### PowerShell (manuellt)

Stäng `hifi-podcast` / `npm run tauri dev` först, annars är `whisper-cli.exe` låst.

CUDA (NVIDIA, ~640 MB):

```powershell
Get-Process hifi-podcast, whisper-cli -ErrorAction SilentlyContinue | Stop-Process -Force
$bin = Join-Path $env:LOCALAPPDATA "consulting.fagerhierta.hifi-podcast\bin"
New-Item -ItemType Directory -Force -Path $bin | Out-Null
$zip = Join-Path $bin "whisper-cublas-12.4.0-bin-x64.zip"
curl.exe -L --http1.1 -o $zip "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-cublas-12.4.0-bin-x64.zip"
Expand-Archive -Path $zip -DestinationPath $bin -Force
Get-ChildItem -Path $bin -Recurse -File | Where-Object {
  ($_.Name -ieq "whisper-cli.exe" -or $_.Extension -ieq ".dll") -and $_.DirectoryName -ne $bin
} | ForEach-Object {
  Copy-Item $_.FullName -Destination (Join-Path $bin $_.Name) -Force
}
Set-Content -Path (Join-Path $bin "hifi-backend.txt") -Value "cuda" -Encoding ascii
Remove-Item $zip -ErrorAction SilentlyContinue
Get-Item $bin\whisper-cli.exe, $bin\ggml-cuda.dll, $bin\cublas64_12.dll
```

CPU-only (om du inte har NVIDIA):

```powershell
Get-Process hifi-podcast, whisper-cli -ErrorAction SilentlyContinue | Stop-Process -Force
$bin = Join-Path $env:LOCALAPPDATA "consulting.fagerhierta.hifi-podcast\bin"
New-Item -ItemType Directory -Force -Path $bin | Out-Null
$zip = Join-Path $bin "whisper-bin-x64.zip"
curl.exe -L --http1.1 -o $zip "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip"
Expand-Archive -Path $zip -DestinationPath $bin -Force
Get-ChildItem -Path $bin -Recurse -File | Where-Object {
  ($_.Name -ieq "whisper-cli.exe" -or $_.Extension -ieq ".dll") -and $_.DirectoryName -ne $bin
} | ForEach-Object {
  Copy-Item $_.FullName -Destination (Join-Path $bin $_.Name) -Force
}
Set-Content -Path (Join-Path $bin "hifi-backend.txt") -Value "cpu" -Encoding ascii
Remove-Item $zip -ErrorAction SilentlyContinue
Get-Item $bin\whisper-cli.exe
```

Körs ungefär:

```
whisper-cli -m ggml-small.bin -f avsnitt.mp3 -l auto -oj -osrt -of …
```

`-l` är `auto`, `sv` eller `en` från podcastinställningen.

**CPU-tid:** `ggml-small` på CPU för ett 40-minutersavsnitt tar ofta **15–60 minuter**. Med **CUDA** på RTX 40-serie (målmaskin: RTX 4070 Laptop) är samma jobb ofta några minuter. GPU används när NVIDIA hittas och whisper-cli är CUDA-bygget. Annars `-ng` (CPU). I Aktivitetshanteraren: GPU-grafen **CUDA** (inte 3D) — 3D kan stå på 0 % medan CUDA arbetar.

**Paus under uppspelning:** transkribering startar inte medan ljud spelas (podd eller musik). Ett pågående `whisper-cli` fryses och fortsätter när uppspelningen stannar, utan att börja om från början.

Jobbet syns som `running` under Statistik → Jobbkö. Logg: `%LOCALAPPDATA%\consulting.fagerhierta.hifi-podcast\logs\whisper-last.log`.

Ett Whisper-jobb körs i taget. Om appen startas om medan ett jobb hänger som `running` läggs det tillbaka i kön. Om CUDA-start misslyckas görs ett försök med CPU (`-ng`).

## 3. Flöde

1. Avsnitt laddas ner (originalfil).
2. Om **Auto-transkribera** är på köas jobbet `transcribe`.
3. Efter lyckad ASR köas `generate_chapters` och `detect_ads`.
4. UI: transkriptpanel (sök i hela biblioteket, klick på träff öppnar det avsnitt som texten tillhör — inte det avsnitt som redan spelas), kapitel, reklamskip under uppspelning om **Hoppa reklam** är på.

## 4. Kapitel

Först inbäddade ID3-kapitel. Annars genererade från transkriptluckor / ~6 min block.

## 5. Reklam

Frasheuristik (specifika sponsorfraser, t.ex. «sponsrad av», «i samarbete med», «this podcast is sponsored») plus tystnadsluckor. Den ensamma frasen «reklam» matchas **inte**, för att undvika falska träffar i vanligt tal. **Skip** under playback. Cut-läge är v2.
