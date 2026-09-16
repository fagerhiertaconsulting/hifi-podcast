use crate::error::{AppError, AppResult};
use crate::intel::{parse_srt, parse_whisper_json, Segment};
use crate::models::AsrStatus;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

pub const MODEL_NAME: &str = "ggml-small.bin";
pub const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";

pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

pub fn bin_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

pub fn model_path(data_dir: &Path) -> PathBuf {
    models_dir(data_dir).join(MODEL_NAME)
}

pub fn find_cli(data_dir: &Path) -> Option<PathBuf> {
    let dir = bin_dir(data_dir);
    let local_names = [
        "whisper-cli.exe",
        "whisper-cli",
        "main.exe",
        "whisper.exe",
    ];
    for name in local_names {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let names = if cfg!(windows) {
        ["whisper-cli.exe", "whisper-cli", "main.exe"]
    } else {
        ["whisper-cli", "whisper", "main"]
    };
    for name in names {
        if let Ok(path) = which::which(name) {
            return Some(path);
        }
    }
    None
}

pub fn status(data_dir: &Path) -> AsrStatus {
    let model = model_path(data_dir);
    let cli = find_cli(data_dir);
    let model_present = model.is_file();
    let cli_present = cli.is_some();
    let hint = match (model_present, cli_present) {
        (true, true) => "Whisper är redo. Transkribering körs lokalt (BYOK av).".into(),
        (false, true) => "Ladda ner ggml-small via Inställningar.".into(),
        (true, false) => {
            "whisper-cli saknas. Använd Inställningar → Ladda ner whisper-cli.".into()
        }
        (false, false) => {
            "Ladda ner whisper-cli och ggml-small via Inställningar.".into()
        }
    };
    AsrStatus {
        byok_enabled: false,
        model_name: MODEL_NAME.into(),
        model_present,
        model_path: model.display().to_string(),
        cli_present,
        cli_path: cli.map(|p| p.display().to_string()),
        hint,
    }
}

pub async fn download_model(data_dir: &Path) -> AppResult<PathBuf> {
    let dest = model_path(data_dir);
    tokio::fs::create_dir_all(models_dir(data_dir)).await?;
    let tmp = dest.with_extension("bin.part");
    let client = reqwest::Client::builder()
        .user_agent("HIFI-podcast/0.1")
        .build()?;
    let response = client.get(MODEL_URL).send().await?;
    if !response.status().is_success() {
        return Err(AppError::msg(format!(
            "Kunde inte hämta modell (HTTP {}).",
            response.status()
        )));
    }
    let mut hasher = Sha256::new();
    let mut file = tokio::fs::File::create(&tmp).await?;
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    let _digest = hex::encode(hasher.finalize());
    tokio::fs::rename(&tmp, &dest).await?;
    Ok(dest)
}

const GITHUB_RELEASES: &str =
    "https://api.github.com/repos/ggml-org/whisper.cpp/releases?per_page=20";
const FALLBACK_WIN_X64: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip";

fn preferred_cli_asset() -> &'static str {
    if cfg!(all(windows, target_arch = "aarch64")) {
        "whisper-bin-win-cpu-arm64.zip"
    } else {
        "whisper-bin-x64.zip"
    }
}

pub fn pick_release_asset_url(json: &str, asset_name: &str) -> Option<String> {
    let releases: serde_json::Value = serde_json::from_str(json).ok()?;
    let items = releases.as_array()?;
    for rel in items {
        let Some(assets) = rel.get("assets").and_then(|a| a.as_array()) else {
            continue;
        };
        for asset in assets {
            if asset.get("name").and_then(|n| n.as_str()) == Some(asset_name) {
                if let Some(url) = asset
                    .get("browser_download_url")
                    .and_then(|u| u.as_str())
                {
                    return Some(url.to_string());
                }
            }
        }
    }
    None
}

async fn http_client() -> AppResult<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("HIFI-podcast/0.1 (+https://github.com/fagerhiertaconsulting/hifi-podcast)")
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()?)
}

async fn download_to_file(url: &str, dest: &Path) -> AppResult<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let client = http_client().await?;
    let response = client.get(url).send().await?;
    if !response.status().is_success() {
        return Err(AppError::msg(format!(
            "Nedladdning misslyckades (HTTP {}) för {url}",
            response.status()
        )));
    }
    let tmp = dest.with_extension("part");
    let mut file = tokio::fs::File::create(&tmp).await?;
    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        file.write_all(&chunk?).await?;
    }
    file.flush().await?;
    drop(file);
    tokio::fs::rename(tmp, dest).await?;
    Ok(())
}

