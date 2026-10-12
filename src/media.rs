use std::path::{Path, PathBuf};
use image::ImageFormat;
use image::imageops::FilterType;
use lofty::prelude::*;
use lofty::probe::Probe;

use crate::config::Config;
use crate::providers::soundcloud::is_soundcloud_url;
use crate::providers::spotify::{download_spotify_entity, is_spotify_url};
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

/// Extracts a SoundCloud search query from the message text (/sc <query> or /soundcloud <query>).
pub fn extract_sc_search_query(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/sc" | "/soundcloud") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// Extracts a YouTube search query from the message text (/yt <query> or /youtube <query>).
pub fn extract_yt_search_query(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/yt" | "/youtube") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// Extracts a Pornhub search query from the message text (/ph <query> or /pornhub <query>).
pub fn extract_ph_search_query(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/ph" | "/pornhub") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// Extracts a Spotify search query from the message text (/sp <query> or /spotify <query>).
pub fn extract_sp_search_query(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/sp" | "/spotify") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
    }
    None
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
pub fn is_audio_request(url: &str, text: &str, user_audio_only_setting: bool) -> bool {
    user_audio_only_setting
        || is_yandex_music_url(url)
        || is_soundcloud_url(url)
        || is_spotify_url(url)
        || text.starts_with("/audio")
        || text.starts_with("/mp3")
        || text.starts_with("/sc")
        || text.starts_with("/sp")
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

/// Formats a compact 10-block visual progress bar message.
pub fn format_progress_bar(stage_label: &str, percent: u8) -> String {
    let clamped = percent.min(100);
    let filled = (usize::from(clamped) + 5) / 10;
    let empty = 10usize.saturating_sub(filled);
    let bar: String = std::iter::repeat('█')
        .take(filled)
        .chain(std::iter::repeat('░').take(empty))
        .collect();
    format!("{stage_label}: {clamped}%\n[{bar}]")
}

/// Reads total transmitted bytes (`tx_bytes`) across non-loopback network interfaces from `/proc/net/dev`.
pub fn read_network_tx_bytes() -> u64 {
    let Ok(content) = std::fs::read_to_string("/proc/net/dev") else {
        return 0;
    };
    let mut total_tx = 0u64;
    for line in content.lines().skip(2) {
        let Some((iface, stats)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        if let Some(tx_str) = stats.split_whitespace().nth(8) {
            if let Ok(tx) = tx_str.parse::<u64>() {
                total_tx = total_tx.saturating_add(tx);
            }
        }
    }
    total_tx
}

/// High-level function to handle download and metadata parsing for any supported platform.
pub async fn fetch_media(
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    temp_dir: &Path,
    config: &Config,
    progress: & std::sync::Arc<std::sync::atomic::AtomicU8>,
) -> Result<MediaOutput, String> {
    if is_yandex_music_url(url) {
        let token = config
            .yandex_music_token
            .as_deref()
            .ok_or_else(|| "YANDEX_MUSIC_TOKEN не настроен в .env файле.".to_string())?;

        let files = download_yandex_track(url, token, temp_dir, config, progress)
            .await
            .map_err(|e| e.to_string())?;

        progress.store(100, std::sync::atomic::Ordering::Relaxed);
        let tracks = process_tracks_blocking(files).await;
        return Ok(MediaOutput::Audio(tracks));
    }

    if is_spotify_url(url) {
        let tracks = download_spotify_entity(url, temp_dir, config, progress)
            .await
            .map_err(|e| e.to_string())?;

        progress.store(100, std::sync::atomic::Ordering::Relaxed);
        return Ok(MediaOutput::Audio(tracks));
    }


    let file_path = download_media(url, audio_only, max_height, temp_dir, config, progress).await?;

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
        assert_eq!(extract_search_query("/ph test", true), None);
    }

    #[test]
    fn test_extract_sc_search_query() {
        assert_eq!(extract_sc_search_query("/sc marry me"), Some("marry me"));
        assert_eq!(extract_sc_search_query("/sc@edibls_bot drake"), Some("drake"));
        assert_eq!(extract_sc_search_query("/soundcloud test query"), Some("test query"));
        assert_eq!(extract_sc_search_query("/sc"), None);
        assert_eq!(extract_sc_search_query("/sc https://soundcloud.com/test"), None);
        assert_eq!(extract_sc_search_query("just words"), None);
    }

    #[test]
    fn test_extract_yt_search_query() {
        assert_eq!(extract_yt_search_query("/yt rick astley"), Some("rick astley"));
        assert_eq!(extract_yt_search_query("/yt@edibls_bot rick astley"), Some("rick astley"));
        assert_eq!(extract_yt_search_query("/youtube rick astley"), Some("rick astley"));
        assert_eq!(extract_yt_search_query("/yt"), None);
        assert_eq!(extract_yt_search_query("/yt https://youtube.com/watch?v=123"), None);
        assert_eq!(extract_yt_search_query("just words"), None);
    }

    #[test]
    fn test_extract_ph_search_query() {
        assert_eq!(extract_ph_search_query("/ph minecraft"), Some("minecraft"));
        assert_eq!(extract_ph_search_query("/ph@edibls_bot blonde pov"), Some("blonde pov"));
        assert_eq!(extract_ph_search_query("/pornhub test query"), Some("test query"));
        assert_eq!(extract_ph_search_query("/ph"), None);
        assert_eq!(extract_ph_search_query("/ph https://pornhub.com/view_video.php?viewkey=123"), None);
        assert_eq!(extract_ph_search_query("just words"), None);
    }

    #[test]
    fn test_extract_sp_search_query() {
        assert_eq!(extract_sp_search_query("/sp rick astley"), Some("rick astley"));
        assert_eq!(extract_sp_search_query("/sp@edibls_bot rick astley"), Some("rick astley"));
        assert_eq!(extract_sp_search_query("/spotify rick astley"), Some("rick astley"));
        assert_eq!(extract_sp_search_query("/sp"), None);
        assert_eq!(extract_sp_search_query("/sp https://open.spotify.com/track/123"), None);
        assert_eq!(extract_sp_search_query("just words"), None);
    }

    #[test]
    fn test_is_audio_request() {
        assert!(is_audio_request("https://youtube.com/watch?v=123", "/audio https://youtube.com/watch?v=123", false));
        assert!(is_audio_request("https://music.yandex.ru/track/123", "https://music.yandex.ru/track/123", false));
        assert!(is_audio_request("https://open.spotify.com/track/123", "https://open.spotify.com/track/123", false));
        assert!(is_audio_request("https://pornhub.com/view_video.php?viewkey=123", "https://pornhub.com/view_video.php?viewkey=123", true));
        assert!(!is_audio_request("https://pornhub.com/view_video.php?viewkey=123", "https://pornhub.com/view_video.php?viewkey=123", false));
    }


    #[test]
    fn test_format_progress_bar() {
        assert_eq!(
            format_progress_bar("📥 Скачивание", 0),
            "📥 Скачивание: 0%\n[░░░░░░░░░░]"
        );
        assert_eq!(
            format_progress_bar("📥 Скачивание", 50),
            "📥 Скачивание: 50%\n[█████░░░░░]"
        );
        assert_eq!(
            format_progress_bar("📤 Отправка", 36),
            "📤 Отправка: 36%\n[████░░░░░░]"
        );
        assert_eq!(
            format_progress_bar("📤 Отправка", 100),
            "📤 Отправка: 100%\n[██████████]"
        );
    }
}

