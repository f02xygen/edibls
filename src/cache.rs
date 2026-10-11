use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use teloxide::types::FileId;

/// Maximum total media size tracked by the cache (5 GB).
const MAX_CACHE_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// Maximum number of cached entries to bound memory usage.
const MAX_CACHE_ENTRIES: usize = 5_000;

#[derive(Clone, Debug)]
pub enum CachedMedia {
    Video(FileId),
    Audio(Vec<FileId>),
}

#[derive(Clone)]
struct CacheEntry {
    media: CachedMedia,
    size_bytes: u64,
}

#[derive(Clone, Default)]
pub struct MediaCache {
    inner: Arc<Mutex<CacheState>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Default)]
struct CacheState {
    media: HashMap<(String, bool, Option<u16>), CacheEntry>,
    media_order: VecDeque<(String, bool, Option<u16>)>,
    total_bytes: u64,
    url_to_id: HashMap<String, u64>,
    id_to_url: HashMap<u64, String>,
    url_order: VecDeque<(u64, String)>,
}

impl MediaCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheState::default())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Retrieves cached Telegram `FileId`(s) for the given URL, mode (`audio_only`), and resolution (`max_height`).
    pub fn get(&self, url: &str, audio_only: bool, max_height: Option<u16>) -> Option<CachedMedia> {
        let state = self.inner.lock().ok()?;
        state
            .media
            .get(&(url.to_string(), audio_only, max_height))
            .map(|entry| entry.media.clone())
    }

    /// Stores Telegram `FileId`(s) for the given URL, mode (`audio_only`), and resolution (`max_height`),
    /// evicting the earliest entries once the 5 GB or entry limit is reached.
    pub fn insert(
        &self,
        url: &str,
        audio_only: bool,
        max_height: Option<u16>,
        media: CachedMedia,
        size_bytes: u64,
    ) {
        if let Ok(mut state) = self.inner.lock() {
            let key = (url.to_string(), audio_only, max_height);

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
        }
    }

    /// Registers a URL and returns a compact numeric ID suitable for Telegram's 64-byte `callback_data`.
    pub fn register_url(&self, url: &str) -> u64 {
        if let Ok(mut state) = self.inner.lock() {
            if let Some(&id) = state.url_to_id.get(url) {
                return id;
            }

            while state.url_order.len() >= MAX_CACHE_ENTRIES {
                if let Some((old_id, old_url)) = state.url_order.pop_front() {
                    state.id_to_url.remove(&old_id);
                    state.url_to_id.remove(&old_url);
                }
            }

            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.url_to_id.insert(url.to_string(), id);
            state.id_to_url.insert(id, url.to_string());
            state.url_order.push_back((id, url.to_string()));
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
}
