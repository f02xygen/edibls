use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::time::Duration;
use tokio::process::Command;

const MAX_SEARCH_RESULTS: usize = 30;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Metadata for a single YouTube video returned from search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YouTubeVideoInfo {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration_str: Option<String>,
    pub url: String,
}

impl YouTubeVideoInfo {
    /// Formats a human-readable label for a Telegram inline keyboard button.
    pub fn button_label(&self) -> String {
        let uploader_str = self.uploader.as_deref().unwrap_or("YouTube");
        let dur = match &self.duration_str {
            Some(d) if !d.is_empty() => format!(" [{d}]"),
            _ => String::new(),
        };

        let base = format!("🎬 {uploader_str} — {}", self.title);
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
struct RawYouTubeItem {
    id: Option<serde_json::Value>,
    title: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    duration: Option<f64>,
    duration_string: Option<String>,
    webpage_url: Option<String>,
    url: Option<String>,
    ie_key: Option<String>,
}

fn yt_http_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(8))
            .build()
            .unwrap_or_default()
    })
}

async fn search_youtube_innertube(query: &str) -> Result<Vec<YouTubeVideoInfo>> {
    let client = yt_http_client();
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": "WEB",
                "clientVersion": "2.20260101.00.00",
                "hl": "ru",
                "gl": "US"
            }
        },
        "query": query
    });

    let resp = client
        .post("https://www.youtube.com/youtubei/v1/search?prettyPrint=false")
        .json(&body)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await?
        .error_for_status()?;

    let json_val: serde_json::Value = resp.json().await?;
    let mut results = Vec::new();
    let mut seen_ids = HashSet::new();

    let sections = json_val
        .pointer("/contents/twoColumnSearchResultsRenderer/primaryContents/sectionListRenderer/contents")
        .and_then(|v| v.as_array());

    if let Some(sections) = sections {
        for s in sections {
            let item_contents = s
                .pointer("/itemSectionRenderer/contents")
                .and_then(|v| v.as_array());
            if let Some(contents) = item_contents {
                for c in contents {
                    if let Some(vr) = c.get("videoRenderer") {
                        let Some(id) = vr.get("videoId").and_then(|v| v.as_str()) else {
                            continue;
                        };
                        if id.is_empty() || !seen_ids.insert(id.to_string()) {
                            continue;
                        }

                        let title = vr
                            .pointer("/title/runs")
                            .and_then(|r| r.as_array())
                            .map(|runs| {
                                runs.iter()
                                    .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
                                    .collect::<String>()
                            })
                            .unwrap_or_else(|| "Без названия".to_string());

                        let uploader = vr
                            .pointer("/ownerText/runs")
                            .and_then(|r| r.as_array())
                            .map(|runs| {
                                runs.iter()
                                    .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
                                    .collect::<String>()
                            });

                        let duration_str = vr
                            .pointer("/lengthText/simpleText")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());

                        results.push(YouTubeVideoInfo {
                            id: id.to_string(),
                            title,
                            uploader,
                            duration_str,
                            url: format!("https://www.youtube.com/watch?v={id}"),
                        });

                        if results.len() >= MAX_SEARCH_RESULTS {
                            break;
                        }
                    }
                }
            }
        }
    }

    if results.is_empty() {
        anyhow::bail!("No Innertube search results");
    }

    Ok(results)
}

/// Searches YouTube videos using direct Innertube API, falling back to yt-dlp flat-playlist.
pub async fn search_youtube_videos(query: &str) -> Result<Vec<YouTubeVideoInfo>> {
    // Fast path: direct Innertube API request (~0.7s)
    if let Ok(videos) = search_youtube_innertube(query).await {
        if !videos.is_empty() {
            return Ok(videos);
        }
    }

    let search_spec = format!("ytsearch{MAX_SEARCH_RESULTS}:{query}");
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
    .context("YouTube search timed out")?
    .context("Failed to execute yt-dlp for YouTube search")?;

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::error!("yt-dlp ytsearch failed: {}", stderr);
        return Err(anyhow::anyhow!("YouTube search process exited with error"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut results = Vec::new();
    let mut seen_ids = HashSet::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(item) = serde_json::from_str::<RawYouTubeItem>(trimmed) {
            // Filter out channels / playlists (YoutubeTab)
            if item.ie_key.as_deref() == Some("YoutubeTab") {
                continue;
            }

            let id = match item.id {
                Some(serde_json::Value::String(s)) => s,
                Some(serde_json::Value::Number(n)) => n.to_string(),
                _ => continue,
            };

            if id.is_empty() || !seen_ids.insert(id.clone()) {
                continue;
            }

            let target_url = item
                .webpage_url
                .filter(|u| !u.trim().is_empty())
                .or(item.url)
                .filter(|u| u.starts_with("http"))
                .unwrap_or_else(|| format!("https://www.youtube.com/watch?v={id}"));

            if target_url.contains("/channel/") || target_url.contains("/user/") {
                continue;
            }

            let title = item.title.unwrap_or_else(|| "Без названия".to_string());
            let uploader = item.uploader.or(item.channel);

            let duration_str = item.duration_string.or_else(|| {
                item.duration.map(|d| {
                    let total_secs = d.round() as u64;
                    let mins = total_secs / 60;
                    let secs = total_secs % 60;
                    format!("{mins}:{secs:02}")
                })
            });

            results.push(YouTubeVideoInfo {
                id,
                title,
                uploader,
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
    fn test_youtube_button_label() {
        let video = YouTubeVideoInfo {
            id: "dQw4w9WgXcQ".to_string(),
            title: "Never Gonna Give You Up".to_string(),
            uploader: Some("Rick Astley".to_string()),
            duration_str: Some("3:34".to_string()),
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".to_string(),
        };
        assert_eq!(video.button_label(), "🎬 Rick Astley — Never Gonna Give You Up [3:34]");
    }
}
