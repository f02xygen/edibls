use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Downloads media via yt-dlp (video or audio-only) and returns the file path.
pub async fn download_media(url: &str, audio_only: bool) -> Result<PathBuf, String> {
    log::info!("Processing URL (audio_only={}): {}", audio_only, url);

    let output_template = "/tmp/edibls_%(id)s.%(ext)s";
    let mut cmd = Command::new("yt-dlp");

    if audio_only {
        // Extract audio stream and convert to mp3
        cmd.arg("-x").arg("--audio-format").arg("mp3");
    } else {
        // Force standard format selection for video
        cmd.arg("-f").arg("b[ext=mp4]/best[ext=mp4]/best");
    }

    // Attach cookies if present in the working directory
    if Path::new("cookies.txt").exists() {
        log::info!("Found cookies.txt, attaching to yt-dlp execution");
        cmd.arg("--cookies").arg("cookies.txt");
    }

    let output = cmd
        .arg("--no-playlist")
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
        Err("Failed to download media from the provided URL".to_string())
    }
}
