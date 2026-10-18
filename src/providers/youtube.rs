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

/// A chapter/section within a YouTube video.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct YouTubeChapter {
    pub title: String,
    pub start_secs: u64,
    pub end_secs: u64,
}

impl YouTubeChapter {
    pub fn button_label(&self) -> String {
        let s_min = self.start_secs / 60;
        let s_sec = self.start_secs % 60;
        let dur = self.end_secs.saturating_sub(self.start_secs);
        let d_min = dur / 60;
        let d_sec = dur % 60;
        let dur_str = format!("{d_min}:{d_sec:02}");

        let base = format!("{:02}:{:02} {}", s_min, s_sec, self.title);
        let max_base_chars = 46usize;
        let truncated = if base.chars().count() > max_base_chars {
            let s: String = base.chars().take(max_base_chars - 1).collect();
            format!("{s}…")
        } else {
            base
        };
        format!("{truncated} [{dur_str}]")
    }
}

/// Extracts YouTube video ID from standard watch/youtu.be/shorts URLs.
pub fn extract_youtube_video_id(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    if host == "youtu.be" {
        let path = parsed.path().trim_start_matches('/');
        if !path.is_empty() {
            return Some(path.to_string());
        }
    } else if host.contains("youtube.com") {
        if parsed.path().starts_with("/shorts/") {
            let path = parsed.path().trim_start_matches("/shorts/").trim_end_matches('/');
            if !path.is_empty() {
                return Some(path.to_string());
            }
        } else if parsed.path().starts_with("/watch") {
            for (k, v) in parsed.query_pairs() {
                if k == "v" && !v.is_empty() {
                    return Some(v.into_owned());
                }
            }
        }
    }
    None
}

fn extract_chapters_from_markers_map(
    val: &serde_json::Value,
    chapters: &mut Vec<(String, u64)>,
) {
    if let Some(obj) = val.as_object() {
        if let Some(markers_map) = obj.get("markersMap").and_then(|v| v.as_array()) {
            for item in markers_map {
                if let Some(ch_list) = item.pointer("/value/chapters").and_then(|v| v.as_array()) {
                    for ch in ch_list {
                        let cr = ch.get("chapterRenderer");
                        if let Some(cr) = cr {
                            let title = cr
                                .pointer("/title/simpleText")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .or_else(|| {
                                    cr.pointer("/title/runs")
                                        .and_then(|r| r.as_array())
                                        .map(|runs| {
                                            runs.iter()
                                                .filter_map(|r| r.get("text").and_then(|t| t.as_str()))
                                                .collect::<String>()
                                        })
                                })
                                .unwrap_or_default();
                            let start_ms = cr
                                .get("timeRangeStartMillis")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            chapters.push((title.trim().to_string(), start_ms / 1000));
                        }
                    }
                }
            }
        }
        for (_, v) in obj {
            extract_chapters_from_markers_map(v, chapters);
        }
    } else if let Some(arr) = val.as_array() {
        for v in arr {
            extract_chapters_from_markers_map(v, chapters);
        }
    }
}

