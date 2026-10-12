use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, InputFile, MessageId, ParseMode, ReplyParameters,
};
use tempfile::TempDir;

use crate::cache::{CachedMedia, MediaCache};
use crate::config::Config;
use crate::media::{
    extract_ph_search_query, extract_sc_search_query, extract_search_query, extract_url,
    extract_yt_search_query, fetch_media, format_progress_bar, get_video_dimensions,
    is_audio_request, read_network_tx_bytes, MediaOutput,
};
use crate::providers::pornhub::{search_pornhub_videos, PornhubVideoInfo};
use crate::providers::soundcloud::{search_soundcloud_tracks, SoundcloudTrackInfo};
use crate::providers::yandex::{search_yandex_tracks, YandexTrackInfo};
use crate::providers::youtube::{search_youtube_videos, YouTubeVideoInfo};
use crate::queue::QueueManager;
use crate::settings::{QualityPreference, UserSettings, UserSettingsStore};

const WELCOME_MESSAGE: &str = "<b>EDIBLS</b> — бот для быстрой загрузки видео и музыки.

<b>Поддерживаемые площадки:</b>
YouTube, TikTok, Instagram, SoundCloud, Яндекс Музыка, Pornhub, VK и др.

<b>Как пользоваться:</b>
• <b>Видео:</b> просто отправь ссылку в чат (под видео будет кнопка <code>🎵 Скачать аудио</code>).
• <b>Только аудио по ссылке:</b> <code>/audio &lt;ссылка&gt;</code> или прямая ссылка на SoundCloud / Яндекс Музыку.
• <b>Поиск на YouTube:</b> <code>/yt &lt;запрос&gt;</code>
• <b>Поиск на SoundCloud:</b> <code>/sc &lt;запрос&gt;</code>
• <b>Поиск на Яндекс Музыке:</b> <code>/ym &lt;запрос&gt;</code> или название трека в ЛС
• <b>Поиск на Pornhub:</b> <code>/ph &lt;запрос&gt;</code>
• <b>Настройки:</b> <code>/settings</code> (разрешение видео, режим «только аудио», отправка файлом, AAC/FLAC для Я.Музыки)

<i>Поддерживаются видео в высоком качестве (до 2 ГБ) без водяных знаков.</i>";


fn build_audio_button(url_id: u64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "🎵 Скачать аудио",
        format!("audio:{url_id}"),
    )]])
}

fn build_quality_selection_keyboard(url_id: u64) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![
        vec![
            InlineKeyboardButton::callback("🎬 1080p", format!("q:1080:{url_id}")),
            InlineKeyboardButton::callback("🎬 720p", format!("q:720:{url_id}")),
            InlineKeyboardButton::callback("🎬 480p", format!("q:480:{url_id}")),
        ],
        vec![
            InlineKeyboardButton::callback("🎬 360p", format!("q:360:{url_id}")),
            InlineKeyboardButton::callback("🎬 240p", format!("q:240:{url_id}")),
        ],
        vec![
            InlineKeyboardButton::callback("🎵 Только аудио", format!("audio:{url_id}")),
        ],
        vec![
            InlineKeyboardButton::callback("✖️ Отмена", "q:cancel"),
        ],
    ])
}

fn build_settings_text(settings: &UserSettings) -> String {
    let q_desc = match settings.quality {
        QualityPreference::AutoBest => "Авто (максимальное)",
        QualityPreference::AskEveryTime => "Вручную (выбирать перед скачиванием)",
    };
    let a_desc = if settings.audio_only {
        "<b>Включён</b> <i>(видеоссылки → аудио)</i>"
    } else {
        "Выключен <i>(видео отправляются как видео)</i>"
    };
    let ym_desc = match settings.ym_quality {
        crate::settings::YandexQualityPreference::Optimal => "AAC 192 kbps ⚡ <i>(быстро, ~10 МБ)</i>",
        crate::settings::YandexQualityPreference::Lossless => "FLAC 💎 <i>(lossless, ~70 МБ)</i>",
    };
    let doc_desc = if settings.send_as_document {
        "<b>Включена</b> <i>(без сжатия Telegram)</i>"
    } else {
        "Выключена <i>(как медиа: видео/музыка)</i>"
    };

    format!(
        "⚙️ <b>Настройки загрузки</b>\n\n\
        🎬 <b>Качество видео:</b> {q_desc}\n\
        🎶 <b>Яндекс Музыка:</b> {ym_desc}\n\
        🎵 <b>Только аудио:</b> {a_desc}\n\
        📎 <b>Отправка файлом:</b> {doc_desc}\n\n\
        <i>Нажми на кнопку, чтобы переключить:</i>"
    )
}

fn build_settings_keyboard(settings: &UserSettings) -> InlineKeyboardMarkup {
    let q_label = match settings.quality {
        QualityPreference::AutoBest => "🎬 Качество видео: Авто ⚡",
        QualityPreference::AskEveryTime => "🎬 Качество видео: Вручную ❓",
    };

    let ym_label = match settings.ym_quality {
        crate::settings::YandexQualityPreference::Optimal => "🎶 Я.Музыка: AAC 192k ⚡",
        crate::settings::YandexQualityPreference::Lossless => "🎶 Я.Музыка: FLAC 💎",
    };

    let audio_label = if settings.audio_only {
        "🎵 Только аудио: ВКЛ ✅"
    } else {
        "🎵 Только аудио: ВЫКЛ ❌"
    };

    let doc_label = if settings.send_as_document {
        "📎 Отправка файлом: ВКЛ ✅"
    } else {
        "📎 Отправка файлом: ВЫКЛ ❌"
    };

    InlineKeyboardMarkup::new(vec![
        // Row 1: Video quality toggle — full width
        vec![InlineKeyboardButton::callback(
            q_label,
            "settings:toggle:quality",
        )],
        // Row 2: Yandex Music quality toggle — full width
        vec![InlineKeyboardButton::callback(
            ym_label,
            "settings:toggle:ym_quality",
        )],
        // Row 3: Audio-only toggle — full width
        vec![InlineKeyboardButton::callback(
            audio_label,
            "settings:toggle:audio",
        )],
        // Row 4: Document toggle — full width
        vec![InlineKeyboardButton::callback(
            doc_label,
            "settings:toggle:document",
        )],
        // Row 5: Close button — full width
        vec![InlineKeyboardButton::callback(
            "✖️ Закрыть",
            "settings:close",
        )],
    ])
}

pub const SEARCH_PAGE_SIZE: usize = 5;

fn format_ph_search_text(query: &str, page: usize, total_pages: usize) -> String {
    if total_pages > 1 {
        format!(
            "Результаты поиска на Pornhub по запросу «{query}» (стр. {}/{}):",
            page + 1,
            total_pages
        )
    } else {
        format!("Результаты поиска на Pornhub по запросу «{query}»:")
    }
}

fn format_ym_search_text(query: &str, page: usize, total_pages: usize) -> String {
    if total_pages > 1 {
        format!(
            "Найдено несколько треков по запросу «{query}» (стр. {}/{}). Выбери нужный:",
            page + 1,
            total_pages
        )
    } else {
        format!("Найдено несколько треков по запросу «{query}». Выбери нужный:")
    }
}

