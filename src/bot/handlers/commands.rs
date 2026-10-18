use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ParseMode, ReplyParameters, UserId};

use crate::bot::views::{
    build_chapters_keyboard, build_settings_keyboard, build_settings_text, WELCOME_MESSAGE,
};
use crate::cache::MediaCache;
use crate::config::Config;
use crate::parsing::{extract_speed_modifier, extract_url, parse_media_modifiers};
use crate::pipeline::process_and_send_media;
use crate::providers::youtube::fetch_youtube_chapters;
use crate::queue::QueueManager;
use crate::settings::UserSettingsStore;

pub enum CommandAction {
    Handled,
    NotACommand,
}

pub async fn handle_command(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    user_id: UserId,
    trimmed: &str,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
    settings: &UserSettingsStore,
) -> ResponseResult<CommandAction> {
    if trimmed == "/start"
        || trimmed.starts_with("/start@")
        || trimmed == "/help"
        || trimmed.starts_with("/help@")
    {
        bot.send_message(chat_id, WELCOME_MESSAGE)
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg_id))
            .await?;
        return Ok(CommandAction::Handled);
    }

    if trimmed == "/settings" || trimmed.starts_with("/settings@") {
        let user_settings = settings.get(user_id);
        bot.send_message(chat_id, build_settings_text(&user_settings))
            .parse_mode(ParseMode::Html)
            .reply_markup(build_settings_keyboard(&user_settings))
            .reply_parameters(ReplyParameters::new(msg_id))
            .await?;
        return Ok(CommandAction::Handled);
    }

    if trimmed == "/clip" || trimmed.starts_with("/clip ") || trimmed.starts_with("/clip@") {
        if extract_url(trimmed).is_none() {
            bot.send_message(
                chat_id,
                "Использование: <code>/clip &lt;url&gt; 01:15-02:30</code> или ответь (Reply) на видео/аудио таймкодом: <code>00:10-00:30</code>",
            )
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg_id))
            .await?;
            return Ok(CommandAction::Handled);
        }
    }

    if trimmed == "/speed" || trimmed.starts_with("/speed ") || trimmed.starts_with("/speed@") {
        if extract_url(trimmed).is_none() {
            bot.send_message(
                chat_id,
                "Использование: <code>/speed 1.5x &lt;url&gt;</code> или ответь (Reply) на видео/аудио сообщением <code>1.5x</code> или <code>2x</code>.",
            )
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg_id))
            .await?;
            return Ok(CommandAction::Handled);
        }
    }

    if trimmed == "/subway" || trimmed.starts_with("/subway ") || trimmed.starts_with("/subway@") {
        if let Some(raw_url) = extract_url(trimmed) {
            let (url, tr, spd, _) = parse_media_modifiers(raw_url);
            let user_settings = settings.get(user_id);
            let speed = spd
                .or_else(|| extract_speed_modifier(trimmed))
                .or_else(|| user_settings.default_speed.to_f32());
            let opts = crate::pipeline::MediaPipelineOptions {
                url,
                audio_only: false,
                max_height: None,
                time_range: tr,
                speed,
                ym_quality: user_settings.ym_quality,
                send_as_document: user_settings.send_as_document,
                with_subway: true,
                source_media: None,
                has_existing_subway: false,
            };
            process_and_send_media(
                bot,
                chat_id,
                Some(msg_id),
                user_id,
                opts,
                config,
                queue,
                cache,
            )
            .await?;
            return Ok(CommandAction::Handled);
        } else {
            bot.send_message(
                chat_id,
                "Использование: <code>/subway &lt;url&gt;</code> или добавь <code>#subway</code> к ссылке.",
            )
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg_id))
            .await?;
            return Ok(CommandAction::Handled);
        }
    }

    if trimmed == "/chapters"
        || trimmed.starts_with("/chapters ")
        || trimmed.starts_with("/chapters@")
    {
        if let Some(url) = extract_url(trimmed) {
            let url_id = cache.register_url(url);
            let chapters = match cache.get_yt_chapters(url_id) {
                Some(c) => c,
                None => {
                    let wait_msg = bot
                        .send_message(chat_id, "⏳ Загружаю список глав видео...")
                        .reply_parameters(ReplyParameters::new(msg_id))
                        .await?;
                    let fetched = fetch_youtube_chapters(url).await.unwrap_or_default();
                    cache.store_yt_chapters(url_id, fetched.clone());
                    let _ = bot.delete_message(chat_id, wait_msg.id).await;
                    fetched
                }
            };
            if chapters.is_empty() {
                bot.send_message(chat_id, "У этого видео нет глав (таймкодов).")
                    .reply_parameters(ReplyParameters::new(msg_id))
                    .await?;
            } else {
                let kb = build_chapters_keyboard(url_id, &chapters, 0);
                let total = chapters.len();
                bot.send_message(
                    chat_id,
                    format!("📑 <b>Главы видео</b> ({total} шт.):\n\nВыбери главу, чтобы скачать нужный фрагмент:"),
                )
                .parse_mode(ParseMode::Html)
                .reply_markup(kb)
                .reply_parameters(ReplyParameters::new(msg_id))
                .await?;
            }
            return Ok(CommandAction::Handled);
        } else {
            bot.send_message(
                chat_id,
                "Использование: <code>/chapters &lt;url&gt;</code> — просмотр глав YouTube видео.",
            )
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg_id))
            .await?;
            return Ok(CommandAction::Handled);
        }
    }

    Ok(CommandAction::NotACommand)
}
