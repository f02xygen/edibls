use crate::parsing::timestamps::TimeRange;

/// Extracts a speed multiplier (e.g. 0.5x, 0.75x, 1.25x, 1.5x, 2x, /speed 2x) from text.
/// Returns Some(multiplier) if speed is between 0.25 and 3.0 and not 1.0.
pub fn extract_speed_modifier(text: &str) -> Option<f32> {
    let words: Vec<&str> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if word.starts_with("http://") || word.starts_with("https://") {
            continue;
        }

        // Check "/speed 1.5" or "speed:1.5" or "speed=2"
        if (*word == "/speed" || *word == "speed") && i + 1 < words.len() {
            let next = words[i + 1].trim_end_matches(['x', 'X']);
            if let Ok(val) = next.parse::<f32>() {
                if (0.25..=3.0).contains(&val) && (val - 1.0).abs() > 0.01 {
                    return Some(val);
                }
            }
        }

        let clean = word
            .trim_start_matches("/speed=")
            .trim_start_matches("speed=")
            .trim_start_matches("speed:")
            .trim_start_matches(['x', 'X']);

        if let Some(num_str) = clean.strip_suffix(['x', 'X']) {
            if let Ok(val) = num_str.parse::<f32>() {
                if (0.25..=3.0).contains(&val) && (val - 1.0).abs() > 0.01 {
                    return Some(val);
                }
            }
        }

        if text.starts_with("/speed") {
            if let Ok(val) = clean.parse::<f32>() {
                if (0.25..=3.0).contains(&val) && (val - 1.0).abs() > 0.01 {
                    return Some(val);
                }
            }
        }
    }
    None
}

/// Extracts base URL and modifiers (time_range, speed, with_subway) from a URL that may contain fragments
/// such as `#clip=10-30#speed=1.50#subway`.
pub fn parse_media_modifiers(url: &str) -> (String, Option<TimeRange>, Option<f32>, bool) {
    if let Some((base, fragment)) = url.split_once('#') {
        let mut time_range = None;
        let mut speed = None;
        let mut with_subway = false;

        let frag_parts: Vec<&str> = fragment.split('#').collect();
        for p in frag_parts {
            if let Some(clip_val) = p.strip_prefix("clip=") {
                if let Some((s_str, e_str)) = clip_val.split_once('-') {
                    if let (Ok(s), Ok(e)) = (s_str.parse::<u64>(), e_str.parse::<u64>()) {
                        if e > s {
                            time_range = Some(TimeRange {
                                start_secs: s,
                                end_secs: e,
                                raw_label: format!("{s}-{e}"),
                            });
                        }
                    }
                }
            } else if let Some(spd_val) = p.strip_prefix("speed=") {
                if let Ok(spd) = spd_val.parse::<f32>() {
                    if (0.25..=3.0).contains(&spd) && (spd - 1.0).abs() > 0.01 {
                        speed = Some(spd);
                    }
                }
            } else if p == "subway" {
                with_subway = true;
            }
        }
        (base.to_string(), time_range, speed, with_subway)
    } else {
        (url.to_string(), None, None, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_speed_modifier() {
        assert_eq!(extract_speed_modifier("https://youtu.be/abc 1.5x"), Some(1.5));
        assert_eq!(extract_speed_modifier("https://youtu.be/abc 2x"), Some(2.0));
        assert_eq!(extract_speed_modifier("https://youtu.be/abc 0.5X"), Some(0.5));
        assert_eq!(extract_speed_modifier("/speed 2x https://youtu.be/abc"), Some(2.0));
        assert_eq!(extract_speed_modifier("/speed 1.25"), Some(1.25));
        assert_eq!(extract_speed_modifier("https://youtu.be/abc 1x"), None);
        assert_eq!(extract_speed_modifier("https://youtu.be/abc 1.0x"), None);
        assert_eq!(extract_speed_modifier("https://youtu.be/abc 5x"), None);
    }

    #[test]
    fn test_parse_media_modifiers() {
        let (base, tr, spd, subway) =
            parse_media_modifiers("https://youtube.com/watch?v=123#clip=10-30#speed=1.50#subway");
        assert_eq!(base, "https://youtube.com/watch?v=123");
        assert!(subway);
        assert_eq!(spd, Some(1.5));
        let tr = tr.unwrap();
        assert_eq!(tr.start_secs, 10);
        assert_eq!(tr.end_secs, 30);

        let (base2, tr2, spd2, subway2) = parse_media_modifiers("https://tiktok.com/@u/1#subway");
        assert_eq!(base2, "https://tiktok.com/@u/1");
        assert!(subway2);
        assert_eq!(tr2, None);
        assert_eq!(spd2, None);
    }
}
