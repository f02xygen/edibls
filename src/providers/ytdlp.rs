use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::config::Config;

/// Downloads media via yt-dlp (video or audio-only) into the provided temp directory.
pub async fn download_media(
    url: &str,
    audio_only: bool,
    temp_dir: &Path,
    config: &Config,
) -> Result<PathBuf, String> {
    log::info!("Processing URL (audio_only={}): {}", audio_only, url);

    let output_template = temp_dir.join("%(id)s.%(ext)s");
    let output_template_str = output_template.to_string_lossy();

    let mut cmd = Command::new("yt-dlp");

    cmd.args([
        "--max-filesize",
        "50M",
        "--match-filter",
        "duration <= 1200", // Max 20 minutes
        "--concurrent-fragments",
        "8",
        "--http-chunk-size",
        "10M",
        "--no-mtime",
        "--no-part",
        "--js-runtimes",
        "deno",
        "--js-runtimes",
        "node",
        "--no-playlist",
    ]);

    // Build combined youtube extractor args so flags do not overwrite each other
    let yt_extractor_args = if let Some(pot_url) = &config.youtube_pot_url {
        format!("youtube:player_client=android,ios,web;po_token=web+{pot_url}")
    } else {
        "youtube:player_client=android,ios,web".to_string()
    };
    cmd.arg("--extractor-args").arg(yt_extractor_args);

    if audio_only {
        cmd.args([
            "-x",
            "--audio-format",
            "mp3",
            "--embed-metadata",
            "--postprocessor-args",
            "ffmpeg:-threads 0",
        ]);
    } else {
        cmd.args([
            "-f",
            "b[filesize<=50M]/bestvideo[filesize<=50M]+bestaudio/best[filesize<=50M]/best",
            "-S",
            "vcodec:h264,fps,res,acodec:m4a",
            "--merge-output-format",
            "mp4",
        ]);
    }

    if let Some(cookies) = &config.cookies_path {
        log::info!("Using cookies from: {:?}", cookies);
        cmd.arg("--cookies").arg(cookies);
    }

    let output = cmd
        .arg("-o")
        .arg(&*output_template_str)
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
