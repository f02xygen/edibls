use std::sync::Arc;
use teloxide::prelude::*;

use crate::cache::MediaCache;
use crate::config::Config;
use crate::queue::QueueManager;
use crate::settings::UserSettingsStore;

pub mod callback;
pub mod handlers;
pub mod views;

pub use callback::*;
pub use handlers::*;
#[allow(unused_imports)]
pub use views::*;

#[allow(unused_imports)]
pub use crate::pipeline::{
    process_and_send_media, try_get_cached_video_file, MediaPipelineOptions,
};

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

    let action = CallbackAction::parse(data);
    handlers::handle_callback_action(&bot, q, action, &config, &queue, &cache, &settings).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::soundcloud::SoundcloudTrackInfo;
    use crate::providers::yandex::YandexTrackInfo;
    use crate::providers::youtube::{YouTubeChapter, YouTubeVideoInfo};
    use crate::settings::{DefaultSearchProvider, QualityPreference, UserSettings};

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
                cover_uri: None,
            })
            .collect();

        let kb = build_track_selection_keyboard(123, &tracks, 0);
        // 3 tracks + 1 switch row + 1 cancel row = 5 rows
        assert_eq!(kb.inline_keyboard.len(), 5);
        assert_eq!(kb.inline_keyboard[3][0].text, "☁️ SoundCloud");
        assert_eq!(kb.inline_keyboard[3][1].text, "🎬 YouTube");
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
                cover_uri: None,
            })
            .collect();

        // Page 0 (out of 3 pages: 5, 5, 2)
        let kb_p0 = build_track_selection_keyboard(123, &tracks, 0);
        // 5 tracks + 1 nav row + 1 switch row + 1 cancel row = 8 rows
        assert_eq!(kb_p0.inline_keyboard.len(), 8);
        let nav_row_p0 = &kb_p0.inline_keyboard[5];
        assert_eq!(nav_row_p0.len(), 2);
        assert_eq!(nav_row_p0[0].text, "1/3");
        assert_eq!(nav_row_p0[1].text, "Вперёд ➡️");

        // Page 1 (middle page)
        let kb_p1 = build_track_selection_keyboard(123, &tracks, 1);
        assert_eq!(kb_p1.inline_keyboard.len(), 8);
        let nav_row_p1 = &kb_p1.inline_keyboard[5];
        assert_eq!(nav_row_p1.len(), 3);
        assert_eq!(nav_row_p1[0].text, "⬅️ Назад");
        assert_eq!(nav_row_p1[1].text, "2/3");
        assert_eq!(nav_row_p1[2].text, "Вперёд ➡️");

        // Page 2 (last page, 2 items)
        let kb_p2 = build_track_selection_keyboard(123, &tracks, 2);
        // 2 tracks + 1 nav row + 1 switch row + 1 cancel row = 5 rows
        assert_eq!(kb_p2.inline_keyboard.len(), 5);
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
        // 5 videos + 1 nav row + 1 switch row + 1 cancel row = 8 rows
        assert_eq!(kb.inline_keyboard.len(), 8);
        let nav_row = &kb.inline_keyboard[5];
        assert_eq!(nav_row.len(), 2);
        assert_eq!(nav_row[0].text, "1/2");
        assert_eq!(nav_row[1].text, "Вперёд ➡️");
        assert_eq!(kb.inline_keyboard[6][0].text, "🎶 Я.Музыка");
        assert_eq!(kb.inline_keyboard[6][1].text, "☁️ SoundCloud");
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
        // 2 tracks (page 1) + 1 nav row + 1 switch row + 1 cancel row = 5 rows
        assert_eq!(kb.inline_keyboard.len(), 5);
        let nav_row = &kb.inline_keyboard[2];
        assert_eq!(nav_row.len(), 2);
        assert_eq!(nav_row[0].text, "⬅️ Назад");
        assert_eq!(nav_row[1].text, "2/2");
        assert_eq!(kb.inline_keyboard[3][0].text, "🎶 Я.Музыка");
        assert_eq!(kb.inline_keyboard[3][1].text, "🎬 YouTube");
    }

    #[test]
    fn test_build_settings_keyboard_and_text() {
        let mut settings = UserSettings::default();
        let kb = build_settings_keyboard(&settings);
        assert_eq!(kb.inline_keyboard.len(), 8);
        assert_eq!(kb.inline_keyboard[0].len(), 1);
        assert_eq!(kb.inline_keyboard[0][0].text, "🎬 Качество видео: Авто ⚡");

        assert_eq!(kb.inline_keyboard[1].len(), 1);
        assert_eq!(kb.inline_keyboard[1][0].text, "⚡ Скорость: 1.0x (Обычная)");

        assert_eq!(kb.inline_keyboard[2].len(), 1);
        assert_eq!(kb.inline_keyboard[2][0].text, "🎶 Я.Музыка: AAC 192k ⚡");

        assert_eq!(kb.inline_keyboard[3].len(), 1);
        assert_eq!(kb.inline_keyboard[3][0].text, "🔍 Поиск в ЛС: Я.Музыка 🎶");

        assert_eq!(kb.inline_keyboard[4].len(), 1);
        assert_eq!(kb.inline_keyboard[4][0].text, "🎵 Только аудио: ВЫКЛ ❌");

        assert_eq!(kb.inline_keyboard[5].len(), 1);
        assert_eq!(kb.inline_keyboard[5][0].text, "📎 Отправка файлом: ВЫКЛ ❌");

        assert_eq!(kb.inline_keyboard[6].len(), 1);
        assert_eq!(kb.inline_keyboard[6][0].text, "🌐 Инлайн-режим (@bot) ⚙️");

        assert_eq!(kb.inline_keyboard[7].len(), 1);
        assert_eq!(kb.inline_keyboard[7][0].text, "✖️ Закрыть");

        settings.quality = QualityPreference::AskEveryTime;
        settings.default_speed = crate::settings::SpeedPreference::Faster15;
        settings.ym_quality = crate::settings::YandexQualityPreference::Lossless;
        settings.default_search = DefaultSearchProvider::SoundCloud;
        settings.audio_only = true;
        settings.send_as_document = true;
        let kb_custom = build_settings_keyboard(&settings);
        assert_eq!(kb_custom.inline_keyboard[0].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[0][0].text, "🎬 Качество видео: Вручную ❓");
        assert_eq!(kb_custom.inline_keyboard[1].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[1][0].text, "⚡ Скорость: 1.5x");
        assert_eq!(kb_custom.inline_keyboard[2].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[2][0].text, "🎶 Я.Музыка: FLAC 💎");
        assert_eq!(kb_custom.inline_keyboard[3].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[3][0].text, "🔍 Поиск в ЛС: SoundCloud ☁️");
        assert_eq!(kb_custom.inline_keyboard[4].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[4][0].text, "🎵 Только аудио: ВКЛ ✅");
        assert_eq!(kb_custom.inline_keyboard[5].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[5][0].text, "📎 Отправка файлом: ВКЛ ✅");
        assert_eq!(kb_custom.inline_keyboard[6].len(), 1);
        assert_eq!(kb_custom.inline_keyboard[6][0].text, "🌐 Инлайн-режим (@bot) ⚙️");

        let text = build_settings_text(&settings);
        assert!(text.contains("Вручную"));
        assert!(text.contains("1.5x"));
        assert!(text.contains("FLAC"));
        assert!(text.contains("SoundCloud"));
        assert!(text.contains("Включён"));
        assert!(text.contains("Включена"));

        // Check inline settings keyboard & text
        let inline_kb = build_inline_settings_keyboard(&settings);
        assert_eq!(inline_kb.inline_keyboard.len(), 5);
        assert_eq!(inline_kb.inline_keyboard[0][0].text, "🔍 Без префикса: Как в поиске (Авто) 🔄");
        assert_eq!(inline_kb.inline_keyboard[1][0].text, "⚡ Отправка из кэша: ВКЛ ✅");
        assert_eq!(inline_kb.inline_keyboard[2][0].text, "🔞 Контент 18+ (Pornhub): ВЫКЛ 🛡️");
        assert_eq!(inline_kb.inline_keyboard[3][0].text, "⬅️ Назад в настройки");

        let inline_text = build_inline_settings_text(&settings);
        assert!(inline_text.contains("Инлайн-режим"));
        assert!(inline_text.contains("Включена"));
        assert!(inline_text.contains("Скрыт"));
    }

    #[test]
    fn test_build_empty_search_keyboard() {
        let kb_ym = build_empty_search_keyboard(77, "ym");
        assert_eq!(kb_ym.inline_keyboard.len(), 2);
        assert_eq!(kb_ym.inline_keyboard[0].len(), 2);
        assert_eq!(kb_ym.inline_keyboard[0][0].text, "☁️ SoundCloud");
        assert_eq!(kb_ym.inline_keyboard[0][1].text, "🎬 YouTube");
        assert_eq!(kb_ym.inline_keyboard[1][0].text, "❌ Закрыть");

        let kb_yt = build_empty_search_keyboard(77, "yt");
        assert_eq!(kb_yt.inline_keyboard[0].len(), 2);
        assert_eq!(kb_yt.inline_keyboard[0][0].text, "🎶 Я.Музыка");
        assert_eq!(kb_yt.inline_keyboard[0][1].text, "☁️ SoundCloud");
    }

    #[test]
    fn test_build_cancel_download_keyboard() {
        let kb = build_cancel_download_keyboard(UserId(12345));
        assert_eq!(kb.inline_keyboard.len(), 1);
        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref data) = kb.inline_keyboard[0][0].kind {
            assert_eq!(data, "dl:cancel:12345");
        } else {
            panic!("Expected CallbackData");
        }
    }

    #[test]
    fn test_build_video_action_buttons() {
        // Standard video (not YT, not Subway)
        let kb = build_video_action_buttons(42, false, false);
        assert_eq!(kb.inline_keyboard.len(), 2);
        assert_eq!(kb.inline_keyboard[0].len(), 3);
        assert_eq!(kb.inline_keyboard[0][0].text, "🎵 Аудио");
        assert_eq!(kb.inline_keyboard[0][1].text, "✂️ Обрезать");
        assert_eq!(kb.inline_keyboard[0][2].text, "⚡ Скорость");
        assert_eq!(kb.inline_keyboard[1].len(), 1);
        assert_eq!(kb.inline_keyboard[1][0].text, "🛹 + Subway Surfers");

        // YouTube video
        let kb_yt = build_video_action_buttons(42, true, false);
        assert_eq!(kb_yt.inline_keyboard.len(), 2);
        assert_eq!(kb_yt.inline_keyboard[1].len(), 2);
        assert_eq!(kb_yt.inline_keyboard[1][0].text, "🛹 + Subway Surfers");
        assert_eq!(kb_yt.inline_keyboard[1][1].text, "📑 Главы видео");

        // Subway Surfers video (no duplicate subway button)
        let kb_sub = build_video_action_buttons(42, false, true);
        assert_eq!(kb_sub.inline_keyboard.len(), 1);
        assert_eq!(kb_sub.inline_keyboard[0].len(), 3);
        assert_eq!(kb_sub.inline_keyboard[0][0].text, "🎵 Аудио");
        assert_eq!(kb_sub.inline_keyboard[0][1].text, "✂️ Обрезать");
        assert_eq!(kb_sub.inline_keyboard[0][2].text, "⚡ Скорость");
    }

    #[test]
    fn test_build_audio_and_speed_keyboards() {
        let kb_audio = build_audio_action_buttons(100);
        assert_eq!(kb_audio.inline_keyboard.len(), 1);
        assert_eq!(kb_audio.inline_keyboard[0].len(), 2);
        assert_eq!(kb_audio.inline_keyboard[0][0].text, "✂️ Обрезать");
        assert_eq!(kb_audio.inline_keyboard[0][1].text, "⚡ Скорость");

        let kb_spd_v = build_speed_selection_keyboard(100, false);
        assert_eq!(kb_spd_v.inline_keyboard.len(), 4);
        assert_eq!(kb_spd_v.inline_keyboard[0].len(), 5);
        assert_eq!(kb_spd_v.inline_keyboard[1].len(), 4);
        assert_eq!(kb_spd_v.inline_keyboard[2].len(), 5);
        assert_eq!(kb_spd_v.inline_keyboard[3].len(), 1);
        assert_eq!(kb_spd_v.inline_keyboard[3][0].text, "⬅️ Назад");
        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref data) = kb_spd_v.inline_keyboard[3][0].kind {
            assert_eq!(data, "spd:back_v:100");
        } else {
            panic!("Expected CallbackData");
        }
        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref data) = kb_spd_v.inline_keyboard[1][1].kind {
            assert_eq!(data, "spd:v:100:1.1");
        } else {
            panic!("Expected CallbackData");
        }

        let kb_spd_a = build_speed_selection_keyboard(100, true);
        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref data) = kb_spd_a.inline_keyboard[3][0].kind {
            assert_eq!(data, "spd:back_a:100");
        } else {
            panic!("Expected CallbackData");
        }
        if let teloxide::types::InlineKeyboardButtonKind::CallbackData(ref data) = kb_spd_a.inline_keyboard[1][2].kind {
            assert_eq!(data, "spd:a:100:1.15");
        } else {
            panic!("Expected CallbackData");
        }
    }

    #[test]
    fn test_build_chapters_keyboard() {
        let chapters: Vec<YouTubeChapter> = (1..=7)
            .map(|i| YouTubeChapter {
                title: format!("Chapter {i}"),
                start_secs: (i - 1) * 60,
                end_secs: i * 60,
            })
            .collect();

        // Page 0 (items 1..=5)
        let kb0 = build_chapters_keyboard(99, &chapters, 0);
        assert_eq!(kb0.inline_keyboard.len(), 7);
        assert_eq!(kb0.inline_keyboard[0][0].text, "00:00 Chapter 1 [1:00]");
        assert_eq!(kb0.inline_keyboard[5][0].text, "1/2");
        assert_eq!(kb0.inline_keyboard[5][1].text, "Вперёд ➡️");

        // Page 1 (items 6..=7)
        let kb1 = build_chapters_keyboard(99, &chapters, 1);
        assert_eq!(kb1.inline_keyboard.len(), 4);
        assert_eq!(kb1.inline_keyboard[0][0].text, "05:00 Chapter 6 [1:00]");
        assert_eq!(kb1.inline_keyboard[2][0].text, "⬅️ Назад");
        assert_eq!(kb1.inline_keyboard[2][1].text, "2/2");
    }
}
