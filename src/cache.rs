use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use teloxide::types::FileId;

use crate::providers::pornhub::PornhubVideoInfo;
use crate::providers::soundcloud::SoundcloudTrackInfo;
use crate::providers::yandex::YandexTrackInfo;
use crate::providers::youtube::{YouTubeChapter, YouTubeVideoInfo};

/// Maximum total media size tracked by the cache (5 GB).
const MAX_CACHE_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// Maximum number of cached entries to bound memory usage.
const MAX_CACHE_ENTRIES: usize = 5_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CachedMedia {
    Video(FileId),
    Audio(Vec<FileId>),
    Photos {
        photo_file_ids: Vec<FileId>,
        audio_file_id: Option<FileId>,
    },
}

#[derive(Clone)]
struct CacheEntry {
    media: CachedMedia,
    size_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedMediaItem {
    url: String,
    audio_only: bool,
    max_height: Option<u16>,
    #[serde(default)]
    send_as_document: bool,
    media: CachedMedia,
    size_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct PersistedUrlItem {
    id: u64,
    url: String,
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedCache {
    next_id: u64,
    #[serde(default)]
    media_items: Vec<PersistedMediaItem>,
    #[serde(default)]
    url_items: Vec<PersistedUrlItem>,
}

#[derive(Clone, Default)]
pub struct MediaCache {
    inner: Arc<Mutex<CacheState>>,
    next_id: Arc<AtomicU64>,
    storage_path: Option<PathBuf>,
}

/// Normalizes media URLs to increase cache hit rate across clients and variations.
pub fn normalize_media_url(raw_url: &str) -> String {
    let trimmed = raw_url.trim();
    let (base_part, fragment_part) = match trimmed.split_once('#') {
        Some((b, f)) => (b.trim(), Some(f.trim())),
        None => (trimmed, None),
    };

    let Ok(mut parsed) = reqwest::Url::parse(base_part) else {
        return trimmed.to_string();
    };

    let host = parsed.host_str().unwrap_or("").to_lowercase();

    // 1. YouTube normalization
    let normalized_base = if host == "youtu.be" {
        let video_id = parsed.path().trim_start_matches('/');
        if !video_id.is_empty() {
            Some(format!("https://www.youtube.com/watch?v={video_id}"))
        } else {
            None
        }
    } else if host.contains("youtube.com") {
        if parsed.path().starts_with("/shorts/") {
            let video_id = parsed.path().trim_start_matches("/shorts/").trim_end_matches('/');
            if !video_id.is_empty() {
                Some(format!("https://www.youtube.com/watch?v={video_id}"))
            } else {
                None
            }
        } else if parsed.path().starts_with("/watch") {
            let mut video_id = None;
            for (k, v) in parsed.query_pairs() {
                if k == "v" {
                    video_id = Some(v.into_owned());
                    break;
                }
            }
            video_id.map(|id| format!("https://www.youtube.com/watch?v={id}"))
        } else {
            None
        }
    } else if host.contains("music.yandex.") || host.contains("yandex.ru") {
        let path = parsed.path();
        if let Some(idx) = path.find("/track/") {
            let track_part = &path[idx + "/track/".len()..];
            let tid = track_part.split('/').next().unwrap_or("");
            if !tid.is_empty() && tid.chars().all(|c| c.is_ascii_digit()) {
                Some(format!("https://music.yandex.ru/track/{tid}"))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let mut result = match normalized_base {
        Some(base) => base,
        None => {
            // Strip common tracking query params
            let tracking_params = [
                "utm_source", "utm_medium", "utm_campaign", "utm_term", "utm_content",
                "si", "feature", "igsh", "igshid", "_t", "_r", "sender_device",
                "is_from_webapp", "share_id", "source", "ref", "fbclid",
            ];

            let query_pairs: Vec<(String, String)> = parsed
                .query_pairs()
                .filter(|(k, _)| !tracking_params.contains(&k.as_ref()))
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();

            parsed.set_query(None);
            if !query_pairs.is_empty() {
                let mut pairs_mut = parsed.query_pairs_mut();
                for (k, v) in &query_pairs {
                    pairs_mut.append_pair(k, v);
                }
            }

            let mut res = parsed.to_string();
            if res.ends_with('/') && !res.ends_with("://") {
                res.pop();
            }
            res
        }
    };

    if let Some(frag) = fragment_part {
        if !frag.is_empty() {
            result.push('#');
            result.push_str(frag);
        }
    }

    result
}

#[derive(Default)]
struct CacheState {
    media: HashMap<(String, bool, Option<u16>, bool), CacheEntry>,
    media_order: VecDeque<(String, bool, Option<u16>, bool)>,
    total_bytes: u64,
    url_to_id: HashMap<String, u64>,
    id_to_url: HashMap<u64, String>,
    url_order: VecDeque<(u64, String)>,
    ph_searches: HashMap<u64, (String, Vec<PornhubVideoInfo>)>,
    ph_search_order: VecDeque<u64>,
    ym_searches: HashMap<u64, (String, Vec<YandexTrackInfo>)>,
    ym_search_order: VecDeque<u64>,
    yt_searches: HashMap<u64, (String, Vec<YouTubeVideoInfo>)>,
    yt_search_order: VecDeque<u64>,
    yt_chapters: HashMap<u64, Vec<YouTubeChapter>>,
    yt_chapter_order: VecDeque<u64>,
    sc_searches: HashMap<u64, (String, Vec<SoundcloudTrackInfo>)>,
    sc_search_order: VecDeque<u64>,
    search_queries: HashMap<u64, String>,
    search_query_order: VecDeque<u64>,
}

impl MediaCache {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheState::default())),
            next_id: Arc::new(AtomicU64::new(1)),
            storage_path: None,
        }
    }

    /// Loads the persistent `MediaCache` from disk if `path` exists, or starts fresh.
    pub fn load_or_create<P: AsRef<Path>>(path: P) -> Self {
        let storage_path = path.as_ref().to_path_buf();
        let mut state = CacheState::default();
        let mut next_id = 1u64;

        if storage_path.exists() {
            match std::fs::read_to_string(&storage_path) {
                Ok(content) => match serde_json::from_str::<PersistedCache>(&content) {
                    Ok(persisted) => {
                        next_id = persisted.next_id.max(1);
                        for item in persisted.media_items {
                            let key = (item.url, item.audio_only, item.max_height, item.send_as_document);
                            state.total_bytes = state.total_bytes.saturating_add(item.size_bytes);
                            state.media_order.push_back(key.clone());
                            state.media.insert(
                                key,
                                CacheEntry {
                                    media: item.media,
                                    size_bytes: item.size_bytes,
                                },
                            );
                        }
                        for u in persisted.url_items {
                            if u.id >= next_id {
                                next_id = u.id + 1;
                            }
                            state.url_to_id.insert(u.url.clone(), u.id);
                            state.id_to_url.insert(u.id, u.url.clone());
                            state.url_order.push_back((u.id, u.url));
                        }
                        log::info!(
                            "Loaded MediaCache from {:?} ({} media entries, {} URLs)",
                            storage_path,
                            state.media.len(),
                            state.url_to_id.len()
                        );
                    }
                    Err(e) => {
                        log::warn!(
                            "Failed to parse cache file {:?}: {}, starting fresh",
                            storage_path,
                            e
                        );
                    }
                },
                Err(e) => {
                    log::warn!(
                        "Failed to read cache file {:?}: {}, starting fresh",
                        storage_path,
                        e
                    );
                }
            }
        }

        Self {
            inner: Arc::new(Mutex::new(state)),
            next_id: Arc::new(AtomicU64::new(next_id)),
            storage_path: Some(storage_path),
        }
    }

    fn save_locked(&self, state: &CacheState) {
        let Some(path) = &self.storage_path else {
            return;
        };

        let media_items: Vec<PersistedMediaItem> = state
            .media_order
            .iter()
            .filter_map(|key| {
                state.media.get(key).map(|entry| PersistedMediaItem {
                    url: key.0.clone(),
                    audio_only: key.1,
                    max_height: key.2,
                    send_as_document: key.3,
                    media: entry.media.clone(),
                    size_bytes: entry.size_bytes,
                })
            })
            .collect();

        let url_items: Vec<PersistedUrlItem> = state
            .url_order
            .iter()
            .map(|(id, url)| PersistedUrlItem {
                id: *id,
                url: url.clone(),
            })
            .collect();

        let persisted = PersistedCache {
            next_id: self.next_id.load(Ordering::Relaxed),
            media_items,
            url_items,
        };

        match serde_json::to_string(&persisted) {
            Ok(json) => {
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
                let tmp_path = path.with_extension("tmp");
                if let Err(e) = std::fs::write(&tmp_path, json) {
                    log::error!("Failed to write temporary MediaCache to {:?}: {}", tmp_path, e);
                } else if let Err(e) = std::fs::rename(&tmp_path, path) {
                    log::error!("Failed to atomic rename MediaCache to {:?}: {}", path, e);
                }
            }
            Err(e) => log::error!("Failed to serialize MediaCache: {}", e),
        }
    }

    /// Retrieves cached Telegram `FileId`(s) for the given URL, mode (`audio_only`), resolution (`max_height`), and format (`send_as_document`).
    pub fn get(&self, url: &str, audio_only: bool, max_height: Option<u16>, send_as_document: bool) -> Option<CachedMedia> {
        let normalized = normalize_media_url(url);
        let state = self.inner.lock().ok()?;
        state
            .media
            .get(&(normalized, audio_only, max_height, send_as_document))
            .map(|entry| entry.media.clone())
    }

    /// Looks up any cached video `FileId` for the given URL, preferring one that matches `preferred_height` if specified.
    pub fn get_cached_video_file_id(&self, url: &str, preferred_height: Option<u16>) -> Option<FileId> {
        let normalized = normalize_media_url(url);
        let state = self.inner.lock().ok()?;
        let mut fallback = None;
        for ((entry_url, audio_only, height, _), entry) in &state.media {
            if !audio_only && entry_url == &normalized {
                if let CachedMedia::Video(ref fid) = entry.media {
                    if *height == preferred_height {
                        return Some(fid.clone());
                    }
                    if fallback.is_none() {
                        fallback = Some(fid.clone());
                    }
                }
            }
        }
        fallback
    }

    /// Looks up any cached audio `FileId` for the given URL.
    pub fn get_cached_audio_file_id(&self, url: &str) -> Option<FileId> {
        let normalized = normalize_media_url(url);
        let state = self.inner.lock().ok()?;
        for ((entry_url, audio_only, _, _), entry) in &state.media {
            if *audio_only && entry_url == &normalized {
                if let CachedMedia::Audio(ref fids) = entry.media {
                    if let Some(first) = fids.first() {
                        return Some(first.clone());
                    }
                }
            }
        }
        None
    }

    /// Stores Telegram `FileId`(s) for the given URL, mode (`audio_only`), resolution (`max_height`), and format (`send_as_document`),
    /// evicting the earliest entries once the 5 GB or entry limit is reached.
    pub fn insert(
        &self,
        url: &str,
        audio_only: bool,
        max_height: Option<u16>,
        send_as_document: bool,
        media: CachedMedia,
        size_bytes: u64,
    ) {
        let normalized = normalize_media_url(url);
        if let Ok(mut state) = self.inner.lock() {
            let key = (normalized, audio_only, max_height, send_as_document);

            if let Some(old) = state.media.remove(&key) {
                state.total_bytes = state.total_bytes.saturating_sub(old.size_bytes);
                state.media_order.retain(|k| k != &key);
            }

            // Evict earliest entries (FIFO) until within the 5 GB and max-entry limits
            while (!state.media_order.is_empty()
                && state.total_bytes.saturating_add(size_bytes) > MAX_CACHE_BYTES)
                || state.media_order.len() >= MAX_CACHE_ENTRIES
            {
                if let Some(oldest_key) = state.media_order.pop_front() {
                    if let Some(removed) = state.media.remove(&oldest_key) {
                        state.total_bytes = state.total_bytes.saturating_sub(removed.size_bytes);
                    }
                } else {
                    break;
                }
            }

            state.total_bytes = state.total_bytes.saturating_add(size_bytes);
            state.media_order.push_back(key.clone());
            state.media.insert(key, CacheEntry { media, size_bytes });
            self.save_locked(&state);
        }
    }

    /// Registers a URL and returns a compact numeric ID suitable for Telegram's 64-byte `callback_data`.
    pub fn register_url(&self, url: &str) -> u64 {
        let normalized = normalize_media_url(url);
        if let Ok(mut state) = self.inner.lock() {
            if let Some(&id) = state.url_to_id.get(&normalized) {
                return id;
            }

            while state.url_order.len() >= MAX_CACHE_ENTRIES {
                if let Some((old_id, old_url)) = state.url_order.pop_front() {
                    state.id_to_url.remove(&old_id);
                    state.url_to_id.remove(&old_url);
                }
            }

            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.url_to_id.insert(normalized.clone(), id);
            state.id_to_url.insert(id, normalized.clone());
            state.url_order.push_back((id, normalized));
            self.save_locked(&state);
            id
        } else {
            0
        }
    }

    /// Resolves a compact numeric ID back to the original URL.
    pub fn resolve_url(&self, id: u64) -> Option<String> {
        let state = self.inner.lock().ok()?;
        state.id_to_url.get(&id).cloned()
    }

    /// Stores a raw search query and returns a compact search ID.
    pub fn store_search_query(&self, query: String) -> u64 {
        if let Ok(mut state) = self.inner.lock() {
            while state.search_query_order.len() >= 500 {
                if let Some(old_id) = state.search_query_order.pop_front() {
                    state.search_queries.remove(&old_id);
                }
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.search_query_order.push_back(id);
            state.search_queries.insert(id, query);
            id
        } else {
            0
        }
    }

    /// Retrieves a search query by search ID across any registered search cache.
    pub fn get_search_query(&self, id: u64) -> Option<String> {
        let state = self.inner.lock().ok()?;
        if let Some(q) = state.search_queries.get(&id) {
            return Some(q.clone());
        }
        if let Some((q, _)) = state.ym_searches.get(&id) {
            return Some(q.clone());
        }
        if let Some((q, _)) = state.sc_searches.get(&id) {
            return Some(q.clone());
        }
        if let Some((q, _)) = state.yt_searches.get(&id) {
            return Some(q.clone());
        }
        if let Some((q, _)) = state.ph_searches.get(&id) {
            return Some(q.clone());
        }
        None
    }

    /// Stores Pornhub search results and returns a compact search ID.
    pub fn store_ph_search(&self, query: String, videos: Vec<PornhubVideoInfo>) -> u64 {
        if let Ok(mut state) = self.inner.lock() {
            while state.ph_search_order.len() >= 500 {
                if let Some(old_id) = state.ph_search_order.pop_front() {
                    state.ph_searches.remove(&old_id);
                }
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.ph_search_order.push_back(id);
            state.search_queries.insert(id, query.clone());
            state.ph_searches.insert(id, (query, videos));
            id
        } else {
            0
        }
    }

    /// Retrieves cached Pornhub search results by search ID.
    pub fn get_ph_search(&self, id: u64) -> Option<(String, Vec<PornhubVideoInfo>)> {
        let state = self.inner.lock().ok()?;
        state.ph_searches.get(&id).cloned()
    }

    /// Stores Yandex Music search results and returns a compact search ID.
    pub fn store_ym_search(&self, query: String, tracks: Vec<YandexTrackInfo>) -> u64 {
        if let Ok(mut state) = self.inner.lock() {
            while state.ym_search_order.len() >= 500 {
                if let Some(old_id) = state.ym_search_order.pop_front() {
                    state.ym_searches.remove(&old_id);
                }
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.ym_search_order.push_back(id);
            state.search_queries.insert(id, query.clone());
            state.ym_searches.insert(id, (query, tracks));
            id
        } else {
            0
        }
    }

    /// Retrieves cached Yandex Music search results by search ID.
    pub fn get_ym_search(&self, id: u64) -> Option<(String, Vec<YandexTrackInfo>)> {
        let state = self.inner.lock().ok()?;
        state.ym_searches.get(&id).cloned()
    }

    /// Stores YouTube search results and returns a compact search ID.
    pub fn store_yt_search(&self, query: String, videos: Vec<YouTubeVideoInfo>) -> u64 {
        if let Ok(mut state) = self.inner.lock() {
            while state.yt_search_order.len() >= 500 {
                if let Some(old_id) = state.yt_search_order.pop_front() {
                    state.yt_searches.remove(&old_id);
                }
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.yt_search_order.push_back(id);
            state.search_queries.insert(id, query.clone());
            state.yt_searches.insert(id, (query, videos));
            id
        } else {
            0
        }
    }

    /// Retrieves cached YouTube search results by search ID.
    pub fn get_yt_search(&self, id: u64) -> Option<(String, Vec<YouTubeVideoInfo>)> {
        let state = self.inner.lock().ok()?;
        state.yt_searches.get(&id).cloned()
    }

    /// Stores YouTube chapters for a registered URL ID.
    pub fn store_yt_chapters(&self, url_id: u64, chapters: Vec<YouTubeChapter>) {
        if let Ok(mut state) = self.inner.lock() {
            while state.yt_chapter_order.len() >= 500 {
                if let Some(old_id) = state.yt_chapter_order.pop_front() {
                    state.yt_chapters.remove(&old_id);
                }
            }
            state.yt_chapter_order.push_back(url_id);
            state.yt_chapters.insert(url_id, chapters);
        }
    }

    /// Retrieves cached YouTube chapters by URL ID.
    pub fn get_yt_chapters(&self, url_id: u64) -> Option<Vec<YouTubeChapter>> {
        let state = self.inner.lock().ok()?;
        state.yt_chapters.get(&url_id).cloned()
    }

    /// Stores SoundCloud search results and returns a compact search ID.
    pub fn store_sc_search(&self, query: String, tracks: Vec<SoundcloudTrackInfo>) -> u64 {
        if let Ok(mut state) = self.inner.lock() {
            while state.sc_search_order.len() >= 500 {
                if let Some(old_id) = state.sc_search_order.pop_front() {
                    state.sc_searches.remove(&old_id);
                }
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.sc_search_order.push_back(id);
            state.search_queries.insert(id, query.clone());
            state.sc_searches.insert(id, (query, tracks));
            id
        } else {
            0
        }
    }

    /// Retrieves cached SoundCloud search results by search ID.
    pub fn get_sc_search(&self, id: u64) -> Option<(String, Vec<SoundcloudTrackInfo>)> {
        let state = self.inner.lock().ok()?;
        state.sc_searches.get(&id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_media_cache_persistence() {
        let tmp = NamedTempFile::new().unwrap();
        let cache = MediaCache::load_or_create(tmp.path());

        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let url_id = cache.register_url(url);
        assert_eq!(url_id, 1);

        cache.insert(
            url,
            false,
            Some(720),
            false,
            CachedMedia::Video(FileId::from("test_file_id_720")),
            10_000_000,
        );

        let reloaded = MediaCache::load_or_create(tmp.path());
        assert_eq!(reloaded.resolve_url(url_id).as_deref(), Some(url));
        assert_eq!(
            reloaded.get(url, false, Some(720), false),
            Some(CachedMedia::Video(FileId::from("test_file_id_720")))
        );
        assert_eq!(reloaded.get(url, false, Some(720), true), None);

        // Next registered URL gets a new non-colliding ID
        let url2_id = reloaded.register_url("https://soundcloud.com/artist/track");
        assert!(url2_id > url_id);
    }

    #[test]
    fn test_ph_search_cache_store_and_get() {
        let cache = MediaCache::new();
        let video = PornhubVideoInfo {
            vkey: "ph123".to_string(),
            title: "Test Video".to_string(),
            uploader: Some("Test Channel".to_string()),
            duration: Some("10:00".to_string()),
        };
        let id = cache.store_ph_search("test query".to_string(), vec![video.clone()]);
        assert_ne!(id, 0);

        let retrieved = cache.get_ph_search(id);
        assert!(retrieved.is_some());
        let (query, list) = retrieved.unwrap();
        assert_eq!(query, "test query");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].vkey, "ph123");
    }

    #[test]
    fn test_ym_search_cache_store_and_get() {
        let cache = MediaCache::new();
        let track = YandexTrackInfo {
            id: "456".to_string(),
            title: "Test Song".to_string(),
            version: None,
            artists: vec!["Artist".to_string()],
            duration_ms: Some(180000),
            cover_uri: None,
        };
        let id = cache.store_ym_search("song query".to_string(), vec![track.clone()]);
        assert_ne!(id, 0);

        let retrieved = cache.get_ym_search(id);
        assert!(retrieved.is_some());
        let (query, list) = retrieved.unwrap();
        assert_eq!(query, "song query");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "456");
    }

    #[test]
    fn test_yt_search_cache_store_and_get() {
        let cache = MediaCache::new();
        let video = YouTubeVideoInfo {
            id: "yt123".to_string(),
            title: "Test YT Video".to_string(),
            uploader: Some("Channel".to_string()),
            duration_str: Some("2:00".to_string()),
            url: "https://www.youtube.com/watch?v=yt123".to_string(),
        };
        let id = cache.store_yt_search("yt query".to_string(), vec![video.clone()]);
        assert_ne!(id, 0);

        let retrieved = cache.get_yt_search(id);
        assert!(retrieved.is_some());
        let (query, list) = retrieved.unwrap();
        assert_eq!(query, "yt query");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "yt123");
    }

    #[test]
    fn test_sc_search_cache_store_and_get() {
        let cache = MediaCache::new();
        let track = SoundcloudTrackInfo {
            id: "sc123".to_string(),
            title: "Test SC Track".to_string(),
            uploader: Some("Artist".to_string()),
            duration_str: Some("3:00".to_string()),
            url: "https://soundcloud.com/test".to_string(),
        };
        let id = cache.store_sc_search("sc query".to_string(), vec![track.clone()]);
        assert_ne!(id, 0);

        let retrieved = cache.get_sc_search(id);
        assert!(retrieved.is_some());
        let (query, list) = retrieved.unwrap();
        assert_eq!(query, "sc query");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "sc123");
    }

    #[test]
    fn test_normalize_media_url() {
        assert_eq!(
            normalize_media_url("https://youtu.be/dQw4w9WgXcQ?si=abc12345"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            normalize_media_url("https://www.youtube.com/shorts/dQw4w9WgXcQ?feature=share"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
        );
        assert_eq!(
            normalize_media_url("https://music.yandex.ru/album/123/track/456?utm_source=telegram"),
            "https://music.yandex.ru/track/456"
        );
        assert_eq!(
            normalize_media_url("https://soundcloud.com/artist/track?si=abc&utm_medium=sharing"),
            "https://soundcloud.com/artist/track"
        );
        assert_eq!(
            normalize_media_url("https://youtu.be/dQw4w9WgXcQ?si=abc#clip=10-20"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ#clip=10-20"
        );
        assert_eq!(
            normalize_media_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ#subway"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ#subway"
        );
        assert_eq!(
            normalize_media_url("https://youtu.be/dQw4w9WgXcQ?si=abc#speed=1.50"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ#speed=1.50"
        );
        assert_eq!(
            normalize_media_url("https://youtu.be/dQw4w9WgXcQ?si=abc#clip=10-30#speed=2.00"),
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ#clip=10-30#speed=2.00"
        );
    }

    #[test]
    fn test_search_query_retrieval() {
        let cache = MediaCache::new();
        let q_id = cache.store_search_query("Bohemian Rhapsody".to_string());
        assert_eq!(cache.get_search_query(q_id).as_deref(), Some("Bohemian Rhapsody"));
    }

    #[test]
    fn test_yt_chapters_cache() {
        let cache = MediaCache::new();
        let chs = vec![
            YouTubeChapter {
                title: "Intro".to_string(),
                start_secs: 0,
                end_secs: 30,
            },
            YouTubeChapter {
                title: "Drop".to_string(),
                start_secs: 30,
                end_secs: 120,
            },
        ];
        cache.store_yt_chapters(55, chs.clone());
        let retrieved = cache.get_yt_chapters(55);
        assert_eq!(retrieved, Some(chs));
        assert_eq!(cache.get_yt_chapters(999), None);
    }

    #[test]
    fn test_get_cached_video_file_id() {
        let cache = MediaCache::new();
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ#clip=10-20";
        cache.insert(
            url,
            false,
            Some(720),
            false,
            CachedMedia::Video(FileId("file_video_720".to_string())),
            1000,
        );

        // Exact match on height
        assert_eq!(
            cache.get_cached_video_file_id(url, Some(720)),
            Some(FileId("file_video_720".to_string()))
        );
        // Fallback when height is None or different
        assert_eq!(
            cache.get_cached_video_file_id(url, None),
            Some(FileId("file_video_720".to_string()))
        );
        assert_eq!(
            cache.get_cached_video_file_id(url, Some(1080)),
            Some(FileId("file_video_720".to_string()))
        );
        // Missing URL
        assert_eq!(
            cache.get_cached_video_file_id("https://youtu.be/other", None),
            None
        );
    }

    #[test]
    fn test_get_cached_audio_file_id() {
        let cache = MediaCache::new();
        let url = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        cache.insert(
            url,
            true,
            None,
            false,
            CachedMedia::Audio(vec![FileId("file_audio_123".to_string())]),
            5000,
        );

        assert_eq!(
            cache.get_cached_audio_file_id(url),
            Some(FileId("file_audio_123".to_string()))
        );
        assert_eq!(
            cache.get_cached_audio_file_id("https://youtu.be/dQw4w9WgXcQ"),
            Some(FileId("file_audio_123".to_string()))
        );
        assert_eq!(
            cache.get_cached_audio_file_id("https://youtu.be/missing"),
            None
        );
    }
}
