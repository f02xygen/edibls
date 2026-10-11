use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InputFile, ParseMode};
use tempfile::TempDir;

use crate::config::Config;
use crate::media::{MediaOutput, extract_url, fetch_media};
use crate::queue::QueueManager;

const WELCOME_MESSAGE: &str = "\
👋 <b>Привет! Я EDIBLS — бот для быстрого скачивания видео и музыки.</b>

✨ <b>Поддерживаемые площадки:</b>
• 🎬 <b>YouTube и Shorts</b> — видео в MP4 или аудиодорожка
• 📱 <b>TikTok</b> — видео без водяных знаков или звук
• 📸 <b>Instagram</b> — Reels и видео-посты
• 🎵 <b>Яндекс Музыка</b> — треки, альбомы и плейлисты с обложками и ID3-тегами
• 🌐 <b>1000+ других сайтов</b> — SoundCloud, VK, Twitter/X, Reddit, Twitch Clips и др.

🚀 <b>Как пользоваться:</b>
1️⃣ <b>Скачать видео:</b>
Просто отправь ссылку в чат:
<code>https://youtu.be/...</code> или <code>https://vt.tiktok.com/...</code>

2️⃣ <b>Скачать только аудио (MP3 с метаданными):</b>
Используй команду <code>/audio</code>, <code>/mp3</code> или припиши <code>audio</code> к ссылке:
<code>/audio https://youtu.be/...</code>
<code>/mp3 https://vt.tiktok.com/...</code>

3️⃣ <b>Яндекс Музыка:</b>
Просто отправь ссылку на трек или альбом — бот сам скачает аудио с обложкой, названием и автором.

ℹ️ <b>Ограничения и очередь:</b>
• Максимальный размер файла: <b>50 МБ</b> (лимит Telegram Bot API)
• Максимальная длительность ролика: <b>20 минут</b>
• Одновременно можно скачивать <b>1 запрос на пользователя</b> (с разных сервисов запросы обрабатываются параллельно)";

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
        let trimmed = text.trim();

        if trimmed == "/start"
            || trimmed.starts_with("/start@")
            || trimmed == "/help"
            || trimmed.starts_with("/help@")
        {
            bot.send_message(msg.chat.id, WELCOME_MESSAGE)
                .parse_mode(ParseMode::Html)
                .await?;
            return Ok(());
        }

        if let Some(url) = extract_url(trimmed) {
            let _user_guard = match queue.try_lock_user(user_id) {
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

            // Lock domain & wait for global download slot
            let service_guard = queue.acquire(service, waiter_guard).await;

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

            let media_result = fetch_media(url, trimmed, temp_dir.path(), &config).await;

            // Release the domain & global download slot immediately after download completes
            // so the next user in queue can start downloading while we upload to Telegram!
            drop(service_guard);

            match media_result {
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
                    if let Err(e) = bot
                        .send_video(msg.chat.id, input_file)
                        .supports_streaming(true)
                        .await
                    {
                        log::error!("Failed to send video {:?}: {:?}", file_path, e);
                    }

                    let _ = bot.delete_message(msg.chat.id, status_msg.id).await;
                }
                Err(err) => {
                    bot.edit_message_text(msg.chat.id, status_msg.id, format!("❌ Ошибка: {err}"))
                        .await?;
                }
            }
            // temp_dir and _user_guard drop here
        } else {
            bot.send_message(
                msg.chat.id,
                "Отправь ссылку на видео/аудио, используй <code>/audio &lt;ссылка&gt;</code> или нажми /start для справки.",
            )
            .parse_mode(ParseMode::Html)
            .await?;
        }
    }
    Ok(())
}
