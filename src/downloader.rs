use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Downloads media via yt-dlp (video or audio-only) and returns the file path.
pub async fn download_media(url: &str, audio_only: bool) -> Result<PathBuf, String> {
    log::info!("Processing URL (audio_only={}): {}", audio_only, url);

    let output_template = "/tmp/edibls_%(id)s.%(ext)s";
    let mut cmd = Command::new("yt-dlp");

    // Base limits & JS runtimes setup
    cmd.args([
        "--max-filesize",
        "50M",
        "--match-filter",
        "duration <= 1200", // Max 20 minutes
        "--js-runtimes",
        "deno,node",
        "--no-playlist",
    ]);

    // YouTube anti-bot & player client bypass
    cmd.args([
        "--extractor-args",
        "youtube:player_client=android,ios,web",
        "--extractor-args",
        "youtube:po_token=web+http://127.0.0.1:4444/get_pot",
    ]);

    if audio_only {
        // Extract audio stream and convert to mp3
        cmd.args(["-x", "--audio-format", "mp3"]);
    } else {
        // Select optimal format under Telegram's 50MB limit
        cmd.args([
            "-f",
            "b[filesize<=50M]/bestvideo[filesize<=50M]+bestaudio/best[filesize<=50M]/best",
        ]);
    }

    // Attach cookies if present in the working directory
    if Path::new("cookies.txt").exists() {
        log::info!("Found cookies.txt, attaching to yt-dlp execution");
        cmd.arg("--cookies").arg("cookies.txt");
    }

    let output = cmd
        .arg("-o")
        .arg(output_template)
        .arg("--print")
        .arg("after_move:filepath")
        .arg(url)
        .output()
        .await
        .map_err(|e| format!("Failed to execute yt-dlp process: {e}"))?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let file_path_str = stdout.lines().last().unwrap_or("").trim();

        if file_path_str.is_empty() {
            Err("yt-dlp finished but did not return a valid file path".to_string())
        } else {
            let path = PathBuf::from(file_path_str);
            log::info!("Downloaded file saved at: {:?}", path);
            Ok(path)
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("yt-dlp execution failed: {}", stderr);

        if stderr.contains("does not pass filter duration") {
            Err("Видео слишком длинное (максимум 20 минут)".to_string())
        } else if stderr.contains("File is larger than max-filesize") {
            Err("Размер файла превышает лимит Telegram в 50 МБ".to_string())
        } else {
            Err("Не удалось скачать медиафайл по указанной ссылке".to_string())
        }
    }
}
