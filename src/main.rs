use std::sync::Arc;
use teloxide::prelude::*;

mod bot;
mod cache;
mod config;
mod media;
mod providers;
mod queue;
mod settings;

use bot::{handle_callback, handle_message};
use cache::MediaCache;
use config::Config;
use queue::QueueManager;
use settings::UserSettingsStore;

#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let mut raw_config = Config::from_env();

    if raw_config.teloxide_token.is_empty() {
        log::error!("TELOXIDE_TOKEN is not set in environment or .env file!");
        std::process::exit(1);
    }

    log::info!("Starting EDIBLS daemon...");
    log::info!(
        "Max concurrent downloads: {}",
        raw_config.max_concurrent_downloads
    );
    if let Some(cookies) = &raw_config.cookies_path {
        log::info!("Cookies enabled: {:?}", cookies);
    }
    if let Some(pot) = &raw_config.youtube_pot_url {
        log::info!("YouTube PO token provider: {}", pot);
    }
    log::info!(
        "Yandex Music downloader binary: {:?}",
        raw_config.yandex_downloader_bin
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .unwrap_or_default();

    let mut bot = Bot::with_client(&raw_config.teloxide_token, client.clone());
    let mut _local_api_child: Option<tokio::process::Child> = None;
    let mut connected_api_url: Option<reqwest::Url> = None;

    if let Some(api_url) = raw_config.telegram_api_url.clone() {
        if let Ok(url) = reqwest::Url::parse(&api_url) {
            for attempt in 1..=5 {
                if client
                    .get(url.clone())
                    .timeout(std::time::Duration::from_secs(2))
                    .send()
                    .await
                    .is_ok()
                {
                    connected_api_url = Some(url.clone());
                    break;
                }
                log::warn!(
                    "Waiting for custom Telegram Bot API at {} (attempt {}/5)...",
                    url,
                    attempt
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        } else {
            log::error!("Failed to parse TELEGRAM_API_URL: {}", api_url);
        }
    }

    if connected_api_url.is_none() {
        let local_url = reqwest::Url::parse("http://127.0.0.1:8081").unwrap();
        if client
            .get(local_url.clone())
            .timeout(std::time::Duration::from_secs(1))
            .send()
            .await
            .is_ok()
        {
            connected_api_url = Some(local_url);
        } else {
            let base_dir = if std::fs::create_dir_all("/app/.telegram-bot-api").is_ok() {
                std::path::PathBuf::from("/app/.telegram-bot-api")
            } else {
                let p = std::env::temp_dir().join("edibls-telegram-bot-api");
                let _ = std::fs::create_dir_all(&p);
                p
            };
            let temp_dir = base_dir.join("tmp");
            let _ = std::fs::create_dir_all(&temp_dir);

            let api_id = std::env::var("TELEGRAM_API_ID")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "2040".to_string());
            let api_hash = std::env::var("TELEGRAM_API_HASH")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "b18441a1ff607e10a989891a5462e627".to_string());

            let token_dir = base_dir.join(&raw_config.teloxide_token);
            if !token_dir.exists() {
                let logout_url = format!(
                    "https://api.telegram.org/bot{}/logOut",
                    raw_config.teloxide_token
                );
                let _ = client
                    .post(&logout_url)
                    .timeout(std::time::Duration::from_secs(5))
                    .send()
                    .await;
            }

            if let Ok(child) = tokio::process::Command::new("telegram-bot-api")
                .args([
                    "--local",
                    &format!("--api-id={api_id}"),
                    &format!("--api-hash={api_hash}"),
                    "--http-port=8081",
                    &format!("--dir={}", base_dir.display()),
                    &format!("--temp-dir={}", temp_dir.display()),
                ])
                .kill_on_drop(true)
                .spawn()
            {
                log::info!("Spawned embedded local telegram-bot-api process on 127.0.0.1:8081");
                _local_api_child = Some(child);
                for _ in 1..=10 {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    if client
                        .get(local_url.clone())
                        .timeout(std::time::Duration::from_secs(1))
                        .send()
                        .await
                        .is_ok()
                    {
                        connected_api_url = Some(local_url);
                        break;
                    }
                }
            }
        }
    }

    if let Some(url) = connected_api_url {
        log::info!("Connected to local Telegram API URL: {}", url);
        bot = bot.set_api_url(url.clone());
        raw_config.telegram_api_url = Some(url.to_string());
        if std::env::var("MAX_FILESIZE_MB").is_err() {
            raw_config.max_filesize_mb = 2000;
        }
        if std::env::var("MAX_DURATION_SECS").is_err() {
            raw_config.max_duration_secs = 7200;
        }
    } else {
        log::warn!(
            "Local Telegram Bot API is unavailable; using official api.telegram.org (49MB limit)"
        );
        raw_config.telegram_api_url = None;
        if std::env::var("MAX_FILESIZE_MB").is_err() {
            raw_config.max_filesize_mb = 49;
        }
    }

    log::info!(
        "Max filesize: {}MB, Max duration: {}s",
        raw_config.max_filesize_mb,
        raw_config.max_duration_secs
    );

    let config = Arc::new(raw_config);

    let queue = QueueManager::new(config.max_concurrent_downloads);
    let cache = MediaCache::new();
    let settings = UserSettingsStore::load_or_create(&config.settings_path);

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(
            move |bot: Bot, msg: Message, config: Arc<Config>, queue: QueueManager, cache: MediaCache, settings: UserSettingsStore| async move {
                tokio::spawn(async move {
                    if let Err(err) = handle_message(bot, msg, config, queue, cache, settings).await {
                        log::error!("Error handling message: {:?}", err);
                    }
                });
                respond(())
            },
        ))
        .branch(Update::filter_callback_query().endpoint(
            move |bot: Bot, q: CallbackQuery, config: Arc<Config>, queue: QueueManager, cache: MediaCache, settings: UserSettingsStore| async move {
                tokio::spawn(async move {
                    if let Err(err) = handle_callback(bot, q, config, queue, cache, settings).await {
                        log::error!("Error handling callback query: {:?}", err);
                    }
                });
                respond(())
            },
        ));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![config, queue, cache, settings])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
