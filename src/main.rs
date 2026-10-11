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
    if let Some(api_url) = raw_config.telegram_api_url.clone() {
        if let Ok(url) = reqwest::Url::parse(&api_url) {
            let mut reachable = false;
            for attempt in 1..=10 {
                if client
                    .get(url.clone())
                    .timeout(std::time::Duration::from_secs(2))
                    .send()
                    .await
                    .is_ok()
                {
                    reachable = true;
                    break;
                }
                log::warn!(
                    "Waiting for custom Telegram Bot API at {} (attempt {}/10)...",
                    url,
                    attempt
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }

            if reachable {
                log::info!("Connected to custom Telegram API URL: {}", url);
                bot = bot.set_api_url(url);
            } else {
                log::warn!(
                    "Custom Telegram API at {} is unreachable; falling back to official api.telegram.org (50MB limit)",
                    url
                );
                raw_config.telegram_api_url = None;
                if std::env::var("MAX_FILESIZE_MB").is_err() {
                    raw_config.max_filesize_mb = 50;
                }
            }
        } else {
            log::error!("Failed to parse TELEGRAM_API_URL: {}", api_url);
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