fn format_yt_search_text(query: &str, page: usize, total_pages: usize) -> String {
    if total_pages > 1 {
        format!(
            "Результаты поиска на YouTube по запросу «{query}» (стр. {}/{}):",
            page + 1,
            total_pages
        )
    } else {
        format!("Результаты поиска на YouTube по запросу «{query}»:")
    }
}

fn format_sc_search_text(query: &str, page: usize, total_pages: usize) -> String {
    if total_pages > 1 {
        format!(
            "Результаты поиска на SoundCloud по запросу «{query}» (стр. {}/{}):",
            page + 1,
            total_pages
        )
    } else {
        format!("Результаты поиска на SoundCloud по запросу «{query}»:")
    }
}

fn build_track_selection_keyboard(
    search_id: u64,
    tracks: &[YandexTrackInfo],
    page: usize,
) -> InlineKeyboardMarkup {
    let total_tracks = tracks.len();
    let total_pages = (total_tracks + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages.saturating_sub(1));

    let start_idx = page * SEARCH_PAGE_SIZE;
    let end_idx = (start_idx + SEARCH_PAGE_SIZE).min(total_tracks);
    let page_tracks = &tracks[start_idx..end_idx];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = page_tracks
        .iter()
        .map(|track| {
            vec![InlineKeyboardButton::callback(
                track.button_label(),
                format!("ym:{}", track.id),
            )]
        })
        .collect();

    if total_pages > 1 {
        let mut nav_row = Vec::with_capacity(3);
        if page > 0 {
            nav_row.push(InlineKeyboardButton::callback(
                "⬅️ Назад",
                format!("ym:p:{search_id}:{}", page - 1),
            ));
        }

        nav_row.push(InlineKeyboardButton::callback(
            format!("{}/{}", page + 1, total_pages),
            format!("ym:noop:{search_id}:{page}"),
        ));

        if page + 1 < total_pages {
            nav_row.push(InlineKeyboardButton::callback(
                "Вперёд ➡️",
                format!("ym:p:{search_id}:{}", page + 1),
            ));
        }
        rows.push(nav_row);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена",
        "ym:cancel",
    )]);

    InlineKeyboardMarkup::new(rows)
}

fn build_ph_selection_keyboard(
    search_id: u64,
    videos: &[PornhubVideoInfo],
    page: usize,
) -> InlineKeyboardMarkup {
    let total_videos = videos.len();
    let total_pages = (total_videos + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages.saturating_sub(1));

    let start_idx = page * SEARCH_PAGE_SIZE;
    let end_idx = (start_idx + SEARCH_PAGE_SIZE).min(total_videos);
    let page_videos = &videos[start_idx..end_idx];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = page_videos
        .iter()
        .map(|video| {
            vec![InlineKeyboardButton::callback(
                video.button_label(),
                format!("ph:{}", video.vkey),
            )]
        })
        .collect();

    if total_pages > 1 {
        let mut nav_row = Vec::with_capacity(3);
        if page > 0 {
            nav_row.push(InlineKeyboardButton::callback(
                "⬅️ Назад",
                format!("ph:p:{search_id}:{}", page - 1),
            ));
        }

        nav_row.push(InlineKeyboardButton::callback(
            format!("{}/{}", page + 1, total_pages),
            format!("ph:noop:{search_id}:{page}"),
        ));

        if page + 1 < total_pages {
            nav_row.push(InlineKeyboardButton::callback(
                "Вперёд ➡️",
                format!("ph:p:{search_id}:{}", page + 1),
            ));
        }
        rows.push(nav_row);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена",
        "ph:cancel",
    )]);

    InlineKeyboardMarkup::new(rows)
}

fn build_yt_selection_keyboard(
    search_id: u64,
    videos: &[YouTubeVideoInfo],
    page: usize,
) -> InlineKeyboardMarkup {
    let total_videos = videos.len();
    let total_pages = (total_videos + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages.saturating_sub(1));

    let start_idx = page * SEARCH_PAGE_SIZE;
    let end_idx = (start_idx + SEARCH_PAGE_SIZE).min(total_videos);
    let page_videos = &videos[start_idx..end_idx];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = page_videos
        .iter()
        .map(|video| {
            vec![InlineKeyboardButton::callback(
                video.button_label(),
                format!("yt:{}", video.id),
            )]
        })
        .collect();

    if total_pages > 1 {
        let mut nav_row = Vec::with_capacity(3);
        if page > 0 {
            nav_row.push(InlineKeyboardButton::callback(
                "⬅️ Назад",
                format!("yt:p:{search_id}:{}", page - 1),
            ));
        }

        nav_row.push(InlineKeyboardButton::callback(
            format!("{}/{}", page + 1, total_pages),
            format!("yt:noop:{search_id}:{page}"),
        ));

        if page + 1 < total_pages {
            nav_row.push(InlineKeyboardButton::callback(
                "Вперёд ➡️",
                format!("yt:p:{search_id}:{}", page + 1),
            ));
        }
        rows.push(nav_row);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена",
        "yt:cancel",
    )]);

    InlineKeyboardMarkup::new(rows)
}

fn build_sc_selection_keyboard(
    search_id: u64,
    tracks: &[SoundcloudTrackInfo],
    page: usize,
) -> InlineKeyboardMarkup {
    let total_tracks = tracks.len();
    let total_pages = (total_tracks + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages.saturating_sub(1));

    let start_idx = page * SEARCH_PAGE_SIZE;
    let end_idx = (start_idx + SEARCH_PAGE_SIZE).min(total_tracks);
    let page_tracks = &tracks[start_idx..end_idx];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = page_tracks
        .iter()
        .enumerate()
        .map(|(offset, track)| {
            let item_idx = start_idx + offset;
            vec![InlineKeyboardButton::callback(
                track.button_label(),
                format!("sc:{search_id}:{item_idx}"),
            )]
        })
        .collect();

    if total_pages > 1 {
        let mut nav_row = Vec::with_capacity(3);
        if page > 0 {
            nav_row.push(InlineKeyboardButton::callback(
                "⬅️ Назад",
                format!("sc:p:{search_id}:{}", page - 1),
            ));
        }

        nav_row.push(InlineKeyboardButton::callback(
            format!("{}/{}", page + 1, total_pages),
            format!("sc:noop:{search_id}:{page}"),
        ));

        if page + 1 < total_pages {
            nav_row.push(InlineKeyboardButton::callback(
                "Вперёд ➡️",
                format!("sc:p:{search_id}:{}", page + 1),
            ));
        }
        rows.push(nav_row);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена",
        "sc:cancel",
    )]);

    InlineKeyboardMarkup::new(rows)
}


