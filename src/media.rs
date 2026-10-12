use std::path::{Path, PathBuf};
use image::ImageFormat;
use image::imageops::FilterType;
use lofty::prelude::*;
use lofty::probe::Probe;

use crate::config::Config;
use crate::providers::soundcloud::is_soundcloud_url;
use crate::providers::yandex::{download_yandex_track, is_yandex_music_url};
use crate::providers::ytdlp::download_media;

pub struct AudioTrack {
    pub path: PathBuf,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub duration_secs: Option<u32>,
    pub cover_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFile {
    pub path: PathBuf,
    pub title: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_secs: Option<u32>,
    pub thumbnail_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub start_secs: u64,
    pub end_secs: u64,
    pub raw_label: String,
}

pub enum MediaOutput {
    Audio(Vec<AudioTrack>),
    Video(VideoFile),
}

/// Parses timestamp strings such as "10", "01:25", "1:05:30" into total seconds.
pub fn parse_timestamp(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        1 => parts[0].parse::<u64>().ok(),
        2 => {
            let mins = parts[0].parse::<u64>().ok()?;
            let secs = parts[1].parse::<u64>().ok()?;
            if secs < 60 {
                Some(mins * 60 + secs)
            } else {
                None
            }
        }
        3 => {
            let hours = parts[0].parse::<u64>().ok()?;
            let mins = parts[1].parse::<u64>().ok()?;
            let secs = parts[2].parse::<u64>().ok()?;
            if mins < 60 && secs < 60 {
                Some(hours * 3600 + mins * 60 + secs)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extracts a time range (e.g. 01:15-02:30 or 1:15 - 2:30) from the user text.
pub fn extract_time_range(text: &str) -> Option<TimeRange> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for sep in ['-', '–', '—'] {
        for word in &words {
            if word.starts_with("http://") || word.starts_with("https://") {
                continue;
            }
            if let Some((left, right)) = word.split_once(sep) {
                if let (Some(s), Some(e)) = (parse_timestamp(left), parse_timestamp(right)) {
                    if e > s {
                        return Some(TimeRange {
                            start_secs: s,
                            end_secs: e,
                            raw_label: format!("{left}-{right}"),
                        });
                    }
                }
            }
        }
        for (i, &w) in words.iter().enumerate() {
            if (w == "-" || w == "–" || w == "—") && i > 0 && i + 1 < words.len() {
                let left = words[i - 1];
                let right = words[i + 1];
                if let (Some(s), Some(e)) = (parse_timestamp(left), parse_timestamp(right)) {
                    if e > s {
                        return Some(TimeRange {
                            start_secs: s,
                            end_secs: e,
                            raw_label: format!("{left}-{right}"),
                        });
                    }
                }
            }
        }
    }
    None
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
        || text.starts_with("/audio")
        || text.starts_with("/mp3")
        || text.starts_with("/sc")
        || text.contains("audio")
}

/// Reads audio tags and resizes the cover image to fit Telegram API restrictions (max 320x320).
pub fn process_audio_metadata(file_path: &Path) -> AudioTrack {
    let cover_path = file_path.with_extension("jpg");
    let mut performer = None;
    let mut title = None;
    let mut duration_secs = None;
    let mut has_cover = false;

    if let Ok(tagged_file) = Probe::open(file_path).and_then(|p| p.read()) {
        let dur = tagged_file.properties().duration().as_secs();
        if dur > 0 {
            duration_secs = Some(dur as u32);
        }

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

    if title.is_none() {
        if let Some(parent) = file_path.parent() {
            let title_file = parent.join("title.txt");
            if let Ok(t) = std::fs::read_to_string(title_file) {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    title = Some(trimmed.to_string());
                }
            }
        }
    }

    AudioTrack {
        path: file_path.to_path_buf(),
        title,
        performer,
        duration_secs,
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
    time_range: Option<&TimeRange>,
    ym_quality: crate::settings::YandexQualityPreference,
    temp_dir: &Path,
    config: &Config,
    progress: &std::sync::Arc<std::sync::atomic::AtomicU8>,
) -> Result<MediaOutput, String> {
    if is_yandex_music_url(url) {
        let token = config
            .yandex_music_token
            .as_deref()
            .ok_or_else(|| "YANDEX_MUSIC_TOKEN не настроен в .env файле.".to_string())?;

        let files = download_yandex_track(url, token, ym_quality, temp_dir, config, progress)
            .await
            .map_err(|e| e.to_string())?;

        progress.store(100, std::sync::atomic::Ordering::Relaxed);
        let tracks = process_tracks_blocking(files).await;
        return Ok(MediaOutput::Audio(tracks));
    }

    let file_path = download_media(url, audio_only, max_height, time_range, temp_dir, config, progress).await?;

    if audio_only {
        let tracks = process_tracks_blocking(vec![file_path]).await;
        Ok(MediaOutput::Audio(tracks))
    } else {
        let (width, height, duration_secs, thumbnail_path) = probe_video_metadata(&file_path).await;
        let title = tokio::fs::read_to_string(temp_dir.join("title.txt"))
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(MediaOutput::Video(VideoFile {
            path: file_path,
            title,
            width,
            height,
            duration_secs,
            thumbnail_path,
        }))
    }
}

/// Probes video dimensions, duration, and extracts a thumbnail frame (max 320x320) via ffprobe and ffmpeg.
pub async fn probe_video_metadata(path: &Path) -> (Option<u32>, Option<u32>, Option<u32>, Option<PathBuf>) {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,duration:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await
        .ok();

    let mut width = None;
    let mut height = None;
    let mut duration_secs = None;

    if let Some(out) = output {
        if out.status.success() {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(stream) = val.pointer("/streams/0") {
                    width = stream.get("width").and_then(|v| v.as_u64()).map(|v| v as u32);
                    height = stream.get("height").and_then(|v| v.as_u64()).map(|v| v as u32);
                    if let Some(d_str) = stream.get("duration").and_then(|v| v.as_str()) {
                        if let Ok(d) = d_str.parse::<f64>() {
                            if d > 0.0 {
                                duration_secs = Some(d.round() as u32);
                            }
                        }
                    }
                }
                if duration_secs.is_none() {
                    if let Some(d_str) = val.pointer("/format/duration").and_then(|v| v.as_str()) {
                        if let Ok(d) = d_str.parse::<f64>() {
                            if d > 0.0 {
                                duration_secs = Some(d.round() as u32);
                            }
                        }
                    }
                }
            }
        }
    }

    let thumb_path = path.with_extension("thumb.jpg");
    let ss_time = match duration_secs {
        Some(d) if d > 3 => "1",
        Some(_) => "0",
        None => "1",
    };

    let thumb_res = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-ss",
            ss_time,
            "-i",
        ])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='min(320,iw)':'min(320,ih)':force_original_aspect_ratio=decrease",
            "-q:v",
            "3",
        ])
        .arg(&thumb_path)
        .kill_on_drop(true)
        .status()
        .await;

    let final_thumb = if thumb_res.map(|s| s.success()).unwrap_or(false) && thumb_path.exists() {
        Some(thumb_path)
    } else {
        None
    };

    (width, height, duration_secs, final_thumb)
}

