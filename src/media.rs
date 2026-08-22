use std::path::{Path, PathBuf};
use image::ImageFormat;
use lofty::prelude::*;
use lofty::probe::Probe;
use tempfile::TempDir;

use crate::downloader::download_media;
use crate::yandex::{download_yandex_track, is_yandex_music_url};

pub struct AudioTrack {
    pub path: PathBuf,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub cover_path: Option<PathBuf>,
}

pub enum MediaOutput {
    Audio(Vec<AudioTrack>),
    Video(PathBuf),
}

/// Extracts the first HTTP/HTTPS URL found in the message text.
pub fn extract_url(text: &str) -> Option<&str> {
    text.split_whitespace()
        .find(|word| word.starts_with("http://") || word.starts_with("https://"))
}

/// Reads audio tags and resizes the cover image to fit Telegram API restrictions (max 320x320).
pub fn process_audio_metadata(file_path: &Path) -> AudioTrack {
    let cover_path = file_path.with_extension("jpg");
    let mut performer = None;
    let mut title = None;
    let mut has_cover = false;

    if let Ok(tagged_file) = Probe::open(file_path).and_then(|p| p.read()) {
        if let Some(tag) = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())
        {
            performer = tag.artist().map(|s| s.to_string());
            title = tag.title().map(|s| s.to_string());

            if let Some(picture) = tag.pictures().first() {
                if let Ok(img) = image::load_from_memory(picture.data()) {
                    let resized = img.thumbnail(320, 320);
                    if resized
                        .save_with_format(&cover_path, ImageFormat::Jpeg)
                        .is_ok()
                    {
                        has_cover = true;
                    }
                }
            }
        }
    }

    AudioTrack {
        path: file_path.to_path_buf(),
        title,
        performer,
        cover_path: if has_cover { Some(cover_path) } else { None },
    }
}

/// High-level function to handle download and metadata parsing for any supported platform.
pub async fn fetch_media(
    url: &str,
    text: &str,
    yandex_token: Option<&str>,
    temp_dir: &Path,
) -> Result<MediaOutput, String> {
    if is_yandex_music_url(url) {
        let token = yandex_token
            .filter(|t| !t.is_empty())
            .ok_or_else(|| "YANDEX_MUSIC_TOKEN is not configured in .env file.".to_string())?;

        let files = download_yandex_track(url, token, temp_dir)
            .await
            .map_err(|e| e.to_string())?;

        let tracks = files.iter().map(|f| process_audio_metadata(f)).collect();
        return Ok(MediaOutput::Audio(tracks));
    }

    let audio_only = text.starts_with("/audio")
        || text.starts_with("/mp3")
        || text.contains("audio");

    let file_path = download_media(url, audio_only).await?;

    if audio_only {
        let track = process_audio_metadata(&file_path);
        Ok(MediaOutput::Audio(vec![track]))
    } else {
        Ok(MediaOutput::Video(file_path))
    }
}
