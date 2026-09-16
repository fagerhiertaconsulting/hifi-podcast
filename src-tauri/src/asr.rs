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
) -> AppResult<(Vec<Segment>, String)> {
    let cli = ensure_cli(data_dir).await?;
    let model = model_path(data_dir);
    if !model.is_file() {
        return Err(AppError::msg("ggml-small.bin saknas. Ladda ner modellen först."));
    }
    let out_dir = audio.parent().unwrap_or(audio);
    let stem = format!("whisper-{}", audio.file_stem().and_then(|s| s.to_str()).unwrap_or("out"));
    let prefix = out_dir.join(&stem);
    let lang = if language.trim().is_empty() || language == "auto" {
        "auto"
    } else {
        language
    };
    let mut cmd = tokio::process::Command::new(&cli);
    cmd.arg("-m")
        .arg(&model)
        .arg("-f")
        .arg(audio)
        .arg("-l")
        .arg(lang)
        .arg("-oj")
        .arg("-osrt")
        .arg("-of")
        .arg(&prefix)
        .kill_on_drop(true);
    let output = cmd.output().await.map_err(|e| {
        AppError::msg(format!("Kunde inte starta whisper-cli: {e}"))
    })?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::msg(format!(
            "whisper-cli misslyckades: {}",
            err.chars().take(500).collect::<String>()
        )));
    }
    let json_path = PathBuf::from(format!("{}.json", prefix.display()));
    let srt_path = PathBuf::from(format!("{}.srt", prefix.display()));
    if json_path.is_file() {
        let raw = tokio::fs::read_to_string(&json_path).await?;
        if let Ok(segs) = parse_whisper_json(&raw) {
            if !segs.is_empty() {
                return Ok((segs, lang.to_string()));
            }
        }
    }
    if srt_path.is_file() {
        let raw = tokio::fs::read_to_string(&srt_path).await?;
        return Ok((parse_srt(&raw), lang.to_string()));
    }
    Err(AppError::msg("whisper-cli skrev ingen json/srt."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_x64_zip_from_releases_json() {
        let json = r#"[{"assets":[{"name":"other.zip","browser_download_url":"https://x"},{"name":"whisper-bin-x64.zip","browser_download_url":"https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip"}]}]"#;
        assert_eq!(
            pick_release_asset_url(json, "whisper-bin-x64.zip").as_deref(),
            Some("https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip")
        );
    }
}
