use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::config::Config;

const YANDEX_API_SEARCH_URL: &str = "https://api.music.yandex.net/search";
const MAX_SEARCH_RESULTS: usize = 30;

/// Metadata for a single Yandex Music track returned from search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct YandexTrackInfo {
    pub id: String,
    pub title: String,
    pub version: Option<String>,
    pub artists: Vec<String>,
    pub duration_ms: Option<u64>,
}

impl YandexTrackInfo {
    /// Returns the canonical Yandex Music track URL compatible with `yandex-music-downloader` and `MediaCache`.
    pub fn canonical_url(&self) -> String {
        format!("https://music.yandex.ru/track/{}", self.id)
    }

    /// Formats a human-readable label for a Telegram inline keyboard button.
    pub fn button_label(&self) -> String {
        let artists_str = if self.artists.is_empty() {
            "Неизвестен".to_string()
        } else {
            self.artists.join(", ")
        };

        let full_title = match &self.version {
            Some(ver) if !ver.trim().is_empty() => format!("{} ({})", self.title, ver.trim()),
            _ => self.title.clone(),
        };

        let duration_suffix = match self.duration_ms {
            Some(ms) if ms > 0 => {
                let total_secs = ms / 1000;
                let mins = total_secs / 60;
                let secs = total_secs % 60;
                format!(" [{mins}:{secs:02}]")
            }
            _ => String::new(),
        };

        let base = format!("🎵 {artists_str} — {full_title}");
        let max_base_chars = 54usize;
        let truncated_base = if base.chars().count() > max_base_chars {
            let s: String = base.chars().take(max_base_chars - 1).collect();
            format!("{s}…")
        } else {
            base
        };

        format!("{truncated_base}{duration_suffix}")
    }
}

#[derive(Deserialize)]
struct SearchApiResponse {
    result: Option<SearchResultBlock>,
}

#[derive(Deserialize)]
struct SearchResultBlock {
    tracks: Option<SearchTracksBlock>,
}

#[derive(Deserialize)]
struct SearchTracksBlock {
    #[serde(default)]
    results: Vec<RawYandexTrack>,
}

#[derive(Deserialize)]
struct RawYandexTrack {
    id: serde_json::Value,
    title: Option<String>,
    version: Option<String>,
    available: Option<bool>,
    #[serde(rename = "durationMs")]
    duration_ms: Option<u64>,
    #[serde(default)]
    artists: Vec<RawYandexArtist>,
}

#[derive(Deserialize)]
struct RawYandexArtist {
    name: Option<String>,
}

/// Checks if the URL belongs to Yandex Music service
pub fn is_yandex_music_url(url: &str) -> bool {
    url.contains("music.yandex.") || url.contains("yandex.ru/music")
}

fn ym_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default()
    })
}

/// Searches Yandex Music tracks by query, applying artist-aware filtering and compilation deduplication.
pub async fn search_yandex_tracks(
    query: &str,
    token: Option<&str>,
) -> Result<Vec<YandexTrackInfo>> {
    let client = ym_http_client();

    let mut req = client
        .get(YANDEX_API_SEARCH_URL)
        .query(&[
            ("text", query),
            ("type", "track"),
            ("page", "0"),
            ("pageSize", "30"),
            ("nocorrect", "false"),
        ])
        .header("X-Yandex-Music-Client", "YandexMusicAndroid/24023621")
        .header("Accept-Language", "ru")
        .header("User-Agent", "Yandex-Music-API");

    if let Some(t) = token.filter(|s| !s.trim().is_empty()) {
        req = req.header("Authorization", format!("OAuth {t}"));
    }

    let resp = req
        .send()
        .await
        .context("Failed to send search request to Yandex Music API")?
        .error_for_status()
        .context("Yandex Music search API returned an error status")?;

    let parsed: SearchApiResponse = resp
        .json()
        .await
        .context("Failed to parse Yandex Music search response JSON")?;

    let raw_results = parsed
        .result
        .and_then(|r| r.tracks)
        .map(|t| t.results)
        .unwrap_or_default();

    let mut tracks = Vec::with_capacity(raw_results.len());
    for raw in raw_results {
        if raw.available == Some(false) {
            continue;
        }
        let id = match &raw.id {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) if !s.is_empty() => s.clone(),
            _ => continue,
        };
        let Some(title) = raw.title.filter(|t| !t.trim().is_empty()) else {
            continue;
        };
        let artists: Vec<String> = raw
            .artists
            .into_iter()
            .filter_map(|a| a.name)
            .filter(|n| !n.trim().is_empty())
            .collect();

        tracks.push(YandexTrackInfo {
            id,
            title,
            version: raw.version,
            artists,
            duration_ms: raw.duration_ms,
        });
    }

    Ok(filter_and_dedup_tracks(query, tracks))
}

