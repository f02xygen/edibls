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

    let config = Arc::new(Config::from_env());

    if config.teloxide_token.is_empty() {
        log::error!("TELOXIDE_TOKEN is not set in environment or .env file!");
        std::process::exit(1);
    }

    log::info!("Starting EDIBLS daemon...");
    log::info!(
        "Max concurrent downloads: {}",
        config.max_concurrent_downloads
    );
    if let Some(cookies) = &config.cookies_path {
        log::info!("Cookies enabled: {:?}", cookies);
    }
    if let Some(pot) = &config.youtube_pot_url {
        log::info!("YouTube PO token provider: {}", pot);
    }
    log::info!(
        "Yandex Music downloader binary: {:?}",
        config.yandex_downloader_bin
    );
    if let Some(api_url) = &config.telegram_api_url {
        log::info!("Custom Telegram API URL: {}", api_url);
    }
    log::info!(
        "Max filesize: {}MB, Max duration: {}s",
        config.max_filesize_mb,
        config.max_duration_secs
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .build()
        .unwrap_or_default();

    let mut bot = Bot::with_client(&config.teloxide_token, client);
    if let Some(api_url) = &config.telegram_api_url {
        if let Ok(url) = reqwest::Url::parse(api_url) {
            bot = bot.set_api_url(url);
        } else {
            log::error!("Failed to parse TELEGRAM_API_URL: {}", api_url);
        }
    }

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