pub async fn process_and_send_media(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    user_id: UserId,
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    ym_quality: crate::settings::YandexQualityPreference,
    send_as_document: bool,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
) -> ResponseResult<()> {
    // For Yandex Music, differentiate AAC vs Lossless in cache using a quality tag in max_height
    let effective_cache_height = if crate::providers::yandex::is_yandex_music_url(url) {
        match ym_quality {
            crate::settings::YandexQualityPreference::Optimal => Some(1),
            crate::settings::YandexQualityPreference::Lossless => Some(2),
        }
    } else {
        max_height
    };

    // 1. Fast path: check Telegram file_id cache (0.1s instant response, bypasses queue)
    if let Some(cached) = cache.get(url, audio_only, effective_cache_height, send_as_document) {
        match cached {
            CachedMedia::Video(file_id) => {
                let url_id = cache.register_url(url);
                if send_as_document {
                    let mut req = bot
                        .send_document(chat_id, InputFile::file_id(file_id))
                        .disable_content_type_detection(true)
                        .reply_markup(build_audio_button(url_id));
                    if let Some(msg_id) = reply_to {
                        req = req.reply_parameters(ReplyParameters::new(msg_id));
                    }
                    req.await?;
                } else {
                    let mut req = bot
                        .send_video(chat_id, InputFile::file_id(file_id))
                        .supports_streaming(true)
                        .reply_markup(build_audio_button(url_id));
                    if let Some(msg_id) = reply_to {
                        req = req.reply_parameters(ReplyParameters::new(msg_id));
                    }
                    req.await?;
                }
                return Ok(());
            }
            CachedMedia::Audio(file_ids) => {
                for file_id in file_ids {
                    if send_as_document {
                        let mut req = bot
                            .send_document(chat_id, InputFile::file_id(file_id))
                            .disable_content_type_detection(true);
                        if let Some(msg_id) = reply_to {
                            req = req.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        req.await?;
                    } else {
                        let mut req = bot.send_audio(chat_id, InputFile::file_id(file_id));
                        if let Some(msg_id) = reply_to {
                            req = req.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        req.await?;
                    }
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
            "⏳ Очередь {} (позиция: {})...",
            service.name(),
            pos
        )
    } else {
        "⏳ Подготовка к загрузке...".to_string()
    };

    let mut status_req = bot.send_message(chat_id, status_text);
    if let Some(msg_id) = reply_to {
        status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
    }
    let status_msg = status_req.await?;

    // Lock domain & wait for global download slot
    let service_guard = queue.acquire(service, waiter_guard).await;

    let _ = bot
        .edit_message_text(chat_id, status_msg.id, format_progress_bar("📥 Скачивание", 0))
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

    let dl_progress = Arc::new(AtomicU8::new(0));
    let dl_shown_pct = Arc::new(AtomicU8::new(0));
    let dl_bot = bot.clone();
    let dl_msg_id = status_msg.id;
    let dl_prog_clone = Arc::clone(&dl_progress);
    let dl_shown_clone = Arc::clone(&dl_shown_pct);
    let dl_updater = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(400));
        interval.tick().await;
        let mut last_pct = 0u8;
        loop {
            interval.tick().await;
            let target_pct = dl_prog_clone.load(Ordering::Relaxed);
            let pct = target_pct.min(last_pct.saturating_add(35)).min(99);
            if pct != last_pct {
                last_pct = pct;
                dl_shown_clone.store(pct, Ordering::Relaxed);
                let _ = dl_bot
                    .edit_message_text(chat_id, dl_msg_id, format_progress_bar("📥 Скачивание", pct))
                    .await;
            }
        }
    });

    let media_result = fetch_media(
        url,
        audio_only,
        max_height,
        ym_quality,
        temp_dir.path(),
        config,
        &dl_progress,
    )
    .await;

    dl_updater.abort();

    let final_dl_pct = dl_progress.load(Ordering::Relaxed);
    if final_dl_pct > 0 {
        if dl_shown_pct.load(Ordering::Relaxed) == 0 {
            let _ = bot
                .edit_message_text(chat_id, status_msg.id, format_progress_bar("📥 Скачивание", 50))
                .await;
        }
        let _ = bot
            .edit_message_text(chat_id, status_msg.id, format_progress_bar("📥 Скачивание", final_dl_pct))
            .await;
    }

    // Release domain & global download slot immediately so next user can start downloading
    drop(service_guard);

    match media_result {
        Ok(MediaOutput::Audio(tracks)) => {
            let total_files_bytes: u64 = tracks
                .iter()
                .filter_map(|t| std::fs::metadata(&t.path).ok().map(|m| m.len()))
                .sum();

            let total_tracks = tracks.len();
            let _ = bot
                .edit_message_text(chat_id, status_msg.id, format_progress_bar("📤 Отправка", 0))
                .await;

            let start_tx = read_network_tx_bytes();
            let ul_shown_pct = Arc::new(AtomicU8::new(0));
            let ul_shown_clone = Arc::clone(&ul_shown_pct);
            let ul_bot = bot.clone();
            let ul_msg_id = status_msg.id;
            let ul_updater = tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(300));
                interval.tick().await;
                let mut last_pct = 0u8;
                loop {
                    interval.tick().await;
                    if total_files_bytes > 0 {
                        let current_tx = read_network_tx_bytes();
                        let sent = current_tx.saturating_sub(start_tx);
                        let raw_pct = (sent as f64 / (total_files_bytes as f64 * 1.05)) * 100.0;
                        let tx_pct = raw_pct.round().clamp(0.0, 95.0) as u8;
                        let pct = if total_files_bytes < 15 * 1024 * 1024 {
                            // Small audio files (e.g. 2-5 MB SoundCloud MP3s) are absorbed by OS TCP buffers
                            // almost immediately while Telegram DC processes sendAudio; step smoothly.
                            last_pct.saturating_add(28).min(92)
                        } else {
                            let floor_pct = last_pct.saturating_add(8).min(25);
                            tx_pct.max(floor_pct).min(last_pct.saturating_add(25)).min(95)
                        };
                        if pct != last_pct {
                            last_pct = pct;
                            ul_shown_clone.store(pct, Ordering::Relaxed);
                            let _ = ul_bot
                                .edit_message_text(
                                    chat_id,
                                    ul_msg_id,
                                    format_progress_bar("📤 Отправка", pct),
                                )
                                .await;
                        }
                    }
                }
            });

            let mut uploaded_ids = Vec::with_capacity(tracks.len());
            let mut total_size: u64 = 0;

            for (idx, track) in tracks.iter().enumerate() {
                if total_tracks > 1 {
                    let track_pct = ((idx as f64 / total_tracks as f64) * 100.0).round().clamp(0.0, 99.0) as u8;
                    ul_shown_pct.store(track_pct, Ordering::Relaxed);
                    let _ = bot
                        .edit_message_text(
                            chat_id,
                            status_msg.id,
                            format_progress_bar(&format!("📤 Отправка [{}/{}]", idx + 1, total_tracks), track_pct),
                        )
                        .await;
                }

                if send_as_document {
                    let mut input_file = InputFile::file(&track.path);
                    let ext = track.path.extension().and_then(|s| s.to_str()).unwrap_or("mp3");
                    let custom_name = match (&track.performer, &track.title) {
                        (Some(p), Some(t)) => Some(format!("{p} - {t}.{ext}")),
                        (None, Some(t)) => Some(format!("{t}.{ext}")),
                        _ => None,
                    };
                    if let Some(name) = custom_name {
                        input_file = input_file.file_name(name);
                    }

                    let mut request = bot
                        .send_document(chat_id, input_file)
                        .disable_content_type_detection(true);

                    if let Some(msg_id) = reply_to {
                        request = request.reply_parameters(ReplyParameters::new(msg_id));
                    }
                    if let Some(cover_path) = &track.cover_path {
                        request = request.thumbnail(InputFile::file(cover_path));
                    }

                    match request.await {
                        Ok(sent_msg) => {
                            if let Some(doc) = sent_msg.document() {
                                total_size = total_size.saturating_add(u64::from(doc.file.size));
                                uploaded_ids.push(doc.file.id.clone());
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to send audio document {:?}: {:?}", track.path, e);
                        }
                    }
                } else {
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
            }

            ul_updater.abort();

            if ul_shown_pct.load(Ordering::Relaxed) == 0 {
                let _ = bot
                    .edit_message_text(chat_id, status_msg.id, format_progress_bar("📤 Отправка", 50))
                    .await;
            }

            let _ = bot
                .edit_message_text(chat_id, status_msg.id, format_progress_bar("📤 Отправка", 100))
                .await;

            if !uploaded_ids.is_empty() {
                cache.insert(url, true, effective_cache_height, send_as_document, CachedMedia::Audio(uploaded_ids), total_size);
            }

            let _ = bot.delete_message(chat_id, status_msg.id).await;
        }
        Ok(MediaOutput::Video(file_path)) => {
            let file_size = tokio::fs::metadata(&file_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);

            let _ = bot
                .edit_message_text(chat_id, status_msg.id, format_progress_bar("📤 Отправка", 0))
                .await;

            let start_tx = read_network_tx_bytes();
            let ul_bot = bot.clone();
            let ul_msg_id = status_msg.id;
            let ul_updater = tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(350));
                interval.tick().await;
                let mut last_pct = 0u8;
                loop {
                    interval.tick().await;
                    if file_size > 0 {
                        let current_tx = read_network_tx_bytes();
                        let sent = current_tx.saturating_sub(start_tx);
                        let raw_pct = (sent as f64 / (file_size as f64 * 1.05)) * 100.0;
                        let tx_pct = raw_pct.round().clamp(0.0, 95.0) as u8;
                        let pct = if file_size < 15 * 1024 * 1024 {
                            last_pct.saturating_add(28).min(92)
                        } else {
                            let floor_pct = last_pct.saturating_add(8).min(25);
                            tx_pct.max(floor_pct).min(last_pct.saturating_add(25)).min(95)
                        };
                        if pct != last_pct {
                            last_pct = pct;
                            let _ = ul_bot
                                .edit_message_text(
                                    chat_id,
                                    ul_msg_id,
                                    format_progress_bar("📤 Отправка", pct),
                                )
                                .await;
                        }
                    }
                }
            });

            let url_id = cache.register_url(url);
            let send_res = if send_as_document {
                let mut request = bot
                    .send_document(chat_id, InputFile::file(&file_path))
                    .disable_content_type_detection(true)
                    .reply_markup(build_audio_button(url_id));

                if let Some(msg_id) = reply_to {
                    request = request.reply_parameters(ReplyParameters::new(msg_id));
                }
                request.await
            } else {
                let video_dims = get_video_dimensions(&file_path).await;
                let input_file = InputFile::file(&file_path);
                let mut request = bot
                    .send_video(chat_id, input_file)
                    .supports_streaming(true)
                    .reply_markup(build_audio_button(url_id));

                if let Some((w, h)) = video_dims {
                    request = request.width(w).height(h);
                }

                if let Some(msg_id) = reply_to {
                    request = request.reply_parameters(ReplyParameters::new(msg_id));
                }
                request.await
            };

            ul_updater.abort();

            match send_res {
                Ok(sent_msg) => {
                    if send_as_document {
                        if let Some(doc) = sent_msg.document() {
                            cache.insert(
                                url,
                                false,
                                max_height,
                                send_as_document,
                                CachedMedia::Video(doc.file.id.clone()),
                                u64::from(doc.file.size),
                            );
                        }
                    } else if let Some(video) = sent_msg.video() {
                        cache.insert(
                            url,
                            false,
                            max_height,
                            send_as_document,
                            CachedMedia::Video(video.file.id.clone()),
                            u64::from(video.file.size),
                        );
                    }
                    let _ = bot.delete_message(chat_id, status_msg.id).await;
                }
                Err(e) => {
                    log::error!("Failed to send video {:?}: {:?}", file_path, e);
                    let err_str = e.to_string();
                    let mut recovered = false;

                    if err_str.contains("Request Entity Too Large") && config.max_filesize_mb > 49 {
                        log::warn!(
                            "Telegram rejected video with Request Entity Too Large; retrying {} with 49MB limit",
                            url
                        );
                        let _ = tokio::fs::remove_file(&file_path).await;
                        let mut fallback_cfg = (**config).clone();
                        fallback_cfg.max_filesize_mb = 49;
                        dl_progress.store(0, Ordering::Relaxed);
                        let _ = bot
                            .edit_message_text(
                                chat_id,
                                status_msg.id,
                                format_progress_bar("📥 Сжатие до 49 МБ", 0),
                            )
                            .await;

                        if let Ok(MediaOutput::Video(smaller_path)) = fetch_media(
                            url,
                            false,
                            Some(480),
                            ym_quality,
                            temp_dir.path(),
                            &fallback_cfg,
                            &dl_progress,
                        )
                        .await
                        {
                            let _ = bot
                                .edit_message_text(
                                    chat_id,
                                    status_msg.id,
                                    format_progress_bar("📤 Отправка", 50),
                                )
                                .await;
                            let retry_res = if send_as_document {
                                let mut retry_req = bot
                                    .send_document(chat_id, InputFile::file(&smaller_path))
                                    .disable_content_type_detection(true)
                                    .reply_markup(build_audio_button(url_id));
                                if let Some(msg_id) = reply_to {
                                    retry_req = retry_req.reply_parameters(ReplyParameters::new(msg_id));
                                }
                                retry_req.await
                            } else {
                                let smaller_dims = get_video_dimensions(&smaller_path).await;
                                let mut retry_req = bot
                                    .send_video(chat_id, InputFile::file(&smaller_path))
                                    .supports_streaming(true)
                                    .reply_markup(build_audio_button(url_id));
                                if let Some((w, h)) = smaller_dims {
                                    retry_req = retry_req.width(w).height(h);
                                }
                                if let Some(msg_id) = reply_to {
                                    retry_req = retry_req.reply_parameters(ReplyParameters::new(msg_id));
                                }
                                retry_req.await
                            };

                            if let Ok(sent_msg) = retry_res {
                                if send_as_document {
                                    if let Some(doc) = sent_msg.document() {
                                        cache.insert(
                                            url,
                                            false,
                                            max_height,
                                            send_as_document,
                                            CachedMedia::Video(doc.file.id.clone()),
                                            u64::from(doc.file.size),
                                        );
                                    }
                                } else if let Some(video) = sent_msg.video() {
                                    cache.insert(
                                        url,
                                        false,
                                        max_height,
                                        send_as_document,
                                        CachedMedia::Video(video.file.id.clone()),
                                        u64::from(video.file.size),
                                    );
                                }
                                let _ = bot.delete_message(chat_id, status_msg.id).await;
                                recovered = true;
                            }
                        }
                    }

                    if !recovered {
                        let _ = bot
                            .edit_message_text(
                                chat_id,
                                status_msg.id,
                                format!("Ошибка отправки видео в Telegram: {e}"),
                            )
                            .await;
                    }
                }
            }
        }
        Err(err) => {
            bot.edit_message_text(chat_id, status_msg.id, format!("Ошибка: {err}"))
                .await?;
        }
    }

    Ok(())
}

