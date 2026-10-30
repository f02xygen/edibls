use std::sync::Arc;
use std::sync::OnceLock;
use teloxide::prelude::*;
use teloxide::types::{
    InlineKeyboardButton, InlineKeyboardMarkup, InlineQuery, InlineQueryResult,
    InlineQueryResultArticle, InlineQueryResultCachedAudio, InlineQueryResultCachedVideo,
    InlineQueryResultsButton, InlineQueryResultsButtonKind, InputMessageContent,
    InputMessageContentText, ParseMode,
};

use crate::cache::MediaCache;
use crate::config::Config;
use crate::providers::pornhub::search_pornhub_videos;
use crate::providers::soundcloud::{search_soundcloud_tracks, SoundcloudTrackInfo};
use crate::providers::yandex::{search_yandex_tracks, YandexTrackInfo};
use crate::providers::youtube::{search_youtube_videos, YouTubeVideoInfo};
use crate::settings::{DefaultSearchProvider, InlineProviderPreference, UserSettingsStore};

static BOT_USERNAME: OnceLock<String> = OnceLock::new();

pub fn init_bot_username(username: String) {
    let _ = BOT_USERNAME.set(username);
}

pub fn get_bot_username() -> Option<&'static str> {
    BOT_USERNAME.get().map(|s| s.as_str())
}

const INLINE_PAGE_SIZE: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineTargetProvider {
    Yandex,
    SoundCloud,
    YouTube,
    Pornhub,
    DefaultOrMixed,
}

pub fn parse_inline_query_prefix(query: &str) -> (InlineTargetProvider, &str) {
    let trimmed = query.trim_start();
    if let Some(rest) = trimmed.strip_prefix("ym ") {
        return (InlineTargetProvider::Yandex, rest.trim());
    }
    if let Some(rest) = trimmed.strip_prefix("я ") {
        return (InlineTargetProvider::Yandex, rest.trim());
    }
    if let Some(rest) = trimmed.strip_prefix("sc ") {
        return (InlineTargetProvider::SoundCloud, rest.trim());
    }
    if let Some(rest) = trimmed.strip_prefix("yt ") {
        return (InlineTargetProvider::YouTube, rest.trim());
    }
    if let Some(rest) = trimmed.strip_prefix("ютуб ") {
        return (InlineTargetProvider::YouTube, rest.trim());
    }
    if let Some(rest) = trimmed.strip_prefix("ph ") {
        return (InlineTargetProvider::Pornhub, rest.trim());
    }
    (InlineTargetProvider::DefaultOrMixed, trimmed.trim())
}

