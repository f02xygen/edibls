use anyhow::{anyhow, Context, Result};
use image::imageops::FilterType;
use image::ImageFormat;
use lofty::config::WriteOptions;
use lofty::picture::{Picture, PictureType};
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;
use serde::Deserialize;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::RwLock;


use crate::config::Config;
use crate::media::AudioTrack;
use crate::providers::ytdlp::download_media;

const MAX_SEARCH_RESULTS: usize = 8;
const MAX_PLAYLIST_TRACKS: usize = 15;

static SPOTIFY_TOKEN_CACHE: RwLock<Option<(String, Instant)>> = RwLock::const_new(None);

/// Metadata for a single Spotify track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotifyTrackInfo {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
    pub url: String,
    pub cover_url: Option<String>,
}

impl SpotifyTrackInfo {
    /// Formats a human-readable label for Telegram inline keyboard buttons.
    pub fn button_label(&self) -> String {
        let artists_str = if self.artists.is_empty() {
            "Неизвестен".to_string()
        } else {
            self.artists.join(", ")
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

        let base = format!("🟢 {artists_str} — {}", self.title);
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

#[derive(Debug, Clone)]
pub enum SpotifyEntity {
    Track(SpotifyTrackInfo),
    Album {
        title: String,
        tracks: Vec<SpotifyTrackInfo>,
    },
    Playlist {
        title: String,
        tracks: Vec<SpotifyTrackInfo>,
    },
}

/// Checks whether the URL is a Spotify link.
pub fn is_spotify_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.contains("open.spotify.com") || lower.contains("spotify.link")
}

/// Resolves possible short links (`spotify.link/...`) to canonical `open.spotify.com/...`.
pub async fn resolve_spotify_url(url: &str) -> Result<String> {
    if url.contains("spotify.link") {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?;
        let resp = client.head(url).send().await?;
        Ok(resp.url().to_string())
    } else {
        Ok(url.to_string())
    }
}

/// Extracts entity type ("track", "album", "playlist") and ID from a Spotify URL.
pub fn parse_spotify_entity(url: &str) -> Option<(&str, &str)> {
    let stripped = url.split('?').next().unwrap_or(url);
    let parts: Vec<&str> = stripped.trim_end_matches('/').split('/').collect();
    if parts.len() >= 2 {
        let id = parts[parts.len() - 1];
        let entity_type = parts[parts.len() - 2];
        if matches!(entity_type, "track" | "album" | "playlist") && !id.is_empty() {
            return Some((entity_type, id));
        }
    }
    None
}

/// Retrieves an anonymous access token for Spotify API queries.
async fn get_anonymous_token() -> Result<String> {
    {
        let cache = SPOTIFY_TOKEN_CACHE.read().await;
        if let Some((ref token, ref exp)) = *cache {
            if exp.elapsed() < Duration::from_secs(3000) {
                return Ok(token.clone());
            }
        }
    }

    // Fetch token from a standard embed page
    let client = reqwest::Client::new();
    let resp = client
        .get("https://open.spotify.com/embed/track/4cOdK2wGLETKBW3PvgPWqT")
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
        .send()
        .await
        .context("Failed to fetch Spotify embed page for token")?
        .text()
        .await?;

    let token = extract_token_from_html(&resp)
        .ok_or_else(|| anyhow!("Failed to extract access token from Spotify embed page"))?;

    let mut cache = SPOTIFY_TOKEN_CACHE.write().await;
    *cache = Some((token.clone(), Instant::now()));

    Ok(token)
}

fn extract_token_from_html(html: &str) -> Option<String> {
    let key = "\"accessToken\":\"";
    if let Some(pos) = html.find(key) {
        let start = pos + key.len();
        if let Some(end) = html[start..].find('"') {
            let tok = &html[start..start + end];
            if !tok.is_empty() {
                return Some(tok.to_string());
            }
        }
    }
    None
}

/// Extracts Next.js data payload JSON from Spotify embed HTML.
fn extract_next_data(html: &str) -> Result<serde_json::Value> {
    let prefix = "<script id=\"__NEXT_DATA__\" type=\"application/json\">";
    let start_pos = html
        .find(prefix)
        .ok_or_else(|| anyhow!("Could not find __NEXT_DATA__ in Spotify embed HTML"))?
        + prefix.len();

    let end_pos = html[start_pos..]
        .find("</script>")
        .ok_or_else(|| anyhow!("Could not find </script> close tag for __NEXT_DATA__"))?
        + start_pos;

    let json_str = &html[start_pos..end_pos];
    serde_json::from_str(json_str).context("Failed to parse __NEXT_DATA__ JSON")
}

/// Fetches full metadata for a Spotify track, album, or playlist without requiring API keys.
pub async fn fetch_spotify_entity(raw_url: &str) -> Result<SpotifyEntity> {
    let canonical_url = resolve_spotify_url(raw_url).await?;
    let (entity_type, id) = parse_spotify_entity(&canonical_url)
        .ok_or_else(|| anyhow!("Не удалось распознать тип объекта Spotify в ссылке"))?;

    let embed_url = format!("https://open.spotify.com/embed/{entity_type}/{id}");
    let client = reqwest::Client::new();
    let html = client
        .get(&embed_url)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")
        .send()
        .await
        .context("Failed to request Spotify embed page")?
        .text()
        .await?;

    // Cache accessToken if present
    if let Some(tok) = extract_token_from_html(&html) {
        let mut cache = SPOTIFY_TOKEN_CACHE.write().await;
        *cache = Some((tok, Instant::now()));
    }

    let root = extract_next_data(&html)?;
    let entity = root
        .pointer("/props/pageProps/state/data/entity")
        .ok_or_else(|| anyhow!("Отсутствует поле entity в данных Spotify"))?;

    match entity_type {
        "track" => {
            let title = entity
                .get("name")
                .or_else(|| entity.get("title"))
                .and_then(|v| v.as_str())
                .unwrap_or("Без названия")
                .to_string();

            let artists = entity
                .get("artists")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let duration_ms = entity.get("duration").and_then(|d| d.as_u64());

            let cover_url = entity
                .pointer("/visualIdentity/image/0/url")
                .or_else(|| entity.pointer("/coverArt/sources/0/url"))
                .and_then(|u| u.as_str())
                .map(String::from);

            Ok(SpotifyEntity::Track(SpotifyTrackInfo {
                id: id.to_string(),
                title,
                artists,
                album: None,
                duration_ms,
                url: format!("https://open.spotify.com/track/{id}"),
                cover_url,
            }))
        }
        "album" | "playlist" => {
            let title = entity
                .get("title")
                .or_else(|| entity.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("Коллекция Spotify")
                .to_string();

            let default_cover = entity
                .pointer("/visualIdentity/image/0/url")
                .or_else(|| entity.pointer("/coverArt/sources/0/url"))
                .and_then(|u| u.as_str())
                .map(String::from);

            let mut tracks = Vec::new();
            if let Some(arr) = entity.get("trackList").and_then(|v| v.as_array()) {
                for item in arr.iter().take(MAX_PLAYLIST_TRACKS) {
                    let track_title = item
                        .get("title")
                        .and_then(|t| t.as_str())
                        .unwrap_or("Трек")
                        .to_string();
                    let artists_str = item
                        .get("subtitle")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    let artists = if artists_str.is_empty() {
                        Vec::new()
                    } else {
                        vec![artists_str]
                    };
                    let duration_ms = item.get("duration").and_then(|d| d.as_u64());

                    let track_id = item
                        .get("uri")
                        .and_then(|u| u.as_str())
                        .and_then(|u| u.strip_prefix("spotify:track:"))
                        .unwrap_or_default();

                    let track_url = if track_id.is_empty() {
                        canonical_url.clone()
                    } else {
                        format!("https://open.spotify.com/track/{track_id}")
                    };

                    tracks.push(SpotifyTrackInfo {
                        id: track_id.to_string(),
                        title: track_title,
                        artists,
                        album: Some(title.clone()),
                        duration_ms,
                        url: track_url,
                        cover_url: default_cover.clone(),
                    });
                }
            }

            if entity_type == "album" {
                Ok(SpotifyEntity::Album { title, tracks })
            } else {
                Ok(SpotifyEntity::Playlist { title, tracks })
            }
        }
        _ => Err(anyhow!("Неподдерживаемый тип объекта Spotify: {entity_type}")),
    }
}

/// Searches Spotify tracks using the anonymous web API.
pub async fn search_spotify_tracks(query: &str) -> Result<Vec<SpotifyTrackInfo>> {
    let token = get_anonymous_token().await?;
    let client = reqwest::Client::new();

    let resp = client
        .get("https://api.spotify.com/v1/search")
        .header("Authorization", format!("Bearer {token}"))
        .query(&[("q", query), ("type", "track"), ("limit", &MAX_SEARCH_RESULTS.to_string())])
        .send()
        .await
        .context("Failed to query Spotify search API")?;

    if !resp.status().is_success() {
        return Err(anyhow!("Spotify search API returned status {}", resp.status()));
    }

    #[derive(Deserialize)]
    struct SpotifySearchResponse {
        tracks: Option<SpotifyTracksBlock>,
    }

    #[derive(Deserialize)]
    struct SpotifyTracksBlock {
        items: Vec<SpotifySearchItem>,
    }

    #[derive(Deserialize)]
    struct SpotifySearchItem {
        id: String,
        name: String,
        duration_ms: Option<u64>,
        artists: Vec<SpotifyArtistItem>,
        album: Option<SpotifyAlbumItem>,
        external_urls: Option<SpotifyUrlsItem>,
    }

    #[derive(Deserialize)]
    struct SpotifyArtistItem {
        name: String,
    }

    #[derive(Deserialize)]
    struct SpotifyAlbumItem {
        name: Option<String>,
        images: Option<Vec<SpotifyImageItem>>,
    }

    #[derive(Deserialize)]
    struct SpotifyImageItem {
        url: String,
    }

    #[derive(Deserialize)]
    struct SpotifyUrlsItem {
        spotify: Option<String>,
    }

    let search_res: SpotifySearchResponse = resp.json().await.context("Failed to deserialize search results")?;
    let mut results = Vec::new();

    if let Some(tracks_block) = search_res.tracks {
        for item in tracks_block.items {
            let artists = item.artists.into_iter().map(|a| a.name).collect();
            let cover_url = item
                .album
                .as_ref()
                .and_then(|alb| alb.images.as_ref())
                .and_then(|imgs| imgs.first())
                .map(|img| img.url.clone());
            let album = item.album.and_then(|alb| alb.name);
            let url = item
                .external_urls
                .and_then(|u| u.spotify)
                .unwrap_or_else(|| format!("https://open.spotify.com/track/{}", item.id));

            results.push(SpotifyTrackInfo {
                id: item.id,
                title: item.name,
                artists,
                album,
                duration_ms: item.duration_ms,
                url,
                cover_url,
            });
        }
    }

    Ok(results)
}

/// Matches a Spotify track with YouTube studio audio and returns the best video URL.
async fn find_matching_youtube_audio(track: &SpotifyTrackInfo) -> Result<String> {
    let artists_str = track.artists.join(" ");
    let search_query = format!("{} {} audio", artists_str, track.title);
    let search_spec = format!("ytsearch5:{search_query}");

    let output = Command::new("yt-dlp")
        .args(["--dump-json", "--flat-playlist", &search_spec])
        .output()
        .await
        .context("Failed to run yt-dlp search for Spotify track")?;

    if !output.status.success() {
        return Err(anyhow!("YouTube audio search failed for Spotify track"));
    }

    #[derive(Deserialize)]
    struct YtMatchItem {
        id: Option<String>,
        url: Option<String>,
        duration: Option<f64>,
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let target_sec = track.duration_ms.map(|ms| (ms / 1000) as i64).unwrap_or(0);

    let mut best_url = None;
    let mut smallest_diff = i64::MAX;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Ok(item) = serde_json::from_str::<YtMatchItem>(trimmed) {
            let target_url = item
                .url
                .filter(|u| u.starts_with("http"))
                .or_else(|| item.id.map(|id| format!("https://www.youtube.com/watch?v={id}")));

            let Some(url) = target_url else {
                continue;
            };

            if best_url.is_none() {
                best_url = Some(url.clone());
            }

            if target_sec > 0 {
                if let Some(dur) = item.duration {
                    let diff = ((dur.round() as i64) - target_sec).abs();
                    if diff < smallest_diff {
                        smallest_diff = diff;
                        best_url = Some(url);
                        if diff <= 5 {
                            break;
                        }
                    }
                }
            }
        }
    }

    best_url.ok_or_else(|| anyhow!("Не удалось найти подходящий аудиопоток для трека «{}»", track.title))
}

/// Embeds Spotify metadata and cover art into the downloaded audio file using lofty.
fn embed_spotify_tags(
    audio_path: &Path,
    track: &SpotifyTrackInfo,
    cover_bytes: Option<&[u8]>,
) {
    if let Ok(mut tagged_file) = Probe::open(audio_path).and_then(|p| p.read()) {
        let tag = match tagged_file.primary_tag_mut() {
            Some(t) => t,
            None => {
                if let Some(t) = tagged_file.first_tag_mut() {
                    t
                } else {
                    let tag_type = tagged_file.primary_tag_type();
                    tagged_file.insert_tag(Tag::new(tag_type));
                    tagged_file.primary_tag_mut().unwrap()
                }
            }
        };

        tag.set_title(track.title.clone());
        if !track.artists.is_empty() {
            tag.set_artist(track.artists.join(", "));
        }
        if let Some(ref alb) = track.album {
            tag.set_album(alb.clone());
        }

        if let Some(bytes) = cover_bytes {
            let pic = Picture::new_unchecked(
                PictureType::CoverFront,
                Some(lofty::picture::MimeType::Jpeg),
                None,
                bytes.to_vec(),
            );
            tag.push_picture(pic);
        }

        let _ = tag.save_to_path(audio_path, WriteOptions::default());
    }
}


/// Downloads a single Spotify track: matches audio on YouTube, downloads via `download_media`, tags, and prepares cover.
pub async fn download_spotify_track(
    track: &SpotifyTrackInfo,
    temp_dir: &Path,
    config: &Config,
    progress: &Arc<AtomicU8>,
) -> Result<AudioTrack> {
    log::info!("Searching and downloading Spotify track: {} - {}", track.artists.join(", "), track.title);

    let yt_url = find_matching_youtube_audio(track).await?;
    let downloaded_path = download_media(&yt_url, true, None, temp_dir, config, progress)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    // Download and resize cover art
    let mut cover_path_opt = None;
    let mut cover_bytes_opt = None;

    if let Some(ref cover_url) = track.cover_url {
        let client = reqwest::Client::new();
        if let Ok(resp) = client.get(cover_url).send().await {
            if let Ok(bytes) = resp.bytes().await {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let resized = img.resize(320, 320, FilterType::Triangle);
                    let target_cover_path = temp_dir.join(format!("cover_{}.jpg", track.id));
                    if resized.save_with_format(&target_cover_path, ImageFormat::Jpeg).is_ok() {
                        cover_path_opt = Some(target_cover_path);
                    }
                }
                cover_bytes_opt = Some(bytes.to_vec());
            }
        }
    }

    // Embed ID3 tags into MP3
    embed_spotify_tags(&downloaded_path, track, cover_bytes_opt.as_deref());

    let performer = if track.artists.is_empty() {
        None
    } else {
        Some(track.artists.join(", "))
    };

    Ok(AudioTrack {
        path: downloaded_path,
        title: Some(track.title.clone()),
        performer,
        cover_path: cover_path_opt,
    })
}

/// High-level function to download any Spotify entity (single track, album, or playlist).
pub async fn download_spotify_entity(
    url: &str,
    temp_dir: &Path,
    config: &Config,
    progress: &Arc<AtomicU8>,
) -> Result<Vec<AudioTrack>> {
    let entity = fetch_spotify_entity(url).await?;

    match entity {
        SpotifyEntity::Track(track) => {
            let audio_track = download_spotify_track(&track, temp_dir, config, progress).await?;
            progress.store(100, Ordering::Relaxed);
            Ok(vec![audio_track])
        }
        SpotifyEntity::Album { title, tracks } | SpotifyEntity::Playlist { title, tracks } => {
            if tracks.is_empty() {
                return Err(anyhow!("В альбоме/плейлисте «{title}» не найдено доступных треков"));
            }

            let total = tracks.len();
            let mut downloaded = Vec::new();

            for (idx, track) in tracks.iter().enumerate() {
                let track_progress = Arc::new(AtomicU8::new(0));
                match download_spotify_track(track, temp_dir, config, &track_progress).await {
                    Ok(t) => downloaded.push(t),
                    Err(e) => {
                        log::warn!("Failed to download track «{}» from {}: {e}", track.title, title);
                    }
                }
                let overall = (((idx + 1) * 100) / total) as u8;
                progress.store(overall.min(100), Ordering::Relaxed);
            }

            if downloaded.is_empty() {
                return Err(anyhow!("Не удалось скачать ни один трек из коллекции «{title}»"));
            }

            Ok(downloaded)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_spotify_url() {
        assert!(is_spotify_url("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"));
        assert!(is_spotify_url("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3"));
        assert!(is_spotify_url("https://spotify.link/abc123xyz"));
        assert!(!is_spotify_url("https://music.yandex.ru/track/123"));
        assert!(!is_spotify_url("https://soundcloud.com/artist/track"));
    }

    #[test]
    fn test_parse_spotify_entity() {
        assert_eq!(
            parse_spotify_entity("https://open.spotify.com/track/4cOdK2wGLETKBW3PvgPWqT"),
            Some(("track", "4cOdK2wGLETKBW3PvgPWqT"))
        );
        assert_eq!(
            parse_spotify_entity("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3?si=abc"),
            Some(("album", "1DFixLWuPkv3KT3TnV35m3"))
        );
        assert_eq!(
            parse_spotify_entity("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M/"),
            Some(("playlist", "37i9dQZF1DXcBWIGoYBM5M"))
        );
        assert_eq!(parse_spotify_entity("https://open.spotify.com/user/123"), None);
    }

    #[test]
    fn test_spotify_button_label() {
        let track = SpotifyTrackInfo {
            id: "123".to_string(),
            title: "Never Gonna Give You Up".to_string(),
            artists: vec!["Rick Astley".to_string()],
            album: None,
            duration_ms: Some(213573),
            url: "https://open.spotify.com/track/123".to_string(),
            cover_url: None,
        };
        assert_eq!(track.button_label(), "🟢 Rick Astley — Never Gonna Give You Up [3:33]");
    }
}
