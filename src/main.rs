use std::sync::Arc;
use teloxide::prelude::*;

mod bot;
mod cache;
mod config;
mod media;
mod providers;
mod queue;

use bot::{handle_callback, handle_message};
use cache::MediaCache;
use config::Config;
use queue::QueueManager;

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

    let bot = Bot::new(&config.teloxide_token);
    let queue = QueueManager::new(config.max_concurrent_downloads);
    let cache = MediaCache::new();

    let handler = dptree::entry()
        .branch(Update::filter_message().endpoint(
            move |bot: Bot, msg: Message, config: Arc<Config>, queue: QueueManager, cache: MediaCache| async move {
                tokio::spawn(async move {
                    if let Err(err) = handle_message(bot, msg, config, queue, cache).await {
                        log::error!("Error handling message: {:?}", err);
                    }
                });
                respond(())
            },
        ))
        .branch(Update::filter_callback_query().endpoint(
            move |bot: Bot, q: CallbackQuery, config: Arc<Config>, queue: QueueManager, cache: MediaCache| async move {
                tokio::spawn(async move {
                    if let Err(err) = handle_callback(bot, q, config, queue, cache).await {
                        log::error!("Error handling callback query: {:?}", err);
                    }
                });
                respond(())
            },
        ));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![config, queue, cache])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}
