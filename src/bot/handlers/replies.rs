use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{MessageId, ParseMode, ReplyParameters, UserId};

use crate::cache::MediaCache;
use crate::config::Config;
use crate::parsing::{
    extract_speed_modifier, extract_time_range, extract_url, is_audio_request,
    parse_media_modifiers,
};
use crate::pipeline::process_and_send_media;
use crate::queue::QueueManager;
use crate::settings::UserSettingsStore;

pub enum ReplyAction {
    Handled,
    NotHandled,
}

pub fn extract_url_from_reply(reply: &Message, cache: &MediaCache) -> Option<String> {
    // 1. Check if the replied message has an inline keyboard with url_id
    if let Some(reply_markup) = reply.reply_markup() {
        for row in &reply_markup.inline_keyboard {
            for btn in row {
                if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref data) = btn.kind {
                    for prefix in &["audio:", "subway:", "speed:", "speed_a:", "clip:", "chapters:"] {
                        if let Some(id_str) = data.strip_prefix(prefix) {
                            if let Ok(url_id) = id_str.parse::<u64>() {
                                if let Some(url) = cache.resolve_url(url_id) {
                                    return Some(url);
                                }
                            }
                        }
                    }
                    if let Some(rest) = data.strip_prefix("spd:") {
                        let parts: Vec<&str> = rest.split(':').collect();
                        if !parts.is_empty() {
                            let id_cand = if parts[0] == "back_v" || parts[0] == "back_a" {
                                parts.get(1).copied()
                            } else {
                                Some(parts[0])
                            };
                            if let Some(id_str) = id_cand {
                                if let Ok(url_id) = id_str.parse::<u64>() {
                                    if let Some(url) = cache.resolve_url(url_id) {
                                        return Some(url);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(rest) = data.strip_prefix("q:") {
                        let parts: Vec<&str> = rest.split(':').collect();
                        if parts.len() == 2 {
                            if let Ok(url_id) = parts[1].parse::<u64>() {
                                if let Some(url) = cache.resolve_url(url_id) {
                                    return Some(url);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Check text or caption of the replied message
    if let Some(text) = reply.text().or_else(|| reply.caption()) {
        if let Some(url) = extract_url(text) {
            return Some(url.to_string());
        }
    }

    // 3. If the replied message was a prompt replying to another message, check parent reply
    if let Some(parent) = reply.reply_to_message() {
        return extract_url_from_reply(parent, cache);
    }

    None
}

pub async fn handle_reply(
    bot: &Bot,
    chat_id: ChatId,
    msg_id: MessageId,
    user_id: UserId,
    reply: &Message,
    trimmed: &str,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
    settings: &UserSettingsStore,
) -> ResponseResult<ReplyAction> {
    if extract_url(trimmed).is_some() {
        return Ok(ReplyAction::NotHandled);
    }

    let source_media = crate::media::extract_source_media_recursive(reply);
    let raw_url_opt = extract_url_from_reply(reply, cache);

    if source_media.is_none() && raw_url_opt.is_none() {
        return Ok(ReplyAction::NotHandled);
    }

    let tr_opt = extract_time_range(trimmed);
    let spd_opt = extract_speed_modifier(trimmed);
    let is_subway_req = trimmed == "/subway"
        || trimmed.starts_with("/subway ")
        || trimmed.contains("#subway");
    let is_audio_req = trimmed == "/audio"
        || trimmed.starts_with("/audio ")
        || trimmed.eq_ignore_ascii_case("аудио")
        || trimmed.eq_ignore_ascii_case("audio")
        || trimmed.contains("#audio");
    let is_clip_req =
        trimmed == "/clip" || trimmed.starts_with("/clip ") || trimmed.starts_with("/clip@");
    let is_speed_req =
        trimmed == "/speed" || trimmed.starts_with("/speed ") || trimmed.starts_with("/speed@");

    if is_clip_req && tr_opt.is_none() {
        bot.send_message(
            chat_id,
            "✂️ Укажи отрезок времени для обрезки, например: <code>00:10-00:30</code> или <code>15-45</code>",
        )
        .parse_mode(ParseMode::Html)
        .reply_parameters(ReplyParameters::new(msg_id))
        .await?;
        return Ok(ReplyAction::Handled);
    }

    if is_speed_req && spd_opt.is_none() {
        bot.send_message(
            chat_id,
            "⚡ Укажи скорость (например: <code>1.5x</code> или <code>2x</code>), либо нажми кнопку <b>⚡ Скорость</b> под сообщением.",
        )
        .parse_mode(ParseMode::Html)
        .reply_parameters(ReplyParameters::new(msg_id))
        .await?;
        return Ok(ReplyAction::Handled);
    }

    if tr_opt.is_some()
        || spd_opt.is_some()
        || is_subway_req
        || is_audio_req
        || is_clip_req
        || is_speed_req
    {
        let raw_url = raw_url_opt.unwrap_or_else(|| "telegram://media".to_string());
        let (url, existing_tr, existing_spd, existing_subway) = parse_media_modifiers(&raw_url);

        if is_subway_req {
            if existing_subway {
                bot.send_message(chat_id, "🛹 К этому видео уже добавлен Subway Surfers!")
                    .reply_parameters(ReplyParameters::new(msg_id))
                    .await?;
                return Ok(ReplyAction::Handled);
            }
            if let Some(crate::media::SourceMedia::Audio(_)) = source_media {
                bot.send_message(
                    chat_id,
                    "🛹 Subway Surfers можно применить только к видео, а не к аудио.",
                )
                .reply_parameters(ReplyParameters::new(msg_id))
                .await?;
                return Ok(ReplyAction::Handled);
            }
        }

        let user_settings = settings.get(user_id);

        let (final_tr, final_spd, final_subway, has_existing_subway) = if source_media.is_some() {
            (tr_opt, spd_opt, is_subway_req, existing_subway)
        } else {
            (
                tr_opt.or(existing_tr),
                spd_opt
                    .or(existing_spd)
                    .or_else(|| user_settings.default_speed.to_f32()),
                is_subway_req || existing_subway,
                false,
            )
        };

        if let Some(ref tr) = final_tr {
            let duration_secs = tr.end_secs.saturating_sub(tr.start_secs);
            if duration_secs > config.max_duration_secs {
                bot.send_message(
                    chat_id,
                    format!(
                        "Длительность выбранного отрезка ({duration_secs} сек.) превышает лимит в {} сек.",
                        config.max_duration_secs
                    ),
                )
                .reply_parameters(ReplyParameters::new(msg_id))
                .await?;
                return Ok(ReplyAction::Handled);
            }
        }

        let has_audio_button = reply
            .reply_markup()
            .map(|rm| {
                rm.inline_keyboard.iter().any(|row| {
                    row.iter().any(|btn| {
                        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref d) = btn.kind
                        {
                            d.starts_with("speed_a:")
                                || d.starts_with("spd:a:")
                                || d.starts_with("spd:back_a:")
                        } else {
                            false
                        }
                    })
                })
            })
            .unwrap_or(false);

        let audio_only = if is_subway_req {
            false
        } else {
            is_audio_req
                || matches!(source_media, Some(crate::media::SourceMedia::Audio(_)))
                || (has_audio_button && !is_subway_req)
                || reply.audio().is_some()
                || is_audio_request(&url, &raw_url, false)
                || (reply.document().is_some()
                    && reply
                        .document()
                        .and_then(|d| d.mime_type.as_ref())
                        .map(|m| m.as_ref().starts_with("audio/"))
                        .unwrap_or(false))
        };

        let opts = crate::pipeline::MediaPipelineOptions {
            url,
            audio_only,
            max_height: None,
            time_range: final_tr,
            speed: final_spd,
            ym_quality: user_settings.ym_quality,
            send_as_document: user_settings.send_as_document,
            with_subway: final_subway,
            source_media,
            has_existing_subway,
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
        return Ok(ReplyAction::Handled);
    }

    Ok(ReplyAction::NotHandled)
}
