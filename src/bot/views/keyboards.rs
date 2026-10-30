use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, UserId};
use crate::providers::pornhub::PornhubVideoInfo;
use crate::providers::soundcloud::SoundcloudTrackInfo;
use crate::providers::yandex::YandexTrackInfo;
use crate::providers::youtube::{YouTubeChapter, YouTubeVideoInfo};
use crate::settings::{QualityPreference, UserSettings};

pub const CHAPTERS_PAGE_SIZE: usize = 5;
pub const SEARCH_PAGE_SIZE: usize = 5;

pub fn build_audio_action_buttons(url_id: u64) -> InlineKeyboardMarkup {
    let row1 = vec![
        InlineKeyboardButton::callback("✂️ Обрезать", format!("clip:{url_id}")),
        InlineKeyboardButton::callback("⚡ Скорость", format!("speed_a:{url_id}")),
    ];
    InlineKeyboardMarkup::new(vec![row1])
}

pub fn build_video_action_buttons(
    url_id: u64,
    is_youtube: bool,
    is_subway: bool,
) -> InlineKeyboardMarkup {
    let row1 = vec![
        InlineKeyboardButton::callback("🎵 Аудио", format!("audio:{url_id}")),
        InlineKeyboardButton::callback("✂️ Обрезать", format!("clip:{url_id}")),
        InlineKeyboardButton::callback("⚡ Скорость", format!("speed:{url_id}")),
    ];
    let mut row2 = Vec::new();
    if !is_subway {
        row2.push(InlineKeyboardButton::callback(
            "🛹 + Subway Surfers",
            format!("subway:{url_id}"),
        ));
    }
    if is_youtube {
        row2.push(InlineKeyboardButton::callback(
            "📑 Главы видео",
            format!("chapters:{url_id}"),
        ));
    }
    let mut rows = vec![row1];
    if !row2.is_empty() {
        rows.push(row2);
    }
    InlineKeyboardMarkup::new(rows)
}

pub fn build_speed_selection_keyboard(url_id: u64, is_audio: bool) -> InlineKeyboardMarkup {
    let tag = if is_audio { "a" } else { "v" };
    let row1 = vec![
        InlineKeyboardButton::callback("0.5x", format!("spd:{tag}:{url_id}:0.5")),
        InlineKeyboardButton::callback("0.75x", format!("spd:{tag}:{url_id}:0.75")),
        InlineKeyboardButton::callback("0.8x", format!("spd:{tag}:{url_id}:0.8")),
        InlineKeyboardButton::callback("0.85x", format!("spd:{tag}:{url_id}:0.85")),
        InlineKeyboardButton::callback("0.9x", format!("spd:{tag}:{url_id}:0.9")),
    ];
    let row2 = vec![
        InlineKeyboardButton::callback("1.05x", format!("spd:{tag}:{url_id}:1.05")),
        InlineKeyboardButton::callback("1.1x", format!("spd:{tag}:{url_id}:1.1")),
        InlineKeyboardButton::callback("1.15x", format!("spd:{tag}:{url_id}:1.15")),
        InlineKeyboardButton::callback("1.2x", format!("spd:{tag}:{url_id}:1.2")),
    ];
    let row3 = vec![
        InlineKeyboardButton::callback("1.25x", format!("spd:{tag}:{url_id}:1.25")),
        InlineKeyboardButton::callback("1.35x", format!("spd:{tag}:{url_id}:1.35")),
        InlineKeyboardButton::callback("1.5x", format!("spd:{tag}:{url_id}:1.5")),
        InlineKeyboardButton::callback("1.75x", format!("spd:{tag}:{url_id}:1.75")),
        InlineKeyboardButton::callback("2.0x", format!("spd:{tag}:{url_id}:2.0")),
    ];
    let back_tag = if is_audio {
        format!("spd:back_a:{url_id}")
    } else {
        format!("spd:back_v:{url_id}")
    };
    let row4 = vec![InlineKeyboardButton::callback("⬅️ Назад", back_tag)];
    InlineKeyboardMarkup::new(vec![row1, row2, row3, row4])
}

pub fn build_chapters_keyboard(
    url_id: u64,
    chapters: &[YouTubeChapter],
    page: usize,
) -> InlineKeyboardMarkup {
    let total = chapters.len();
    let total_pages = (total + CHAPTERS_PAGE_SIZE - 1) / CHAPTERS_PAGE_SIZE;
    let total_pages = total_pages.max(1);
    let page = page.min(total_pages.saturating_sub(1));

    let start = page * CHAPTERS_PAGE_SIZE;
    let end = (start + CHAPTERS_PAGE_SIZE).min(total);
    let page_chapters = &chapters[start..end];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = page_chapters
        .iter()
        .enumerate()
        .map(|(offset, ch)| {
            let global_idx = start + offset;
            vec![InlineKeyboardButton::callback(
                ch.button_label(),
                format!("ch:get:{url_id}:{global_idx}"),
            )]
        })
        .collect();

    if total_pages > 1 {
        let mut nav = Vec::new();
        if page > 0 {
            nav.push(InlineKeyboardButton::callback(
                "⬅️ Назад",
                format!("ch:p:{url_id}:{}", page - 1),
            ));
        }
        nav.push(InlineKeyboardButton::callback(
            format!("{}/{}", page + 1, total_pages),
            "noop",
        ));
        if page + 1 < total_pages {
            nav.push(InlineKeyboardButton::callback(
                "Вперёд ➡️",
                format!("ch:p:{url_id}:{}", page + 1),
            ));
        }
        rows.push(nav);
    }

    rows.push(vec![InlineKeyboardButton::callback(
        "✖️ Закрыть",
        "ch:close",
    )]);

    InlineKeyboardMarkup::new(rows)
}