async fn handle_yandex_search(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    user_id: UserId,
    query: &str,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
    settings: &UserSettingsStore,
) -> ResponseResult<()> {
    if config.yandex_music_token.is_none() {
        let mut req = bot.send_message(
            chat_id,
            "Поиск недоступен: YANDEX_MUSIC_TOKEN не настроен в .env файле.",
        );
        if let Some(msg_id) = reply_to {
            req = req.reply_parameters(ReplyParameters::new(msg_id));
        }
        req.await?;
        return Ok(());
    }

    let mut status_req = bot.send_message(chat_id, format!("🔍 Ищу на Яндекс Музыке: «{query}»..."));
    if let Some(msg_id) = reply_to {
        status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
    }
    let status_msg = status_req.await?;

    match search_yandex_tracks(query, config.yandex_music_token.as_deref()).await {
        Ok(tracks) if tracks.is_empty() => {
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("По запросу «{query}» ничего не найдено на Яндекс Музыке."),
                )
                .await;
        }
        Ok(tracks) if tracks.len() == 1 => {
            let url = tracks[0].canonical_url();
            let _ = bot.delete_message(chat_id, status_msg.id).await;
            let user_settings = settings.get(user_id);
            process_and_send_media(
                bot,
                chat_id,
                reply_to,
                user_id,
                &url,
                true,
                None,
                user_settings.ym_quality,
                user_settings.send_as_document,
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
                .edit_message_text(chat_id, status_msg.id, text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await;
        }
        Err(e) => {
            log::error!("Yandex Music search error for query {:?}: {:?}", query, e);
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("Ошибка при поиске на Яндекс Музыке: {e}"),
                )
                .await;
        }
    }

    Ok(())
}

