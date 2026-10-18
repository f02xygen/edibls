#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeRange {
    pub start_secs: u64,
    pub end_secs: u64,
    pub raw_label: String,
}

/// Parses timestamp strings such as "10", "10s", "01:25", "1:05:30" into total seconds.
pub fn parse_timestamp(s: &str) -> Option<u64> {
    let mut s = s.trim();
    if s.is_empty() {
        return None;
    }
    s = s.trim_end_matches(['s', 'S', 'с', 'С']);
    s = s.trim_end_matches("sec").trim_end_matches("сек");
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        1 => parts[0].trim().parse::<u64>().ok(),
        2 => {
            let mins = parts[0].trim().parse::<u64>().ok()?;
            let secs = parts[1].trim().parse::<u64>().ok()?;
            if secs < 60 {
                Some(mins * 60 + secs)
            } else {
                None
            }
        }
        3 => {
            let hours = parts[0].trim().parse::<u64>().ok()?;
            let mins = parts[1].trim().parse::<u64>().ok()?;
            let secs = parts[2].trim().parse::<u64>().ok()?;
            if mins < 60 && secs < 60 {
                Some(hours * 3600 + mins * 60 + secs)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Extracts a time range (e.g. 01:15-02:30, 15-45, 10..40, 00:10 00:30, /clip 30) from the user text.
pub fn extract_time_range(text: &str) -> Option<TimeRange> {
    let clean_text = text.trim();
    let words: Vec<&str> = clean_text.split_whitespace().collect();

    // 1. Check for separators inside a word, e.g. "01:15-02:30", "15-45", "10..40", "00:10—00:30"
    for sep in ["-", "–", "—", ".."] {
        for word in &words {
            if word.starts_with("http://") || word.starts_with("https://") {
                continue;
            }
            if let Some((left, right)) = word.split_once(sep) {
                if let (Some(s), Some(e)) = (parse_timestamp(left), parse_timestamp(right)) {
                    if e > s {
                        return Some(TimeRange {
                            start_secs: s,
                            end_secs: e,
                            raw_label: format!("{left}-{right}"),
                        });
                    }
                }
            }
        }
    }

    // 2. Check for separated words, e.g. "1:00 - 2:00" or "00:15 .. 00:45"
    for (i, &w) in words.iter().enumerate() {
        if (w == "-" || w == "–" || w == "—" || w == "..") && i > 0 && i + 1 < words.len() {
            let left = words[i - 1];
            let right = words[i + 1];
            if !left.starts_with("http") && !right.starts_with("http") {
                if let (Some(s), Some(e)) = (parse_timestamp(left), parse_timestamp(right)) {
                    if e > s {
                        return Some(TimeRange {
                            start_secs: s,
                            end_secs: e,
                            raw_label: format!("{left}-{right}"),
                        });
                    }
                }
            }
        }
    }

    // 3. Check adjacent timestamp words without separator, e.g. "00:10 00:30"
    for i in 0..words.len().saturating_sub(1) {
        let left = words[i];
        let right = words[i + 1];
        if !left.starts_with("http") && !right.starts_with("http") && (left.contains(':') || right.contains(':')) {
            if let (Some(s), Some(e)) = (parse_timestamp(left), parse_timestamp(right)) {
                if e > s {
                    return Some(TimeRange {
                        start_secs: s,
                        end_secs: e,
                        raw_label: format!("{left}-{right}"),
                    });
                }
            }
        }
    }

    // 4. Single timestamp limit if command is /clip or clip (e.g. "/clip 30" or "/clip 00:45") -> 0 to N
    if clean_text.starts_with("/clip") || clean_text.starts_with("clip") {
        for word in &words {
            if word.starts_with('/') || word.starts_with("http") {
                continue;
            }
            if let Some(e) = parse_timestamp(word) {
                if e > 0 {
                    return Some(TimeRange {
                        start_secs: 0,
                        end_secs: e,
                        raw_label: format!("0-{e}"),
                    });
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_timestamp() {
        assert_eq!(parse_timestamp("10"), Some(10));
        assert_eq!(parse_timestamp("10s"), Some(10));
        assert_eq!(parse_timestamp("10с"), Some(10));
        assert_eq!(parse_timestamp("01:25"), Some(85));
        assert_eq!(parse_timestamp("1:05:30"), Some(3930));
        assert_eq!(parse_timestamp("invalid"), None);
        assert_eq!(parse_timestamp("01:65"), None);
    }

    #[test]
    fn test_extract_time_range() {
        let res = extract_time_range("https://youtu.be/abc 01:00-02:00").unwrap();
        assert_eq!(res.start_secs, 60);
        assert_eq!(res.end_secs, 120);

        let res2 = extract_time_range("clip 15-45 https://tiktok.com/@u/video/1").unwrap();
        assert_eq!(res2.start_secs, 15);
        assert_eq!(res2.end_secs, 45);

        let res3 = extract_time_range("/clip 30 https://youtu.be/abc").unwrap();
        assert_eq!(res3.start_secs, 0);
        assert_eq!(res3.end_secs, 30);
    }

    #[test]
    fn test_enhanced_time_range_formats() {
        let r1 = extract_time_range("https://youtu.be/xyz 10..40").unwrap();
        assert_eq!(r1.start_secs, 10);
        assert_eq!(r1.end_secs, 40);

        let r2 = extract_time_range("https://youtu.be/xyz 00:10 00:30").unwrap();
        assert_eq!(r2.start_secs, 10);
        assert_eq!(r2.end_secs, 30);

        let r3 = extract_time_range("https://youtu.be/xyz 1:00 - 2:00").unwrap();
        assert_eq!(r3.start_secs, 60);
        assert_eq!(r3.end_secs, 120);
    }
}
