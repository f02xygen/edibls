use crate::settings::{QualityPreference, UserSettings};

pub use crate::media::format_progress_bar;

pub const WELCOME_MESSAGE: &str = "\
Привет! Я бот для скачивания медиа в высоком качестве.

<b>Возможности:</b>
• <b>Скачивание:</b> отправь ссылку на YouTube, TikTok, Instagram, VK, SoundCloud, Яндекс Музыку, Pornhub и сотни других сайтов.
• <b>Нарезка клипов:</b> отправь <code>/clip &lt;ссылка&gt; 01:15-02:30</code> или <code>&lt;ссылка&gt; 15-45</code>, либо ответь таймкодом.
• <b>Скорость (0.5x..2x):</b> кнопка <code>⚡ Скорость</code> под медиа, модификатор в ссылке (<code>&lt;ссылка&gt; 1.5x</code>), либо ответь на медиа: <code>1.5x</code> или <code>2x</code>.
• <b>Управление ответами (Reply):</b> ответь на любое видео/аудио сообщением <code>2x</code>, <code>10-40</code>, <code>/audio</code> или <code>/subway</code>.
• <b>Subway Surfers:</b> кнопка <code>🛹 + Subway Surfers</code> под видео или <code>/subway &lt;url&gt;</code>.
• <b>Только аудио:</b> <code>/audio &lt;ссылка&gt;</code> или прямая ссылка на SoundCloud / Яндекс Музыку.
• <b>Поиск:</b> <code>/yt</code> (YouTube), <code>/sc</code> (SoundCloud), <code>/ym</code> (Яндекс Музыка), <code>/ph</code> (Pornhub).
• <b>Настройки:</b> <code>/settings</code> (качество, скорость по умолчанию, аудио, отправка файлом, FLAC)

<i>Поддерживаются медиа в высоком качестве (до 2 ГБ) без водяных знаков.</i>";

pub fn build_settings_text(settings: &UserSettings) -> String {
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
    let search_desc = match settings.default_search {
        crate::settings::DefaultSearchProvider::Yandex => "Яндекс Музыка 🎶",
        crate::settings::DefaultSearchProvider::SoundCloud => "SoundCloud ☁️",
        crate::settings::DefaultSearchProvider::YouTube => "YouTube 🎬",
    };

    format!(
        "⚙️ <b>Настройки загрузки</b>\n\n\
        🎬 <b>Качество видео:</b> {q_desc}\n\
        ⚡ <b>Скорость по умолчанию:</b> {}\n\
        🎶 <b>Яндекс Музыка:</b> {ym_desc}\n\
        🔍 <b>Поиск в ЛС:</b> {search_desc}\n\
        🎵 <b>Только аудио:</b> {a_desc}\n\
        📎 <b>Отправка файлом:</b> {doc_desc}\n\n\
        <i>Нажми на кнопку, чтобы переключить:</i>",
        settings.default_speed.label()
    )
}

pub fn format_ph_search_text(query: &str, page: usize, total_pages: usize) -> String {
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

pub fn format_ym_search_text(query: &str, page: usize, total_pages: usize) -> String {
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

pub fn format_yt_search_text(query: &str, page: usize, total_pages: usize) -> String {
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

pub fn format_sc_search_text(query: &str, page: usize, total_pages: usize) -> String {
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
