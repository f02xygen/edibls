use std::path::{Path, PathBuf};
use image::ImageFormat;
use image::imageops::FilterType;
use lofty::prelude::*;
use lofty::probe::Probe;

use crate::config::Config;
use crate::providers::yandex::{download_yandex_track, is_yandex_music_url};
use crate::providers::ytdlp::download_media;

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

/// Extracts a Yandex Music track search query from the message text.
/// Supports `/ym <query>`, `/audio <query>`, `/mp3 <query>`, `/search <query>` in any chat,
/// as well as plain text (without links or slash commands) in private chats.
pub fn extract_search_query(text: &str, is_private: bool) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/ym" | "/audio" | "/mp3" | "/search") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
            return None;
        }
    }

    if is_private {
        Some(trimmed)
    } else {
        None
    }
}

/// Determines whether the request should be processed as audio-only.
pub fn is_audio_request(url: &str, text: &str) -> bool {
    is_yandex_music_url(url)
        || text.starts_with("/audio")
        || text.starts_with("/mp3")
        || text.contains("audio")
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
                    let resized = img.resize(320, 320, FilterType::Triangle);
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

/// Offloads blocking tag & cover image processing to the blocking threadpool.
async fn process_tracks_blocking(files: Vec<PathBuf>) -> Vec<AudioTrack> {
    tokio::task::spawn_blocking(move || {
        files
            .into_iter()
            .map(|f| process_audio_metadata(&f))
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// High-level function to handle download and metadata parsing for any supported platform.
pub async fn fetch_media(
    url: &str,
    audio_only: bool,
    temp_dir: &Path,
    config: &Config,
) -> Result<MediaOutput, String> {
    if is_yandex_music_url(url) {
        let token = config
            .yandex_music_token
            .as_deref()
            .ok_or_else(|| "YANDEX_MUSIC_TOKEN не настроен в .env файле.".to_string())?;

        let files = download_yandex_track(url, token, temp_dir, config)
            .await
            .map_err(|e| e.to_string())?;

        let tracks = process_tracks_blocking(files).await;
        return Ok(MediaOutput::Audio(tracks));
    }

    let file_path = download_media(url, audio_only, temp_dir, config).await?;

    if audio_only {
        let tracks = process_tracks_blocking(vec![file_path]).await;
        Ok(MediaOutput::Audio(tracks))
    } else {
        Ok(MediaOutput::Video(file_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_search_query_private_vs_group() {
        assert_eq!(extract_search_query("Кино Кукушка", true), Some("Кино Кукушка"));
        assert_eq!(extract_search_query("Кино Кукушка", false), None);
    }

    #[test]
    fn test_extract_search_query_commands() {
        assert_eq!(extract_search_query("/ym Кукушка", false), Some("Кукушка"));
        assert_eq!(extract_search_query("/ym@edibls_bot Кино - Кукушка", false), Some("Кино - Кукушка"));
        assert_eq!(extract_search_query("/audio Кукушка", true), Some("Кукушка"));
        assert_eq!(extract_search_query("/audio https://youtu.be/abc", true), None);
        assert_eq!(extract_search_query("/ym", true), None);
    }
}

