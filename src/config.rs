use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Config {
    pub teloxide_token: String,
    pub yandex_music_token: Option<String>,
    pub max_concurrent_downloads: usize,
    pub cookies_path: Option<PathBuf>,
    pub youtube_pot_url: Option<String>,
    pub yandex_downloader_bin: PathBuf,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        let teloxide_token = env::var("TELOXIDE_TOKEN").unwrap_or_default();
        let yandex_music_token = env::var("YANDEX_MUSIC_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty());

        let max_concurrent_downloads = env::var("MAX_CONCURRENT_DOWNLOADS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);

        let raw_cookies_path = env::var("COOKIES_PATH").unwrap_or_else(|_| "cookies.txt".to_string());
        let cookies_path = if Path::new(&raw_cookies_path).exists() {
            Some(PathBuf::from(raw_cookies_path))
        } else {
            None
        };

        let youtube_pot_url = env::var("YOUTUBE_POT_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());

        let yandex_downloader_bin = if let Ok(bin) = env::var("YANDEX_DOWNLOADER_BIN") {
            PathBuf::from(bin)
        } else if Path::new("./venv/bin/yandex-music-downloader").exists() {
            PathBuf::from("./venv/bin/yandex-music-downloader")
        } else {
            PathBuf::from("yandex-music-downloader")
        };

        Self {
            teloxide_token,
            yandex_music_token,
            max_concurrent_downloads,
            cookies_path,
            youtube_pot_url,
            yandex_downloader_bin,
        }
    }
}
