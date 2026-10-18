use crate::providers::soundcloud::is_soundcloud_url;
use crate::providers::yandex::is_yandex_music_url;

/// Extracts the first HTTP/HTTPS URL found in the message text.
pub fn extract_url(text: &str) -> Option<&str> {
    text.split_whitespace()
        .find(|word| word.starts_with("http://") || word.starts_with("https://"))
}

/// Extracts all unique HTTP/HTTPS URLs found in the message text (up to max 5 URLs).
pub fn extract_all_urls(text: &str) -> Vec<&str> {
    let mut urls = Vec::new();
    for word in text.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") {
            let clean = word.trim_end_matches([',', '.', ';', '!', '?', ')', ']', '>', '"', '\'']);
            if !urls.contains(&clean) {
                urls.push(clean);
                if urls.len() >= 5 {
                    break;
                }
            }
        }
    }
    urls
}

/// Extracts a SoundCloud search query from the message text (/sc <query> or /soundcloud <query>).
pub fn extract_sc_search_query(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/sc" | "/soundcloud") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// Extracts a YouTube search query from the message text (/yt <query> or /youtube <query>).
pub fn extract_yt_search_query(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/yt" | "/youtube") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// Extracts a Pornhub search query from the message text (/ph <query> or /pornhub <query>).
pub fn extract_ph_search_query(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/ph" | "/pornhub") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// Extracts a Yandex Music track search query from the message text.
/// Supports `/ym <query>`, `/audio <query>`, `/mp3 <query>`, `/search <query>` in any chat,
/// as well as plain text (without links or slash commands) in private chats.
pub fn extract_search_query(text: &str, is_private: bool) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() || extract_url(trimmed).is_some() {
        return None;
    }

    if let Some(first_word) = trimmed.split_whitespace().next() {
        if first_word.starts_with('/') {
            let cmd = first_word.split('@').next().unwrap_or(first_word);
            if matches!(cmd, "/ym" | "/audio" | "/mp3" | "/search") {
                let rest = trimmed[first_word.len()..].trim();
                if !rest.is_empty() {
                    return Some(rest);
                }
            }
            return None;
        }
    }

    if is_private {
        Some(trimmed)
    } else {
        None
    }
}

/// Determines whether the request should be processed as audio-only.
pub fn is_audio_request(url: &str, text: &str, user_audio_only_setting: bool) -> bool {
    user_audio_only_setting
        || is_yandex_music_url(url)
        || is_soundcloud_url(url)
        || text.starts_with("/audio")
        || text.starts_with("/mp3")
        || text.starts_with("/sc")
        || text.contains("audio")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_search_query_private_vs_group() {
        assert_eq!(extract_search_query("Кино Кукушка", true), Some("Кино Кукушка"));
        assert_eq!(extract_search_query("Кино Кукушка", false), None);
    }

    #[test]
    fn test_extract_search_query_commands() {
        assert_eq!(extract_search_query("/ym Кукушка", false), Some("Кукушка"));
        assert_eq!(extract_search_query("/ym@edibls_bot Кино - Кукушка", false), Some("Кино - Кукушка"));
        assert_eq!(extract_search_query("/audio Кукушка", true), Some("Кукушка"));
        assert_eq!(extract_search_query("/audio https://youtu.be/abc", true), None);
        assert_eq!(extract_search_query("/ym", true), None);
        assert_eq!(extract_search_query("/ph test", true), None);
    }

    #[test]
    fn test_extract_sc_search_query() {
        assert_eq!(extract_sc_search_query("/sc marry me"), Some("marry me"));
        assert_eq!(extract_sc_search_query("/sc@edibls_bot drake"), Some("drake"));
        assert_eq!(extract_sc_search_query("/soundcloud queen"), Some("queen"));
        assert_eq!(extract_sc_search_query("/sc https://soundcloud.com/artist/track"), None);
        assert_eq!(extract_sc_search_query("just text"), None);
    }

    #[test]
    fn test_extract_yt_search_query() {
        assert_eq!(extract_yt_search_query("/yt cat videos"), Some("cat videos"));
        assert_eq!(extract_yt_search_query("/youtube@edibls_bot rust tutorial"), Some("rust tutorial"));
        assert_eq!(extract_yt_search_query("/yt https://youtube.com/watch?v=123"), None);
        assert_eq!(extract_yt_search_query("cat videos"), None);
    }

    #[test]
    fn test_extract_ph_search_query() {
        assert_eq!(extract_ph_search_query("/ph blondes"), Some("blondes"));
        assert_eq!(extract_ph_search_query("/pornhub@edibls_bot redheads"), Some("redheads"));
        assert_eq!(extract_ph_search_query("/ph https://pornhub.com/view_video.php?viewkey=123"), None);
        assert_eq!(extract_ph_search_query("blondes"), None);
    }

    #[test]
    fn test_extract_all_urls() {
        let text = "Here are links: https://tiktok.com/@u/1 and https://instagram.com/p/123/ also duplicate https://tiktok.com/@u/1";
        let urls = extract_all_urls(text);
        assert_eq!(urls, vec!["https://tiktok.com/@u/1", "https://instagram.com/p/123/"]);
    }

    #[test]
    fn test_is_audio_request() {
        assert!(is_audio_request("https://music.yandex.ru/album/1/track/2", "download", false));
        assert!(is_audio_request("https://soundcloud.com/artist/track", "give me this", false));
        assert!(is_audio_request("https://youtube.com/watch?v=123", "/audio", false));
        assert!(is_audio_request("https://youtube.com/watch?v=123", "/mp3", false));
        assert!(is_audio_request("https://youtube.com/watch?v=123", "please audio", false));
        assert!(is_audio_request("https://youtube.com/watch?v=123", "just link", true));
        assert!(!is_audio_request("https://youtube.com/watch?v=123", "just link", false));
    }
}
