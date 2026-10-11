use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::config::Config;
use crate::providers::pornhub::is_pornhub_url;

/// Downloads media via yt-dlp (video or audio-only) into the provided temp directory.
pub async fn download_media(
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    temp_dir: &Path,
    config: &Config,
) -> Result<PathBuf, String> {
    log::info!(
        "Processing URL (audio_only={}, max_height={:?}): {}",
        audio_only,
        max_height,
        url
    );

    let output_template = temp_dir.join("%(id)s.%(ext)s");
    let output_template_str = output_template.to_string_lossy();

    let max_filesize_str = format!("{}M", config.max_filesize_mb);
    let match_filter = format!("duration <= {}", config.max_duration_secs);

    // For Pornhub CDN (ev.phncdn.com), rate=500k&burst=3800k throttles chunks larger than 3.8MB.
    // Setting chunk size to 3M lets each range request download at unthrottled burst speed.
    let http_chunk_size = if is_pornhub_url(url) { "3M" } else { "10M" };

    let mut cmd = Command::new("yt-dlp");

    cmd.args([
        "--max-filesize",
        &max_filesize_str,
        "--match-filter",
        &match_filter,
        "--concurrent-fragments",
        "8",
        "--http-chunk-size",
        http_chunk_size,
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
    } else if let Some(height) = max_height {
        let format_selector = format!(
            "bestvideo[height<={height}][filesize<={max_filesize_str}]+bestaudio/best[height<={height}][filesize<={max_filesize_str}]/bestvideo[height<={height}]+bestaudio/best[height<={height}]/b[filesize<={max_filesize_str}]/best"
        );
        let sort_criteria = format!("res:{height},vcodec:h264,fps,acodec:m4a");
        cmd.args([
            "-f",
            &format_selector,
            "-S",
            &sort_criteria,
            "--merge-output-format",
            "mp4",
        ]);
    } else {
        let format_selector = format!(
            "b[filesize<={max_filesize_str}]/bestvideo[filesize<={max_filesize_str}]+bestaudio/best[filesize<={max_filesize_str}]/best"
        );
        cmd.args([
            "-f",
            &format_selector,
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
            Err(format!(
                "Видео слишком длинное (максимум {} минут)",
                config.max_duration_secs / 60
            ))
        } else if stderr.contains("File is larger than max-filesize") {
            Err(format!(
                "Размер файла превышает лимит в {} МБ",
                config.max_filesize_mb
            ))
        } else {
            Err("Не удалось скачать медиафайл по указанной ссылке".to_string())
        }
    }
}