async fn handle_soundcloud_search(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    user_id: UserId,
    query: &str,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
    settings: &UserSettingsStore,
) -> ResponseResult<()> {
    let mut status_req = bot.send_message(chat_id, format!("🔍 Ищу на SoundCloud: «{query}»..."));
    if let Some(msg_id) = reply_to {
        status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
    }
    let status_msg = status_req.await?;

    match search_soundcloud_tracks(query).await {
        Ok(tracks) if tracks.is_empty() => {
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("По запросу «{query}» ничего не найдено на SoundCloud."),
                )
                .await;
        }
        Ok(tracks) if tracks.len() == 1 => {
            let url = tracks[0].url.clone();
            let _ = bot.delete_message(chat_id, status_msg.id).await;
            let user_settings = settings.get(user_id);
            process_and_send_media(
                bot,
                chat_id,
                reply_to,
                user_id,
                &url,
                true,
                None,
                user_settings.ym_quality,
                user_settings.send_as_document,
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
                .edit_message_text(chat_id, status_msg.id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(e) => {
            log::error!("SoundCloud search error for query {:?}: {:?}", query, e);
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("Ошибка при поиске на SoundCloud: {e}"),
                )
                .await;
        }
    }

    Ok(())
}

async fn handle_youtube_search(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    query: &str,
    cache: &MediaCache,
) -> ResponseResult<()> {
    let mut status_req = bot.send_message(chat_id, format!("🔍 Ищу на YouTube: «{query}»..."));
    if let Some(msg_id) = reply_to {
        status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
    }
    let status_msg = status_req.await?;

    match search_youtube_videos(query).await {
        Ok(videos) if videos.is_empty() => {
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("По запросу «{query}» ничего не найдено на YouTube."),
                )
                .await;
        }
        Ok(videos) => {
            let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
            let total_pages = total_pages.max(1);
            let search_id = cache.store_yt_search(query.to_string(), videos.clone());
            let text = format_yt_search_text(query, 0, total_pages);
            let keyboard = build_yt_selection_keyboard(search_id, &videos, 0);

            let _ = bot
                .edit_message_text(chat_id, status_msg.id, text)
                .reply_markup(keyboard)
                .await;
        }
        Err(e) => {
            log::error!("YouTube search error for query {:?}: {:?}", query, e);
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("Ошибка при поиске на YouTube: {e}"),
                )
                .await;
        }
    }

    Ok(())
}

