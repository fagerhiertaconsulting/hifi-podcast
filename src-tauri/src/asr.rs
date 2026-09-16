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
    let local = if cfg!(windows) {
        bin_dir(data_dir).join("whisper-cli.exe")
    } else {
        bin_dir(data_dir).join("whisper-cli")
    };
    if local.is_file() {
        return Some(local);
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
            "Lägg whisper-cli i appens bin-mapp eller i PATH (se docs/v1-asr.md).".into()
        }
        (false, false) => {
            "Ladda ner ggml-small och installera whisper.cpp CLI (se docs/v1-asr.md).".into()
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

pub async fn transcribe_file(
    data_dir: &Path,
    audio: &Path,
    language: &str,
) -> AppResult<(Vec<Segment>, String)> {
    let cli = find_cli(data_dir).ok_or_else(|| {
        AppError::msg("whisper-cli saknas. Se docs/v1-asr.md.")
    })?;
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