fn extract_cli_zip(zip_path: &Path, dest_dir: &Path) -> AppResult<()> {
    std::fs::create_dir_all(dest_dir)?;
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::msg(format!("Kunde inte öppna whisper-zip: {e}")))?;
    let mut extracted = 0u32;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::msg(format!("zip-fel: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let name = PathBuf::from(entry.name());
        if name.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            continue;
        }
        let file_name = match name.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let lower = file_name.to_ascii_lowercase();
        let keep = lower.ends_with(".exe")
            || lower.ends_with(".dll")
            || lower == "whisper-cli"
            || lower.ends_with(".so")
            || lower.ends_with(".dylib");
        if !keep {
            continue;
        }
        let out = dest_dir.join(file_name);
        let mut outfile = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut outfile)?;
        extracted += 1;
    }
    if extracted == 0 {
        return Err(AppError::msg(
            "Zip-filen innehöll ingen whisper-cli/dll. Prova manuell installation (docs/v1-asr.md).",
        ));
    }
    Ok(())
}

pub async fn download_cli(data_dir: &Path) -> AppResult<PathBuf> {
    if !cfg!(windows) {
        return Err(AppError::msg(
            "Automatisk whisper-cli-nedladdning stöds på Windows. På andra OS: installera whisper-cli i PATH.",
        ));
    }
    let dest_dir = bin_dir(data_dir);
    tokio::fs::create_dir_all(&dest_dir).await?;
    let client = http_client().await?;
    let url = match client.get(GITHUB_RELEASES).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await.unwrap_or_default();
            pick_release_asset_url(&body, preferred_cli_asset())
                .unwrap_or_else(|| FALLBACK_WIN_X64.to_string())
        }
        _ => FALLBACK_WIN_X64.to_string(),
    };
    let zip_path = dest_dir.join("whisper-cli-download.zip");
    download_to_file(&url, &zip_path).await?;
    extract_cli_zip(&zip_path, &dest_dir)?;
    let _ = tokio::fs::remove_file(&zip_path).await;
    find_cli(data_dir).ok_or_else(|| {
        AppError::msg("whisper-cli.exe hittades inte efter uppackning.")
    })
}

pub async fn ensure_cli(data_dir: &Path) -> AppResult<PathBuf> {
    if let Some(existing) = find_cli(data_dir) {
        return Ok(existing);
    }
    download_cli(data_dir).await
}