async fn handle_pornhub_search(
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

            let mut req = bot
                .send_message(chat_id, text)
                .reply_markup(keyboard);
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

        if trimmed == "/settings" || trimmed.starts_with("/settings@") {
            let user_settings = settings.get(user_id);
            bot.send_message(msg.chat.id, build_settings_text(&user_settings))
                .parse_mode(ParseMode::Html)
                .reply_markup(build_settings_keyboard(&user_settings))
                .reply_parameters(ReplyParameters::new(msg.id))
                .await?;
            return Ok(());
        }

        if let Some(query) = extract_yt_search_query(trimmed) {
            handle_youtube_search(
                &bot,
                msg.chat.id,
                Some(msg.id),
                query,
                &cache,
            )
            .await?;
            return Ok(());
        }

        if let Some(query) = extract_sc_search_query(trimmed) {
            handle_soundcloud_search(
                &bot,
                msg.chat.id,
                Some(msg.id),
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

        if let Some(url) = extract_url(trimmed) {
            let user_settings = settings.get(user_id);
            let audio_only = is_audio_request(url, trimmed, user_settings.audio_only);

            // If user prefers manual quality selection and it is not an audio request or Yandex Music
            if !audio_only
                && user_settings.quality == QualityPreference::AskEveryTime
                && !crate::providers::yandex::is_yandex_music_url(url)
            {
                let url_id = cache.register_url(url);
                bot.send_message(msg.chat.id, "Выбери желаемое качество для этого видео:")
                    .reply_markup(build_quality_selection_keyboard(url_id))
                    .reply_parameters(ReplyParameters::new(msg.id))
                    .await?;
            } else {
                process_and_send_media(
                    &bot,
                    msg.chat.id,
                    Some(msg.id),
                    user_id,
                    url,
                    audio_only,
                    None,
                    user_settings.ym_quality,
                    user_settings.send_as_document,
                    &config,
                    &queue,
                    &cache,
                )
                .await?;
            }
        } else if let Some(query) = extract_search_query(trimmed, msg.chat.is_private()) {
            handle_yandex_search(
                &bot,
                msg.chat.id,
                Some(msg.id),
                user_id,
                query,
                &config,
                &queue,
                &cache,
                &settings,
            )
            .await?;
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

pub async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    config: Arc<Config>,
    queue: QueueManager,
    cache: MediaCache,
    settings: UserSettingsStore,
) -> ResponseResult<()> {
    let Some(data) = q.data.as_deref() else {
        return Ok(());
    };

    if let Some(sub) = data.strip_prefix("settings:") {
        if sub == "close" {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
            return Ok(());
        }

        let user_id = q.from.id;
        let updated = match sub {
            "toggle:quality" | "q:auto" | "q:manual" => settings.toggle_quality(user_id),
            "toggle:ym_quality" | "ym:optimal" | "ym:lossless" => settings.toggle_ym_quality(user_id),
            "toggle:audio" => settings.toggle_audio_only(user_id),
            "toggle:document" => settings.toggle_send_as_document(user_id),
            _ => settings.get(user_id),
        };

        let alert_text = match sub {
            "toggle:quality" | "q:auto" | "q:manual" => match updated.quality {
                QualityPreference::AutoBest => "Качество видео: Авто (максимальное)",
                QualityPreference::AskEveryTime => "Качество видео: Вручную перед скачиванием",
            },
            "toggle:ym_quality" | "ym:optimal" | "ym:lossless" => match updated.ym_quality {
                crate::settings::YandexQualityPreference::Optimal => "Яндекс Музыка: AAC 192 kbps ⚡ (быстро)",
                crate::settings::YandexQualityPreference::Lossless => "Яндекс Музыка: FLAC 💎 (lossless)",
            },
            "toggle:audio" => {
                if updated.audio_only {
                    "Режим «Только аудио» ВКЛЮЧЁН"
                } else {
                    "Режим «Только аудио» ВЫКЛЮЧЕН"
                }
            }
            "toggle:document" => {
                if updated.send_as_document {
                    "Отправка файлом (без сжатия) ВКЛЮЧЕНА"
                } else {
                    "Отправка файлом ВЫКЛЮЧЕНА"
                }
            }
            _ => "Настройки обновлены",
        };

        let _ = bot.answer_callback_query(q.id.clone()).text(alert_text).await;

        if let Some(msg) = q.regular_message() {
            let _ = bot
                .edit_message_text(msg.chat.id, msg.id, build_settings_text(&updated))
                .parse_mode(ParseMode::Html)
                .reply_markup(build_settings_keyboard(&updated))
                .await;
        }
        return Ok(());
    }

    if let Some(id_str) = data.strip_prefix("audio:") {
        let Ok(url_id) = id_str.parse::<u64>() else {
            return Ok(());
        };

        let Some(url) = cache.resolve_url(url_id) else {
            bot.answer_callback_query(q.id)
                .text("Ссылка устарела, отправь её заново")
                .await?;
            return Ok(());
        };

        let _ = bot
            .answer_callback_query(q.id.clone())
            .text("🎵 Извлекаю аудио...")
            .await;

        if let Some(msg) = q.regular_message() {
            let chat_id = msg.chat.id;
            let msg_id = msg.id;

            // Remove the inline button so it isn't clicked repeatedly
            let _ = bot.edit_message_reply_markup(chat_id, msg_id).await;

            let user_settings = settings.get(q.from.id);
            process_and_send_media(
                &bot,
                chat_id,
                Some(msg_id),
                q.from.id,
                &url,
                true,
                None,
                user_settings.ym_quality,
                user_settings.send_as_document,
                &config,
                &queue,
                &cache,
            )
            .await?;
        }
    } else if let Some(q_payload) = data.strip_prefix("q:") {
        if q_payload == "cancel" {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
            return Ok(());
        }

        // Format is q:<height>:<url_id>
        let parts: Vec<&str> = q_payload.split(':').collect();
        if parts.len() == 2 {
            let Ok(height) = parts[0].parse::<u16>() else {
                return Ok(());
            };
            let Ok(url_id) = parts[1].parse::<u64>() else {
                return Ok(());
            };

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
                process_and_send_media(
                    &bot,
                    chat_id,
                    reply_to,
                    q.from.id,
                    &url,
                    false,
                    Some(height),
                    user_settings.ym_quality,
                    user_settings.send_as_document,
                    &config,
                    &queue,
                    &cache,
                )
                .await?;
            }
        }
    } else if let Some(ph_payload) = data.strip_prefix("ph:") {
        if ph_payload == "cancel" {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
            return Ok(());
        }

        if let Some(rest) = ph_payload.strip_prefix("p:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((query, videos)) = cache.get_ph_search(search_id) {
                        let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_ph_search_text(&query, page_idx, total_pages);
                        let keyboard = build_ph_selection_keyboard(search_id, &videos, page_idx);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot
                                .edit_message_text(msg.chat.id, msg.id, text)
                                .reply_markup(keyboard)
                                .await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                        return Ok(());
                    } else {
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text("Результаты поиска устарели, выполни поиск заново.")
                            .await;
                        return Ok(());
                    }
                }
            }
            return Ok(());
        }

        if let Some(rest) = ph_payload.strip_prefix("noop:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((_, videos)) = cache.get_ph_search(search_id) {
                        let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text(format!("Страница {} из {}", page_idx + 1, total_pages))
                            .await;
                        return Ok(());
                    }
                }
            }
            let _ = bot.answer_callback_query(q.id.clone()).await;
            return Ok(());
        }

        let url = format!("https://www.pornhub.com/view_video.php?viewkey={ph_payload}");
        let user_settings = settings.get(q.from.id);

        if let Some(msg) = q.regular_message() {
            let chat_id = msg.chat.id;
            let reply_to = msg.reply_to_message().map(|m| m.id);

            if user_settings.audio_only {
                let _ = bot
                    .answer_callback_query(q.id.clone())
                    .text("🎵 Скачиваю аудио...")
                    .await;
                let _ = bot.delete_message(chat_id, msg.id).await;

                process_and_send_media(
                    &bot,
                    chat_id,
                    reply_to,
                    q.from.id,
                    &url,
                    true,
                    None,
                    user_settings.ym_quality,
                    user_settings.send_as_document,
                    &config,
                    &queue,
                    &cache,
                )
                .await?;
            } else if user_settings.quality == QualityPreference::AskEveryTime {
                let _ = bot.answer_callback_query(q.id.clone()).await;
                let url_id = cache.register_url(&url);

                let _ = bot
                    .edit_message_text(chat_id, msg.id, "Выбери желаемое качество для этого видео:")
                    .reply_markup(build_quality_selection_keyboard(url_id))
                    .await;
            } else {
                let _ = bot
                    .answer_callback_query(q.id.clone())
                    .text("Скачиваю выбранное видео...")
                    .await;
                let _ = bot.delete_message(chat_id, msg.id).await;

                process_and_send_media(
                    &bot,
                    chat_id,
                    reply_to,
                    q.from.id,
                    &url,
                    false,
                    None,
                    user_settings.ym_quality,
                    user_settings.send_as_document,
                    &config,
                    &queue,
                    &cache,
                )
                .await?;
            }
        }
    } else if let Some(ym_payload) = data.strip_prefix("ym:") {
        if ym_payload == "cancel" {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
            return Ok(());
        }

        if let Some(rest) = ym_payload.strip_prefix("p:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((query, tracks)) = cache.get_ym_search(search_id) {
                        let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_ym_search_text(&query, page_idx, total_pages);
                        let keyboard = build_track_selection_keyboard(search_id, &tracks, page_idx);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot
                                .edit_message_text(msg.chat.id, msg.id, text)
                                .reply_markup(keyboard)
                                .await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                        return Ok(());
                    } else {
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text("Результаты поиска устарели, выполни поиск заново.")
                            .await;
                        return Ok(());
                    }
                }
            }
            return Ok(());
        }

        if let Some(rest) = ym_payload.strip_prefix("noop:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((_, tracks)) = cache.get_ym_search(search_id) {
                        let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text(format!("Страница {} из {}", page_idx + 1, total_pages))
                            .await;
                        return Ok(());
                    }
                }
            }
            let _ = bot.answer_callback_query(q.id.clone()).await;
            return Ok(());
        }

        if ym_payload.is_empty() || !ym_payload.chars().all(|c| c.is_ascii_digit()) {
            return Ok(());
        }

        let _ = bot
            .answer_callback_query(q.id.clone())
            .text("Скачиваю выбранный трек...")
            .await;

        if let Some(msg) = q.regular_message() {
            let chat_id = msg.chat.id;
            let reply_to = msg.reply_to_message().map(|m| m.id);
            let _ = bot.delete_message(chat_id, msg.id).await;

            let url = format!("https://music.yandex.ru/track/{ym_payload}");
            let user_settings = settings.get(q.from.id);
            process_and_send_media(
                &bot,
                chat_id,
                reply_to,
                q.from.id,
                &url,
                true,
                None,
                user_settings.ym_quality,
                user_settings.send_as_document,
                &config,
                &queue,
                &cache,
            )
            .await?;
        }
    } else if let Some(yt_payload) = data.strip_prefix("yt:") {
        if yt_payload == "cancel" {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
            return Ok(());
        }

        if let Some(rest) = yt_payload.strip_prefix("p:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((query, videos)) = cache.get_yt_search(search_id) {
                        let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_yt_search_text(&query, page_idx, total_pages);
                        let keyboard = build_yt_selection_keyboard(search_id, &videos, page_idx);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot
                                .edit_message_text(msg.chat.id, msg.id, text)
                                .reply_markup(keyboard)
                                .await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                        return Ok(());
                    } else {
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text("Результаты поиска устарели, выполни поиск заново.")
                            .await;
                        return Ok(());
                    }
                }
            }
            return Ok(());
        }

        if let Some(rest) = yt_payload.strip_prefix("noop:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((_, videos)) = cache.get_yt_search(search_id) {
                        let total_pages = (videos.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text(format!("Страница {} из {}", page_idx + 1, total_pages))
                            .await;
                        return Ok(());
                    }
                }
            }
            let _ = bot.answer_callback_query(q.id.clone()).await;
            return Ok(());
        }

        if let Some(msg) = q.regular_message() {
            let chat_id = msg.chat.id;
            let reply_to = msg.reply_to_message().map(|m| m.id);
            let user_settings = settings.get(q.from.id);
            let url = format!("https://www.youtube.com/watch?v={yt_payload}");

            if user_settings.audio_only {
                let _ = bot
                    .answer_callback_query(q.id.clone())
                    .text("🎵 Скачиваю аудио...")
                    .await;
                let _ = bot.delete_message(chat_id, msg.id).await;

                process_and_send_media(
                    &bot,
                    chat_id,
                    reply_to,
                    q.from.id,
                    &url,
                    true,
                    None,
                    user_settings.ym_quality,
                    user_settings.send_as_document,
                    &config,
                    &queue,
                    &cache,
                )
                .await?;
            } else if user_settings.quality == QualityPreference::AskEveryTime {
                let _ = bot.answer_callback_query(q.id.clone()).await;
                let url_id = cache.register_url(&url);

                let _ = bot
                    .edit_message_text(chat_id, msg.id, "Выбери желаемое качество для этого видео:")
                    .reply_markup(build_quality_selection_keyboard(url_id))
                    .await;
            } else {
                let _ = bot
                    .answer_callback_query(q.id.clone())
                    .text("Скачиваю выбранное видео...")
                    .await;
                let _ = bot.delete_message(chat_id, msg.id).await;

                process_and_send_media(
                    &bot,
                    chat_id,
                    reply_to,
                    q.from.id,
                    &url,
                    false,
                    None,
                    user_settings.ym_quality,
                    user_settings.send_as_document,
                    &config,
                    &queue,
                    &cache,
                )
                .await?;
            }
        }
    } else if let Some(sc_payload) = data.strip_prefix("sc:") {
        if sc_payload == "cancel" {
            let _ = bot.answer_callback_query(q.id.clone()).await;
            if let Some(msg) = q.regular_message() {
                let _ = bot.delete_message(msg.chat.id, msg.id).await;
            }
            return Ok(());
        }

        if let Some(rest) = sc_payload.strip_prefix("p:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((query, tracks)) = cache.get_sc_search(search_id) {
                        let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let text = format_sc_search_text(&query, page_idx, total_pages);
                        let keyboard = build_sc_selection_keyboard(search_id, &tracks, page_idx);
                        if let Some(msg) = q.regular_message() {
                            let _ = bot
                                .edit_message_text(msg.chat.id, msg.id, text)
                                .reply_markup(keyboard)
                                .await;
                        }
                        let _ = bot.answer_callback_query(q.id.clone()).await;
                        return Ok(());
                    } else {
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text("Результаты поиска устарели, выполни поиск заново.")
                            .await;
                        return Ok(());
                    }
                }
            }
            return Ok(());
        }

        if let Some(rest) = sc_payload.strip_prefix("noop:") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(search_id), Ok(page_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    if let Some((_, tracks)) = cache.get_sc_search(search_id) {
                        let total_pages = (tracks.len() + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE;
                        let total_pages = total_pages.max(1);
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text(format!("Страница {} из {}", page_idx + 1, total_pages))
                            .await;
                        return Ok(());
                    }
                }
            }
            let _ = bot.answer_callback_query(q.id.clone()).await;
            return Ok(());
        }

        let parts: Vec<&str> = sc_payload.split(':').collect();
        if parts.len() == 2 {
            if let (Ok(search_id), Ok(item_idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                if let Some((_, tracks)) = cache.get_sc_search(search_id) {
                    if let Some(track) = tracks.get(item_idx) {
                        let track_url = track.url.clone();
                        let _ = bot
                            .answer_callback_query(q.id.clone())
                            .text("Скачиваю трек с SoundCloud...")
                            .await;

                        if let Some(msg) = q.regular_message() {
                            let chat_id = msg.chat.id;
                            let reply_to = msg.reply_to_message().map(|m| m.id);
                            let _ = bot.delete_message(chat_id, msg.id).await;

                            let user_settings = settings.get(q.from.id);
                            process_and_send_media(
                                &bot,
                                chat_id,
                                reply_to,
                                q.from.id,
                                &track_url,
                                true,
                                None,
                                user_settings.ym_quality,
                                user_settings.send_as_document,
                                &config,
                                &queue,
                                &cache,
                            )
                            .await?;
                        }
                        return Ok(());
                    }
                }
            }
        }

        let _ = bot
            .answer_callback_query(q.id)
            .text("Трек устарел, выполни поиск заново")
            .await;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_search_texts() {
        assert_eq!(
            format_ph_search_text("test", 0, 1),
            "Результаты поиска на Pornhub по запросу «test»:"
        );
        assert_eq!(
            format_ph_search_text("test", 1, 3),
            "Результаты поиска на Pornhub по запросу «test» (стр. 2/3):"
        );
        assert_eq!(
            format_ym_search_text("test", 0, 1),
            "Найдено несколько треков по запросу «test». Выбери нужный:"
        );
        assert_eq!(
            format_ym_search_text("test", 0, 2),
            "Найдено несколько треков по запросу «test» (стр. 1/2). Выбери нужный:"
        );
    }

    #[test]
    fn test_build_track_selection_keyboard_single_page() {
        let tracks: Vec<YandexTrackInfo> = (1..=3)
            .map(|i| YandexTrackInfo {
                id: i.to_string(),
                title: format!("Song {i}"),
                version: None,
                artists: vec!["Artist".to_string()],
                duration_ms: Some(180000),
            })
            .collect();

        let kb = build_track_selection_keyboard(123, &tracks, 0);
        // 3 tracks + 1 cancel row
        assert_eq!(kb.inline_keyboard.len(), 4);
    }

    #[test]
    fn test_build_track_selection_keyboard_pagination() {
        let tracks: Vec<YandexTrackInfo> = (1..=12)
            .map(|i| YandexTrackInfo {
                id: i.to_string(),
                title: format!("Song {i}"),
                version: None,
                artists: vec!["Artist".to_string()],
                duration_ms: Some(180000),
            })
            .collect();

        // Page 0 (out of 3 pages: 5, 5, 2)
        let kb_p0 = build_track_selection_keyboard(123, &tracks, 0);
        // 5 tracks + 1 nav row + 1 cancel row = 7 rows
        assert_eq!(kb_p0.inline_keyboard.len(), 7);
        let nav_row_p0 = &kb_p0.inline_keyboard[5];
        assert_eq!(nav_row_p0.len(), 2);
        assert_eq!(nav_row_p0[0].text, "1/3");
        assert_eq!(nav_row_p0[1].text, "Вперёд ➡️");

        // Page 1 (middle page)
        let kb_p1 = build_track_selection_keyboard(123, &tracks, 1);
        assert_eq!(kb_p1.inline_keyboard.len(), 7);
        let nav_row_p1 = &kb_p1.inline_keyboard[5];
        assert_eq!(nav_row_p1.len(), 3);
        assert_eq!(nav_row_p1[0].text, "⬅️ Назад");
        assert_eq!(nav_row_p1[1].text, "2/3");
        assert_eq!(nav_row_p1[2].text, "Вперёд ➡️");

        // Page 2 (last page, 2 items)
        let kb_p2 = build_track_selection_keyboard(123, &tracks, 2);
        assert_eq!(kb_p2.inline_keyboard.len(), 4);
        let nav_row_p2 = &kb_p2.inline_keyboard[2];
        assert_eq!(nav_row_p2.len(), 2);
        assert_eq!(nav_row_p2[0].text, "⬅️ Назад");
        assert_eq!(nav_row_p2[1].text, "3/3");
    }

    #[test]
    fn test_format_yt_sc_search_texts() {
        assert_eq!(
            format_yt_search_text("Queen", 0, 1),
            "Результаты поиска на YouTube по запросу «Queen»:"
        );
        assert_eq!(
            format_yt_search_text("Queen", 1, 4),
            "Результаты поиска на YouTube по запросу «Queen» (стр. 2/4):"
        );
        assert_eq!(
            format_sc_search_text("Hans Zimmer", 0, 1),
            "Результаты поиска на SoundCloud по запросу «Hans Zimmer»:"
        );
        assert_eq!(
            format_sc_search_text("Hans Zimmer", 2, 5),
            "Результаты поиска на SoundCloud по запросу «Hans Zimmer» (стр. 3/5):"
        );
    }

    #[test]
    fn test_build_yt_selection_keyboard() {
        let videos: Vec<YouTubeVideoInfo> = (1..=7)
            .map(|i| YouTubeVideoInfo {
                id: format!("yt_{i}"),
                title: format!("Video {i}"),
                uploader: Some("Channel".to_string()),
                duration_str: Some("3:00".to_string()),
                url: format!("https://www.youtube.com/watch?v=yt_{i}"),
            })
            .collect();

        let kb = build_yt_selection_keyboard(42, &videos, 0);
        // 5 videos + 1 nav row + 1 cancel row = 7 rows
        assert_eq!(kb.inline_keyboard.len(), 7);
        let nav_row = &kb.inline_keyboard[5];
        assert_eq!(nav_row.len(), 2);
        assert_eq!(nav_row[0].text, "1/2");
        assert_eq!(nav_row[1].text, "Вперёд ➡️");
    }

    #[test]
    fn test_build_sc_selection_keyboard() {
        let tracks: Vec<SoundcloudTrackInfo> = (1..=7)
            .map(|i| SoundcloudTrackInfo {
                id: format!("sc_{i}"),
                title: format!("Track {i}"),
                uploader: Some("Artist".to_string()),
                duration_str: Some("3:30".to_string()),
                url: format!("https://soundcloud.com/artist/track_{i}"),
            })
            .collect();

        let kb = build_sc_selection_keyboard(99, &tracks, 1);
        // 2 tracks (page 1) + 1 nav row + 1 cancel row = 4 rows
        assert_eq!(kb.inline_keyboard.len(), 4);
        let nav_row = &kb.inline_keyboard[2];
        assert_eq!(nav_row.len(), 2);
        assert_eq!(nav_row[0].text, "⬅️ Назад");
        assert_eq!(nav_row[1].text, "2/2");
    }

    #[test]
    fn test_build_settings_keyboard_and_text() {
        let mut settings = UserSettings::default();
        let kb = build_settings_keyboard(&settings);
        // 5 rows: Video (1 col), YM (1 col), Audio toggle (1 col), Document toggle (1 col), Close (1 col)
        assert_eq!(kb.inline_keyboard.len(), 5);
        assert_eq!(kb.inline_keyboard[0].len(), 1);
        assert_eq!(kb.inline_keyboard[0][0].text, "🎬 Качество видео: Авто ⚡");

        assert_eq!(kb.inline_keyboard[1].len(), 1);
        assert_eq!(kb.inline_keyboard[1][0].text, "🎶 Я.Музыка: AAC 192k ⚡");

        assert_eq!(kb.inline_keyboard[2].len(), 1);
        assert_eq!(kb.inline_keyboard[2][0].text, "🎵 Только аудио: ВЫКЛ ❌");

        assert_eq!(kb.inline_keyboard[3].len(), 1);
        assert_eq!(kb.inline_keyboard[3][0].text, "📎 Отправка файлом: ВЫКЛ ❌");

        assert_eq!(kb.inline_keyboard[4].len(), 1);
        assert_eq!(kb.inline_keyboard[4][0].text, "✖️ Закрыть");

        settings.quality = QualityPreference::AskEveryTime;
        settings.ym_quality = crate::settings::YandexQualityPreference::Lossless;
        settings.audio_only = true;
        settings.send_as_document = true;
        let kb_custom = build_settings_keyboard(&settings);
        assert_eq!(kb_custom.inline_keyboard[0].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[0][0].text, "🎬 Качество видео: Вручную ❓");
        assert_eq!(kb_custom.inline_keyboard[1].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[1][0].text, "🎶 Я.Музыка: FLAC 💎");
        assert_eq!(kb_custom.inline_keyboard[2].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[2][0].text, "🎵 Только аудио: ВКЛ ✅");
        assert_eq!(kb_custom.inline_keyboard[3].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[3][0].text, "📎 Отправка файлом: ВКЛ ✅");

        let text = build_settings_text(&settings);
        assert!(text.contains("Вручную"));
        assert!(text.contains("FLAC"));
        assert!(text.contains("Включён"));
        assert!(text.contains("Включена"));
    }
}
