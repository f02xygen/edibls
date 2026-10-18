use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{ParseMode, ReplyParameters, UserId};

use crate::bot::callback::{CallbackAction, SearchProviderType, SettingsToggle};
use crate::bot::views::*;
use crate::cache::MediaCache;
use crate::config::Config;
use crate::media::{
    extract_source_media_recursive, is_audio_request, parse_media_modifiers, SourceMedia,
};
use crate::pipeline::MediaPipelineOptions;
use crate::providers::youtube::fetch_youtube_chapters;
use crate::queue::QueueManager;
use crate::settings::{DefaultSearchProvider, QualityPreference, UserSettingsStore};

pub async fn handle_callback_action(
    bot: &Bot,
    q: CallbackQuery,
    action: CallbackAction,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
    settings: &UserSettingsStore,
) -> ResponseResult<()> {
    match action {
        CallbackAction::CancelDownload { user_id } => {
            let caller_uid = q.from.id.0;
            if caller_uid != user_id {
                let _ = bot
                    .answer_callback_query(q.id)
                    .text("Вы не можете отменить чужую загрузку.")
                    .await;
                return Ok(());
            }

            let was_cancelled = queue.cancel_user_download(UserId(user_id));
            if was_cancelled {
                let _ = bot.answer_callback_query(q.id.clone()).text("Загрузка отменяется...").await;
                if let Some(msg) = q.regular_message() {
                    let _ = bot
                        .edit_message_text(msg.chat.id, msg.id, "❌ Загрузка отменена.")
                        .await;
                }
            } else {
                let _ = bot
                    .answer_callback_query(q.id.clone())
                    .text("Загрузка уже завершена или не найдена.")
                    .await;
            }
        }
        CallbackAction::CloseSearch | CallbackAction::ChaptersClose => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
        }
        CallbackAction::SwitchSearch { target, search_id } => {
            let Some(query) = cache.get_search_query(search_id) else {
                let _ = bot
                    .answer_callback_query(q.id)
                    .text("Поисковый запрос устарел, выполните поиск заново.")
                    .await;
                return Ok(());
            };

            let _ = bot.answer_callback_query(q.id.clone()).await;

            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let reply_to = msg.reply_to_message().map(|m| m.id);
                let existing_msg_id = Some(msg.id);

                match target {
                    SearchProviderType::Yandex => {
                        crate::bot::handle_yandex_search(
                            bot,
                            chat_id,
                            reply_to,
                            existing_msg_id,
                            q.from.id,
                            &query,
                            config,
                            queue,
                            cache,
                            settings,
                        )
                        .await?;
                    }
                    SearchProviderType::SoundCloud => {
                        crate::bot::handle_soundcloud_search(
                            bot,
                            chat_id,
                            reply_to,
                            existing_msg_id,
                            q.from.id,
                            &query,
                            config,
                            queue,
                            cache,
                            settings,
                        )
                        .await?;
                    }
                    SearchProviderType::YouTube => {
                        crate::bot::handle_youtube_search(
                            bot,
                            chat_id,
                            reply_to,
                            existing_msg_id,
                            &query,
                            cache,
                        )
                        .await?;
                    }
                    SearchProviderType::Pornhub => {
                        crate::bot::handle_pornhub_search(
                            bot,
                            chat_id,
                            reply_to,
                            &query,
                            cache,
                        )
                        .await?;
                    }
                }
            }
        }
        CallbackAction::SettingsClose => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
        }
        CallbackAction::SettingsToggle(toggle) => {
            let user_id = q.from.id;
            let updated = match toggle {
                SettingsToggle::Quality => settings.toggle_quality(user_id),
                SettingsToggle::Speed => settings.toggle_speed(user_id),
                SettingsToggle::YmQuality => settings.toggle_ym_quality(user_id),
                SettingsToggle::DefaultSearch => settings.toggle_default_search(user_id),
                SettingsToggle::AudioOnly => settings.toggle_audio_only(user_id),
                SettingsToggle::SendAsDocument => settings.toggle_send_as_document(user_id),
            };

            let alert_text = match toggle {
                SettingsToggle::Quality => match updated.quality {
                    QualityPreference::AutoBest => "Качество видео: Авто (максимальное)",
                    QualityPreference::AskEveryTime => "Качество видео: Вручную перед скачиванием",
                },
                SettingsToggle::Speed => match updated.default_speed {
                    crate::settings::SpeedPreference::Normal => "Скорость по умолчанию: 1.0x (Обычная)",
                    crate::settings::SpeedPreference::Slower075 => "Скорость по умолчанию: 0.75x",
                    crate::settings::SpeedPreference::Faster125 => "Скорость по умолчанию: 1.25x",
                    crate::settings::SpeedPreference::Faster15 => "Скорость по умолчанию: 1.5x",
                    crate::settings::SpeedPreference::Faster20 => "Скорость по умолчанию: 2.0x",
                },
                SettingsToggle::YmQuality => match updated.ym_quality {
                    crate::settings::YandexQualityPreference::Optimal => "Яндекс Музыка: AAC 192 kbps ⚡ (быстро)",
                    crate::settings::YandexQualityPreference::Lossless => "Яндекс Музыка: FLAC 💎 (lossless)",
                },
                SettingsToggle::DefaultSearch => match updated.default_search {
                    DefaultSearchProvider::Yandex => "Поиск по умолчанию: 🎶 Яндекс Музыка",
                    DefaultSearchProvider::SoundCloud => "Поиск по умолчанию: ☁️ SoundCloud",
                    DefaultSearchProvider::YouTube => "Поиск по умолчанию: 🎬 YouTube",
                },
                SettingsToggle::AudioOnly => {
                    if updated.audio_only {
                        "Режим «Только аудио» ВКЛЮЧЁН"
                    } else {
                        "Режим «Только аудио» ВЫКЛЮЧЕН"
                    }
                }
                SettingsToggle::SendAsDocument => {
                    if updated.send_as_document {
                        "Отправка файлом (без сжатия) ВКЛЮЧЕНА"
                    } else {
                        "Отправка файлом ВЫКЛЮЧЕНА"
                    }
                }
            };

            let _ = bot.answer_callback_query(q.id.clone()).text(alert_text).await;

            if let Some(msg) = q.regular_message() {
                let _ = bot
                    .edit_message_text(msg.chat.id, msg.id, build_settings_text(&updated))
                    .parse_mode(ParseMode::Html)
                    .reply_markup(build_settings_keyboard(&updated))
                    .await;
            }
        }
        CallbackAction::Noop => {
            let _ = bot.answer_callback_query(q.id).await;
        }
        CallbackAction::Subway { url_id } => {
            let Some(raw_url) = cache.resolve_url(url_id) else {
                bot.answer_callback_query(q.id)
                    .text("Ссылка устарела, отправь её заново")
                    .await?;
                return Ok(());
            };

            let (url, _tr, _spd, existing_subway) = parse_media_modifiers(&raw_url);
            if existing_subway {
                bot.answer_callback_query(q.id)
                    .text("Subway Surfers уже добавлен к этому видео")
                    .await?;
                return Ok(());
            }

            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("🛹 Добавляю Subway Surfers...")
                .await;

            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let msg_id = msg.id;
                let source_media = extract_source_media_recursive(msg);
                let user_settings = settings.get(q.from.id);

                let opts = MediaPipelineOptions {
                    url,
                    audio_only: false,
                    max_height: None,
                    time_range: None,
                    speed: None,
                    ym_quality: user_settings.ym_quality,
                    send_as_document: user_settings.send_as_document,
                    with_subway: true,
                    source_media,
                    has_existing_subway: false,
                };

                crate::bot::process_and_send_media(
                    bot,
                    chat_id,
                    Some(msg_id),
                    q.from.id,
                    opts,
                    config,
                    queue,
                    cache,
                )
                .await?;
            }
        }
        CallbackAction::SpeedMenu { url_id, is_audio } => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let kb = build_speed_selection_keyboard(url_id, is_audio);
                let _ = bot.edit_message_reply_markup(msg.chat.id, msg.id).reply_markup(kb).await;
            }
        }
        CallbackAction::SpeedBack { url_id, is_audio } => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                if is_audio {
                    let kb = build_audio_action_buttons(url_id);
                    let _ = bot.edit_message_reply_markup(msg.chat.id, msg.id).reply_markup(kb).await;
                } else if let Some(raw_url) = cache.resolve_url(url_id) {
                    let (url, _tr, _spd, with_subway) = parse_media_modifiers(&raw_url);
                    let is_yt = url.contains("youtube.com") || url.contains("youtu.be");
                    let kb = build_video_action_buttons(url_id, is_yt, with_subway);
                    let _ = bot.edit_message_reply_markup(msg.chat.id, msg.id).reply_markup(kb).await;
                }
            }
        }
        CallbackAction::SpeedApply { url_id, speed, is_audio } => {
            let Some(raw_url) = cache.resolve_url(url_id) else {
                bot.answer_callback_query(q.id)
                    .text("Ссылка устарела, отправь её заново")
                    .await?;
                return Ok(());
            };

            let (url, _tr, _spd, existing_subway) = parse_media_modifiers(&raw_url);
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text(format!("⚡ Меняю скорость на {speed}x..."))
                .await;

            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let msg_id = msg.id;
                let source_media = extract_source_media_recursive(msg);
                let audio_only = is_audio
                    || matches!(source_media, Some(SourceMedia::Audio(_)))
                    || is_audio_request(&url, &raw_url, false);

                let user_settings = settings.get(q.from.id);
                let opts = MediaPipelineOptions {
                    url,
                    audio_only,
                    max_height: None,
                    time_range: None,
                    speed: Some(speed),
                    ym_quality: user_settings.ym_quality,
                    send_as_document: user_settings.send_as_document,
                    with_subway: false,
                    source_media,
                    has_existing_subway: existing_subway,
                };

                crate::bot::process_and_send_media(
                    bot,
                    chat_id,
                    Some(msg_id),
                    q.from.id,
                    opts,
                    config,
                    queue,
                    cache,
                )
                .await?;
            }
        }
        CallbackAction::ClipPrompt { url_id: _ } => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                bot.send_message(
                    msg.chat.id,
                    "✂️ Чтобы обрезать этот файл, ответь (Reply) на него с нужным таймкодом, например:\n<code>00:10-00:30</code> или <code>15-45</code>",
                )
                .parse_mode(ParseMode::Html)
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
            }
        }
        CallbackAction::ChaptersMenu { url_id } => {
            let Some(url) = cache.resolve_url(url_id) else {
                bot.answer_callback_query(q.id)
                    .text("Ссылка устарела, отправь её заново")
                    .await?;
                return Ok(());
            };

            let chapters = match cache.get_yt_chapters(url_id) {
                Some(c) => c,
                None => {
                    let _ = bot
                        .answer_callback_query(q.id.clone())
                        .text("⏳ Загружаю список глав...")
                        .await;
                    let fetched = fetch_youtube_chapters(&url).await.unwrap_or_default();
                    cache.store_yt_chapters(url_id, fetched.clone());
                    fetched
                }
            };

            if chapters.is_empty() {
                bot.answer_callback_query(q.id)
                    .text("У этого видео нет глав (таймкодов).")
                    .await?;
                return Ok(());
            }

            let _ = bot.answer_callback_query(q.id.clone()).await;

            if let Some(msg) = q.regular_message() {
                let kb = build_chapters_keyboard(url_id, &chapters, 0);
                let total = chapters.len();
                let text = format!("📑 <b>Главы видео</b> ({total} шт.):\n\nВыбери главу, чтобы скачать нужный фрагмент:");
                let req = bot
                    .send_message(msg.chat.id, text)
                    .parse_mode(ParseMode::Html)
                    .reply_markup(kb)
                    .reply_parameters(ReplyParameters::new(msg.id));
                req.await?;
            }
        }
        CallbackAction::ChaptersPage { url_id, page } => {
            if let Some(chapters) = cache.get_yt_chapters(url_id) {
                let _ = bot.answer_callback_query(q.id.clone()).await;
                if let Some(msg) = q.regular_message() {
                    let kb = build_chapters_keyboard(url_id, &chapters, page);
                    let _ = bot.edit_message_reply_markup(msg.chat.id, msg.id).reply_markup(kb).await;
                }
            }
        }
        CallbackAction::ChaptersGet { url_id, chapter_idx } => {
            let Some(url) = cache.resolve_url(url_id) else {
                bot.answer_callback_query(q.id).text("Ссылка устарела").await?;
                return Ok(());
            };
            let Some(chapters) = cache.get_yt_chapters(url_id) else {
                bot.answer_callback_query(q.id).text("Список глав устарел").await?;
                return Ok(());
            };

            if let Some(ch) = chapters.get(chapter_idx) {
                let _ = bot
                    .answer_callback_query(q.id.clone())
                    .text(format!("Скачиваю главу: {}", ch.title))
                    .await;

                if let Some(msg) = q.regular_message() {
                    let chat_id = msg.chat.id;
                    let msg_id = msg.id;
                    let user_settings = settings.get(q.from.id);
                    let tr = crate::media::TimeRange {
                        start_secs: ch.start_secs,
                        end_secs: ch.end_secs,
                        raw_label: ch.title.clone(),
                    };
                    let (clean_url, _ch_tr, spd, sw) = parse_media_modifiers(&url);
                    let opts = MediaPipelineOptions {
                        url: clean_url,
                        audio_only: false,
                        max_height: None,
                        time_range: Some(tr),
                        speed: spd,
                        ym_quality: user_settings.ym_quality,
                        send_as_document: user_settings.send_as_document,
                        with_subway: sw,
                        source_media: None,
                        has_existing_subway: false,
                    };
                    crate::bot::process_and_send_media(
                        bot,
                        chat_id,
                        Some(msg_id),
                        q.from.id,
                        opts,
                        config,
                        queue,
                        cache,
                    )
                    .await?;
                }
            }
        }
        CallbackAction::ExtractAudio { url_id } => {
            let Some(raw_url) = cache.resolve_url(url_id) else {
                bot.answer_callback_query(q.id)
                    .text("Ссылка устарела, отправь её заново")
                    .await?;
                return Ok(());
            };

            let (url, tr, spd, _) = parse_media_modifiers(&raw_url);
            let _ = bot
                .answer_callback_query(q.id.clone())
                .text("🎵 Извлекаю аудио...")
                .await;

            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let msg_id = msg.id;
                let _ = bot.edit_message_reply_markup(chat_id, msg_id).await;

                let source_media = extract_source_media_recursive(msg);
                let user_settings = settings.get(q.from.id);
                let opts = MediaPipelineOptions {
                    url,
                    audio_only: true,
                    max_height: None,
                    time_range: if source_media.is_some() { None } else { tr },
                    speed: if source_media.is_some() { None } else { spd },
                    ym_quality: user_settings.ym_quality,
                    send_as_document: user_settings.send_as_document,
                    with_subway: false,
                    source_media,
                    has_existing_subway: false,
                };
                crate::bot::process_and_send_media(
                    bot,
                    chat_id,
                    Some(msg_id),
                    q.from.id,
                    opts,
                    config,
                    queue,
                    cache,
                )
                .await?;
            }
        }
        CallbackAction::QualityCancel => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
        }
        CallbackAction::QualitySelect { height, url_id } => {
            let Some(url) = cache.resolve_url(url_id) else {
                bot.answer_callback_query(q.id)
                    .text("Ссылка устарела, отправь её заново")
                    .await?;
                return Ok(());
            };

            let _ = bot
                .answer_callback_query(q.id.clone())
                .text(format!("Скачиваю в {height}p..."))
                .await;

            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let reply_to = msg.reply_to_message().map(|m| m.id);
                let _ = bot.delete_message(chat_id, msg.id).await;

                let user_settings = settings.get(q.from.id);
                let (clean_url, tr, spd, sw) = parse_media_modifiers(&url);
                let final_spd = spd.or_else(|| user_settings.default_speed.to_f32());
                let opts = MediaPipelineOptions {
                    url: clean_url,
                    audio_only: false,
                    max_height: Some(height),
                    time_range: tr,
                    speed: final_spd,
                    ym_quality: user_settings.ym_quality,
                    send_as_document: user_settings.send_as_document,
                    with_subway: sw,
                    source_media: None,
                    has_existing_subway: false,
                };
                crate::bot::process_and_send_media(
                    bot,
                    chat_id,
                    reply_to,
                    q.from.id,
                    opts,
                    config,
                    queue,
                    cache,
                )
                .await?;
            }
        }
        CallbackAction::SearchCancel(_) => {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
        }
        CallbackAction::SearchPage { provider, search_id, page } => {
            match provider {
                SearchProviderType::Pornhub => {
                    if let Some((query, videos)) = cache.get_ph_search(search_id) {
                        let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_ph_search_text(&query, page, total_pages);
                        let keyboard = build_ph_selection_keyboard(search_id, &videos, page);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot.edit_message_text(msg.chat.id, msg.id, text).reply_markup(keyboard).await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                    }
                }
                SearchProviderType::Yandex => {
                    if let Some((query, tracks)) = cache.get_ym_search(search_id) {
                        let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_ym_search_text(&query, page, total_pages);
                        let keyboard = build_track_selection_keyboard(search_id, &tracks, page);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot.edit_message_text(msg.chat.id, msg.id, text).reply_markup(keyboard).await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                    }
                }
                SearchProviderType::YouTube => {
                    if let Some((query, videos)) = cache.get_yt_search(search_id) {
                        let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_yt_search_text(&query, page, total_pages);
                        let keyboard = build_yt_selection_keyboard(search_id, &videos, page);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot.edit_message_text(msg.chat.id, msg.id, text).reply_markup(keyboard).await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                    }
                }
                SearchProviderType::SoundCloud => {
                    if let Some((query, tracks)) = cache.get_sc_search(search_id) {
                        let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_sc_search_text(&query, page, total_pages);
                        let keyboard = build_sc_selection_keyboard(search_id, &tracks, page);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot.edit_message_text(msg.chat.id, msg.id, text).reply_markup(keyboard).await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                    }
                }
            }
        }
        CallbackAction::SearchNoop { provider, search_id, page } => {
            let total_pages = match provider {
                SearchProviderType::Pornhub => cache.get_ph_search(search_id).map(|(_, v)| (v.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE),
                SearchProviderType::Yandex => cache.get_ym_search(search_id).map(|(_, t)| (t.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE),
                SearchProviderType::YouTube => cache.get_yt_search(search_id).map(|(_, v)| (v.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE),
                SearchProviderType::SoundCloud => cache.get_sc_search(search_id).map(|(_, t)| (t.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE),
            }.unwrap_or(1).max(1);

            let _ = bot
                .answer_callback_query(q.id.clone())
                .text(format!("Страница {} из {}", page + 1, total_pages))
                .await;
        }
        CallbackAction::SearchSelectPh { viewkey } => {
            let url = format!("https://www.pornhub.com/view_video.php?viewkey={viewkey}");
            let user_settings = settings.get(q.from.id);
            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let reply_to = msg.reply_to_message().map(|m| m.id);

                if user_settings.audio_only {
                    let _ = bot.answer_callback_query(q.id.clone()).text("🎵 Скачиваю аудио...").await;
                    let _ = bot.delete_message(chat_id, msg.id).await;
                    let mut opts = MediaPipelineOptions::new(url);
                    opts.audio_only = true;
                    opts.speed = user_settings.default_speed.to_f32();
                    opts.ym_quality = user_settings.ym_quality;
                    opts.send_as_document = user_settings.send_as_document;
                    crate::bot::process_and_send_media(bot, chat_id, reply_to, q.from.id, opts, config, queue, cache).await?;
                } else if user_settings.quality == QualityPreference::AskEveryTime {
                    let _ = bot.answer_callback_query(q.id.clone()).await;
                    let url_id = cache.register_url(&url);
                    let _ = bot.edit_message_text(chat_id, msg.id, "Выбери желаемое качество для этого видео:")
                        .reply_markup(build_quality_selection_keyboard(url_id)).await;
                } else {
                    let _ = bot.answer_callback_query(q.id.clone()).text("Скачиваю выбранное видео...").await;
                    let _ = bot.delete_message(chat_id, msg.id).await;
                    let mut opts = MediaPipelineOptions::new(url);
                    opts.speed = user_settings.default_speed.to_f32();
                    opts.ym_quality = user_settings.ym_quality;
                    opts.send_as_document = user_settings.send_as_document;
                    crate::bot::process_and_send_media(bot, chat_id, reply_to, q.from.id, opts, config, queue, cache).await?;
                }
            }
        }
        CallbackAction::SearchSelectYm { track_id } => {
            let _ = bot.answer_callback_query(q.id.clone()).text("Скачиваю выбранный трек...").await;
            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let reply_to = msg.reply_to_message().map(|m| m.id);
                let _ = bot.delete_message(chat_id, msg.id).await;

                let url = format!("https://music.yandex.ru/track/{track_id}");
                let user_settings = settings.get(q.from.id);
                let mut opts = MediaPipelineOptions::new(url);
                opts.audio_only = true;
                opts.speed = user_settings.default_speed.to_f32();
                opts.ym_quality = user_settings.ym_quality;
                opts.send_as_document = user_settings.send_as_document;
                crate::bot::process_and_send_media(bot, chat_id, reply_to, q.from.id, opts, config, queue, cache).await?;
            }
        }
        CallbackAction::SearchSelectYt { video_id } => {
            let url = format!("https://www.youtube.com/watch?v={video_id}");
            let user_settings = settings.get(q.from.id);
            if let Some(msg) = q.regular_message() {
                let chat_id = msg.chat.id;
                let reply_to = msg.reply_to_message().map(|m| m.id);

                if user_settings.audio_only {
                    let _ = bot.answer_callback_query(q.id.clone()).text("🎵 Скачиваю аудио...").await;
                    let _ = bot.delete_message(chat_id, msg.id).await;
                    let mut opts = MediaPipelineOptions::new(url);
                    opts.audio_only = true;
                    opts.speed = user_settings.default_speed.to_f32();
                    opts.ym_quality = user_settings.ym_quality;
                    opts.send_as_document = user_settings.send_as_document;
                    crate::bot::process_and_send_media(bot, chat_id, reply_to, q.from.id, opts, config, queue, cache).await?;
                } else if user_settings.quality == QualityPreference::AskEveryTime {
                    let _ = bot.answer_callback_query(q.id.clone()).await;
                    let url_id = cache.register_url(&url);
                    let _ = bot.edit_message_text(chat_id, msg.id, "Выбери желаемое качество для этого видео:")
                        .reply_markup(build_quality_selection_keyboard(url_id)).await;
                } else {
                    let _ = bot.answer_callback_query(q.id.clone()).text("Скачиваю выбранное видео...").await;
                    let _ = bot.delete_message(chat_id, msg.id).await;
                    let mut opts = MediaPipelineOptions::new(url);
                    opts.speed = user_settings.default_speed.to_f32();
                    opts.ym_quality = user_settings.ym_quality;
                    opts.send_as_document = user_settings.send_as_document;
                    crate::bot::process_and_send_media(bot, chat_id, reply_to, q.from.id, opts, config, queue, cache).await?;
                }
            }
        }
        CallbackAction::SearchSelectSc { search_id, item_idx } => {
            if let Some((_, tracks)) = cache.get_sc_search(search_id) {
                if let Some(track) = tracks.get(item_idx) {
                    let track_url = track.url.clone();
                    let _ = bot.answer_callback_query(q.id.clone()).text("Скачиваю трек с SoundCloud...").await;

                    if let Some(msg) = q.regular_message() {
                        let chat_id = msg.chat.id;
                        let reply_to = msg.reply_to_message().map(|m| m.id);
                        let _ = bot.delete_message(chat_id, msg.id).await;

                        let user_settings = settings.get(q.from.id);
                        let mut opts = MediaPipelineOptions::new(track_url);
                        opts.audio_only = true;
                        opts.speed = user_settings.default_speed.to_f32();
                        opts.ym_quality = user_settings.ym_quality;
                        opts.send_as_document = user_settings.send_as_document;
                        crate::bot::process_and_send_media(bot, chat_id, reply_to, q.from.id, opts, config, queue, cache).await?;
                    }
                    return Ok(());
                }
            }
            let _ = bot.answer_callback_query(q.id).text("Трек устарел, выполни поиск заново").await;
        }
        CallbackAction::Unknown(data) => {
            log::warn!("Unhandled callback query data: {}", data);
            let _ = bot.answer_callback_query(q.id).await;
        }
    }

    Ok(())
}
