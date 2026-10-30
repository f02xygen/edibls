use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::{
    FileId, InputFile, InputMedia, InputMediaPhoto, MessageId, ReplyParameters, UserId,
};
use tempfile::TempDir;

use crate::bot::views::{
    build_audio_action_buttons, build_cancel_download_keyboard, build_video_action_buttons,
    format_progress_bar,
};
use crate::cache::{CachedMedia, MediaCache};
use crate::config::Config;
use crate::media::{
    attach_subway_surfers, extract_audio_from_video, fetch_media, find_subway_asset,
    probe_video_metadata, process_media_cut_or_speed, read_network_tx_bytes, MediaOutput,
    SourceMedia, TimeRange, VideoFile,
};
use crate::queue::QueueManager;
use crate::settings::YandexQualityPreference;

#[derive(Debug, Clone)]
pub struct MediaPipelineOptions {
    pub url: String,
    pub audio_only: bool,
    pub max_height: Option<u16>,
    pub time_range: Option<TimeRange>,
    pub speed: Option<f32>,
    pub ym_quality: YandexQualityPreference,
    pub send_as_document: bool,
    pub with_subway: bool,
    pub source_media: Option<SourceMedia>,
    pub has_existing_subway: bool,
}

impl MediaPipelineOptions {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            audio_only: false,
            max_height: None,
            time_range: None,
            speed: None,
            ym_quality: YandexQualityPreference::Optimal,
            send_as_document: false,
            with_subway: false,
            source_media: None,
            has_existing_subway: false,
        }
    }
}