pub async fn handle_inline_query(
    bot: Bot,
    q: InlineQuery,
    config: Arc<Config>,
    cache: MediaCache,
    settings: UserSettingsStore,
) -> ResponseResult<()> {
    let user_settings = settings.get(q.from.id);
    let bot_username = get_bot_username().unwrap_or("bot");

    let offset: usize = q.offset.parse().unwrap_or(0);

    // 1. Handle empty query: return interactive help menu & quick provider suggestions
    if q.query.trim().is_empty() {
        let help_results = build_empty_query_results(bot_username);
        let button = InlineQueryResultsButton {
            text: "⚙️ Настройки инлайн-поиска".to_string(),
            kind: InlineQueryResultsButtonKind::StartParameter("settings".to_string()),
        };

        bot.answer_inline_query(q.id, help_results)
            .cache_time(10)
            .is_personal(true)
            .button(button)
            .await?;
        return Ok(());
    }

    // 2. Parse prefix and query
    let (target, query_text) = parse_inline_query_prefix(&q.query);
    if query_text.is_empty() {
        let help_results = build_empty_query_results(bot_username);
        bot.answer_inline_query(q.id, help_results)
            .cache_time(5)
            .is_personal(true)
            .await?;
        return Ok(());
    }

    let search_fut = async {
        let mut res: Vec<InlineQueryResult> = Vec::new();

        match target {
            InlineTargetProvider::Yandex => {
                if let Ok(tracks) = search_yandex_tracks(query_text, config.yandex_music_token.as_deref()).await {
                    res.extend(format_yandex_results(&tracks, &cache, user_settings.inline_instant_cached, bot_username));
                }
            }
            InlineTargetProvider::SoundCloud => {
                if let Ok(tracks) = search_soundcloud_tracks(query_text).await {
                    res.extend(format_soundcloud_results(&tracks, &cache, user_settings.inline_instant_cached, bot_username));
                }
            }
            InlineTargetProvider::YouTube => {
                if let Ok(videos) = search_youtube_videos(query_text).await {
                    res.extend(format_youtube_results(&videos, &cache, user_settings.inline_instant_cached, bot_username));
                }
            }
            InlineTargetProvider::Pornhub => {
                if let Ok(videos) = search_pornhub_videos(query_text).await {
                    for (idx, vid) in videos.into_iter().enumerate() {
                        let url = vid.canonical_url();
                        let url_id = cache.register_url(&url);
                        let mut article = InlineQueryResultArticle::new(
                            format!("ph_{}_{}", vid.vkey, idx),
                            format!("🔞 {}", vid.title),
                            InputMessageContent::Text(InputMessageContentText::new(format!(
                                "🔞 <b>{}</b>\n⏱ <b>Длительность:</b> {}\n\n{}",
                                vid.title,
                                vid.duration.as_deref().unwrap_or("—"),
                                url
                            )).parse_mode(ParseMode::Html)),
                        ).description(format!("Pornhub • {}", vid.duration.as_deref().unwrap_or("Видео")));

                        let dl_url = format!("https://t.me/{bot_username}?start=dl_{url_id}");
                        let mut keyboard_rows = vec![];
                        if let Ok(dl_u) = dl_url.parse() {
                            if let Ok(site_u) = url.parse() {
                                keyboard_rows.push(vec![
                                    InlineKeyboardButton::url("⬇️ Скачать видео", dl_u),
                                    InlineKeyboardButton::url("🔗 На сайт", site_u),
                                ]);
                            }
                        }
                        if !keyboard_rows.is_empty() {
                            article.reply_markup = Some(InlineKeyboardMarkup::new(keyboard_rows));
                        }
                        res.push(InlineQueryResult::Article(article));
                    }
                }
            }
            InlineTargetProvider::DefaultOrMixed => {
                match user_settings.inline_provider {
                    InlineProviderPreference::UseDefaultSearch => {
                        match user_settings.default_search {
                            DefaultSearchProvider::Yandex => {
                                if let Ok(tracks) = search_yandex_tracks(query_text, config.yandex_music_token.as_deref()).await {
                                    res.extend(format_yandex_results(&tracks, &cache, user_settings.inline_instant_cached, bot_username));
                                }
                            }
                            DefaultSearchProvider::SoundCloud => {
                                if let Ok(tracks) = search_soundcloud_tracks(query_text).await {
                                    res.extend(format_soundcloud_results(&tracks, &cache, user_settings.inline_instant_cached, bot_username));
                                }
                            }
                            DefaultSearchProvider::YouTube => {
                                if let Ok(videos) = search_youtube_videos(query_text).await {
                                    res.extend(format_youtube_results(&videos, &cache, user_settings.inline_instant_cached, bot_username));
                                }
                            }
                        }
                    }
                    InlineProviderPreference::Yandex => {
                        if let Ok(tracks) = search_yandex_tracks(query_text, config.yandex_music_token.as_deref()).await {
                            res.extend(format_yandex_results(&tracks, &cache, user_settings.inline_instant_cached, bot_username));
                        }
                    }
                    InlineProviderPreference::SoundCloud => {
                        if let Ok(tracks) = search_soundcloud_tracks(query_text).await {
                            res.extend(format_soundcloud_results(&tracks, &cache, user_settings.inline_instant_cached, bot_username));
                        }
                    }
                    InlineProviderPreference::YouTube => {
                        if let Ok(videos) = search_youtube_videos(query_text).await {
                            res.extend(format_youtube_results(&videos, &cache, user_settings.inline_instant_cached, bot_username));
                        }
                    }
                    InlineProviderPreference::Mixed => {
                        let ym_fut = search_yandex_tracks(query_text, config.yandex_music_token.as_deref());
                        let yt_fut = search_youtube_videos(query_text);
                        let (ym_res, yt_res) = tokio::join!(ym_fut, yt_fut);

                        let ym_items = match ym_res {
                            Ok(t) => format_yandex_results(&t, &cache, user_settings.inline_instant_cached, bot_username),
                            Err(_) => Vec::new(),
                        };
                        let yt_items = match yt_res {
                            Ok(v) => format_youtube_results(&v, &cache, user_settings.inline_instant_cached, bot_username),
                            Err(_) => Vec::new(),
                        };

                        let mut interleaved = Vec::with_capacity(ym_items.len() + yt_items.len());
                        let max_len = ym_items.len().max(yt_items.len());
                        for i in 0..max_len {
                            if i < ym_items.len() {
                                interleaved.push(ym_items[i].clone());
                            }
                            if i < yt_items.len() {
                                interleaved.push(yt_items[i].clone());
                            }
                        }
                        res.extend(interleaved);
                    }
                }
            }
        }
        res
    };

    let results = tokio::time::timeout(std::time::Duration::from_millis(1500), search_fut)
        .await
        .unwrap_or_default();

    // Pagination slice
    let total_results = results.len();
    let page_items: Vec<InlineQueryResult> = if offset < total_results {
        results.into_iter().skip(offset).take(INLINE_PAGE_SIZE).collect()
    } else {
        Vec::new()
    };

    let next_offset = if offset + INLINE_PAGE_SIZE < total_results {
        (offset + INLINE_PAGE_SIZE).to_string()
    } else {
        String::new()
    };

    let button = InlineQueryResultsButton {
        text: "⚙️ Настройки инлайн-поиска".to_string(),
        kind: InlineQueryResultsButtonKind::StartParameter("settings".to_string()),
    };

    bot.answer_inline_query(q.id, page_items)
        .cache_time(300)
        .is_personal(true)
        .next_offset(next_offset)
        .button(button)
        .await?;

    Ok(())
}

