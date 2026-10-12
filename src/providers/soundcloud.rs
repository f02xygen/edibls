use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use tokio::process::Command;

const MAX_SEARCH_RESULTS: usize = 8;

/// Metadata for a single SoundCloud track returned from search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoundcloudTrackInfo {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration_str: Option<String>,
    pub url: String,
}

impl SoundcloudTrackInfo {
    /// Formats a human-readable label for a Telegram inline keyboard button.
    pub fn button_label(&self) -> String {
        let uploader_str = self.uploader.as_deref().unwrap_or("SoundCloud");
        let dur = match &self.duration_str {
            Some(d) if !d.is_empty() => format!(" [{d}]"),
            _ => String::new(),
        };

        let base = format!("🎵 {uploader_str} — {}", self.title);
        let max_base_chars = 54usize;
        let truncated_base = if base.chars().count() > max_base_chars {
            let s: String = base.chars().take(max_base_chars - 1).collect();
            format!("{s}…")
        } else {
            base
        };

        format!("{truncated_base}{dur}")
    }
}

#[derive(Debug, Deserialize)]
struct RawSoundcloudItem {
    id: Option<serde_json::Value>,
    title: Option<String>,
    uploader: Option<String>,
    duration: Option<f64>,
    duration_string: Option<String>,
    webpage_url: Option<String>,
    url: Option<String>,
}

/// Checks whether the URL is a SoundCloud link.
pub fn is_soundcloud_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("soundcloud.com") || lower.contains("on.soundcloud.com")
}

/// Searches SoundCloud tracks using yt-dlp flat-playlist scsearch.
pub async fn search_soundcloud_tracks(query: &str) -> Result<Vec<SoundcloudTrackInfo>> {
    let search_spec = format!("scsearch{MAX_SEARCH_RESULTS}:{query}");
    let output = Command::new("yt-dlp")
        .args(["--dump-json", "--flat-playlist", &search_spec])
        .output()
        .await
        .context("Failed to execute yt-dlp for SoundCloud search")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("yt-dlp scsearch failed: {}", stderr);
        return Err(anyhow::anyhow!("SoundCloud search process exited with error"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    let mut seen_urls = HashSet::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(item) = serde_json::from_str::<RawSoundcloudItem>(trimmed) {
            let target_url = item
                .webpage_url
                .filter(|u| !u.trim().is_empty())
                .or(item.url)
                .unwrap_or_default();

            if target_url.is_empty() || !seen_urls.insert(target_url.clone()) {
                continue;
            }

            let title = item.title.unwrap_or_else(|| "Без названия".to_string());
            let id = match item.id {
                Some(serde_json::Value::String(s)) => s,
                Some(serde_json::Value::Number(n)) => n.to_string(),
                _ => target_url.clone(),
            };

            let duration_str = item.duration_string.or_else(|| {
                item.duration.map(|d| {
                    let total_secs = d.round() as u64;
                    let mins = total_secs / 60;
                    let secs = total_secs % 60;
                    format!("{mins}:{secs:02}")
                })
            });

            results.push(SoundcloudTrackInfo {
                id,
                title,
                uploader: item.uploader,
                duration_str,
                url: target_url,
            });

            if results.len() >= MAX_SEARCH_RESULTS {
                break;
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_soundcloud_url() {
        assert!(is_soundcloud_url("https://soundcloud.com/artist/track"));
        assert!(is_soundcloud_url("https://on.soundcloud.com/abc12"));
        assert!(is_soundcloud_url("http://soundcloud.com/test"));
        assert!(!is_soundcloud_url("https://music.yandex.ru/track/123"));
        assert!(!is_soundcloud_url("https://youtube.com/watch?v=abc"));
    }

    #[test]
    fn test_soundcloud_button_label() {
        let track = SoundcloudTrackInfo {
            id: "123".to_string(),
            title: "Test Track".to_string(),
            uploader: Some("Artist".to_string()),
            duration_str: Some("3:45".to_string()),
            url: "https://soundcloud.com/artist/test-track".to_string(),
        };
        assert_eq!(track.button_label(), "🎵 Artist — Test Track [3:45]");
    }
}