/// Normalizes text for comparison: lowercase, replaces 'ё' with 'е', strips punctuation into single spaces.
fn normalize_text(s: &str) -> String {
    let mapped: String = s
        .to_lowercase()
        .chars()
        .map(|c| {
            if c == 'ё' {
                'е'
            } else if c.is_alphanumeric() {
                c
            } else {
                ' '
            }
        })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tokenize(s: &str) -> Vec<String> {
    normalize_text(s)
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// Checks whether `sub` tokens are all present in `sup` tokens.
fn tokens_subset(sub: &[String], sup: &[String]) -> bool {
    !sub.is_empty() && sub.iter().all(|t| sup.contains(t))
}

/// Checks if a track matches both an artist name and track title from the user's query.
fn track_matches_artist_and_title(query: &str, track: &YandexTrackInfo) -> bool {
    let title_full = match &track.version {
        Some(v) => format!("{} {}", track.title, v),
        None => track.title.clone(),
    };
    let title_tokens = tokenize(&title_full);
    let base_title_tokens = tokenize(&track.title);

    // 1. Check explicit separator (" - ", " — ", " – ")
    for sep in [" - ", " — ", " – "] {
        if let Some((left, right)) = query.split_once(sep) {
            let left_tokens = tokenize(left);
            let right_tokens = tokenize(right);
            if !left_tokens.is_empty() && !right_tokens.is_empty() {
                for artist in &track.artists {
                    let artist_tokens = tokenize(artist);
                    let direct = (tokens_subset(&artist_tokens, &left_tokens)
                        || tokens_subset(&left_tokens, &artist_tokens))
                        && (tokens_subset(&right_tokens, &title_tokens)
                            || tokens_subset(&base_title_tokens, &right_tokens));
                    let reversed = (tokens_subset(&artist_tokens, &right_tokens)
                        || tokens_subset(&right_tokens, &artist_tokens))
                        && (tokens_subset(&left_tokens, &title_tokens)
                            || tokens_subset(&base_title_tokens, &left_tokens));
                    if direct || reversed {
                        return true;
                    }
                }
            }
        }
    }

    // 2. Check space-separated query containing both artist and title tokens
    let q_tokens = tokenize(query);
    if q_tokens.len() < 2 {
        return false;
    }

    // If the entire query is already identical to the track's base title, don't treat part of the title as an artist
    if q_tokens == base_title_tokens {
        return false;
    }

    for artist in &track.artists {
        let artist_tokens = tokenize(artist);
        if artist_tokens.is_empty() || artist_tokens.len() >= q_tokens.len() {
            continue;
        }
        if tokens_subset(&artist_tokens, &q_tokens) {
            let rem_tokens: Vec<String> = q_tokens
                .iter()
                .filter(|t| !artist_tokens.contains(t))
                .cloned()
                .collect();

            if !rem_tokens.is_empty()
                && (tokens_subset(&rem_tokens, &title_tokens)
                    || tokens_subset(&base_title_tokens, &rem_tokens))
            {
                return true;
            }
        }
    }

    false
}

/// Filters out tracks from unrelated artists when an artist is present in the query,
/// and deduplicates identical compilation re-uploads.
pub fn filter_and_dedup_tracks(
    query: &str,
    tracks: Vec<YandexTrackInfo>,
) -> Vec<YandexTrackInfo> {
    let any_artist_title_match = tracks
        .iter()
        .any(|t| track_matches_artist_and_title(query, t));

    let filtered: Vec<YandexTrackInfo> = if any_artist_title_match {
        tracks
            .into_iter()
            .filter(|t| track_matches_artist_and_title(query, t))
            .collect()
    } else {
        tracks
    };

    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for track in filtered {
        let mut sorted_artists: Vec<String> =
            track.artists.iter().map(|a| normalize_text(a)).collect();
        sorted_artists.sort();
        let artists_key = sorted_artists.join(",");
        let title_key = normalize_text(&track.title);
        let version_key = track
            .version
            .as_deref()
            .map(normalize_text)
            .unwrap_or_default();

        if seen.insert((artists_key, title_key, version_key)) {
            deduped.push(track);
            if deduped.len() >= MAX_SEARCH_RESULTS {
                break;
            }
        }
    }

    deduped
}

/// Downloads track/album/playlist from Yandex Music into the destination directory
pub async fn download_yandex_track(
    url: &str,
    token: &str,
    ym_quality: crate::settings::YandexQualityPreference,
    output_dir: &Path,
    config: &Config,
    progress: &Arc<AtomicU8>,
) -> Result<Vec<PathBuf>> {
    let quality_arg = match ym_quality {
        crate::settings::YandexQualityPreference::Optimal => "1",
        crate::settings::YandexQualityPreference::Lossless => "2",
    };

    let mut child = Command::new(&config.yandex_downloader_bin)
        .arg("--quality")
        .arg(quality_arg)
        .arg("--token")
        .arg(token)
        .arg("--path-pattern")
        .arg("#title")
        .arg("--embed-cover")
        .arg("--dir")
        .arg(output_dir)
        .arg("--url")
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "Failed to execute {}. Make sure it is installed and present in PATH or specified in YANDEX_DOWNLOADER_BIN.",
                config.yandex_downloader_bin.display()
            )
        })?;

    let stdout_pipe = child.stdout.take().expect("stdout piped");
    let stderr_pipe = child.stderr.take().expect("stderr piped");

    let prog_clone = Arc::clone(progress);
    prog_clone.store(10, Ordering::Relaxed);

    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout_pipe).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if line.starts_with("[") {
                if let Some(bracket_end) = line.find("]") {
                    let inside = &line[1..bracket_end];
                    if let Some((cur_s, tot_s)) = inside.split_once("/") {
                        if let (Ok(cur), Ok(tot)) = (cur_s.trim().parse::<u32>(), tot_s.trim().parse::<u32>()) {
                            if tot > 0 {
                                let base_pct = ((cur.saturating_sub(1)) as f32 / tot as f32 * 80.0) as u8;
                                let track_pct = (base_pct + 15).min(90);
                                let prev = prog_clone.load(Ordering::Relaxed);
                                if track_pct > prev {
                                    prog_clone.store(track_pct, Ordering::Relaxed);
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let prog_ticker = Arc::clone(progress);
    let ticker_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
        interval.tick().await;
        loop {
            interval.tick().await;
            let cur = prog_ticker.load(Ordering::Relaxed);
            if cur < 90 {
                prog_ticker.store(cur.saturating_add(8).min(90), Ordering::Relaxed);
            }
        }
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr_pipe).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            collected.push_str(&line);
            collected.push('\n');
        }
        collected
    });

    let status = child
        .wait()
        .await
        .context("Failed waiting for yandex-music-downloader process")?;

    ticker_task.abort();
    let _ = stdout_task.await;
    let _ = stderr_task.await;

    if !status.success() {
        return Err(anyhow!(
            "yandex-music-downloader process exited with non-zero status"
        ));
    }

    progress.store(100, Ordering::Relaxed);

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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tracks() -> Vec<YandexTrackInfo> {
        vec![
            YandexTrackInfo {
                id: "100".to_string(),
                title: "Кукушка".to_string(),
                version: None,
                artists: vec!["Кино".to_string()],
                duration_ms: Some(399_000),
            },
            // Duplicate of Кино — Кукушка from another compilation album
            YandexTrackInfo {
                id: "101".to_string(),
                title: "Кукушка".to_string(),
                version: None,
                artists: vec!["Кино".to_string()],
                duration_ms: Some(399_000),
            },
            YandexTrackInfo {
                id: "200".to_string(),
                title: "Кукушка".to_string(),
                version: None,
                artists: vec!["Полина Гагарина".to_string()],
                duration_ms: Some(232_000),
            },
            YandexTrackInfo {
                id: "300".to_string(),
                title: "Кукушка".to_string(),
                version: None,
                artists: vec!["Земфира".to_string()],
                duration_ms: Some(310_000),
            },
        ]
    }

    #[test]
    fn test_search_by_title_only_keeps_different_artists_and_dedups_compilations() {
        let results = filter_and_dedup_tracks("Кукушка", sample_tracks());
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].id, "100");
        assert_eq!(results[1].id, "200");
        assert_eq!(results[2].id, "300");
    }

    #[test]
    fn test_search_with_artist_filters_out_other_artists() {
        let results = filter_and_dedup_tracks("Кино Кукушка", sample_tracks());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "100");
        assert_eq!(results[0].artists, vec!["Кино".to_string()]);

        let results_sep = filter_and_dedup_tracks("Полина Гагарина - Кукушка", sample_tracks());
        assert_eq!(results_sep.len(), 1);
        assert_eq!(results_sep[0].id, "200");
    }

    #[test]
    fn test_search_with_artist_keeps_distinct_versions_of_same_artist() {
        let mut tracks = sample_tracks();
        tracks.push(YandexTrackInfo {
            id: "102".to_string(),
            title: "Кукушка".to_string(),
            version: Some("Live".to_string()),
            artists: vec!["Кино".to_string()],
            duration_ms: Some(405_000),
        });

        let results = filter_and_dedup_tracks("Кино Кукушка", tracks);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "100");
        assert_eq!(results[1].id, "102");
    }
}