fn build_empty_query_results(bot_username: &str) -> Vec<InlineQueryResult> {
    let card_help = InlineQueryResultArticle::new(
        "help_guide".to_string(),
        "💡 Как пользоваться инлайн-поиском".to_string(),
        InputMessageContent::Text(InputMessageContentText::new(
            "💡 <b>Интерактивный инлайн-поиск EDIBLS</b>\n\n\
            Введите название трека или видео прямо в строке ввода любого чата!\n\n\
            <b>Быстрые префиксы поиска:</b>\n\
            • <code>ym &lt;запрос&gt;</code> — Поиск в Яндекс Музыке\n\
            • <code>sc &lt;запрос&gt;</code> — Поиск в SoundCloud\n\
            • <code>yt &lt;запрос&gt;</code> — Поиск в YouTube\n\
            • <code>ph &lt;запрос&gt;</code> — Поиск в Pornhub (18+)\n\n\
            <i>Если префикс не указан, используется сервис из ваших настроек.</i>"
        ).parse_mode(ParseMode::Html)),
    ).description("Введите поисковый запрос или используйте префиксы ym, sc, yt");

    let card_ym = InlineQueryResultArticle::new(
        "help_ym".to_string(),
        "🎶 Яндекс Музыка (префикс ym)".to_string(),
        InputMessageContent::Text(InputMessageContentText::new(
            "🎶 <b>Яндекс Музыка</b>\n\
            Для поиска музыки наберите в чате:\n\
            <code>@bot ym &lt;название песни или артист&gt;</code>\n\n\
            <i>Поддерживается максимальное качество (FLAC / AAC) и мгновенная отправка из кэша!</i>"
        ).parse_mode(ParseMode::Html)),
    ).description("Пример: @bot ym Queen Bohemian Rhapsody");

    let card_sc = InlineQueryResultArticle::new(
        "help_sc".to_string(),
        "☁️ SoundCloud (префикс sc)".to_string(),
        InputMessageContent::Text(InputMessageContentText::new(
            "☁️ <b>SoundCloud</b>\n\
            Для поиска наберите в чате:\n\
            <code>@bot sc &lt;запрос или ремикс&gt;</code>\n\n\
            <i>Идеально для фонка, ремиксов и независимой музыки.</i>"
        ).parse_mode(ParseMode::Html)),
    ).description("Пример: @bot sc phonk drift");

    let card_yt = InlineQueryResultArticle::new(
        "help_yt".to_string(),
        "🎬 YouTube (префикс yt)".to_string(),
        InputMessageContent::Text(InputMessageContentText::new(
            "🎬 <b>YouTube</b>\n\
            Для поиска видеороликов наберите в чате:\n\
            <code>@bot yt &lt;название видео&gt;</code>"
        ).parse_mode(ParseMode::Html)),
    ).description("Пример: @bot yt funny cats");

    let mut card_settings = InlineQueryResultArticle::new(
        "help_settings".to_string(),
        "⚙️ Настройки бота и поиска".to_string(),
        InputMessageContent::Text(InputMessageContentText::new(
            "⚙️ <b>Настройки EDIBLS</b>\n\
            Вы можете настроить качество аудио, скорость по умолчанию, отправку без сжатия и инлайн-режим."
        ).parse_mode(ParseMode::Html)),
    ).description("Нажмите для перехода в меню настроек бота");

    let settings_url = format!("https://t.me/{bot_username}?start=settings");
    if let Ok(u) = settings_url.parse() {
        card_settings.reply_markup = Some(InlineKeyboardMarkup::new(vec![
            vec![InlineKeyboardButton::url("⚙️ Открыть настройки в боте", u)]
        ]));
    }

    vec![
        InlineQueryResult::Article(card_help),
        InlineQueryResult::Article(card_ym),
        InlineQueryResult::Article(card_sc),
        InlineQueryResult::Article(card_yt),
        InlineQueryResult::Article(card_settings),
    ]
}

