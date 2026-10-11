use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use teloxide::types::FileId;

#[derive(Clone, Debug)]
pub enum CachedMedia {
    Video(FileId),
    Audio(Vec<FileId>),
}

#[derive(Clone, Default)]
pub struct MediaCache {
    inner: Arc<Mutex<CacheState>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Default)]
struct CacheState {
    media: HashMap<(String, bool), CachedMedia>,
    url_to_id: HashMap<String, u64>,
    id_to_url: HashMap<u64, String>,
}

impl MediaCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(CacheState::default())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Retrieves cached Telegram `FileId`(s) for the given URL and mode (`audio_only`).
    pub fn get(&self, url: &str, audio_only: bool) -> Option<CachedMedia> {
        let state = self.inner.lock().ok()?;
        state.media.get(&(url.to_string(), audio_only)).cloned()
    }

    /// Stores Telegram `FileId`(s) for the given URL and mode (`audio_only`).
    pub fn insert(&self, url: &str, audio_only: bool, media: CachedMedia) {
        if let Ok(mut state) = self.inner.lock() {
            state.media.insert((url.to_string(), audio_only), media);
        }
    }

    /// Registers a URL and returns a compact numeric ID suitable for Telegram's 64-byte `callback_data`.
    pub fn register_url(&self, url: &str) -> u64 {
        if let Ok(mut state) = self.inner.lock() {
            if let Some(&id) = state.url_to_id.get(url) {
                return id;
            }
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            state.url_to_id.insert(url.to_string(), id);
            state.id_to_url.insert(id, url.to_string());
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
