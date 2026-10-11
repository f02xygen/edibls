use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use teloxide::types::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum QualityPreference {
    #[default]
    AutoBest,     // Do not ask, download best quality
    AskEveryTime, // Prompt with quality buttons before downloading
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UserSettings {
    #[serde(default)]
    pub quality: QualityPreference,
    #[serde(default)]
    pub audio_only: bool, // If true, all media requests are extracted as MP3
}

#[derive(Clone)]
pub struct UserSettingsStore {
    inner: Arc<Mutex<HashMap<u64, UserSettings>>>,
    storage_path: PathBuf,
}

impl UserSettingsStore {
    pub fn load_or_create<P: AsRef<Path>>(path: P) -> Self {
        let storage_path = path.as_ref().to_path_buf();
        let map = if storage_path.exists() {
            match std::fs::read_to_string(&storage_path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                    log::warn!("Failed to parse settings file {:?}: {}, starting fresh", storage_path, e);
                    HashMap::new()
                }),
                Err(e) => {
                    log::warn!("Failed to read settings file {:?}: {}, starting fresh", storage_path, e);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        Self {
            inner: Arc::new(Mutex::new(map)),
            storage_path,
        }
    }

    pub fn get(&self, user_id: UserId) -> UserSettings {
        let guard = self.inner.lock().unwrap();
        guard.get(&user_id.0).copied().unwrap_or_default()
    }

    pub fn set_quality(&self, user_id: UserId, quality: QualityPreference) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.quality = quality;
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    pub fn toggle_audio_only(&self, user_id: UserId) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.audio_only = !entry.audio_only;
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    fn save(&self) {
        let guard = self.inner.lock().unwrap();
        match serde_json::to_string_pretty(&*guard) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.storage_path, json) {
                    log::error!("Failed to persist settings to {:?}: {}", self.storage_path, e);
                }
            }
            Err(e) => log::error!("Failed to serialize settings: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_user_settings_defaults_and_toggle() {
        let tmp = NamedTempFile::new().unwrap();
        let store = UserSettingsStore::load_or_create(tmp.path());
        let uid = UserId(12345);

        assert_eq!(store.get(uid), UserSettings {
            quality: QualityPreference::AutoBest,
            audio_only: false,
        });

        let updated = store.toggle_audio_only(uid);
        assert!(updated.audio_only);
        assert_eq!(store.get(uid).audio_only, true);

        let updated_q = store.set_quality(uid, QualityPreference::AskEveryTime);
        assert_eq!(updated_q.quality, QualityPreference::AskEveryTime);

        // Reload store from same file
        let reloaded = UserSettingsStore::load_or_create(tmp.path());
        assert_eq!(reloaded.get(uid).quality, QualityPreference::AskEveryTime);
        assert_eq!(reloaded.get(uid).audio_only, true);
    }
}
