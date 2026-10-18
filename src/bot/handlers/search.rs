use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId, ParseMode, ReplyParameters, UserId};

use crate::bot::views::{
    build_empty_search_keyboard, build_ph_selection_keyboard, build_sc_selection_keyboard,
    build_track_selection_keyboard, build_yt_selection_keyboard, format_ph_search_text,
    format_sc_search_text, format_ym_search_text, format_yt_search_text, SEARCH_PAGE_SIZE,
};
use crate::cache::MediaCache;
use crate::config::Config;
use crate::pipeline::{process_and_send_media, MediaPipelineOptions};
use crate::providers::pornhub::search_pornhub_videos;
use crate::providers::soundcloud::search_soundcloud_tracks;
use crate::providers::yandex::search_yandex_tracks;
use crate::providers::youtube::search_youtube_videos;
use crate::queue::QueueManager;
use crate::settings::UserSettingsStore;

pub async fn handle_yandex_search(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    existing_msg_id: Option<MessageId>,
    user_id: UserId,
    query: &str,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
    settings: &UserSettingsStore,
) -> ResponseResult<()> {
    if config.yandex_music_token.is_none() {
        let msg = "Поиск недоступен: YANDEX_MUSIC_TOKEN не настроен в .env файле.";
        if let Some(mid) = existing_msg_id {
            let _ = bot.edit_message_text(chat_id, mid, msg).await;
        } else {
            let mut req = bot.send_message(chat_id, msg);
            if let Some(msg_id) = reply_to {
                req = req.reply_parameters(ReplyParameters::new(msg_id));
            }
            req.await?;
        }
        return Ok(());
    }

    let status_msg_id = if let Some(mid) = existing_msg_id {
        let _ = bot
            .edit_message_text(chat_id, mid, format!("🔍 Ищу на Яндекс Музыке: «{query}»..."))
            .reply_markup(InlineKeyboardMarkup::default())
            .await;
        mid
    } else {
        let mut status_req =
            bot.send_message(chat_id, format!("🔍 Ищу на Яндекс Музыке: «{query}»..."));
        if let Some(msg_id) = reply_to {
            status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
        }
        let status_msg = status_req.await?;
        status_msg.id
    };

    match search_yandex_tracks(query, config.yandex_music_token.as_deref()).await {
        Ok(tracks) if tracks.is_empty() => {
            let search_id = cache.store_search_query(query.to_string());
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg_id,
                    format!("По запросу «{query}» ничего не найдено на Яндекс Музыке."),
                )
                .reply_markup(build_empty_search_keyboard(search_id, "ym"))
                .await;
        }
        Ok(tracks) if tracks.len() == 1 => {
            let url = tracks[0].canonical_url();
            let _ = bot.delete_message(chat_id, status_msg_id).await;
            let user_settings = settings.get(user_id);
            let mut opts = MediaPipelineOptions::new(url);
            opts.audio_only = true;
            opts.speed = user_settings.default_speed.to_f32();
            opts.ym_quality = user_settings.ym_quality;
            opts.send_as_document = user_settings.send_as_document;
            process_and_send_media(
                bot,
                chat_id,
                reply_to,
                user_id,
                opts,
                config,
                queue,
                cache,
            )
            .await?;
        }
        Ok(tracks) => {
            let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
            let total_pages = total_pages.max(1);
            let search_id = cache.store_ym_search(query.to_string(), tracks.clone());
            let text = format_ym_search_text(query, 0, total_pages);
            let keyboard = build_track_selection_keyboard(search_id, &tracks, 0);

            let _ = bot
                .edit_message_text(chat_id, status_msg_id, text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await;
        }
        Err(e) => {
            log::error!("Yandex Music search error for query {:?}: {:?}", query, e);
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg_id,
                    format!("Ошибка при поиске на Яндекс Музыке: {e}"),
                )
                .await;
        }
    }

    Ok(())
}

