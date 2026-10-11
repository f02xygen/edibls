use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::InputFile;
use tempfile::TempDir;

use crate::config::Config;
use crate::media::{MediaOutput, extract_url, fetch_media};
use crate::queue::QueueManager;

pub async fn handle_update(
    bot: Bot,
    msg: Message,
    config: Arc<Config>,
    queue: QueueManager,
) -> ResponseResult<()> {
    let user_id = match &msg.from {
        Some(user) => user.id,
        None => return Ok(()),
    };

    if let Some(text) = msg.text() {
        if let Some(url) = extract_url(text) {
            let user_guard = match queue.try_lock_user(user_id) {
                Some(guard) => guard,
                None => {
                    bot.send_message(
                        msg.chat.id,
                        "⚠️ У тебя уже скачивается один файл! Дождись его завершения.",
                    )
                    .await?;
                    return Ok(());
                }
            };

            let (service, pos, waiter_guard) = queue.register(url);

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

            // Lock domain & wait for global slot. Both guards are wrapped into RAII ServiceGuard.
            let _guard = queue.acquire(service, waiter_guard, user_guard).await;

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

            match fetch_media(url, text, temp_dir.path(), &config).await {
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
                }
                Err(err) => {
                    bot.edit_message_text(msg.chat.id, status_msg.id, format!("❌ Ошибка: {err}"))
                        .await?;
                }
            }
            // temp_dir is dropped here, automatically deleting all downloaded video/audio/covers
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
