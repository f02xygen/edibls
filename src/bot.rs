use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId, ParseMode, ReplyParameters,
};
use tempfile::TempDir;

use crate::cache::{CachedMedia, MediaCache};
use crate::config::Config;
use crate::media::{MediaOutput, extract_url, fetch_media, is_audio_request};
use crate::queue::QueueManager;

const WELCOME_MESSAGE: &str = "\
<b>EDIBLS</b> — бот для скачивания видео и музыки.

<b>Площадки:</b> YouTube, TikTok, Instagram, Яндекс Музыка, SoundCloud, VK и др.

<b>Как пользоваться:</b>
• <b>Видео:</b> просто отправь ссылку (под видео появится кнопка для извлечения MP3).
• <b>Аудио:</b> используй <code>/audio &lt;ссылка&gt;</code> или отправь ссылку на Яндекс Музыку.

<i>Лимиты: до 50 МБ, длительность до 20 минут.</i>";

fn build_audio_button(url_id: u64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "Скачать MP3",
        format!("audio:{url_id}"),
    )]])
}

pub async fn process_and_send_media(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    user_id: UserId,
    url: &str,
    audio_only: bool,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
) -> ResponseResult<()> {
    // 1. Fast path: check Telegram file_id cache (0.1s instant response, bypasses queue)
    if let Some(cached) = cache.get(url, audio_only) {
        match cached {
            CachedMedia::Video(file_id) => {
                let url_id = cache.register_url(url);
                let mut req = bot
                    .send_video(chat_id, InputFile::file_id(file_id))
                    .supports_streaming(true)
                    .reply_markup(build_audio_button(url_id));
                if let Some(msg_id) = reply_to {
                    req = req.reply_parameters(ReplyParameters::new(msg_id));
                }
                req.await?;
                return Ok(());
            }
            CachedMedia::Audio(file_ids) => {
                for file_id in file_ids {
                    let mut req = bot.send_audio(chat_id, InputFile::file_id(file_id));
                    if let Some(msg_id) = reply_to {
                        req = req.reply_parameters(ReplyParameters::new(msg_id));
                    }
                    req.await?;
                }
                return Ok(());
            }
        }
    }

    // 2. Slow path: acquire user lock and queue slot
    let _user_guard = match queue.try_lock_user(user_id) {
        Some(guard) => guard,
        None => {
            let mut req = bot.send_message(
                chat_id,
                "У тебя уже скачивается один файл. Дождись его завершения.",
            );
            if let Some(msg_id) = reply_to {
                req = req.reply_parameters(ReplyParameters::new(msg_id));
            }
            req.await?;
            return Ok(());
        }
    };

    let (service, pos, waiter_guard) = queue.register(url);

    let status_text = if pos > 1 {
        format!(
            "Сервис по {} сейчас занят. Твоя позиция в очереди: {}...",
            service.name(),
            pos
        )
    } else {
        "Ожидание слота загрузки...".to_string()
    };

    let mut status_req = bot.send_message(chat_id, status_text);
    if let Some(msg_id) = reply_to {
        status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
    }
    let status_msg = status_req.await?;

    // Lock domain & wait for global download slot
    let service_guard = queue.acquire(service, waiter_guard).await;

    let _ = bot
        .edit_message_text(chat_id, status_msg.id, "Начинаю скачивание...")
        .await;

    let temp_dir = match TempDir::new() {
        Ok(dir) => dir,
        Err(e) => {
            bot.edit_message_text(
                chat_id,
                status_msg.id,
                format!("Не удалось создать временную директорию: {e}"),
            )
            .await?;
            return Ok(());
        }
    };

    let media_result = fetch_media(url, audio_only, temp_dir.path(), config).await;

    // Release domain & global download slot immediately so next user can start downloading
    drop(service_guard);

    match media_result {
        Ok(MediaOutput::Audio(tracks)) => {
            bot.edit_message_text(
                chat_id,
                status_msg.id,
                format!("Отправляю {} аудио...", tracks.len()),
            )
            .await?;

            let mut uploaded_ids = Vec::with_capacity(tracks.len());
            let mut total_size: u64 = 0;

            for track in tracks {
                let input_file = InputFile::file(&track.path);
                let mut request = bot.send_audio(chat_id, input_file);

                if let Some(msg_id) = reply_to {
                    request = request.reply_parameters(ReplyParameters::new(msg_id));
                }
                if let Some(p) = &track.performer {
                    request = request.performer(p);
                }
                if let Some(t) = &track.title {
                    request = request.title(t);
                }
                if let Some(cover_path) = &track.cover_path {
                    request = request.thumbnail(InputFile::file(cover_path));
                }

                match request.await {
                    Ok(sent_msg) => {
                        if let Some(audio) = sent_msg.audio() {
                            total_size = total_size.saturating_add(u64::from(audio.file.size));
                            uploaded_ids.push(audio.file.id.clone());
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to send audio {:?}: {:?}", track.path, e);
                    }
                }
            }

            if !uploaded_ids.is_empty() {
                cache.insert(url, true, CachedMedia::Audio(uploaded_ids), total_size);
            }

            let _ = bot.delete_message(chat_id, status_msg.id).await;
        }
        Ok(MediaOutput::Video(file_path)) => {
            bot.edit_message_text(chat_id, status_msg.id, "Отправляю видео...")
                .await?;

            let url_id = cache.register_url(url);
            let input_file = InputFile::file(&file_path);
            let mut request = bot
                .send_video(chat_id, input_file)
                .supports_streaming(true)
                .reply_markup(build_audio_button(url_id));

            if let Some(msg_id) = reply_to {
                request = request.reply_parameters(ReplyParameters::new(msg_id));
            }

            match request.await {
                Ok(sent_msg) => {
                    if let Some(video) = sent_msg.video() {
                        cache.insert(
                            url,
                            false,
                            CachedMedia::Video(video.file.id.clone()),
                            u64::from(video.file.size),
                        );
                    }
                }
                Err(e) => {
                    log::error!("Failed to send video {:?}: {:?}", file_path, e);
                }
            }

            let _ = bot.delete_message(chat_id, status_msg.id).await;
        }
        Err(err) => {
            bot.edit_message_text(chat_id, status_msg.id, format!("Ошибка: {err}"))
                .await?;
        }
    }

    Ok(())
}

pub async fn handle_message(
    bot: Bot,
    msg: Message,
    config: Arc<Config>,
    queue: QueueManager,
    cache: MediaCache,
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
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
            return Ok(());
        }

        if let Some(url) = extract_url(trimmed) {
            let audio_only = is_audio_request(url, trimmed);
            process_and_send_media(
                &bot,
                msg.chat.id,
                Some(msg.id),
                user_id,
                url,
                audio_only,
                &config,
                &queue,
                &cache,
            )
            .await?;
        } else if msg.chat.is_private() {
            // Only send fallback hint in private chats so the bot stays quiet in group chats
            bot.send_message(
                msg.chat.id,
                "Отправь ссылку на видео/аудио, используй <code>/audio &lt;ссылка&gt;</code> или нажми /start для справки.",
            )
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
        }
    }
    Ok(())
}

pub async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    config: Arc<Config>,
    queue: QueueManager,
    cache: MediaCache,
) -> ResponseResult<()> {
    let Some(data) = q.data.as_deref() else {
        return Ok(());
    };

    if let Some(id_str) = data.strip_prefix("audio:") {
        let Ok(url_id) = id_str.parse::<u64>() else {
            return Ok(());
        };

        let Some(url) = cache.resolve_url(url_id) else {
            bot.answer_callback_query(q.id)
                .text("Ссылка устарела, отправь её заново с /audio")
                .await?;
            return Ok(());
        };

        let _ = bot
            .answer_callback_query(q.id.clone())
            .text("Извлекаю аудио (MP3)...")
            .await;

        if let Some(msg) = q.regular_message() {
            let chat_id = msg.chat.id;
            let msg_id = msg.id;

            // Remove the inline button so it isn't clicked repeatedly
            let _ = bot.edit_message_reply_markup(chat_id, msg_id).await;

            process_and_send_media(
                &bot,
                chat_id,
                Some(msg_id),
                q.from.id,
                &url,
                true,
                &config,
                &queue,
                &cache,
            )
            .await?;
        }
    }

    Ok(())
}