pub fn build_cancel_download_keyboard(user_id: UserId) -> InlineKeyboardMarkup {
    InlineKeyboardMarkup::new(vec![vec![InlineKeyboardButton::callback(
        "✖️ Отменить",
        format!("dl:cancel:{}", user_id.0),
    )]])
}

pub fn build_quality_selection_keyboard(url_id: u64) -> InlineKeyboardMarkup {
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

pub fn build_settings_keyboard(settings: &UserSettings) -> InlineKeyboardMarkup {
    let q_label = match settings.quality {
        QualityPreference::AutoBest => "🎬 Качество видео: Авто ⚡",
        QualityPreference::AskEveryTime => "🎬 Качество видео: Вручную ❓",
    };

    let speed_label = format!("⚡ Скорость: {}", settings.default_speed.label());

    let ym_label = match settings.ym_quality {
        crate::settings::YandexQualityPreference::Optimal => "🎶 Я.Музыка: AAC 192k ⚡",
        crate::settings::YandexQualityPreference::Lossless => "🎶 Я.Музыка: FLAC 💎",
    };

    let search_label = match settings.default_search {
        crate::settings::DefaultSearchProvider::Yandex => "🔍 Поиск в ЛС: Я.Музыка 🎶",
        crate::settings::DefaultSearchProvider::SoundCloud => "🔍 Поиск в ЛС: SoundCloud ☁️",
        crate::settings::DefaultSearchProvider::YouTube => "🔍 Поиск в ЛС: YouTube 🎬",
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
        vec![InlineKeyboardButton::callback(q_label, "settings:toggle:quality")],
        vec![InlineKeyboardButton::callback(speed_label, "settings:toggle:speed")],
        vec![InlineKeyboardButton::callback(ym_label, "settings:toggle:ym_quality")],
        vec![InlineKeyboardButton::callback(search_label, "settings:toggle:search")],
        vec![InlineKeyboardButton::callback(audio_label, "settings:toggle:audio")],
        vec![InlineKeyboardButton::callback(doc_label, "settings:toggle:document")],
        vec![InlineKeyboardButton::callback("🌐 Инлайн-режим (@bot) ⚙️", "settings:menu:inline")],
        vec![InlineKeyboardButton::callback("✖️ Закрыть", "settings:close")],
    ])
}

pub fn build_inline_settings_keyboard(settings: &UserSettings) -> InlineKeyboardMarkup {
    let prov_label = format!("🔍 Без префикса: {}", settings.inline_provider.label());
    let instant_label = if settings.inline_instant_cached {
        "⚡ Отправка из кэша: ВКЛ ✅"
    } else {
        "⚡ Отправка из кэша: ВЫКЛ ❌"
    };
    let nsfw_label = if settings.inline_allow_nsfw {
        "🔞 Контент 18+ (Pornhub): ВКЛ ⚠️"
    } else {
        "🔞 Контент 18+ (Pornhub): ВЫКЛ 🛡️"
    };

    InlineKeyboardMarkup::new(vec![
        vec![InlineKeyboardButton::callback(prov_label, "settings:toggle:inline_provider")],
        vec![InlineKeyboardButton::callback(instant_label, "settings:toggle:inline_instant")],
        vec![InlineKeyboardButton::callback(nsfw_label, "settings:toggle:inline_nsfw")],
        vec![InlineKeyboardButton::callback("⬅️ Назад в настройки", "settings:menu:main")],
        vec![InlineKeyboardButton::callback("✖️ Закрыть", "settings:close")],
    ])
}

pub fn build_track_selection_keyboard(
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

    rows.push(vec![
        InlineKeyboardButton::callback("☁️ SoundCloud", format!("sw:sc:{search_id}")),
        InlineKeyboardButton::callback("🎬 YouTube", format!("sw:yt:{search_id}")),
    ]);

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена",
        "ym:cancel",
    )]);

    InlineKeyboardMarkup::new(rows)
}

pub fn build_ph_selection_keyboard(
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

pub fn build_yt_selection_keyboard(
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

    rows.push(vec![
        InlineKeyboardButton::callback("🎶 Я.Музыка", format!("sw:ym:{search_id}")),
        InlineKeyboardButton::callback("☁️ SoundCloud", format!("sw:sc:{search_id}")),
    ]);

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена",
        "yt:cancel",
    )]);

    InlineKeyboardMarkup::new(rows)
}

pub fn build_sc_selection_keyboard(
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

    rows.push(vec![
        InlineKeyboardButton::callback("🎶 Я.Музыка", format!("sw:ym:{search_id}")),
        InlineKeyboardButton::callback("🎬 YouTube", format!("sw:yt:{search_id}")),
    ]);

    rows.push(vec![InlineKeyboardButton::callback(
        "❌ Отмена",
        "sc:cancel",
    )]);

    InlineKeyboardMarkup::new(rows)
}

pub fn build_empty_search_keyboard(search_id: u64, current: &str) -> InlineKeyboardMarkup {
    let mut switch_row = Vec::new();
    if current != "ym" {
        switch_row.push(InlineKeyboardButton::callback(
            "🎶 Я.Музыка",
            format!("sw:ym:{search_id}"),
        ));
    }
    if current != "sc" {
        switch_row.push(InlineKeyboardButton::callback(
            "☁️ SoundCloud",
            format!("sw:sc:{search_id}"),
        ));
    }
    if current != "yt" {
        switch_row.push(InlineKeyboardButton::callback(
            "🎬 YouTube",
            format!("sw:yt:{search_id}"),
        ));
    }
    InlineKeyboardMarkup::new(vec![
        switch_row,
        vec![InlineKeyboardButton::callback("❌ Закрыть", "search:close")],
    ])
}
