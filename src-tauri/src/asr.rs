use crate::error::{AppError, AppResult};
use crate::intel::{parse_srt, parse_whisper_json, Segment};
use crate::models::AsrStatus;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub const MODEL_NAME: &str = "ggml-small.bin";
pub const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin";
/// Hugging Face LFS SHA-256 for ggml-small.bin (ggerganov/whisper.cpp).
pub const MODEL_SHA256: &str = "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b";

pub fn models_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("models")
}

pub fn bin_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

pub fn model_path(data_dir: &Path) -> PathBuf {
    models_dir(data_dir).join(MODEL_NAME)
}

pub fn nvidia_gpu_present() -> bool {
    #[cfg(windows)]
    {
        std::path::Path::new(r"C:\Windows\System32\nvcuda.dll").is_file()
            || std::path::Path::new(r"C:\Windows\SysWOW64\nvcuda.dll").is_file()
            || which::which("nvidia-smi").is_ok()
            || nvidia_driver_registered()
    }
    #[cfg(not(windows))]
    {
        which::which("nvidia-smi").is_ok()
            || Path::new("/dev/nvidia0").exists()
            || Path::new("/proc/driver/nvidia/version").is_file()
    }
}

#[cfg(windows)]
fn nvidia_driver_registered() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\NVIDIA Corporation\\Global")
        .is_ok()
}

/// CUDA whisper-cli uses the NVIDIA GPU (RTX 4070 Laptop on the target machine).
/// A CPU build cannot, even when the GPU is present.
pub fn use_nvidia_gpu(nvidia_present: bool, cli_backend: &str) -> bool {
    nvidia_present && cli_backend.eq_ignore_ascii_case("cuda")
}

pub fn gpu_disable_arg(use_gpu: bool) -> Option<&'static str> {
    if use_gpu {
        None
    } else {
        Some("-ng")
    }
}

/// Shared playback flag. Transcription waits and freezes whisper-cli while this is set.
#[derive(Clone, Debug)]
pub struct PlaybackGate {
    playing: Arc<AtomicBool>,
}