pub async fn handle_soundcloud_search(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    existing_msg_id: Option<MessageId>,
    user_id: UserId,
    query: &str,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
    settings: &UserSettingsStore,
) -> ResponseResult<()> {
    let status_msg_id = if let Some(mid) = existing_msg_id {
        let _ = bot
            .edit_message_text(chat_id, mid, format!("🔍 Ищу на SoundCloud: «{query}»..."))
            .reply_markup(InlineKeyboardMarkup::default())
            .await;
        mid
    } else {
        let mut status_req =
            bot.send_message(chat_id, format!("🔍 Ищу на SoundCloud: «{query}»..."));
        if let Some(msg_id) = reply_to {
            status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
        }
        let status_msg = status_req.await?;
        status_msg.id
    };

    match search_soundcloud_tracks(query).await {
        Ok(tracks) if tracks.is_empty() => {
            let search_id = cache.store_search_query(query.to_string());
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg_id,
                    format!("По запросу «{query}» ничего не найдено на SoundCloud."),
                )
                .reply_markup(build_empty_search_keyboard(search_id, "sc"))
                .await;
        }
        Ok(tracks) if tracks.len() == 1 => {
            let url = tracks[0].url.clone();
            let _ = bot.delete_message(chat_id, status_msg_id).await;
            let user_settings = settings.get(user_id);
            let mut opts = MediaPipelineOptions::new(url);
            opts.audio_only = true;
            opts.speed = user_settings.default_speed.to_f32();
            opts.ym_quality = user_settings.ym_quality;
            opts.send_as_document = user_settings.send_as_document;
            process_and_send_media(
                bot,
                chat_id,
                reply_to,
                user_id,
                opts,
                config,
                queue,
                cache,
            )
            .await?;
        }
        Ok(tracks) => {
            let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
            let total_pages = total_pages.max(1);
            let search_id = cache.store_sc_search(query.to_string(), tracks.clone());
            let text = format_sc_search_text(query, 0, total_pages);
            let keyboard = build_sc_selection_keyboard(search_id, &tracks, 0);

            let _ = bot
                .edit_message_text(chat_id, status_msg_id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(e) => {
            log::error!("SoundCloud search error for query {:?}: {:?}", query, e);
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg_id,
                    format!("Ошибка при поиске на SoundCloud: {e}"),
                )
                .await;
        }
    }

    Ok(())
}

pub async fn handle_youtube_search(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    existing_msg_id: Option<MessageId>,
    query: &str,
    cache: &MediaCache,
) -> ResponseResult<()> {
    let status_msg_id = if let Some(mid) = existing_msg_id {
        let _ = bot
            .edit_message_text(chat_id, mid, format!("🔍 Ищу на YouTube: «{query}»..."))
            .reply_markup(InlineKeyboardMarkup::default())
            .await;
        mid
    } else {
        let mut status_req =
            bot.send_message(chat_id, format!("🔍 Ищу на YouTube: «{query}»..."));
        if let Some(msg_id) = reply_to {
            status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
        }
        let status_msg = status_req.await?;
        status_msg.id
    };

    match search_youtube_videos(query).await {
        Ok(videos) if videos.is_empty() => {
            let search_id = cache.store_search_query(query.to_string());
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg_id,
                    format!("По запросу «{query}» ничего не найдено на YouTube."),
                )
                .reply_markup(build_empty_search_keyboard(search_id, "yt"))
                .await;
        }
        Ok(videos) => {
            let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
            let total_pages = total_pages.max(1);
            let search_id = cache.store_yt_search(query.to_string(), videos.clone());
            let text = format_yt_search_text(query, 0, total_pages);
            let keyboard = build_yt_selection_keyboard(search_id, &videos, 0);

            let _ = bot
                .edit_message_text(chat_id, status_msg_id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(e) => {
            log::error!("YouTube search error for query {:?}: {:?}", query, e);
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg_id,
                    format!("Ошибка при поиске на YouTube: {e}"),
                )
                .await;
        }
    }

    Ok(())
}

pub async fn handle_pornhub_search(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    query: &str,
    cache: &MediaCache,
) -> ResponseResult<()> {
    match search_pornhub_videos(query).await {
        Ok(videos) if videos.is_empty() => {
            let mut req = bot.send_message(
                chat_id,
                format!("По запросу «{query}» ничего не найдено на Pornhub."),
            );
            if let Some(msg_id) = reply_to {
                req = req.reply_parameters(ReplyParameters::new(msg_id));
            }
            req.await?;
        }
        Ok(videos) => {
            let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
            let total_pages = total_pages.max(1);
            let search_id = cache.store_ph_search(query.to_string(), videos.clone());
            let text = format_ph_search_text(query, 0, total_pages);
            let keyboard = build_ph_selection_keyboard(search_id, &videos, 0);

            let mut req = bot.send_message(chat_id, text).reply_markup(keyboard);
            if let Some(msg_id) = reply_to {
                req = req.reply_parameters(ReplyParameters::new(msg_id));
            }
            req.await?;
        }
        Err(e) => {
            log::error!("Pornhub search error for query {:?}: {:?}", query, e);
            let mut req = bot.send_message(
                chat_id,
                format!("Ошибка при поиске на Pornhub: {e}"),
            );
            if let Some(msg_id) = reply_to {
                req = req.reply_parameters(ReplyParameters::new(msg_id));
            }
            req.await?;
        }
    }

    Ok(())
}