/// Probes video file dimensions (width, height) using ffprobe.
#[allow(dead_code)]
pub async fn get_video_dimensions(path: &Path) -> Option<(u32, u32)> {
    let (w, h, _, _) = probe_video_metadata(path).await;
    match (w, h) {
        (Some(width), Some(height)) if width > 0 && height > 0 => Some((width, height)),
        _ => None,
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
    fn test_is_audio_request() {
        assert!(is_audio_request("https://youtube.com/watch?v=123", "/audio https://youtube.com/watch?v=123", false));
        assert!(is_audio_request("https://music.yandex.ru/track/123", "https://music.yandex.ru/track/123", false));
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

    #[test]
    fn test_parse_timestamp() {
        assert_eq!(parse_timestamp("45"), Some(45));
        assert_eq!(parse_timestamp("01:15"), Some(75));
        assert_eq!(parse_timestamp("1:02:03"), Some(3723));
        assert_eq!(parse_timestamp("invalid"), None);
        assert_eq!(parse_timestamp("01:75"), None);
    }

    #[test]
    fn test_extract_time_range() {
        let r1 = extract_time_range("https://youtu.be/123 01:15-02:30").unwrap();
        assert_eq!(r1.start_secs, 75);
        assert_eq!(r1.end_secs, 150);

        let r2 = extract_time_range("/clip https://youtu.be/123 1:00 - 2:00").unwrap();
        assert_eq!(r2.start_secs, 60);
        assert_eq!(r2.end_secs, 120);

        let r3 = extract_time_range("https://youtu.be/123 00:01:15–00:02:30").unwrap();
        assert_eq!(r3.start_secs, 75);
        assert_eq!(r3.end_secs, 150);

        assert!(extract_time_range("https://youtu.be/123").is_none());
        assert!(extract_time_range("https://youtu.be/123 02:00-01:00").is_none());
    }
}