impl PlaybackGate {
    pub fn new() -> Self {
        Self {
            playing: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_playing(&self, playing: bool) {
        self.playing.store(playing, Ordering::SeqCst);
    }

    pub fn is_playing(&self) -> bool {
        self.playing.load(Ordering::SeqCst)
    }
}

impl Default for PlaybackGate {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhisperPause {
    Suspend,
    Resume,
    Keep,
}

pub fn transcription_allowed(audio_playing: bool) -> bool {
    !audio_playing
}

pub fn whisper_pause_transition(audio_playing: bool, process_suspended: bool) -> WhisperPause {
    match (audio_playing, process_suspended) {
        (true, false) => WhisperPause::Suspend,
        (false, true) => WhisperPause::Resume,
        _ => WhisperPause::Keep,
    }
}

fn suspend_pid(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { kill(pid as i32, SIGSTOP) == 0 }
    }
    #[cfg(windows)]
    {
        windows_process(pid, true)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

fn resume_pid(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { kill(pid as i32, SIGCONT) == 0 }
    }
    #[cfg(windows)]
    {
        windows_process(pid, false)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
const SIGSTOP: i32 = if cfg!(target_os = "linux") { 19 } else { 17 };
#[cfg(unix)]
const SIGCONT: i32 = if cfg!(target_os = "linux") { 18 } else { 19 };

#[cfg(unix)]
unsafe extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

#[cfg(windows)]
fn windows_process(pid: u32, suspend: bool) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};
    unsafe {
        let handle = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
        if handle.is_null() {
            return false;
        }
        let status = if suspend {
            NtSuspendProcess(handle)
        } else {
            NtResumeProcess(handle)
        };
        let _ = CloseHandle(handle);
        status == 0
    }
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtSuspendProcess(process: *mut core::ffi::c_void) -> i32;
    fn NtResumeProcess(process: *mut core::ffi::c_void) -> i32;
}

#[cfg(all(unix, test))]
fn process_is_alive(pid: u32) -> bool {
    unsafe { kill(pid as i32, 0) == 0 }
}

pub fn cli_backend(data_dir: &Path) -> String {
    let dir = bin_dir(data_dir);
    if find_cli(data_dir).is_none() {
        return "saknas".into();
    }
    let cuda_markers = [
        "ggml-cuda.dll",
        "cublas64_12.dll",
        "cublas64_11.dll",
        "cudart64_12.dll",
        "cudart64_110.dll",
    ];
    if cuda_markers.iter().any(|n| dir.join(n).is_file()) {
        return "cuda".into();
    }
    if let Ok(s) = std::fs::read_to_string(dir.join("hifi-backend.txt")) {
        let t = s.trim().to_ascii_lowercase();
        if t == "cuda" || t == "cpu" {
            return t;
        }
    }
    "cpu".into()
}

pub fn find_cli(data_dir: &Path) -> Option<PathBuf> {
    let dir = bin_dir(data_dir);
    let local_names = ["whisper-cli.exe", "whisper-cli", "main.exe", "whisper.exe"];
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
    let nvidia_present = nvidia_gpu_present();
    let backend = cli_backend(data_dir);
    let hint = match (model_present, cli_present, backend.as_str(), nvidia_present) {
        (true, true, "cuda", true) => {
            "Whisper är redo med NVIDIA CUDA. Transkribering använder GPU och pausas medan ljud spelas."
                .into()
        }
        (true, true, "cuda", false) => {
            "Whisper CUDA är installerad men ingen NVIDIA-GPU hittades. Transkribering körs på CPU och pausas medan ljud spelas."
                .into()
        }
        (true, true, _, true) => {
            "Whisper är redo på CPU. NVIDIA GPU hittad — Ladda ner whisper-cli igen för CUDA (~640 MB)."
                .into()
        }
        (true, true, _, false) => {
            "Whisper är redo (CPU). Transkribering pausas medan ljud spelas.".into()
        }
        (false, true, _, _) => "Ladda ner ggml-small via Inställningar.".into(),
        (true, false, _, true) => {
            "whisper-cli saknas. Ladda ner whisper-cli (CUDA) via Inställningar.".into()
        }
        (true, false, _, false) => {
            "whisper-cli saknas. Använd Inställningar → Ladda ner whisper-cli.".into()
        }
        (false, false, _, true) => {
            "Ladda ner whisper-cli (CUDA) och ggml-small via Inställningar.".into()
        }
        (false, false, _, false) => {
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
        cli_backend: backend,
        nvidia_present,
        hint,
    }
}

pub async fn download_model(data_dir: &Path) -> AppResult<PathBuf> {
    let dest = model_path(data_dir);
    tokio::fs::create_dir_all(models_dir(data_dir)).await?;
    if dest.is_file() {
        match verify_file_sha256(&dest, MODEL_SHA256).await {
            Ok(()) => return Ok(dest),
            Err(_) => {
                tokio::fs::remove_file(&dest).await.ok();
            }
        }
    }
    let client = crate::net::http_client()?;
    let mut hasher = Sha256::new();
    crate::net::download_to_path(
        &client,
        MODEL_URL,
        &dest,
        crate::net::MAX_MODEL_BYTES,
        Some(&mut hasher),
    )
    .await?;
    let digest = hex::encode(hasher.finalize());
    if let Err(err) = verify_digest(&digest, MODEL_SHA256) {
        let _ = tokio::fs::remove_file(&dest).await;
        return Err(err);
    }
    Ok(dest)
}

pub fn verify_digest(digest: &str, expected: &str) -> AppResult<()> {
    if digest.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(AppError::msg(format!(
            "SHA-256 stämmer inte för {MODEL_NAME}. Förväntat {expected}, fick {digest}. Filen raderas. Försök ladda ner igen."
        )))
    }
}

async fn verify_file_sha256(path: &Path, expected: &str) -> AppResult<()> {
    let data = tokio::fs::read(path).await?;
    let digest = hex::encode(Sha256::digest(&data));
    verify_digest(&digest, expected)
}

const GITHUB_RELEASES: &str =
    "https://api.github.com/repos/ggml-org/whisper.cpp/releases?per_page=20";
const FALLBACK_WIN_X64: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-x64.zip";
const FALLBACK_WIN_CUDA: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-cublas-12.4.0-bin-x64.zip";

fn preferred_cli_assets() -> Vec<&'static str> {
    if nvidia_gpu_present() {
        vec![
            "whisper-cublas-12.4.0-bin-x64.zip",
            "whisper-cublas-11.8.0-bin-x64.zip",
            "whisper-bin-x64.zip",
        ]
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        vec!["whisper-bin-win-cpu-arm64.zip"]
    } else {
        vec!["whisper-bin-x64.zip"]
    }
}

pub fn pick_release_asset_url(json: &str, asset_name: &str) -> Option<String> {
    pick_first_asset_url(json, &[asset_name])
}

pub fn pick_first_asset_url(json: &str, asset_names: &[&str]) -> Option<String> {
    let releases: serde_json::Value = serde_json::from_str(json).ok()?;
    let items = releases.as_array()?;
    for name in asset_names {
        for rel in items {
            let Some(assets) = rel.get("assets").and_then(|a| a.as_array()) else {
                continue;
            };
            for asset in assets {
                if asset.get("name").and_then(|n| n.as_str()) == Some(*name) {
                    if let Some(url) = asset.get("browser_download_url").and_then(|u| u.as_str()) {
                        return Some(url.to_string());
                    }
                }
            }
        }
    }
    None
}

async fn http_client() -> AppResult<reqwest::Client> {
    crate::net::http_client()
}

async fn download_to_file(url: &str, dest: &Path) -> AppResult<()> {
    let client = http_client().await?;
    crate::net::download_to_path(&client, url, dest, crate::net::MAX_MODEL_BYTES, None).await?;
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
        if name
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
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
    let names = preferred_cli_assets();
    let fallback = if nvidia_gpu_present() {
        FALLBACK_WIN_CUDA
    } else {
        FALLBACK_WIN_X64
    };
    let client = http_client().await?;
    let url =
        match crate::net::get_limited(&client, GITHUB_RELEASES, crate::net::MAX_FEED_BYTES).await {
            Ok(body) => {
                let body = String::from_utf8_lossy(&body);
                pick_first_asset_url(&body, &names).unwrap_or_else(|| fallback.to_string())
            }
            Err(_) => fallback.to_string(),
        };
    let zip_path = dest_dir.join("whisper-cli-download.zip");
    download_to_file(&url, &zip_path).await?;
    extract_cli_zip(&zip_path, &dest_dir)?;
    let _ = tokio::fs::remove_file(&zip_path).await;
    let backend = if url.contains("cublas") || url.contains("cuda") {
        "cuda"
    } else {
        "cpu"
    };
    let _ = tokio::fs::write(dest_dir.join("hifi-backend.txt"), backend).await;
    find_cli(data_dir)
        .ok_or_else(|| AppError::msg("whisper-cli.exe hittades inte efter uppackning."))
}

pub fn require_cli(data_dir: &Path) -> AppResult<PathBuf> {
    find_cli(data_dir).ok_or_else(|| {
        AppError::msg(
            "whisper-cli saknas. Ladda ner den via Inställningar (CUDA-zip är ca 640 MB). Appen hämtar den aldrig automatiskt.",
        )
    })
}

async fn run_whisper_cli(
    cli: &Path,
    data_dir: &Path,
    model: &Path,
    audio: &Path,
    lang: &str,
    prefix: &Path,
    use_gpu: bool,
    gate: &PlaybackGate,
    progress: &Option<tokio::sync::mpsc::UnboundedSender<String>>,
    log: &mut tokio::fs::File,
) -> AppResult<std::process::ExitStatus> {
    let mut announced_pause = false;
    while !transcription_allowed(gate.is_playing()) {
        if !announced_pause {
            if let Some(tx) = progress {
                let _ = tx.send("Pausad medan ljud spelas.".into());
            }
            announced_pause = true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let mut cmd = tokio::process::Command::new(cli);
    cmd.current_dir(bin_dir(data_dir))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .arg("-m")
        .arg(model)
        .arg("-f")
        .arg(audio)
        .arg("-l")
        .arg(lang);
    if let Some(flag) = gpu_disable_arg(use_gpu) {
        cmd.arg(flag);
    }
    cmd.arg("-pp")
        .arg("-oj")
        .arg("-osrt")
        .arg("-of")
        .arg(prefix)
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
    let watchdog = child.id().map(|pid| {
        let gate = gate.clone();
        let progress = progress.clone();
        let suspended = Arc::new(AtomicBool::new(false));
        let suspended_flag = suspended.clone();
        let handle = tokio::spawn(async move {
            let mut is_suspended = false;
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                match whisper_pause_transition(gate.is_playing(), is_suspended) {
                    WhisperPause::Suspend => {
                        if suspend_pid(pid) {
                            is_suspended = true;
                            suspended_flag.store(true, Ordering::SeqCst);
                            if let Some(tx) = &progress {
                                let _ = tx.send("Pausad medan ljud spelas.".into());
                            }
                        }
                    }
                    WhisperPause::Resume => {
                        if resume_pid(pid) {
                            is_suspended = false;
                            suspended_flag.store(false, Ordering::SeqCst);
                            if let Some(tx) = &progress {
                                let _ = tx.send("Transkribering återupptas.".into());
                            }
                        }
                    }
                    WhisperPause::Keep => {}
                }
            }
        });
        (handle, pid, suspended)
    });
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
            if let Some(tx) = progress {
                if line_looks_useful(&line) {
                    let _ = tx.send(line);
                }
            }
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|e| AppError::msg(format!("whisper-cli avbröts: {e}")))?;
    if let Some((handle, pid, suspended)) = watchdog {
        handle.abort();
        if suspended.load(Ordering::SeqCst) {
            let _ = resume_pid(pid);
        }
    }
    Ok(status)
}

pub async fn transcribe_file(
    data_dir: &Path,
    audio: &Path,
    language: &str,
    gate: &PlaybackGate,
    progress: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> AppResult<(Vec<Segment>, String)> {
    let cli = require_cli(data_dir)?;
    let model = model_path(data_dir);
    if !model.is_file() {
        return Err(AppError::msg(
            "ggml-small.bin saknas. Ladda ner modellen först.",
        ));
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
    let backend = cli_backend(data_dir);
    let want_gpu = use_nvidia_gpu(nvidia_gpu_present(), &backend);
    let header = format!(
        "cli={} model={} audio={} backend={} nvidia={} gpu={}\n",
        cli.display(),
        model.display(),
        audio.display(),
        backend,
        nvidia_gpu_present(),
        want_gpu
    );
    log.write_all(header.as_bytes()).await?;

    let mut status = run_whisper_cli(
        &cli, data_dir, &model, audio, lang, &prefix, want_gpu, gate, &progress, &mut log,
    )
    .await?;
    if !status.success() && want_gpu {
        let _ = log
            .write_all(b"\n--- GPU failed, retrying CPU (-ng) ---\n")
            .await;
        if let Some(tx) = &progress {
            let _ = tx.send("GPU misslyckades, faller tillbaka till CPU…".into());
        }
        status = run_whisper_cli(
            &cli, data_dir, &model, audio, lang, &prefix, false, gate, &progress, &mut log,
        )
        .await?;
    }
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
    if let Some((segments, used)) =
        read_whisper_outputs(&prefix, audio, out_dir, &stem, lang).await?
    {
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
        || l.contains("cuda")
        || l.contains("gpu")
        || l.contains("device")
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
    fn prefers_cublas_zip_when_listed_first() {
        let json = r#"[{"assets":[
            {"name":"whisper-bin-x64.zip","browser_download_url":"https://cpu"},
            {"name":"whisper-cublas-12.4.0-bin-x64.zip","browser_download_url":"https://cuda"}
        ]}]"#;
        assert_eq!(
            pick_first_asset_url(
                json,
                &["whisper-cublas-12.4.0-bin-x64.zip", "whisper-bin-x64.zip"]
            )
            .as_deref(),
            Some("https://cuda")
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

    #[test]
    fn sha256_fails_closed_on_mismatch() {
        let err = verify_digest("abcd", MODEL_SHA256).unwrap_err();
        assert!(err.to_string().contains("SHA-256"));
        assert!(verify_digest(MODEL_SHA256, MODEL_SHA256).is_ok());
    }

    #[test]
    fn keeps_ggml_small_and_uses_nvidia_gpu_only_with_cuda_build() {
        assert_eq!(MODEL_NAME, "ggml-small.bin");
        assert!(MODEL_URL.starts_with("https://huggingface.co/ggerganov/whisper.cpp/"));
        assert!(MODEL_URL.ends_with("ggml-small.bin"));
        assert!(use_nvidia_gpu(true, "cuda"));
        assert!(!use_nvidia_gpu(false, "cuda"));
        assert!(!use_nvidia_gpu(true, "cpu"));
        assert!(!use_nvidia_gpu(true, "saknas"));
        assert!(gpu_disable_arg(true).is_none());
        assert_eq!(gpu_disable_arg(false), Some("-ng"));
    }

    #[test]
    fn transcription_pauses_while_audio_plays_and_resumes_when_it_stops() {
        let gate = PlaybackGate::new();
        assert!(transcription_allowed(gate.is_playing()));
        assert_eq!(
            whisper_pause_transition(gate.is_playing(), false),
            WhisperPause::Keep
        );
        gate.set_playing(true);
        assert!(!transcription_allowed(gate.is_playing()));
        assert_eq!(whisper_pause_transition(true, false), WhisperPause::Suspend);
        assert_eq!(whisper_pause_transition(true, true), WhisperPause::Keep);
        gate.set_playing(false);
        assert!(transcription_allowed(gate.is_playing()));
        assert_eq!(whisper_pause_transition(false, true), WhisperPause::Resume);
        assert_eq!(whisper_pause_transition(false, false), WhisperPause::Keep);
    }

    #[cfg(unix)]
    #[test]
    fn suspends_child_while_audio_would_be_playing_and_resumes() {
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("sleep");
        let pid = child.id();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_eq!(whisper_pause_transition(true, false), WhisperPause::Suspend);
            assert!(suspend_pid(pid), "SIGSTOP");
            assert!(process_is_alive(pid));
            assert_eq!(whisper_pause_transition(true, true), WhisperPause::Keep);
            assert_eq!(whisper_pause_transition(false, true), WhisperPause::Resume);
            assert!(resume_pid(pid), "SIGCONT");
            assert!(process_is_alive(pid));
        }));
        let _ = child.kill();
        let _ = child.wait();
        if let Err(err) = result {
            std::panic::resume_unwind(err);
        }
    }
}
