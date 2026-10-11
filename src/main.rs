use std::sync::Arc;
use teloxide::prelude::*;

mod bot;
mod config;
mod media;
mod providers;
mod queue;

use bot::handle_update;
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

    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let config = Arc::clone(&config);
        let queue = queue.clone();

        async move {
            tokio::spawn(async move {
                if let Err(err) = handle_update(bot, msg, config, queue).await {
                    log::error!("Error handling update: {:?}", err);
                }
            });

            Ok(())
        }
    })
    .await;
}
