use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::process::Command;

const MAX_SEARCH_RESULTS: usize = 30;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);

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

fn sc_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_default()
    })
}

static SC_CLIENT_ID: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);

async fn get_sc_client_id(client: &reqwest::Client) -> Option<String> {
    if let Ok(guard) = SC_CLIENT_ID.read() {
        if let Some(ref id) = *guard {
            return Some(id.clone());
        }
    }

    // Try reading cached client_id from disk
    for cache_path in [
        "/home/appuser/.cache/yt-dlp/soundcloud/client_id.json",
        "/root/.cache/yt-dlp/soundcloud/client_id.json",
    ] {
        if let Ok(content) = tokio::fs::read_to_string(cache_path).await {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(id) = val.get("data").and_then(|v| v.as_str()) {
                    if !id.is_empty() {
                        if let Ok(mut guard) = SC_CLIENT_ID.write() {
                            *guard = Some(id.to_string());
                        }
                        return Some(id.to_string());
                    }
                }
            }
        }
    }

    // Fallback: scrape from soundcloud.com
    let html = client
        .get("https://soundcloud.com")
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;

    let mut script_urls = Vec::new();
    for part in html.split("src=\"") {
        if let Some(url) = part.split('"').next() {
            if url.contains("sndcdn.com/assets/") && url.ends_with(".js") {
                script_urls.push(url.to_string());
            }
        }
    }

    for script_url in script_urls.into_iter().rev().take(5) {
        if let Ok(js) = client.get(&script_url).send().await {
            if let Ok(text) = js.text().await {
                if let Some(idx) = text.find("client_id:\"") {
                    let rest = &text[idx + "client_id:\"".len()..];
                    if let Some(end) = rest.find('"') {
                        let id = &rest[..end];
                        if id.len() > 10 && id.chars().all(|c| c.is_ascii_alphanumeric()) {
                            if let Ok(mut guard) = SC_CLIENT_ID.write() {
                                *guard = Some(id.to_string());
                            }
                            return Some(id.to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

async fn search_soundcloud_direct(query: &str) -> Result<Vec<SoundcloudTrackInfo>> {
    let client = sc_http_client();
    let client_id = get_sc_client_id(client)
        .await
        .context("Failed to obtain SoundCloud client_id")?;

    let resp = client
        .get("https://api-v2.soundcloud.com/search/tracks")
        .query(&[
            ("q", query),
            ("limit", "30"),
            ("offset", "0"),
            ("linked_partitioning", "1"),
            ("client_id", &client_id),
        ])
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await?
        .error_for_status()?;

    let json_val: serde_json::Value = resp.json().await?;
    let collection = json_val
        .get("collection")
        .and_then(|v| v.as_array())
        .context("No collection in SoundCloud search response")?;

    let mut results = Vec::new();
    let mut seen_urls = HashSet::new();

    for item in collection {
        let permalink_url = item
            .get("permalink_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if permalink_url.is_empty() || !seen_urls.insert(permalink_url.to_string()) {
            continue;
        }

        let id = match item.get("id") {
            Some(serde_json::Value::Number(n)) => n.to_string(),
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => permalink_url.to_string(),
        };

        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Без названия")
            .to_string();

        let uploader = item
            .pointer("/user/username")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let duration_str = item
            .get("duration")
            .and_then(|v| v.as_f64())
            .map(|d_ms| {
                let total_secs = (d_ms / 1000.0).round() as u64;
                let mins = total_secs / 60;
                let secs = total_secs % 60;
                format!("{mins}:{secs:02}")
            });

        results.push(SoundcloudTrackInfo {
            id,
            title,
            uploader,
            duration_str,
            url: permalink_url.to_string(),
        });

        if results.len() >= MAX_SEARCH_RESULTS {
            break;
        }
    }

    if results.is_empty() {
        anyhow::bail!("No tracks in SoundCloud direct search");
    }

    Ok(results)
}

/// Searches SoundCloud tracks using direct API v2, falling back to yt-dlp flat-playlist.
pub async fn search_soundcloud_tracks(query: &str) -> Result<Vec<SoundcloudTrackInfo>> {
    // Fast path: direct SoundCloud API v2 search (~0.35s)
    if let Ok(tracks) = search_soundcloud_direct(query).await {
        if !tracks.is_empty() {
            return Ok(tracks);
        }
    }

    let search_spec = format!("scsearch{MAX_SEARCH_RESULTS}:{query}");
    let output = tokio::time::timeout(
        SEARCH_TIMEOUT,
        Command::new("yt-dlp")
            .args([
                "--dump-json",
                "--flat-playlist",
                "--no-warnings",
                "--ignore-errors",
                &search_spec,
            ])
            .output(),
    )
    .await
    .context("SoundCloud search timed out")?
    .context("Failed to execute yt-dlp for SoundCloud search")?;

    if !output.status.success() && output.stdout.is_empty() {
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
