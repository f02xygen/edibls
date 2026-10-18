use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{ParseMode, ReplyParameters};

use crate::bot::handlers::commands::{handle_command, CommandAction};
use crate::bot::handlers::replies::{handle_reply, ReplyAction};
use crate::bot::handlers::search::{
    handle_pornhub_search, handle_soundcloud_search, handle_yandex_search, handle_youtube_search,
};
use crate::bot::views::build_quality_selection_keyboard;
use crate::cache::MediaCache;
use crate::config::Config;
use crate::parsing::{
    extract_all_urls, extract_ph_search_query, extract_sc_search_query, extract_search_query,
    extract_speed_modifier, extract_time_range, extract_url, extract_yt_search_query,
    is_audio_request, parse_media_modifiers,
};
use crate::pipeline::{process_and_send_media, MediaPipelineOptions};
use crate::queue::QueueManager;
use crate::settings::{DefaultSearchProvider, QualityPreference, UserSettingsStore};

pub async fn handle_message(
    bot: Bot,
    msg: Message,
    config: Arc<Config>,
    queue: QueueManager,
    cache: MediaCache,
    settings: UserSettingsStore,
) -> ResponseResult<()> {
    let user_id = match &msg.from {
        Some(user) => user.id,
        None => return Ok(()),
    };

    if let Some(text) = msg.text().or_else(|| msg.caption()) {
        let trimmed = text.trim();

        // 1. Try commands (/start, /help, /settings, etc.)
        match handle_command(
            &bot,
            msg.chat.id,
            msg.id,
            user_id,
            trimmed,
            &config,
            &queue,
            &cache,
            &settings,
        )
        .await?
        {
            CommandAction::Handled => return Ok(()),
            CommandAction::NotACommand => {}
        }

        // 2. Try reply-to-media actions (timecodes, speed, #audio, subway)
        if let Some(reply) = msg.reply_to_message() {
            match handle_reply(
                &bot,
                msg.chat.id,
                msg.id,
                user_id,
                reply,
                trimmed,
                &config,
                &queue,
                &cache,
                &settings,
            )
            .await?
            {
                ReplyAction::Handled => return Ok(()),
                ReplyAction::NotHandled => {}
            }
        }

        // 3. Search commands with explicit query (/yt, /sc, /ph)
        if let Some(query) = extract_yt_search_query(trimmed) {
            handle_youtube_search(&bot, msg.chat.id, Some(msg.id), None, query, &cache).await?;
            return Ok(());
        }

        if let Some(query) = extract_sc_search_query(trimmed) {
            handle_soundcloud_search(
                &bot,
                msg.chat.id,
                Some(msg.id),
                None,
                user_id,
                query,
                &config,
                &queue,
                &cache,
                &settings,
            )
            .await?;
            return Ok(());
        }

        if let Some(query) = extract_ph_search_query(trimmed) {
            handle_pornhub_search(&bot, msg.chat.id, Some(msg.id), query, &cache).await?;
            return Ok(());
        }

        // 4. Multiple URLs in a single message
        let all_urls = extract_all_urls(trimmed);
        if all_urls.len() > 1 {
            let user_settings = settings.get(user_id);
            let with_subway = trimmed.contains("#subway");
            let speed = extract_speed_modifier(trimmed)
                .or_else(|| user_settings.default_speed.to_f32());
            for url in all_urls {
                let (clean_url, tr_frag, speed_frag, subway_frag) = parse_media_modifiers(url);
                let final_speed = speed.or(speed_frag);
                let final_subway = with_subway || subway_frag;
                let audio_only = is_audio_request(&clean_url, trimmed, user_settings.audio_only);
                let opts = MediaPipelineOptions {
                    url: clean_url,
                    audio_only,
                    max_height: None,
                    time_range: tr_frag,
                    speed: final_speed,
                    ym_quality: user_settings.ym_quality,
                    send_as_document: user_settings.send_as_document,
                    with_subway: final_subway,
                    source_media: None,
                    has_existing_subway: false,
                };
                let _ = process_and_send_media(
                    &bot,
                    msg.chat.id,
                    Some(msg.id),
                    user_id,
                    opts,
                    &config,
                    &queue,
                    &cache,
                )
                .await;
            }
            return Ok(());
        }

        // 5. Single URL
        if let Some(url) = all_urls.first().copied().or_else(|| extract_url(trimmed)) {
            let (clean_url, tr_frag, speed_frag, subway_frag) = parse_media_modifiers(url);
            let time_range = extract_time_range(trimmed).or(tr_frag);

            if let Some(ref tr) = time_range {
                let duration_secs = tr.end_secs.saturating_sub(tr.start_secs);
                if duration_secs > config.max_duration_secs {
                    bot.send_message(
                        msg.chat.id,
                        format!(
                            "Длительность выбранного отрезка ({duration_secs} сек.) превышает лимит в {} сек.",
                            config.max_duration_secs
                        ),
                    )
                    .reply_parameters(ReplyParameters::new(msg.id))
                    .await?;
                    return Ok(());
                }
            }

            let user_settings = settings.get(user_id);
            let speed = extract_speed_modifier(trimmed)
                .or(speed_frag)
                .or_else(|| user_settings.default_speed.to_f32());
            let audio_only = is_audio_request(&clean_url, trimmed, user_settings.audio_only);
            let with_subway = trimmed.contains("#subway") || subway_frag;

            // If user prefers manual quality selection and it is not an audio request, Subway, Yandex Music, a clip, or speed
            if !audio_only
                && !with_subway
                && time_range.is_none()
                && speed.is_none()
                && user_settings.quality == QualityPreference::AskEveryTime
                && !crate::providers::yandex::is_yandex_music_url(&clean_url)
            {
                let url_id = cache.register_url(&clean_url);
                bot.send_message(msg.chat.id, "Выбери желаемое качество для этого видео:")
                    .reply_markup(build_quality_selection_keyboard(url_id))
                    .reply_parameters(ReplyParameters::new(msg.id))
                    .await?;
            } else {
                let opts = MediaPipelineOptions {
                    url: clean_url,
                    audio_only,
                    max_height: None,
                    time_range,
                    speed,
                    ym_quality: user_settings.ym_quality,
                    send_as_document: user_settings.send_as_document,
                    with_subway,
                    source_media: None,
                    has_existing_subway: false,
                };
                process_and_send_media(
                    &bot,
                    msg.chat.id,
                    Some(msg.id),
                    user_id,
                    opts,
                    &config,
                    &queue,
                    &cache,
                )
                .await?;
            }
            return Ok(());
        } else if let Some(query) = extract_search_query(trimmed, msg.chat.is_private()) {
            // 6. Generic text search
            let is_explicit_ym = trimmed.starts_with("/ym")
                || trimmed.starts_with("/audio")
                || trimmed.starts_with("/mp3")
                || trimmed.starts_with("/search");
            let user_settings = settings.get(user_id);
            if is_explicit_ym {
                handle_yandex_search(
                    &bot,
                    msg.chat.id,
                    Some(msg.id),
                    None,
                    user_id,
                    query,
                    &config,
                    &queue,
                    &cache,
                    &settings,
                )
                .await?;
            } else {
                match user_settings.default_search {
                    DefaultSearchProvider::Yandex => {
                        handle_yandex_search(
                            &bot,
                            msg.chat.id,
                            Some(msg.id),
                            None,
                            user_id,
                            query,
                            &config,
                            &queue,
                            &cache,
                            &settings,
                        )
                        .await?;
                    }
                    DefaultSearchProvider::SoundCloud => {
                        handle_soundcloud_search(
                            &bot,
                            msg.chat.id,
                            Some(msg.id),
                            None,
                            user_id,
                            query,
                            &config,
                            &queue,
                            &cache,
                            &settings,
                        )
                        .await?;
                    }
                    DefaultSearchProvider::YouTube => {
                        handle_youtube_search(
                            &bot,
                            msg.chat.id,
                            Some(msg.id),
                            None,
                            query,
                            &cache,
                        )
                        .await?;
                    }
                }
            }
        } else if msg.chat.is_private() {
            // Only send fallback hint in private chats so the bot stays quiet in group chats
            bot.send_message(
                msg.chat.id,
                "Отправь ссылку на видео/аудио, используй <code>/yt &lt;запрос&gt;</code> для поиска на YouTube, <code>/sc &lt;запрос&gt;</code> для SoundCloud, <code>/ym &lt;запрос&gt;</code> для Яндекс Музыки, или <code>/ph &lt;запрос&gt;</code> для Pornhub.",
            )
            .parse_mode(ParseMode::Html)
            .reply_parameters(ReplyParameters::new(msg.id))
            .await?;
        }
    }
    Ok(())
}
