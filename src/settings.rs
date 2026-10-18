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
pub enum YandexQualityPreference {
    #[default]
    Optimal, // AAC 192kbps (fast download & upload, ~10 MB)
    Lossless, // FLAC (lossless, 50-80 MB)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DefaultSearchProvider {
    #[default]
    Yandex,
    SoundCloud,
    YouTube,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpeedPreference {
    #[default]
    Normal,     // 1.0x
    Slower075,  // 0.75x
    Faster125,  // 1.25x
    Faster15,   // 1.5x
    Faster20,   // 2.0x
}

impl SpeedPreference {
    pub fn multiplier(&self) -> f32 {
        match self {
            SpeedPreference::Normal => 1.0,
            SpeedPreference::Slower075 => 0.75,
            SpeedPreference::Faster125 => 1.25,
            SpeedPreference::Faster15 => 1.5,
            SpeedPreference::Faster20 => 2.0,
        }
    }

    pub fn to_f32(&self) -> Option<f32> {
        match self {
            SpeedPreference::Normal => None,
            _ => Some(self.multiplier()),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SpeedPreference::Normal => "1.0x (Обычная)",
            SpeedPreference::Slower075 => "0.75x",
            SpeedPreference::Faster125 => "1.25x",
            SpeedPreference::Faster15 => "1.5x",
            SpeedPreference::Faster20 => "2.0x",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UserSettings {
    #[serde(default)]
    pub quality: QualityPreference,
    #[serde(default)]
    pub audio_only: bool, // If true, all media requests are extracted as audio
    #[serde(default)]
    pub ym_quality: YandexQualityPreference,
    #[serde(default)]
    pub send_as_document: bool, // If true, media is sent as document without Telegram compression
    #[serde(default)]
    pub default_search: DefaultSearchProvider,
    #[serde(default)]
    pub default_speed: SpeedPreference,
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

    #[allow(dead_code)]
    pub fn set_quality(&self, user_id: UserId, quality: QualityPreference) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.quality = quality;
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    pub fn toggle_quality(&self, user_id: UserId) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.quality = match entry.quality {
            QualityPreference::AutoBest => QualityPreference::AskEveryTime,
            QualityPreference::AskEveryTime => QualityPreference::AutoBest,
        };
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

    #[allow(dead_code)]
    pub fn set_ym_quality(&self, user_id: UserId, ym_quality: YandexQualityPreference) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.ym_quality = ym_quality;
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    pub fn toggle_ym_quality(&self, user_id: UserId) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.ym_quality = match entry.ym_quality {
            YandexQualityPreference::Optimal => YandexQualityPreference::Lossless,
            YandexQualityPreference::Lossless => YandexQualityPreference::Optimal,
        };
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    pub fn toggle_send_as_document(&self, user_id: UserId) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.send_as_document = !entry.send_as_document;
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    pub fn toggle_default_search(&self, user_id: UserId) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.default_search = match entry.default_search {
            DefaultSearchProvider::Yandex => DefaultSearchProvider::SoundCloud,
            DefaultSearchProvider::SoundCloud => DefaultSearchProvider::YouTube,
            DefaultSearchProvider::YouTube => DefaultSearchProvider::Yandex,
        };
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    pub fn toggle_speed(&self, user_id: UserId) -> UserSettings {
        let mut guard = self.inner.lock().unwrap();
        let entry = guard.entry(user_id.0).or_default();
        entry.default_speed = match entry.default_speed {
            SpeedPreference::Normal => SpeedPreference::Faster125,
            SpeedPreference::Faster125 => SpeedPreference::Faster15,
            SpeedPreference::Faster15 => SpeedPreference::Faster20,
            SpeedPreference::Faster20 => SpeedPreference::Slower075,
            SpeedPreference::Slower075 => SpeedPreference::Normal,
        };
        let updated = *entry;
        drop(guard);
        self.save();
        updated
    }

    fn save(&self) {
        let guard = self.inner.lock().unwrap();
        match serde_json::to_string_pretty(&*guard) {
            Ok(json) => {
                if let Some(parent) = self.storage_path.parent() {
                    if !parent.as_os_str().is_empty() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                }
                let tmp_path = self.storage_path.with_extension("tmp");
                if let Err(e) = std::fs::write(&tmp_path, json) {
                    log::error!("Failed to write temporary settings to {:?}: {}", tmp_path, e);
                } else if let Err(e) = std::fs::rename(&tmp_path, &self.storage_path) {
                    log::error!("Failed to atomic rename settings to {:?}: {}", self.storage_path, e);
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
            ym_quality: YandexQualityPreference::Optimal,
            send_as_document: false,
            default_search: DefaultSearchProvider::Yandex,
            default_speed: SpeedPreference::Normal,
        });

        let updated = store.toggle_audio_only(uid);
        assert!(updated.audio_only);
        assert_eq!(store.get(uid).audio_only, true);

        let updated_q = store.toggle_quality(uid);
        assert_eq!(updated_q.quality, QualityPreference::AskEveryTime);
        assert_eq!(store.get(uid).quality, QualityPreference::AskEveryTime);

        let updated_ym = store.toggle_ym_quality(uid);
        assert_eq!(updated_ym.ym_quality, YandexQualityPreference::Lossless);

        let updated_doc = store.toggle_send_as_document(uid);
        assert!(updated_doc.send_as_document);
        assert_eq!(store.get(uid).send_as_document, true);

        let updated_search = store.toggle_default_search(uid);
        assert_eq!(updated_search.default_search, DefaultSearchProvider::SoundCloud);
        let updated_search_yt = store.toggle_default_search(uid);
        assert_eq!(updated_search_yt.default_search, DefaultSearchProvider::YouTube);

        let updated_spd = store.toggle_speed(uid);
        assert_eq!(updated_spd.default_speed, SpeedPreference::Faster125);

        // Reload store from same file
        let reloaded = UserSettingsStore::load_or_create(tmp.path());
        assert_eq!(reloaded.get(uid).quality, QualityPreference::AskEveryTime);
        assert_eq!(reloaded.get(uid).audio_only, true);
        assert_eq!(reloaded.get(uid).ym_quality, YandexQualityPreference::Lossless);
        assert_eq!(reloaded.get(uid).send_as_document, true);
        assert_eq!(reloaded.get(uid).default_search, DefaultSearchProvider::YouTube);
        assert_eq!(reloaded.get(uid).default_speed, SpeedPreference::Faster125);
    }
}
