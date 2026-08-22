use std::env;
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tempfile::TempDir;
use tokio::fs;

mod downloader;
mod media;
mod queue;
mod yandex;

use media::{MediaOutput, extract_url, fetch_media};
use queue::QueueManager;

async fn handle_update(
    bot: Bot,
    msg: Message,
    yandex_token: Option<String>,
    queue: QueueManager,
) -> ResponseResult<()> {
    let user_id = match msg.from() {
        Some(user) => user.id,
        None => return Ok(()),
    };

    if let Some(text) = msg.text() {
        if let Some(url) = extract_url(text) {
            if !queue.try_lock_user(user_id).await {
                bot.send_message(
                    msg.chat.id,
                    "⚠️ У тебя уже скачивается один файл! Дождись его завершения.",
                )
                .await?;
                return Ok(());
            }

            let (service, pos) = queue.register(url);

            let status_msg = if pos > 1 {
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "⏳ Сервис по {} сейчас занят. Твоя позиция в очереди: {}...",
                        service.name(),
                        pos
                    ),
                )
                .await?
            } else {
                bot.send_message(msg.chat.id, "⏳ Ожидание слота загрузки...")
                    .await?
            };

            // Lock domain & wait for global slot
            let _guard = queue.acquire(service, user_id).await;

            let _ = bot
                .edit_message_text(msg.chat.id, status_msg.id, "⏳ Начинаю скачивание...")
                .await;

            let temp_dir = match TempDir::new() {
                Ok(dir) => dir,
                Err(e) => {
                    bot.send_message(
                        msg.chat.id,
                        format!("❌ Failed to create temp directory: {e}"),
                    )
                    .await?;
                    return Ok(());
                }
            };

            match fetch_media(url, text, yandex_token.as_deref(), temp_dir.path()).await {
                Ok(MediaOutput::Audio(tracks)) => {
                    bot.edit_message_text(
                        msg.chat.id,
                        status_msg.id,
                        format!("📤 Отправляю {} аудио...", tracks.len()),
                    )
                    .await?;

                    for track in tracks {
                        let input_file = InputFile::file(&track.path);
                        let mut request = bot.send_audio(msg.chat.id, input_file);

                        if let Some(p) = &track.performer {
                            request = request.performer(p);
                        }
                        if let Some(t) = &track.title {
                            request = request.title(t);
                        }
                        if let Some(cover_path) = &track.cover_path {
                            request = request.thumbnail(InputFile::file(cover_path));
                        }

                        if let Err(e) = request.await {
                            log::error!("Failed to send audio {:?}: {:?}", track.path, e);
                        }

                        if let Some(cover_path) = &track.cover_path {
                            let _ = fs::remove_file(cover_path).await;
                        }
                    }
                    let _ = bot.delete_message(msg.chat.id, status_msg.id).await;
                }
                Ok(MediaOutput::Video(file_path)) => {
                    bot.edit_message_text(msg.chat.id, status_msg.id, "📤 Отправляю видео...")
                        .await?;

                    let input_file = InputFile::file(&file_path);
                    if let Err(e) = bot.send_video(msg.chat.id, input_file).await {
                        log::error!("Failed to send video {:?}: {:?}", file_path, e);
                    }

                    let _ = bot.delete_message(msg.chat.id, status_msg.id).await;
                    let _ = fs::remove_file(&file_path).await;
                }
                Err(err) => {
                    bot.edit_message_text(msg.chat.id, status_msg.id, format!("❌ Ошибка: {err}"))
                        .await?;
                }
            }
        } else {
            bot.send_message(
                msg.chat.id,
                "Отправь ссылку на видео/аудио или используй `/audio <ссылка>`.",
            )
            .await?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    pretty_env_logger::init();

    log::info!("Starting EDIBLS daemon...");

    let bot = Bot::from_env();
    let yandex_token = env::var("YANDEX_MUSIC_TOKEN").ok();

    // Max 3 global parallel tasks, but strictly 1 task per service (TikTok, Yandex, etc.)
    let queue = QueueManager::new(3);

    teloxide::repl(bot, move |bot: Bot, msg: Message| {
        let yandex_token = yandex_token.clone();
        let queue = queue.clone();

        async move {
            tokio::spawn(async move {
                if let Err(err) = handle_update(bot, msg, yandex_token, queue).await {
                    log::error!("Error handling update: {:?}", err);
                }
            });

            Ok(())
        }
    })
    .await;
}
