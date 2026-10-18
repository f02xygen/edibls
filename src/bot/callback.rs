#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchProviderType {
    Yandex,
    SoundCloud,
    YouTube,
    Pornhub,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsToggle {
    Quality,
    Speed,
    YmQuality,
    DefaultSearch,
    AudioOnly,
    SendAsDocument,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CallbackAction {
    CancelDownload { user_id: u64 },
    CloseSearch,
    SwitchSearch { target: SearchProviderType, search_id: u64 },
    SettingsClose,
    SettingsToggle(SettingsToggle),
    Noop,
    Subway { url_id: u64 },
    SpeedMenu { url_id: u64, is_audio: bool },
    SpeedBack { url_id: u64, is_audio: bool },
    SpeedApply { url_id: u64, speed: f32, is_audio: bool },
    ClipPrompt { url_id: u64 },
    ChaptersMenu { url_id: u64 },
    ChaptersPage { url_id: u64, page: usize },
    ChaptersGet { url_id: u64, chapter_idx: usize },
    ChaptersClose,
    ExtractAudio { url_id: u64 },
    QualityCancel,
    QualitySelect { height: u16, url_id: u64 },
    SearchCancel(SearchProviderType),
    SearchPage { provider: SearchProviderType, search_id: u64, page: usize },
    SearchNoop { provider: SearchProviderType, search_id: u64, page: usize },
    SearchSelectYm { track_id: String },
    SearchSelectYt { video_id: String },
    SearchSelectSc { search_id: u64, item_idx: usize },
    SearchSelectPh { viewkey: String },
    Unknown(String),
}

impl CallbackAction {
    pub fn parse(data: &str) -> Self {
        if let Some(uid_str) = data.strip_prefix("dl:cancel:") {
            if let Ok(uid) = uid_str.parse::<u64>() {
                return Self::CancelDownload { user_id: uid };
            }
        }

        if data == "search:close" {
            return Self::CloseSearch;
        }

        if let Some(sw) = data.strip_prefix("sw:") {
            let parts: Vec<&str> = sw.split(':').collect();
            if parts.len() == 2 {
                if let Ok(sid) = parts[1].parse::<u64>() {
                    let provider = match parts[0] {
                        "ym" => Some(SearchProviderType::Yandex),
                        "sc" => Some(SearchProviderType::SoundCloud),
                        "yt" => Some(SearchProviderType::YouTube),
                        _ => None,
                    };
                    if let Some(p) = provider {
                        return Self::SwitchSearch { target: p, search_id: sid };
                    }
                }
            }
        }

        if let Some(sub) = data.strip_prefix("settings:") {
            return match sub {
                "close" => Self::SettingsClose,
                "toggle:quality" | "q:auto" | "q:manual" => Self::SettingsToggle(SettingsToggle::Quality),
                "toggle:speed" => Self::SettingsToggle(SettingsToggle::Speed),
                "toggle:ym_quality" | "ym:optimal" | "ym:lossless" => Self::SettingsToggle(SettingsToggle::YmQuality),
                "toggle:search" => Self::SettingsToggle(SettingsToggle::DefaultSearch),
                "toggle:audio" => Self::SettingsToggle(SettingsToggle::AudioOnly),
                "toggle:document" => Self::SettingsToggle(SettingsToggle::SendAsDocument),
                _ => Self::Unknown(data.to_string()),
            };
        }

        if data == "noop" {
            return Self::Noop;
        }

        if data == "ch:close" {
            return Self::ChaptersClose;
        }

        if let Some(id_str) = data.strip_prefix("subway:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::Subway { url_id: uid };
            }
        }

        if let Some(id_str) = data.strip_prefix("speed:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::SpeedMenu { url_id: uid, is_audio: false };
            }
        }

        if let Some(id_str) = data.strip_prefix("speed_a:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::SpeedMenu { url_id: uid, is_audio: true };
            }
        }

        if let Some(id_str) = data.strip_prefix("spd:back_v:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::SpeedBack { url_id: uid, is_audio: false };
            }
        }

        if let Some(id_str) = data.strip_prefix("spd:back_a:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::SpeedBack { url_id: uid, is_audio: true };
            }
        }

        if let Some(spd_payload) = data.strip_prefix("spd:") {
            if let Some(rest) = spd_payload.strip_prefix("a:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(uid), Ok(spd)) = (parts[0].parse::<u64>(), parts[1].parse::<f32>()) {
                        return Self::SpeedApply { url_id: uid, speed: spd, is_audio: true };
                    }
                }
            } else if let Some(rest) = spd_payload.strip_prefix("v:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(uid), Ok(spd)) = (parts[0].parse::<u64>(), parts[1].parse::<f32>()) {
                        return Self::SpeedApply { url_id: uid, speed: spd, is_audio: false };
                    }
                }
            } else {
                let parts: Vec<&str> = spd_payload.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(uid), Ok(spd)) = (parts[0].parse::<u64>(), parts[1].parse::<f32>()) {
                        return Self::SpeedApply { url_id: uid, speed: spd, is_audio: false };
                    }
                }
            }
        }

        if let Some(id_str) = data.strip_prefix("clip:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::ClipPrompt { url_id: uid };
            }
        }

        if let Some(id_str) = data.strip_prefix("chapters:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::ChaptersMenu { url_id: uid };
            }
        }

        if let Some(p_payload) = data.strip_prefix("ch:p:") {
            let parts: Vec<&str> = p_payload.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(uid), Ok(page)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    return Self::ChaptersPage { url_id: uid, page };
                }
            }
        }

        if let Some(get_payload) = data.strip_prefix("ch:get:") {
            let parts: Vec<&str> = get_payload.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(uid), Ok(idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    return Self::ChaptersGet { url_id: uid, chapter_idx: idx };
                }
            }
        }

        if let Some(id_str) = data.strip_prefix("audio:") {
            if let Ok(uid) = id_str.parse::<u64>() {
                return Self::ExtractAudio { url_id: uid };
            }
        }

        if let Some(q_payload) = data.strip_prefix("q:") {
            if q_payload == "cancel" {
                return Self::QualityCancel;
            }
            let parts: Vec<&str> = q_payload.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(h), Ok(uid)) = (parts[0].parse::<u16>(), parts[1].parse::<u64>()) {
                    return Self::QualitySelect { height: h, url_id: uid };
                }
            }
        }

        if let Some(ph) = data.strip_prefix("ph:") {
            if ph == "cancel" {
                return Self::SearchCancel(SearchProviderType::Pornhub);
            }
            if let Some(rest) = ph.strip_prefix("p:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchPage { provider: SearchProviderType::Pornhub, search_id: sid, page: p };
                    }
                }
            }
            if let Some(rest) = ph.strip_prefix("noop:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchNoop { provider: SearchProviderType::Pornhub, search_id: sid, page: p };
                    }
                }
            }
            return Self::SearchSelectPh { viewkey: ph.to_string() };
        }

        if let Some(ym) = data.strip_prefix("ym:") {
            if ym == "cancel" {
                return Self::SearchCancel(SearchProviderType::Yandex);
            }
            if let Some(rest) = ym.strip_prefix("p:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchPage { provider: SearchProviderType::Yandex, search_id: sid, page: p };
                    }
                }
            }
            if let Some(rest) = ym.strip_prefix("noop:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchNoop { provider: SearchProviderType::Yandex, search_id: sid, page: p };
                    }
                }
            }
            return Self::SearchSelectYm { track_id: ym.to_string() };
        }

        if let Some(yt) = data.strip_prefix("yt:") {
            if yt == "cancel" {
                return Self::SearchCancel(SearchProviderType::YouTube);
            }
            if let Some(rest) = yt.strip_prefix("p:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchPage { provider: SearchProviderType::YouTube, search_id: sid, page: p };
                    }
                }
            }
            if let Some(rest) = yt.strip_prefix("noop:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchNoop { provider: SearchProviderType::YouTube, search_id: sid, page: p };
                    }
                }
            }
            return Self::SearchSelectYt { video_id: yt.to_string() };
        }

        if let Some(sc) = data.strip_prefix("sc:") {
            if sc == "cancel" {
                return Self::SearchCancel(SearchProviderType::SoundCloud);
            }
            if let Some(rest) = sc.strip_prefix("p:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchPage { provider: SearchProviderType::SoundCloud, search_id: sid, page: p };
                    }
                }
            }
            if let Some(rest) = sc.strip_prefix("noop:") {
                let parts: Vec<&str> = rest.split(':').collect();
                if parts.len() == 2 {
                    if let (Ok(sid), Ok(p)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                        return Self::SearchNoop { provider: SearchProviderType::SoundCloud, search_id: sid, page: p };
                    }
                }
            }
            let parts: Vec<&str> = sc.split(':').collect();
            if parts.len() == 2 {
                if let (Ok(sid), Ok(idx)) = (parts[0].parse::<u64>(), parts[1].parse::<usize>()) {
                    return Self::SearchSelectSc { search_id: sid, item_idx: idx };
                }
            }
        }

        Self::Unknown(data.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_callback_actions() {
        assert_eq!(
            CallbackAction::parse("dl:cancel:12345"),
            CallbackAction::CancelDownload { user_id: 12345 }
        );
        assert_eq!(CallbackAction::parse("search:close"), CallbackAction::CloseSearch);
        assert_eq!(
            CallbackAction::parse("sw:yt:99"),
            CallbackAction::SwitchSearch { target: SearchProviderType::YouTube, search_id: 99 }
        );
        assert_eq!(
            CallbackAction::parse("settings:toggle:speed"),
            CallbackAction::SettingsToggle(SettingsToggle::Speed)
        );
        assert_eq!(
            CallbackAction::parse("spd:a:42:1.15"),
            CallbackAction::SpeedApply { url_id: 42, speed: 1.15, is_audio: true }
        );
        assert_eq!(
            CallbackAction::parse("spd:v:42:1.25"),
            CallbackAction::SpeedApply { url_id: 42, speed: 1.25, is_audio: false }
        );
        assert_eq!(
            CallbackAction::parse("spd:42:1.5"),
            CallbackAction::SpeedApply { url_id: 42, speed: 1.5, is_audio: false }
        );
        assert_eq!(
            CallbackAction::parse("q:720:55"),
            CallbackAction::QualitySelect { height: 720, url_id: 55 }
        );
        assert_eq!(
            CallbackAction::parse("ym:123456"),
            CallbackAction::SearchSelectYm { track_id: "123456".to_string() }
        );
    }
}