fn format_yandex_results(
    tracks: &[YandexTrackInfo],
    cache: &MediaCache,
    instant_cached: bool,
    bot_username: &str,
) -> Vec<InlineQueryResult> {
    let mut results: Vec<InlineQueryResult> = Vec::with_capacity(tracks.len());

    for (idx, track) in tracks.iter().enumerate() {
        let url = track.canonical_url();

        // 1. Check cache for instant delivery
        if instant_cached {
            if let Some(file_id) = cache.get_cached_audio_file_id(&url) {
                let cached_audio = InlineQueryResultCachedAudio::new(
                    format!("ym_{}_{}", track.id, idx),
                    file_id,
                );
                results.push(InlineQueryResult::CachedAudio(cached_audio));
                continue;
            }
        }

        // 2. Uncached track: Return rich Article card with thumbnail cover
        let artists_str = if track.artists.is_empty() {
            "Неизвестный исполнитель".to_string()
        } else {
            track.artists.join(", ")
        };

        let duration_str = match track.duration_ms {
            Some(ms) if ms > 0 => {
                let total_secs = ms / 1000;
                let mins = total_secs / 60;
                let secs = total_secs % 60;
                format!("{mins}:{secs:02}")
            }
            _ => "—".to_string(),
        };

        let message_text = format!(
            "🎵 <b>{} — {}</b>\n\
            ⏱ <b>Длительность:</b> {}\n\
            💿 <b>Источник:</b> <i>Яндекс Музыка</i>\n\n\
            <i>Нажмите кнопку ниже, чтобы получить трек через бота:</i>",
            artists_str, track.title, duration_str
        );

        let url_id = cache.register_url(&url);
        let dl_url = format!("https://t.me/{bot_username}?start=dl_{url_id}");
        let mut keyboard_rows = vec![];
        if let Ok(u) = dl_url.parse() {
            keyboard_rows.push(vec![
                InlineKeyboardButton::url("⬇️ Скачать трек в боте", u),
                InlineKeyboardButton::url(
                    "🎧 В Яндекс Музыке",
                    url.parse().unwrap_or_else(|_| "https://music.yandex.ru".parse().unwrap()),
                ),
            ]);
        }

        let mut article = InlineQueryResultArticle::new(
            format!("ym_{}_{}", track.id, idx),
            format!("🎵 {} — {}", artists_str, track.title),
            InputMessageContent::Text(
                InputMessageContentText::new(message_text).parse_mode(ParseMode::Html),
            ),
        )
        .description(format!("{} • [{}] • Яндекс Музыка", artists_str, duration_str));

        if let Some(cover) = track.cover_url() {
            if let Ok(parsed_url) = reqwest::Url::parse(&cover) {
                article.thumbnail_url = Some(parsed_url);
            }
        }

        if !keyboard_rows.is_empty() {
            article.reply_markup = Some(InlineKeyboardMarkup::new(keyboard_rows));
        }

        results.push(InlineQueryResult::Article(article));
    }

    results
}