pub async fn transcribe_file(
    data_dir: &Path,
    audio: &Path,
    language: &str,
    progress: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> AppResult<(Vec<Segment>, String)> {
    let cli = ensure_cli(data_dir).await?;
    let model = model_path(data_dir);
    if !model.is_file() {
        return Err(AppError::msg("ggml-small.bin saknas. Ladda ner modellen först."));
    }
    if !audio.is_file() {
        return Err(AppError::msg(format!(
            "Ljudfilen saknas på disk: {}",
            audio.display()
        )));
    }
    let out_dir = audio.parent().unwrap_or(audio);
    let stem = format!(
        "whisper-{}",
        audio.file_stem().and_then(|s| s.to_str()).unwrap_or("out")
    );
    let prefix = out_dir.join(&stem);
    let lang = if language.trim().is_empty() || language == "auto" {
        "auto"
    } else {
        language
    };
    let logs = data_dir.join("logs");
    tokio::fs::create_dir_all(&logs).await?;
    let log_path = logs.join("whisper-last.log");
    let mut log = tokio::fs::File::create(&log_path).await?;
    let header = format!(
        "cli={} model={} audio={}\n",
        cli.display(),
        model.display(),
        audio.display()
    );
    log.write_all(header.as_bytes()).await?;

    let mut cmd = tokio::process::Command::new(&cli);
    cmd.current_dir(bin_dir(data_dir))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("GGML_VK_VISIBLE_DEVICES", "")
        .arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(audio)
        .arg("-l")
        .arg(lang)
        .arg("-ng")
        .arg("-pp")
        .arg("-oj")
        .arg("-osrt")
        .arg("-of")
        .arg(&prefix)
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().map_err(|e| {
        AppError::msg(format!(
            "Kunde inte starta whisper-cli ({e}). Kontrollera att DLL-filer ligger i {}.",
            bin_dir(data_dir).display()
        ))
    })?;
    if let Some(out) = child.stdout.take() {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut r = out;
            let mut buf = [0u8; 8192];
            loop {
                match r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = log.write_all(line.as_bytes()).await;
            let _ = log.write_all(b"\n").await;
            if let Some(tx) = &progress {
                if line_looks_useful(&line) {
                    let _ = tx.send(line);
                }
            }
        }
    }
    let status = child.wait().await.map_err(|e| {
        AppError::msg(format!("whisper-cli avbröts: {e}"))
    })?;
    drop(progress);
    if !status.success() {
        let tail = tokio::fs::read_to_string(&log_path)
            .await
            .unwrap_or_default();
        let snippet: String = tail
            .chars()
            .rev()
            .take(800)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        return Err(AppError::msg(format!(
            "whisper-cli misslyckades (kod {:?}). Logg: {} — {}",
            status.code(),
            log_path.display(),
            snippet
        )));
    }
    if let Some((segments, used)) = read_whisper_outputs(&prefix, audio, out_dir, &stem, lang).await? {
        return Ok((segments, used));
    }
    Err(AppError::msg(format!(
        "whisper-cli skrev ingen json/srt. Se {}.",
        log_path.display()
    )))
}

fn line_looks_useful(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("progress")
        || l.contains('%')
        || l.contains("whisper_")
        || l.contains("system_info")
        || l.contains("error")
        || l.contains("n_len")
}

async fn read_whisper_outputs(
    prefix: &Path,
    audio: &Path,
    out_dir: &Path,
    stem: &str,
    lang: &str,
) -> AppResult<Option<(Vec<Segment>, String)>> {
    let mut json_candidates = vec![
        prefix.with_extension("json"),
        PathBuf::from(format!("{}.json", prefix.display())),
        audio.with_extension("json"),
        out_dir.join(format!("{stem}.json")),
    ];
    let mut srt_candidates = vec![
        prefix.with_extension("srt"),
        PathBuf::from(format!("{}.srt", prefix.display())),
        audio.with_extension("srt"),
        out_dir.join(format!("{stem}.srt")),
    ];
    if let Ok(mut rd) = tokio::fs::read_dir(out_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let name = entry.file_name();
            let n = name.to_string_lossy();
            if n.starts_with(stem) && n.ends_with(".json") {
                json_candidates.push(entry.path());
            }
            if n.starts_with(stem) && n.ends_with(".srt") {
                srt_candidates.push(entry.path());
            }
        }
    }
    for json_path in json_candidates {
        if json_path.is_file() {
            let raw = tokio::fs::read_to_string(&json_path).await?;
            if let Ok(segs) = parse_whisper_json(&raw) {
                if !segs.is_empty() {
                    return Ok(Some((segs, lang.to_string())));
                }
            }
        }
    }
    for srt_path in srt_candidates {
        if srt_path.is_file() {
            let raw = tokio::fs::read_to_string(&srt_path).await?;
            let segs = parse_srt(&raw);
            if !segs.is_empty() {
                return Ok(Some((segs, lang.to_string())));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn picks_x64_zip_from_releases_json() {
        let json = r#"[{"assets":[{"name":"other.zip","browser_download_url":"https://x"},{"name":"whisper-bin-x64.zip","browser_download_url":"https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip"}]}]"#;
        assert_eq!(
            pick_release_asset_url(json, "whisper-bin-x64.zip").as_deref(),
            Some("https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip")
        );
    }

    #[test]
    fn extracts_cli_from_nested_zip() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("t.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("Release/whisper-cli.exe", opts).unwrap();
            zip.write_all(b"fake-cli").unwrap();
            zip.start_file("Release/ggml.dll", opts).unwrap();
            zip.write_all(b"dll").unwrap();
            zip.finish().unwrap();
        }
        let dest = bin_dir(dir.path());
        extract_cli_zip(&zip_path, &dest).unwrap();
        assert_eq!(
            std::fs::read(dest.join("whisper-cli.exe")).unwrap(),
            b"fake-cli"
        );
        assert!(dest.join("ggml.dll").is_file());
        assert!(find_cli(dir.path()).is_some());
    }
}