pub async fn try_get_cached_video_file(
    bot: &Bot,
    file_id: &FileId,
    temp_dir: &Path,
) -> Option<PathBuf> {
    let file = match bot.get_file(file_id.clone()).await {
        Ok(f) => f,
        Err(e) => {
            log::warn!("bot.get_file failed for file_id {:?}: {e}", file_id);
            return None;
        }
    };

    let p = PathBuf::from(&file.path);
    if p.is_absolute() && tokio::fs::metadata(&p).await.is_ok() {
        log::info!("Reusing cached media directly from local path: {:?}", p);
        return Some(p);
    }

    // Fallback: try downloading via bot-api to temp_dir
    let ext = Path::new(&file.path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp4");
    let dest = temp_dir.join(format!("cached_source_file.{ext}"));
    match tokio::fs::File::create(&dest).await {
        Ok(mut dest_file) => match bot.download_file(&file.path, &mut dest_file).await {
            Ok(_) => {
                log::info!("Downloaded cached media from bot-api to {:?}", dest);
                Some(dest)
            }
            Err(e) => {
                log::warn!("Failed to download file {}: {e}", file.path);
                None
            }
        },
        Err(e) => {
            log::warn!("Failed to create temp file {:?}: {e}", dest);
            None
        }
    }
}

pub async fn process_and_send_media(
    bot: &Bot,
    chat_id: ChatId,
    reply_to: Option<MessageId>,
    user_id: UserId,
    opts: MediaPipelineOptions,
    config: &Arc<Config>,
    queue: &QueueManager,
    cache: &MediaCache,
) -> ResponseResult<()> {
    let is_transformed = opts.time_range.is_some()
        || opts.speed.map(|s| (s - 1.0).abs() > 0.01).unwrap_or(false)
        || opts.with_subway
        || opts.source_media.is_some();
    let resulting_subway = opts.with_subway || opts.has_existing_subway;

    // 1. Fast path: check file_id cache ONLY if clean original media with no source_media
    if !is_transformed && opts.source_media.is_none() {
        let cache_height = if opts.audio_only { None } else { opts.max_height };
        if let Some(cached) = cache.get(
            &opts.url,
            opts.audio_only,
            cache_height,
            opts.send_as_document,
        ) {
            log::info!(
                "Fast path: serving from cache for {} (audio_only: {}, max_height: {:?}, doc: {})",
                opts.url,
                opts.audio_only,
                cache_height,
                opts.send_as_document
            );
            match cached {
                CachedMedia::Video(file_id) => {
                    let url_id = cache.register_url(&opts.url);
                    let is_yt = opts.url.contains("youtube.com") || opts.url.contains("youtu.be");
                    let reply_markup = build_video_action_buttons(url_id, is_yt, false);

                    if opts.send_as_document {
                        let mut req = bot
                            .send_document(chat_id, InputFile::file_id(file_id))
                            .disable_content_type_detection(true)
                            .reply_markup(reply_markup);
                        if let Some(msg_id) = reply_to {
                            req = req.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        req.await?;
                    } else {
                        let mut req = bot
                            .send_video(chat_id, InputFile::file_id(file_id))
                            .supports_streaming(true)
                            .reply_markup(reply_markup);
                        if let Some(msg_id) = reply_to {
                            req = req.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        req.await?;
                    }
                    return Ok(());
                }
                CachedMedia::Audio(file_ids) => {
                    let url_id = cache.register_url(&opts.url);
                    let reply_markup = build_audio_action_buttons(url_id);
                    for file_id in file_ids {
                        if opts.send_as_document {
                            let mut req = bot
                                .send_document(chat_id, InputFile::file_id(file_id))
                                .disable_content_type_detection(true)
                                .reply_markup(reply_markup.clone());
                            if let Some(msg_id) = reply_to {
                                req = req.reply_parameters(ReplyParameters::new(msg_id));
                            }
                            req.await?;
                        } else {
                            let mut req = bot
                                .send_audio(chat_id, InputFile::file_id(file_id))
                                .reply_markup(reply_markup.clone());
                            if let Some(msg_id) = reply_to {
                                req = req.reply_parameters(ReplyParameters::new(msg_id));
                            }
                            req.await?;
                        }
                    }
                    return Ok(());
                }
                CachedMedia::Photos {
                    photo_file_ids,
                    audio_file_id,
                } => {
                    for chunk in photo_file_ids.chunks(10) {
                        let media_items: Vec<InputMedia> = chunk
                            .iter()
                            .map(|fid| {
                                InputMedia::Photo(InputMediaPhoto::new(InputFile::file_id(fid.clone())))
                            })
                            .collect();
                        let mut req = bot.send_media_group(chat_id, media_items);
                        if let Some(msg_id) = reply_to {
                            req = req.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        let _ = req.await;
                    }
                    if let Some(afid) = audio_file_id {
                        let url_id = cache.register_url(&opts.url);
                        let reply_markup = build_audio_action_buttons(url_id);
                        let mut req = bot
                            .send_audio(chat_id, InputFile::file_id(afid))
                            .reply_markup(reply_markup);
                        if let Some(msg_id) = reply_to {
                            req = req.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        let _ = req.await;
                    }
                    return Ok(());
                }
            }
        }
    }

    // 2. Slow path: acquire user lock with cancellation receiver
    let (_user_guard, mut cancel_rx) = match queue.try_lock_user(user_id) {
        Some((guard, rx)) => (guard, rx),
        None => {
            let mut req = bot.send_message(
                chat_id,
                "У тебя уже выполняется одна операция. Дождись её завершения.",
            );
            if let Some(msg_id) = reply_to {
                req = req.reply_parameters(ReplyParameters::new(msg_id));
            }
            req.await?;
            return Ok(());
        }
    };

    let dl_progress = Arc::new(AtomicU8::new(0));

    // Immediately send initial status message with the CANCEL BUTTON so user can cancel right away
    let initial_status_text = if opts.with_subway {
        "⏳ Подготовка видео с Subway Surfers..."
    } else if opts.time_range.is_some() {
        "⏳ Нарезка клипа..."
    } else if opts.speed.is_some() {
        "⏳ Изменение скорости..."
    } else if opts.audio_only && opts.source_media.is_some() {
        "⏳ Извлечение аудио..."
    } else {
        "⏳ Подготовка к загрузке..."
    };

    let mut status_req = bot
        .send_message(chat_id, initial_status_text)
        .reply_markup(build_cancel_download_keyboard(user_id));
    if let Some(msg_id) = reply_to {
        status_req = status_req.reply_parameters(ReplyParameters::new(msg_id));
    }
    let status_msg = status_req.await?;

    // Check if we can reuse an already cached/downloaded base video or audio when rendering Subway Surfers, speed, or clip
    let mut reused_base_video: Option<(VideoFile, TempDir)> = None;
    let mut reused_base_audio: Option<(crate::media::AudioTrack, TempDir)> = None;
    let needs_transform = opts.with_subway
        || opts.time_range.is_some()
        || opts.speed.map(|s| (s - 1.0).abs() > 0.01).unwrap_or(false);

    if opts.source_media.is_none() && !opts.audio_only && needs_transform {
        if let Some(base_fid) = cache.get_cached_video_file_id(&opts.url, opts.max_height) {
            log::info!(
                "Attempting to reuse cached base video {:?} for {}",
                base_fid,
                opts.url
            );
            let reuse_fut = async {
                if let Ok(td) = TempDir::new() {
                    if let Some(video_path) =
                        try_get_cached_video_file(bot, &base_fid, td.path()).await
                    {
                        log::info!("Reusing base video file at {:?}", video_path);
                        let mut current_path = video_path;
                        if opts.time_range.is_some()
                            || opts.speed.map(|s| (s - 1.0).abs() > 0.01).unwrap_or(false)
                        {
                            let processed_p = td.path().join("reused_cut_speed.mp4");
                            if process_media_cut_or_speed(
                                &current_path,
                                opts.time_range.as_ref(),
                                opts.speed,
                                false,
                                &processed_p,
                            )
                            .await
                            .is_ok()
                            {
                                current_path = processed_p;
                            }
                        }
                        let (w, h, dur, thumb) = probe_video_metadata(&current_path).await;
                        return Some((
                            VideoFile {
                                path: current_path,
                                title: Some("Video".to_string()),
                                width: w,
                                height: h,
                                duration_secs: dur,
                                thumbnail_path: thumb,
                            },
                            td,
                        ));
                    }
                }
                None
            };

            let reuse_res = tokio::select! {
                _ = cancel_rx.changed() => {
                    let _ = bot.delete_message(chat_id, status_msg.id).await;
                    return Ok(());
                }
                res = reuse_fut => res,
            };

            if let Some(reused) = reuse_res {
                reused_base_video = Some(reused);
            }
        }
    } else if opts.source_media.is_none() && opts.audio_only && needs_transform {
        if let Some(base_fid) = cache.get_cached_audio_file_id(&opts.url) {
            log::info!(
                "Attempting to reuse cached base audio {:?} for {}",
                base_fid,
                opts.url
            );
            let reuse_fut = async {
                if let Ok(td) = TempDir::new() {
                    if let Some(audio_path) =
                        try_get_cached_video_file(bot, &base_fid, td.path()).await
                    {
                        log::info!("Reusing base audio file at {:?}", audio_path);
                        let mut current_path = audio_path;
                        if opts.time_range.is_some()
                            || opts.speed.map(|s| (s - 1.0).abs() > 0.01).unwrap_or(false)
                        {
                            let ext = current_path
                                .extension()
                                .and_then(|s| s.to_str())
                                .unwrap_or("mp3");
                            let processed_p = td.path().join(format!("reused_cut_speed.{ext}"));
                            if process_media_cut_or_speed(
                                &current_path,
                                opts.time_range.as_ref(),
                                opts.speed,
                                true,
                                &processed_p,
                            )
                            .await
                            .is_ok()
                            {
                                current_path = processed_p;
                            }
                        }
                        let track = crate::media::process_audio_metadata(&current_path);
                        return Some((track, td));
                    }
                }
                None
            };

            let reuse_res = tokio::select! {
                _ = cancel_rx.changed() => {
                    let _ = bot.delete_message(chat_id, status_msg.id).await;
                    return Ok(());
                }
                res = reuse_fut => res,
            };

            if let Some(reused) = reuse_res {
                reused_base_audio = Some(reused);
            }
        }
    }

    let (media_result, temp_dir) = if let Some(ref src) = opts.source_media {
        let td = match TempDir::new() {
            Ok(d) => d,
            Err(e) => {
                let _ = bot
                    .edit_message_text(
                        chat_id,
                        status_msg.id,
                        format!("Ошибка создания временной папки: {e}"),
                    )
                    .await;
                return Ok(());
            }
        };

        let file_id = match src {
            SourceMedia::Video(fid) | SourceMedia::Audio(fid) => fid,
        };

        let fetch_fut = try_get_cached_video_file(bot, file_id, td.path());
        let file_path = tokio::select! {
            _ = cancel_rx.changed() => {
                let _ = bot.delete_message(chat_id, status_msg.id).await;
                return Ok(());
            }
            res = fetch_fut => match res {
                Some(p) => p,
                None => {
                    let _ = bot
                        .edit_message_text(
                            chat_id,
                            status_msg.id,
                            "Не удалось получить исходный файл из сообщения.",
                        )
                        .await;
                    return Ok(());
                }
            }
        };

        if opts.audio_only {
            let out_audio = td.path().join("extracted_audio.mp3");
            let process_res = match src {
                SourceMedia::Video(_) => {
                    extract_audio_from_video(
                        &file_path,
                        opts.time_range.as_ref(),
                        opts.speed,
                        &out_audio,
                    )
                    .await
                }
                SourceMedia::Audio(_) => {
                    process_media_cut_or_speed(
                        &file_path,
                        opts.time_range.as_ref(),
                        opts.speed,
                        true,
                        &out_audio,
                    )
                    .await
                }
            };

            let audio_track = match process_res {
                Ok(_) => crate::media::process_audio_metadata(&out_audio),
                Err(e) => {
                    let _ = bot
                        .edit_message_text(
                            chat_id,
                            status_msg.id,
                            format!("Ошибка обработки аудио: {e}"),
                        )
                        .await;
                    return Ok(());
                }
            };
            (Ok(MediaOutput::Audio(vec![audio_track])), td)
        } else {
            let mut current_video_path = file_path;

            if opts.time_range.is_some()
                || opts.speed.map(|s| (s - 1.0).abs() > 0.01).unwrap_or(false)
            {
                let processed_p = td.path().join("processed_cut_speed.mp4");
                let cut_res = process_media_cut_or_speed(
                    &current_video_path,
                    opts.time_range.as_ref(),
                    opts.speed,
                    false,
                    &processed_p,
                )
                .await;

                match cut_res {
                    Ok(_) => current_video_path = processed_p,
                    Err(e) => {
                        let _ = bot
                            .edit_message_text(
                                chat_id,
                                status_msg.id,
                                format!("Ошибка изменения скорости/обрезки: {e}"),
                            )
                            .await;
                        return Ok(());
                    }
                }
            }

            let (w, h, dur, thumb) = probe_video_metadata(&current_video_path).await;
            let vfile = VideoFile {
                path: current_video_path,
                title: Some("Video".to_string()),
                width: w,
                height: h,
                duration_secs: dur,
                thumbnail_path: thumb,
            };
            (Ok(MediaOutput::Video(vfile)), td)
        }
    } else if let Some((orig_video, temp_dir)) = reused_base_video {
        (Ok(MediaOutput::Video(orig_video)), temp_dir)
    } else if let Some((orig_audio, temp_dir)) = reused_base_audio {
        (Ok(MediaOutput::Audio(vec![orig_audio])), temp_dir)
    } else {
        let (service, pos, waiter_guard) = queue.register(&opts.url);

        if pos > 1 {
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("⏳ Очередь {} (позиция: {})...", service.name(), pos),
                )
                .reply_markup(build_cancel_download_keyboard(user_id))
                .await;
        }

        // Lock domain & wait for global download slot (with early cancellation)
        let service_guard = tokio::select! {
            _ = async {
                while cancel_rx.changed().await.is_ok() {
                    if *cancel_rx.borrow() {
                        break;
                    }
                }
            } => {
                let _ = bot.delete_message(chat_id, status_msg.id).await;
                return Ok(());
            }
            guard = queue.acquire(service, waiter_guard) => guard,
        };

        let _ = bot
            .edit_message_text(
                chat_id,
                status_msg.id,
                format_progress_bar("📥 Скачивание", 0),
            )
            .reply_markup(build_cancel_download_keyboard(user_id))
            .await;

        let temp_dir = match TempDir::new() {
            Ok(dir) => dir,
            Err(e) => {
                bot.edit_message_text(
                    chat_id,
                    status_msg.id,
                    format!("Не удалось создать временную директорию: {e}"),
                )
                .reply_markup(build_cancel_download_keyboard(user_id))
                .await?;
                return Ok(());
            }
        };

        let dl_shown_pct = Arc::new(AtomicU8::new(0));
        let dl_bot = bot.clone();
        let dl_msg_id = status_msg.id;
        let dl_prog_clone = Arc::clone(&dl_progress);
        let dl_shown_clone = Arc::clone(&dl_shown_pct);
        let dl_user_id = user_id;
        let dl_updater = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
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
                        .edit_message_text(
                            chat_id,
                            dl_msg_id,
                            format_progress_bar("📥 Скачивание", pct),
                        )
                        .reply_markup(build_cancel_download_keyboard(dl_user_id))
                        .await;
                }
            }
        });

        let media_result = tokio::select! {
            _ = cancel_rx.changed() => {
                dl_updater.abort();
                drop(service_guard);
                drop(_user_guard);
                let _ = bot.delete_message(chat_id, status_msg.id).await;
                return Ok(());
            }
            res = fetch_media(
                &opts.url,
                opts.audio_only,
                opts.max_height,
                opts.time_range.as_ref(),
                opts.speed,
                opts.ym_quality,
                temp_dir.path(),
                config,
                &dl_progress,
            ) => res,
        };

        dl_updater.abort();

        let final_dl_pct = dl_progress.load(Ordering::Relaxed);
        if final_dl_pct > 0 {
            if dl_shown_pct.load(Ordering::Relaxed) == 0 {
                let _ = bot
                    .edit_message_text(
                        chat_id,
                        status_msg.id,
                        format_progress_bar("📥 Скачивание", 50),
                    )
                    .reply_markup(build_cancel_download_keyboard(user_id))
                    .await;
            }
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format_progress_bar("📥 Скачивание", final_dl_pct),
                )
                .reply_markup(build_cancel_download_keyboard(user_id))
                .await;
        }

        // Release domain & global download slot immediately so next user can start downloading
        drop(service_guard);

        (media_result, temp_dir)
    };

    let mut media_result = media_result;
    if opts.with_subway {
        if let Ok(MediaOutput::Video(ref orig_video)) = media_result {
            if let Some(subway_asset) = find_subway_asset() {
                let subway_out = temp_dir.path().join("subway_surfers_merged.mp4");
                let _ = bot
                    .edit_message_text(
                        chat_id,
                        status_msg.id,
                        "🛹 Рендеринг Subway Surfers...",
                    )
                    .reply_markup(build_cancel_download_keyboard(user_id))
                    .await;

                // Ticker for animated status + guaranteed active cancel button during Subway Surfers rendering
                let subway_bot = bot.clone();
                let subway_msg_id = status_msg.id;
                let subway_uid = user_id;
                let subway_ticker = tokio::spawn(async move {
                    let mut sec = 0u32;
                    let mut interval =
                        tokio::time::interval(std::time::Duration::from_secs(3));
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        sec += 3;
                        let dots = match (sec / 3) % 4 {
                            0 => "   ",
                            1 => ".  ",
                            2 => ".. ",
                            _ => "...",
                        };
                        let _ = subway_bot
                            .edit_message_text(
                                chat_id,
                                subway_msg_id,
                                format!("🛹 Рендеринг Subway Surfers{dots} ({sec}с)"),
                            )
                            .reply_markup(build_cancel_download_keyboard(subway_uid))
                            .await;
                    }
                });

                let subway_res = tokio::select! {
                    _ = cancel_rx.changed() => {
                        subway_ticker.abort();
                        let _ = bot.delete_message(chat_id, status_msg.id).await;
                        return Ok(());
                    }
                    res = attach_subway_surfers(&orig_video.path, &subway_asset, &subway_out) => res,
                };
                subway_ticker.abort();

                match subway_res {
                    Ok(subway_vid) => {
                        media_result = Ok(MediaOutput::Video(subway_vid));
                    }
                    Err(err) => {
                        log::error!("Failed to attach subway surfers: {err}");
                        let _ = bot
                            .send_message(
                                chat_id,
                                format!("⚠️ Не удалось приклеить Subway Surfers: {err}"),
                            )
                            .reply_parameters(ReplyParameters::new(status_msg.id))
                            .await;
                        let _ = bot.delete_message(chat_id, status_msg.id).await;
                        return Ok(());
                    }
                }
            } else {
                log::error!("Subway surfers asset not found in assets/subway_surfers.mp4");
                let _ = bot
                    .send_message(
                        chat_id,
                        "⚠️ Файл assets/subway_surfers.mp4 не найден на сервере.",
                    )
                    .reply_parameters(ReplyParameters::new(status_msg.id))
                    .await;
                let _ = bot.delete_message(chat_id, status_msg.id).await;
                return Ok(());
            }
        }
    }

    match media_result {
        Ok(MediaOutput::Audio(tracks)) => {
            let total_files_bytes: u64 = tracks
                .iter()
                .filter_map(|t| std::fs::metadata(&t.path).ok().map(|m| m.len()))
                .sum();

            let total_tracks = tracks.len();
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format_progress_bar("📤 Отправка", 0),
                )
                .reply_markup(build_cancel_download_keyboard(user_id))
                .await;

            let start_tx = read_network_tx_bytes();
            let ul_shown_pct = Arc::new(AtomicU8::new(0));
            let ul_shown_clone = Arc::clone(&ul_shown_pct);
            let ul_bot = bot.clone();
            let ul_msg_id = status_msg.id;
            let ul_user_id = user_id;
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
                                .reply_markup(build_cancel_download_keyboard(ul_user_id))
                                .await;
                        }
                    }
                }
            });

            let mut button_target_url = opts.url.clone();
            if resulting_subway {
                button_target_url = format!("{button_target_url}#subway");
            }
            let url_id = cache.register_url(&button_target_url);
            let reply_markup = build_audio_action_buttons(url_id);

            let mut uploaded_ids = Vec::with_capacity(tracks.len());
            let mut total_size: u64 = 0;

            for (idx, track) in tracks.iter().enumerate() {
                if total_tracks > 1 {
                    let track_pct = ((idx as f64 / total_tracks as f64) * 100.0)
                        .round()
                        .clamp(0.0, 99.0) as u8;
                    ul_shown_pct.store(track_pct, Ordering::Relaxed);
                    let _ = bot
                        .edit_message_text(
                            chat_id,
                            status_msg.id,
                            format_progress_bar(
                                &format!("📤 Отправка [{}/{}]", idx + 1, total_tracks),
                                track_pct,
                            ),
                        )
                        .reply_markup(build_cancel_download_keyboard(user_id))
                        .await;
                }

                if opts.send_as_document {
                    let mut input_file = InputFile::file(&track.path);
                    let ext = track
                        .path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("mp3");
                    let custom_name = match (&track.performer, &track.title) {
                        (Some(p), Some(t)) => Some(format!("{p} - {t}.{ext}")),
                        (None, Some(t)) => Some(format!("{t}.{ext}")),
                        _ => None,
                    };
                    if let Some(name) = custom_name {
                        input_file = input_file.file_name(name);
                    }

                    let send_doc_fut = async {
                        let mut attempts = 0;
                        loop {
                            let mut request = bot
                                .send_document(chat_id, input_file.clone())
                                .disable_content_type_detection(true)
                                .reply_markup(reply_markup.clone());

                            if let Some(msg_id) = reply_to {
                                request = request.reply_parameters(ReplyParameters::new(msg_id));
                            }
                            if let Some(cover_path) = &track.cover_path {
                                request = request.thumbnail(InputFile::file(cover_path));
                            }

                            match request.await {
                                Ok(msg) => return Ok(msg),
                                Err(teloxide::RequestError::RetryAfter(dur)) if attempts < 3 => {
                                    log::warn!("Telegram rate limit hit during audio doc send ({:?}); retrying in {:?}", dur, dur);
                                    tokio::time::sleep(dur.duration() + std::time::Duration::from_millis(500)).await;
                                    attempts += 1;
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    };

                    let send_res = tokio::select! {
                        _ = cancel_rx.changed() => {
                            ul_updater.abort();
                            let _ = bot.delete_message(chat_id, status_msg.id).await;
                            return Ok(());
                        }
                        res = send_doc_fut => res,
                    };

                    match send_res {
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
                    let send_audio_fut = async {
                        let mut attempts = 0;
                        loop {
                            let input_file = InputFile::file(&track.path);
                            let mut request = bot
                                .send_audio(chat_id, input_file)
                                .reply_markup(reply_markup.clone());

                            if let Some(msg_id) = reply_to {
                                request = request.reply_parameters(ReplyParameters::new(msg_id));
                            }
                            if let Some(p) = &track.performer {
                                request = request.performer(p);
                            }
                            if let Some(t) = &track.title {
                                request = request.title(t);
                            }
                            if let Some(d) = track.duration_secs {
                                request = request.duration(d);
                            }
                            if let Some(cover_path) = &track.cover_path {
                                request = request.thumbnail(InputFile::file(cover_path));
                            }

                            match request.await {
                                Ok(msg) => return Ok(msg),
                                Err(teloxide::RequestError::RetryAfter(dur)) if attempts < 3 => {
                                    log::warn!("Telegram rate limit hit during audio send ({:?}); retrying in {:?}", dur, dur);
                                    tokio::time::sleep(dur.duration() + std::time::Duration::from_millis(500)).await;
                                    attempts += 1;
                                }
                                Err(e) => return Err(e),
                            }
                        }
                    };

                    let send_res = tokio::select! {
                        _ = cancel_rx.changed() => {
                            ul_updater.abort();
                            let _ = bot.delete_message(chat_id, status_msg.id).await;
                            return Ok(());
                        }
                        res = send_audio_fut => res,
                    };

                    match send_res {
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
                    .edit_message_text(
                        chat_id,
                        status_msg.id,
                        format_progress_bar("📤 Отправка", 50),
                    )
                    .reply_markup(build_cancel_download_keyboard(user_id))
                    .await;
            }

            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format_progress_bar("📤 Отправка", 100),
                )
                .reply_markup(build_cancel_download_keyboard(user_id))
                .await;

            let should_cache = !is_transformed && opts.source_media.is_none();
            if should_cache && !uploaded_ids.is_empty() {
                cache.insert(
                    &opts.url,
                    true,
                    opts.max_height,
                    opts.send_as_document,
                    CachedMedia::Audio(uploaded_ids),
                    total_size,
                );
            }

            let _ = bot.delete_message(chat_id, status_msg.id).await;
        }
        Ok(MediaOutput::Video(video_file)) => {
            let file_path = video_file.path.clone();
            let file_size = tokio::fs::metadata(&file_path)
                .await
                .map(|m| m.len())
                .unwrap_or(0);

            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format_progress_bar("📤 Отправка", 0),
                )
                .reply_markup(build_cancel_download_keyboard(user_id))
                .await;

            let start_tx = read_network_tx_bytes();
            let ul_bot = bot.clone();
            let ul_msg_id = status_msg.id;
            let ul_user_id = user_id;
            let ul_updater = tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
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
                            tx_pct
                                .max(floor_pct)
                                .min(last_pct.saturating_add(25))
                                .min(95)
                        };
                        if pct != last_pct {
                            last_pct = pct;
                            let _ = ul_bot
                                .edit_message_text(
                                    chat_id,
                                    ul_msg_id,
                                    format_progress_bar("📤 Отправка", pct),
                                )
                                .reply_markup(build_cancel_download_keyboard(ul_user_id))
                                .await;
                        }
                    }
                }
            });

            let mut button_target_url = opts.url.clone();
            if resulting_subway {
                button_target_url = format!("{button_target_url}#subway");
            }
            let url_id = cache.register_url(&button_target_url);
            let is_yt = opts.url.contains("youtube.com") || opts.url.contains("youtu.be");
            let reply_markup = build_video_action_buttons(url_id, is_yt && !resulting_subway, resulting_subway);

            let send_fut = async {
                let mut attempts = 0;
                loop {
                    let res = if opts.send_as_document {
                        let mut input_file = InputFile::file(&file_path);
                        let ext = file_path
                            .extension()
                            .and_then(|s| s.to_str())
                            .unwrap_or("mp4");
                        if let Some(t) = &video_file.title {
                            let clean_title: String = t
                                .chars()
                                .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_')
                                .collect();
                            let clean_title = clean_title.trim();
                            if !clean_title.is_empty() {
                                input_file = input_file.file_name(format!("{clean_title}.{ext}"));
                            }
                        }

                        let mut request = bot
                            .send_document(chat_id, input_file)
                            .disable_content_type_detection(true)
                            .reply_markup(reply_markup.clone());

                        if let Some(msg_id) = reply_to {
                            request = request.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        if let Some(thumb) = &video_file.thumbnail_path {
                            request = request.thumbnail(InputFile::file(thumb));
                        }
                        request.await
                    } else {
                        let input_file = InputFile::file(&file_path);
                        let mut request = bot
                            .send_video(chat_id, input_file)
                            .supports_streaming(true)
                            .reply_markup(reply_markup.clone());

                        if let (Some(w), Some(h)) = (video_file.width, video_file.height) {
                            request = request.width(w).height(h);
                        }
                        if let Some(d) = video_file.duration_secs {
                            request = request.duration(d);
                        }
                        if let Some(thumb) = &video_file.thumbnail_path {
                            request = request.thumbnail(InputFile::file(thumb));
                        }

                        if let Some(msg_id) = reply_to {
                            request = request.reply_parameters(ReplyParameters::new(msg_id));
                        }
                        request.await
                    };

                    match res {
                        Ok(msg) => return Ok(msg),
                        Err(teloxide::RequestError::RetryAfter(dur)) if attempts < 3 => {
                            log::warn!("Telegram rate limit hit during video send ({:?}); retrying...", dur);
                            tokio::time::sleep(dur.duration() + std::time::Duration::from_millis(500)).await;
                            attempts += 1;
                        }
                        Err(e) => return Err(e),
                    }
                }
            };

            let send_res = tokio::select! {
                _ = cancel_rx.changed() => {
                    ul_updater.abort();
                    let _ = bot.delete_message(chat_id, status_msg.id).await;
                    return Ok(());
                }
                res = send_fut => res,
            };

            ul_updater.abort();

            match send_res {
                Ok(sent_msg) => {
                    let should_cache = !is_transformed && opts.source_media.is_none();
                    if should_cache {
                        if opts.send_as_document {
                            if let Some(doc) = sent_msg.document() {
                                cache.insert(
                                    &opts.url,
                                    false,
                                    opts.max_height,
                                    opts.send_as_document,
                                    CachedMedia::Video(doc.file.id.clone()),
                                    u64::from(doc.file.size),
                                );
                            }
                        } else if let Some(video) = sent_msg.video() {
                            cache.insert(
                                &opts.url,
                                false,
                                opts.max_height,
                                opts.send_as_document,
                                CachedMedia::Video(video.file.id.clone()),
                                u64::from(video.file.size),
                            );
                        }
                    }
                    let _ = bot.delete_message(chat_id, status_msg.id).await;
                }
                Err(e) => {
                    log::error!("Failed to send video: {:?}", e);
                    let mut recovered = false;

                    let err_str = e.to_string();
                    let is_entity_too_large =
                        err_str.contains("Request Entity Too Large") || err_str.contains("413");

                    if is_entity_too_large && opts.max_height.map(|h| h > 480).unwrap_or(true) {
                        log::warn!(
                            "Video too large for Telegram API ({} bytes). Attempting 480p fallback for {}",
                            file_size,
                            opts.url
                        );
                        let _ = bot
                            .edit_message_text(
                                chat_id,
                                status_msg.id,
                                "⚠️ Файл больше 2000 МБ. Пробую скачать в 480p...",
                            )
                            .reply_markup(build_cancel_download_keyboard(user_id))
                            .await;

                        let mut fallback_cfg = (**config).clone();
                        fallback_cfg.max_filesize_mb = 2000;

                        let fallback_fetch = fetch_media(
                            &opts.url,
                            false,
                            Some(480),
                            opts.time_range.as_ref(),
                            opts.speed,
                            opts.ym_quality,
                            temp_dir.path(),
                            &fallback_cfg,
                            &dl_progress,
                        );

                        let fallback_res = tokio::select! {
                            _ = cancel_rx.changed() => {
                                let _ = bot.delete_message(chat_id, status_msg.id).await;
                                return Ok(());
                            }
                            res = fallback_fetch => res,
                        };

                        if let Ok(MediaOutput::Video(smaller_video)) = fallback_res {
                            let _ = bot
                                .edit_message_text(
                                    chat_id,
                                    status_msg.id,
                                    format_progress_bar("📤 Отправка", 50),
                                )
                                .reply_markup(build_cancel_download_keyboard(user_id))
                                .await;
                            let smaller_path = smaller_video.path.clone();
                            let retry_fut = async {
                                let mut attempts = 0;
                                loop {
                                    let res = if opts.send_as_document {
                                        let mut input_file = InputFile::file(&smaller_path);
                                        let ext = smaller_path
                                            .extension()
                                            .and_then(|s| s.to_str())
                                            .unwrap_or("mp4");
                                        if let Some(t) = &smaller_video.title {
                                            let clean_title: String = t
                                                .chars()
                                                .filter(|c| {
                                                    c.is_alphanumeric()
                                                        || *c == ' '
                                                        || *c == '-'
                                                        || *c == '_'
                                                })
                                                .collect();
                                            let clean_title = clean_title.trim();
                                            if !clean_title.is_empty() {
                                                input_file = input_file
                                                    .file_name(format!("{clean_title}.{ext}"));
                                            }
                                        }
                                        let mut retry_req = bot
                                            .send_document(chat_id, input_file)
                                            .disable_content_type_detection(true)
                                            .reply_markup(reply_markup.clone());
                                        if let Some(msg_id) = reply_to {
                                            retry_req = retry_req
                                                .reply_parameters(ReplyParameters::new(msg_id));
                                        }
                                        if let Some(thumb) = &smaller_video.thumbnail_path {
                                            retry_req = retry_req.thumbnail(InputFile::file(thumb));
                                        }
                                        retry_req.await
                                    } else {
                                        let mut retry_req = bot
                                            .send_video(chat_id, InputFile::file(&smaller_path))
                                            .supports_streaming(true)
                                            .reply_markup(reply_markup.clone());
                                        if let (Some(w), Some(h)) =
                                            (smaller_video.width, smaller_video.height)
                                        {
                                            retry_req = retry_req.width(w).height(h);
                                        }
                                        if let Some(d) = smaller_video.duration_secs {
                                            retry_req = retry_req.duration(d);
                                        }
                                        if let Some(thumb) = &smaller_video.thumbnail_path {
                                            retry_req = retry_req.thumbnail(InputFile::file(thumb));
                                        }
                                        if let Some(msg_id) = reply_to {
                                            retry_req = retry_req
                                                .reply_parameters(ReplyParameters::new(msg_id));
                                        }
                                        retry_req.await
                                    };

                                    match res {
                                        Ok(msg) => return Ok(msg),
                                        Err(teloxide::RequestError::RetryAfter(dur)) if attempts < 3 => {
                                            log::warn!("Telegram rate limit hit during fallback video send ({:?}); retrying...", dur);
                                            tokio::time::sleep(dur.duration() + std::time::Duration::from_millis(500)).await;
                                            attempts += 1;
                                        }
                                        Err(e) => return Err(e),
                                    }
                                }
                            };

                            let retry_res = tokio::select! {
                                _ = cancel_rx.changed() => {
                                    let _ = bot.delete_message(chat_id, status_msg.id).await;
                                    return Ok(());
                                }
                                res = retry_fut => res,
                            };

                            if let Ok(sent_msg) = retry_res {
                                let should_cache = !is_transformed && opts.source_media.is_none();
                                if should_cache {
                                    if opts.send_as_document {
                                        if let Some(doc) = sent_msg.document() {
                                            cache.insert(
                                                &opts.url,
                                                false,
                                                Some(480),
                                                opts.send_as_document,
                                                CachedMedia::Video(doc.file.id.clone()),
                                                u64::from(doc.file.size),
                                            );
                                        }
                                    } else if let Some(video) = sent_msg.video() {
                                        cache.insert(
                                            &opts.url,
                                            false,
                                            Some(480),
                                            opts.send_as_document,
                                            CachedMedia::Video(video.file.id.clone()),
                                            u64::from(video.file.size),
                                        );
                                    }
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
        Ok(MediaOutput::Photos(album)) => {
            let _ = bot
                .edit_message_text(
                    chat_id,
                    status_msg.id,
                    format_progress_bar("📤 Отправка фото", 50),
                )
                .reply_markup(build_cancel_download_keyboard(user_id))
                .await;

            let mut uploaded_photo_ids = Vec::new();
            let mut total_size = 0u64;

            for chunk in album.photos.chunks(10) {
                let media_items: Vec<InputMedia> = chunk
                    .iter()
                    .map(|p| InputMedia::Photo(InputMediaPhoto::new(InputFile::file(p))))
                    .collect();

                let mut req = bot.send_media_group(chat_id, media_items);
                if let Some(msg_id) = reply_to {
                    req = req.reply_parameters(ReplyParameters::new(msg_id));
                }

                let send_res = tokio::select! {
                    _ = cancel_rx.changed() => {
                        let _ = bot.delete_message(chat_id, status_msg.id).await;
                        return Ok(());
                    }
                    res = req => res,
                };

                match send_res {
                    Ok(messages) => {
                        for m in messages {
                            if let Some(photos) = m.photo() {
                                if let Some(largest) = photos.last() {
                                    total_size =
                                        total_size.saturating_add(u64::from(largest.file.size));
                                    uploaded_photo_ids.push(largest.file.id.clone());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to send media group: {:?}", e);
                    }
                }
            }

            let mut uploaded_audio_id = None;
            if let Some(audio_track) = &album.audio {
                let input_file = InputFile::file(&audio_track.path);
                let mut req = bot.send_audio(chat_id, input_file);
                if let Some(msg_id) = reply_to {
                    req = req.reply_parameters(ReplyParameters::new(msg_id));
                }
                if let Some(p) = &audio_track.performer {
                    req = req.performer(p);
                }
                if let Some(t) = &audio_track.title {
                    req = req.title(t);
                }
                if let Some(d) = audio_track.duration_secs {
                    req = req.duration(d);
                }
                if let Some(cover) = &audio_track.cover_path {
                    req = req.thumbnail(InputFile::file(cover));
                }

                let audio_res = tokio::select! {
                    _ = cancel_rx.changed() => {
                        let _ = bot.delete_message(chat_id, status_msg.id).await;
                        return Ok(());
                    }
                    res = req => res,
                };

                match audio_res {
                    Ok(sent_msg) => {
                        if let Some(audio) = sent_msg.audio() {
                            total_size = total_size.saturating_add(u64::from(audio.file.size));
                            uploaded_audio_id = Some(audio.file.id.clone());
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to send photo album audio: {:?}", e);
                    }
                }
            }

            let should_cache = !is_transformed && opts.source_media.is_none();
            if should_cache && !uploaded_photo_ids.is_empty() {
                cache.insert(
                    &opts.url,
                    false,
                    None,
                    false,
                    CachedMedia::Photos {
                        photo_file_ids: uploaded_photo_ids,
                        audio_file_id: uploaded_audio_id,
                    },
                    total_size,
                );
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