fn format_soundcloud_results(
    tracks: &[SoundcloudTrackInfo],
    cache: &MediaCache,
    instant_cached: bool,
    bot_username: &str,
) -> Vec<InlineQueryResult> {
    let mut results = Vec::with_capacity(tracks.len());

    for (idx, track) in tracks.iter().enumerate() {
        let url_id = cache.register_url(&track.url);

        if instant_cached {
            if let Some(file_id) = cache.get_cached_audio_file_id(&track.url) {
                let cached_audio = InlineQueryResultCachedAudio::new(
                    format!("sc_{}_{}", track.id, idx),
                    file_id,
                );
                results.push(InlineQueryResult::CachedAudio(cached_audio));
                continue;
            }
        }

        let uploader_str = track.uploader.as_deref().unwrap_or("SoundCloud");
        let dur_str = track.duration_str.as_deref().unwrap_or("—");

        let message_text = format!(
            "☁️ <b>{} — {}</b>\n\
            ⏱ <b>Длительность:</b> {}\n\
            💿 <b>Источник:</b> <i>SoundCloud</i>\n\n\
            <i>Нажмите кнопку ниже, чтобы получить трек через бота:</i>",
            uploader_str, track.title, dur_str
        );

        let dl_url = format!("https://t.me/{bot_username}?start=dl_{url_id}");
        let mut keyboard_rows = vec![];
        if let Ok(u) = dl_url.parse() {
            keyboard_rows.push(vec![
                InlineKeyboardButton::url("⬇️ Скачать трек в боте", u),
                InlineKeyboardButton::url("🎧 В SoundCloud", track.url.parse().unwrap_or_else(|_| "https://soundcloud.com".parse().unwrap())),
            ]);
        }

        let mut article = InlineQueryResultArticle::new(
            format!("sc_{}_{}", track.id, idx),
            format!("☁️ {} — {}", uploader_str, track.title),
            InputMessageContent::Text(
                InputMessageContentText::new(message_text).parse_mode(ParseMode::Html),
            ),
        )
        .description(format!("{} • [{}] • SoundCloud", uploader_str, dur_str));

        if !keyboard_rows.is_empty() {
            article.reply_markup = Some(InlineKeyboardMarkup::new(keyboard_rows));
        }

        results.push(InlineQueryResult::Article(article));
    }

    results
}