/// Fetches chapters via YouTube Innertube `next` endpoint (fast, bot-check immune, ~0.3s).
async fn fetch_youtube_chapters_innertube(video_id: &str) -> Option<Vec<YouTubeChapter>> {
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
        "videoId": video_id
    });

    let resp = client
        .post("https://www.youtube.com/youtubei/v1/next?prettyPrint=false")
        .json(&body)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let val = resp.json::<serde_json::Value>().await.ok()?;
    let mut raw_chapters: Vec<(String, u64)> = Vec::new();
    extract_chapters_from_markers_map(&val, &mut raw_chapters);

    if raw_chapters.is_empty() {
        return None;
    }

    let mut result = Vec::new();
    for (i, (title, start_secs)) in raw_chapters.iter().enumerate() {
        let end_secs = if i + 1 < raw_chapters.len() {
            raw_chapters[i + 1].1
        } else {
            start_secs.saturating_add(60)
        };

        if end_secs > *start_secs {
            let chapter_title = if title.is_empty() {
                format!("Глава {}", i + 1)
            } else {
                title.clone()
            };
            result.push(YouTubeChapter {
                title: chapter_title,
                start_secs: *start_secs,
                end_secs,
            });
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Extracts chapters from a YouTube video via Innertube next API, falling back to yt-dlp metadata.
pub async fn fetch_youtube_chapters(url: &str) -> Option<Vec<YouTubeChapter>> {
    // Fast path: direct Innertube `next` API (~0.3s), doesn't get blocked by bot-checks
    if let Some(video_id) = extract_youtube_video_id(url) {
        if let Some(chapters) = fetch_youtube_chapters_innertube(&video_id).await {
            if !chapters.is_empty() {
                return Some(chapters);
            }
        }
    }

    // Fallback: yt-dlp dump-single-json
    let output = Command::new("yt-dlp")
        .args([
            "--dump-single-json",
            "--skip-download",
            "--no-playlist",
        ])
        .arg(url)
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let val = serde_json::from_slice::<serde_json::Value>(&output.stdout).ok()?;
    let chapters_arr = val.get("chapters")?.as_array()?;
    if chapters_arr.is_empty() {
        return None;
    }

    let total_duration = val.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0) as u64;

    let mut result = Vec::new();
    for (i, item) in chapters_arr.iter().enumerate() {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let start_time = item
            .get("start_time")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as u64;

        let end_time = if let Some(e) = item.get("end_time").and_then(|v| v.as_f64()) {
            e as u64
        } else if i + 1 < chapters_arr.len() {
            chapters_arr[i + 1]
                .get("start_time")
                .and_then(|v| v.as_f64())
                .unwrap_or(start_time as f64 + 60.0) as u64
        } else {
            total_duration.max(start_time + 1)
        };

        if end_time > start_time {
            let chapter_title = if title.is_empty() {
                format!("Глава {}", i + 1)
            } else {
                title
            };
            result.push(YouTubeChapter {
                title: chapter_title,
                start_secs: start_time,
                end_secs: end_time,
            });
        }
    }

    if result.is_empty() {
        None
    } else {
        Some(result)
    }
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

    #[test]
    fn test_youtube_chapter_button_label() {
        let chapter = YouTubeChapter {
            title: "Intro".to_string(),
            start_secs: 75,
            end_secs: 150,
        };
        assert_eq!(chapter.button_label(), "01:15 Intro [1:15]");
    }

    #[test]
    fn test_extract_youtube_video_id() {
        assert_eq!(
            extract_youtube_video_id("https://youtu.be/tzb30ZoYINs?si=0PoAdykecttcR1UB"),
            Some("tzb30ZoYINs".to_string())
        );
        assert_eq!(
            extract_youtube_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
        assert_eq!(
            extract_youtube_video_id("https://youtube.com/shorts/abcdef12345?feature=share"),
            Some("abcdef12345".to_string())
        );
        assert_eq!(extract_youtube_video_id("https://soundcloud.com/test"), None);
    }

    #[test]
    fn test_extract_chapters_from_markers_map() {
        let json_data = serde_json::json!({
            "markersMap": [{
                "value": {
                    "chapters": [
                        {
                            "chapterRenderer": {
                                "title": {"simpleText": "Вступление"},
                                "timeRangeStartMillis": 0
                            }
                        },
                        {
                            "chapterRenderer": {
                                "title": {"runs": [{"text": "Глава 2"}]},
                                "timeRangeStartMillis": 150000
                            }
                        }
                    ]
                }
            }]
        });

        let mut chapters = Vec::new();
        extract_chapters_from_markers_map(&json_data, &mut chapters);
        assert_eq!(chapters.len(), 2);
        assert_eq!(chapters[0], ("Вступление".to_string(), 0));
        assert_eq!(chapters[1], ("Глава 2".to_string(), 150));
    }
}
