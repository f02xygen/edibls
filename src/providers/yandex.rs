use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};
use tokio::process::Command;

use crate::config::Config;

/// Checks if the URL belongs to Yandex Music service
pub fn is_yandex_music_url(url: &str) -> bool {
    url.contains("music.yandex.") || url.contains("yandex.ru/music")
}

/// Downloads track/album/playlist from Yandex Music into the destination directory
pub async fn download_yandex_track(
    url: &str,
    token: &str,
    output_dir: &Path,
    config: &Config,
) -> Result<Vec<PathBuf>> {
    let status = Command::new(&config.yandex_downloader_bin)
        .arg("--quality")
        .arg("2")
        .arg("--token")
        .arg(token)
        .arg("--path-pattern")
        .arg("#title")
        .arg("--embed-cover")
        .arg("--dir")
        .arg(output_dir)
        .arg("--url")
        .arg(url)
        .status()
        .await
        .with_context(|| {
            format!(
                "Failed to execute {}. Make sure it is installed and present in PATH or specified in YANDEX_DOWNLOADER_BIN.",
                config.yandex_downloader_bin.display()
            )
        })?;

    if !status.success() {
        return Err(anyhow!(
            "yandex-music-downloader process exited with non-zero status"
        ));
    }

    let mut downloaded_files = Vec::new();
    let mut read_dir = tokio::fs::read_dir(output_dir).await?;

    while let Some(entry) = read_dir.next_entry().await? {
        let path = entry.path();
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if matches!(ext, "mp3" | "flac" | "m4a") {
                    downloaded_files.push(path);
                }
            }
        }
    }

    if downloaded_files.is_empty() {
        return Err(anyhow!("No audio files were found after downloading"));
    }

    Ok(downloaded_files)
}