fn format_youtube_results(
    videos: &[YouTubeVideoInfo],
    cache: &MediaCache,
    instant_cached: bool,
    bot_username: &str,
) -> Vec<InlineQueryResult> {
    let mut results = Vec::with_capacity(videos.len());

    for (idx, vid) in videos.iter().enumerate() {
        let url_id = cache.register_url(&vid.url);

        if instant_cached {
            if let Some(video_file_id) = cache.get_cached_video_file_id(&vid.url, None) {
                let cached_video = InlineQueryResultCachedVideo::new(
                    format!("yt_v_{}_{}", vid.id, idx),
                    video_file_id,
                    vid.title.clone(),
                );
                results.push(InlineQueryResult::CachedVideo(cached_video));
                continue;
            } else if let Some(audio_file_id) = cache.get_cached_audio_file_id(&vid.url) {
                let cached_audio = InlineQueryResultCachedAudio::new(
                    format!("yt_a_{}_{}", vid.id, idx),
                    audio_file_id,
                );
                results.push(InlineQueryResult::CachedAudio(cached_audio));
                continue;
            }
        }

        let channel_str = vid.uploader.as_deref().unwrap_or("YouTube");
        let dur_str = vid.duration_str.as_deref().unwrap_or("—");

        let message_text = format!(
            "🎬 <b>{}</b>\n\
            👤 <b>Канал:</b> {}\n\
            ⏱ <b>Длительность:</b> {}\n\
            🔗 {}\n\n\
            <i>Нажмите кнопку ниже, чтобы скачать видео или аудио:</i>",
            vid.title, channel_str, dur_str, vid.url
        );

        let dl_url = format!("https://t.me/{bot_username}?start=dl_{url_id}");
        let mut keyboard_rows = vec![];
        if let Ok(u) = dl_url.parse() {
            keyboard_rows.push(vec![
                InlineKeyboardButton::url("⬇️ Скачать в боте", u),
                InlineKeyboardButton::url("▶️ Смотреть на YouTube", vid.url.parse().unwrap_or_else(|_| "https://youtube.com".parse().unwrap())),
            ]);
        }

        let mut article = InlineQueryResultArticle::new(
            format!("yt_{}_{}", vid.id, idx),
            format!("🎬 {}", vid.title),
            InputMessageContent::Text(
                InputMessageContentText::new(message_text).parse_mode(ParseMode::Html),
            ),
        )
        .description(format!("{} • [{}] • YouTube", channel_str, dur_str));

        if let Ok(thumb_url) = reqwest::Url::parse(&vid.thumbnail_url()) {
            article.thumbnail_url = Some(thumb_url);
        }

        if !keyboard_rows.is_empty() {
            article.reply_markup = Some(InlineKeyboardMarkup::new(keyboard_rows));
        }

        results.push(InlineQueryResult::Article(article));
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inline_query_prefix() {
        assert_eq!(
            parse_inline_query_prefix("ym  queen "),
            (InlineTargetProvider::Yandex, "queen")
        );
        assert_eq!(
            parse_inline_query_prefix("я королева"),
            (InlineTargetProvider::Yandex, "королева")
        );
        assert_eq!(
            parse_inline_query_prefix("sc phonk drift"),
            (InlineTargetProvider::SoundCloud, "phonk drift")
        );
        assert_eq!(
            parse_inline_query_prefix("yt funny cats"),
            (InlineTargetProvider::YouTube, "funny cats")
        );
        assert_eq!(
            parse_inline_query_prefix("ютуб котики"),
            (InlineTargetProvider::YouTube, "котики")
        );
        assert_eq!(
            parse_inline_query_prefix("ph ginger"),
            (InlineTargetProvider::Pornhub, "ginger")
        );
        assert_eq!(
            parse_inline_query_prefix("bohemian rhapsody"),
            (InlineTargetProvider::DefaultOrMixed, "bohemian rhapsody")
        );
        assert_eq!(
            parse_inline_query_prefix("   "),
            (InlineTargetProvider::DefaultOrMixed, "")
        );
    }

    #[test]
    fn test_bot_username_singleton() {
        init_bot_username("test_edibls_bot".to_string());
        assert_eq!(get_bot_username(), Some("test_edibls_bot"));
    }

    #[tokio::test]
    async fn test_format_yandex_results_cached_vs_article() {
        use crate::cache::CachedMedia;
        use teloxide::types::FileId;

        let cache = MediaCache::new();
        let track = YandexTrackInfo {
            id: "123456".to_string(),
            title: "Test Track".to_string(),
            version: None,
            artists: vec!["Test Artist".to_string()],
            duration_ms: Some(215000),
            cover_uri: Some("avatars.yandex.net/get-music-content/123/%%".to_string()),
        };

        // 1. Uncached track -> Article card with cover thumbnail
        let results_uncached = format_yandex_results(&[track.clone()], &cache, true, "test_bot");
        assert_eq!(results_uncached.len(), 1);
        match &results_uncached[0] {
            InlineQueryResult::Article(article) => {
                assert!(article.title.contains("Test Artist — Test Track"));
                assert!(article.description.as_deref().unwrap().contains("3:35"));
                assert!(article.thumbnail_url.as_ref().unwrap().as_str().contains("200x200"));
                let kb = article.reply_markup.as_ref().unwrap();
                let dl_btn = &kb.inline_keyboard[0][0];
                assert!(dl_btn.text.contains("Скачать трек"));
            }
            _ => panic!("Expected Article for uncached track"),
        }

        // 2. Cached track with instant_cached = true -> CachedAudio
        let fid = FileId::from("cached_telegram_audio_fid_999");
        cache.insert(
            &track.canonical_url(),
            true,
            None,
            false,
            CachedMedia::Audio(vec![fid.clone()]),
            5000,
        );

        let results_cached = format_yandex_results(&[track.clone()], &cache, true, "test_bot");
        assert_eq!(results_cached.len(), 1);
        match &results_cached[0] {
            InlineQueryResult::CachedAudio(audio) => {
                assert_eq!(audio.audio_file_id, fid);
            }
            _ => panic!("Expected CachedAudio for cached track"),
        }

        // 3. Cached track with instant_cached = false -> Fallback Article
        let results_cached_disabled = format_yandex_results(&[track], &cache, false, "test_bot");
        assert_eq!(results_cached_disabled.len(), 1);
        assert!(matches!(results_cached_disabled[0], InlineQueryResult::Article(_)));
    }

    #[test]
    fn test_empty_query_results() {
        let results = build_empty_query_results("edibls_bot");
        assert_eq!(results.len(), 5);
        for res in results {
            assert!(matches!(res, InlineQueryResult::Article(_)));
        }
    }
}
