use std::path::{Path, PathBuf};
use image::ImageFormat;
use image::imageops::FilterType;
use lofty::prelude::*;
use lofty::probe::Probe;

use crate::config::Config;
use crate::providers::yandex::{download_yandex_track, is_yandex_music_url};
use crate::providers::ytdlp::download_media;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    pub path: PathBuf,
    pub title: Option<String>,
    pub performer: Option<String>,
    pub duration_secs: Option<u32>,
    pub cover_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFile {
    pub path: PathBuf,
    pub title: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_secs: Option<u32>,
    pub thumbnail_path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoAlbum {
    pub photos: Vec<PathBuf>,
    pub audio: Option<AudioTrack>,
    pub title: Option<String>,
}

use teloxide::types::{FileId, Message};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceMedia {
    Video(FileId),
    Audio(FileId),
}

pub fn extract_source_media(msg: &Message) -> Option<SourceMedia> {
    if let Some(v) = msg.video() {
        return Some(SourceMedia::Video(v.file.id.clone()));
    }
    if let Some(vn) = msg.video_note() {
        return Some(SourceMedia::Video(vn.file.id.clone()));
    }
    if let Some(a) = msg.audio() {
        return Some(SourceMedia::Audio(a.file.id.clone()));
    }
    if let Some(vo) = msg.voice() {
        return Some(SourceMedia::Audio(vo.file.id.clone()));
    }
    if let Some(doc) = msg.document() {
        let is_audio = doc
            .mime_type
            .as_ref()
            .map(|m| m.as_ref().starts_with("audio/"))
            .unwrap_or(false)
            || doc
                .file_name
                .as_ref()
                .map(|n| {
                    let lower = n.to_lowercase();
                    lower.ends_with(".mp3")
                        || lower.ends_with(".m4a")
                        || lower.ends_with(".flac")
                        || lower.ends_with(".wav")
                        || lower.ends_with(".ogg")
                })
                .unwrap_or(false);

        if is_audio {
            return Some(SourceMedia::Audio(doc.file.id.clone()));
        } else {
            return Some(SourceMedia::Video(doc.file.id.clone()));
        }
    }
    None
}

pub fn extract_source_media_recursive(msg: &Message) -> Option<SourceMedia> {
    if let Some(src) = extract_source_media(msg) {
        return Some(src);
    }
    if let Some(parent) = msg.reply_to_message() {
        return extract_source_media_recursive(parent);
    }
    None
}

pub enum MediaOutput {
    Audio(Vec<AudioTrack>),
    Video(VideoFile),
    Photos(PhotoAlbum),
}

#[allow(unused_imports)]
pub use crate::parsing::{
    extract_all_urls, extract_ph_search_query, extract_sc_search_query, extract_search_query,
    extract_speed_modifier, extract_time_range, extract_url, extract_yt_search_query,
    is_audio_request, parse_media_modifiers, parse_timestamp, TimeRange,
};

#[allow(dead_code)]
pub fn parse_clip_fragment(url: &str) -> (String, Option<TimeRange>) {
    let (base, tr, _, _) = parse_media_modifiers(url);
    (base, tr)
}

#[allow(unused_imports)]
pub use crate::ffmpeg::{
    attach_subway_surfers, build_atempo_filter, extract_audio_from_video, find_subway_asset,
    get_video_dimensions, has_audio_stream, probe_video_metadata, process_media_cut_or_speed,
};

/// Reads audio tags and resizes the cover image to fit Telegram API restrictions (max 320x320).
pub fn process_audio_metadata(file_path: &Path) -> AudioTrack {
    let cover_path = file_path.with_extension("jpg");
    let mut performer = None;
    let mut title = None;
    let mut duration_secs = None;
    let mut has_cover = false;

    if let Ok(tagged_file) = Probe::open(file_path).and_then(|p| p.read()) {
        let dur = tagged_file.properties().duration().as_secs();
        if dur > 0 {
            duration_secs = Some(dur as u32);
        }

        if let Some(tag) = tagged_file
            .primary_tag()
            .or_else(|| tagged_file.first_tag())
        {
            performer = tag.artist().map(|s| s.to_string());
            title = tag.title().map(|s| s.to_string());

            if let Some(picture) = tag.pictures().first() {
                if let Ok(img) = image::load_from_memory(picture.data()) {
                    let resized = img.resize(320, 320, FilterType::Triangle);
                    if resized
                        .save_with_format(&cover_path, ImageFormat::Jpeg)
                        .is_ok()
                    {
                        has_cover = true;
                    }
                }
            }
        }
    }

    if title.is_none() {
        if let Some(parent) = file_path.parent() {
            let title_file = parent.join("title.txt");
            if let Ok(t) = std::fs::read_to_string(title_file) {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    title = Some(trimmed.to_string());
                }
            }
        }
    }

    AudioTrack {
        path: file_path.to_path_buf(),
        title,
        performer,
        duration_secs,
        cover_path: if has_cover { Some(cover_path) } else { None },
    }
}

/// Offloads blocking tag & cover image processing to the blocking threadpool.
async fn process_tracks_blocking(files: Vec<PathBuf>) -> Vec<AudioTrack> {
    tokio::task::spawn_blocking(move || {
        files
            .into_iter()
            .map(|f| process_audio_metadata(&f))
            .collect()
        })
    .await
    .unwrap_or_default()
}

/// Formats a compact 10-block visual progress bar message.
pub fn format_progress_bar(stage_label: &str, percent: u8) -> String {
    let clamped = percent.min(100);
    let filled = (usize::from(clamped) + 5) / 10;
    let empty = 10usize.saturating_sub(filled);
    let bar: String = std::iter::repeat('█')
        .take(filled)
        .chain(std::iter::repeat('░').take(empty))
        .collect();
    format!("{stage_label}: {clamped}%\n[{bar}]")
}

/// Reads total transmitted bytes (`tx_bytes`) across non-loopback network interfaces from `/proc/net/dev`.
pub fn read_network_tx_bytes() -> u64 {
    let Ok(content) = std::fs::read_to_string("/proc/net/dev") else {
        return 0;
    };
    let mut total_tx = 0u64;
    for line in content.lines().skip(2) {
        let Some((iface, stats)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        if let Some(tx_str) = stats.split_whitespace().nth(8) {
            if let Ok(tx) = tx_str.parse::<u64>() {
                total_tx = total_tx.saturating_add(tx);
            }
        }
    }
    total_tx
}

/// High-level function to handle download and metadata parsing for any supported platform.
pub async fn fetch_media(
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    time_range: Option<&TimeRange>,
    speed: Option<f32>,
    ym_quality: crate::settings::YandexQualityPreference,
    temp_dir: &Path,
    config: &Config,
    progress: &std::sync::Arc<std::sync::atomic::AtomicU8>,
) -> Result<MediaOutput, String> {
    if is_yandex_music_url(url) {
        let token = config
            .yandex_music_token
            .as_deref()
            .ok_or_else(|| "YANDEX_MUSIC_TOKEN не настроен в .env файле.".to_string())?;

        let files = download_yandex_track(url, token, ym_quality, temp_dir, config, progress)
            .await
            .map_err(|e| e.to_string())?;

        progress.store(95, std::sync::atomic::Ordering::Relaxed);

        let mut processed_files = Vec::new();
        if time_range.is_some() || (speed.is_some() && (speed.unwrap() - 1.0).abs() > 0.01) {
            for (idx, f) in files.into_iter().enumerate() {
                let ext = f.extension().and_then(|s| s.to_str()).unwrap_or("mp3");
                let out_p = temp_dir.join(format!("ym_cut_speed_{idx}.{ext}"));
                process_media_cut_or_speed(&f, time_range, speed, true, &out_p).await?;
                processed_files.push(out_p);
            }
        } else {
            processed_files = files;
        }

        progress.store(100, std::sync::atomic::Ordering::Relaxed);
        let tracks = process_tracks_blocking(processed_files).await;
        return Ok(MediaOutput::Audio(tracks));
    }

    let downloaded = download_media(url, audio_only, max_height, time_range, temp_dir, config, progress).await?;
    let mut file_path = downloaded.path;

    let needs_cut = time_range.is_some() && !downloaded.was_clipped_by_ytdlp;
    let needs_speed = speed.is_some() && (speed.unwrap() - 1.0).abs() > 0.01;

    if needs_cut || needs_speed {
        let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or(if audio_only { "mp3" } else { "mp4" });
        let out_p = temp_dir.join(format!("processed_final.{ext}"));
        let tr_to_apply = if needs_cut { time_range } else { None };
        process_media_cut_or_speed(&file_path, tr_to_apply, speed, audio_only, &out_p).await?;
        file_path = out_p;
    }

    if audio_only {
        let tracks = process_tracks_blocking(vec![file_path]).await;
        Ok(MediaOutput::Audio(tracks))
    } else {
        let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
        let is_image = matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp");

        let mut image_files = Vec::new();
        let mut audio_file = None;
        if let Ok(mut entries) = tokio::fs::read_dir(temp_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let p = entry.path();
                if p.is_file() {
                    let e = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
                    if matches!(e.as_str(), "jpg" | "jpeg" | "png" | "webp") {
                        if !p.file_name().and_then(|n| n.to_str()).unwrap_or("").ends_with("thumb.jpg") {
                            image_files.push(p);
                        }
                    } else if matches!(e.as_str(), "mp3" | "m4a" | "ogg" | "wav" | "flac") {
                        audio_file = Some(p);
                    }
                }
            }
        }
        image_files.sort();

        let title = tokio::fs::read_to_string(temp_dir.join("title.txt"))
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        if is_image || (!image_files.is_empty() && !matches!(ext.as_str(), "mp4" | "mkv" | "webm" | "mov" | "avi")) {
            let audio_track = if let Some(ap) = audio_file {
                let tracks = process_tracks_blocking(vec![ap]).await;
                tracks.into_iter().next()
            } else {
                None
            };
            return Ok(MediaOutput::Photos(PhotoAlbum {
                photos: if image_files.is_empty() { vec![file_path] } else { image_files },
                audio: audio_track,
                title,
            }));
        }

        let (width, height, duration_secs, thumbnail_path) = probe_video_metadata(&file_path).await;

        Ok(MediaOutput::Video(VideoFile {
            path: file_path,
            title,
            width,
            height,
            duration_secs,
            thumbnail_path,
        }))
    }
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
        assert_eq!(extract_sc_search_query("/soundcloud test query"), Some("test query"));
        assert_eq!(extract_sc_search_query("/sc"), None);
        assert_eq!(extract_sc_search_query("/sc https://soundcloud.com/test"), None);
        assert_eq!(extract_sc_search_query("just words"), None);
    }

    #[test]
    fn test_extract_yt_search_query() {
        assert_eq!(extract_yt_search_query("/yt rick astley"), Some("rick astley"));
        assert_eq!(extract_yt_search_query("/yt@edibls_bot rick astley"), Some("rick astley"));
        assert_eq!(extract_yt_search_query("/youtube rick astley"), Some("rick astley"));
        assert_eq!(extract_yt_search_query("/yt"), None);
        assert_eq!(extract_yt_search_query("/yt https://youtube.com/watch?v=123"), None);
        assert_eq!(extract_yt_search_query("just words"), None);
    }

    #[test]
    fn test_extract_ph_search_query() {
        assert_eq!(extract_ph_search_query("/ph minecraft"), Some("minecraft"));
        assert_eq!(extract_ph_search_query("/ph@edibls_bot blonde pov"), Some("blonde pov"));
        assert_eq!(extract_ph_search_query("/pornhub test query"), Some("test query"));
        assert_eq!(extract_ph_search_query("/ph"), None);
        assert_eq!(extract_ph_search_query("/ph https://pornhub.com/view_video.php?viewkey=123"), None);
        assert_eq!(extract_ph_search_query("just words"), None);
    }

    #[test]
    fn test_is_audio_request() {
        assert!(is_audio_request("https://youtube.com/watch?v=123", "/audio https://youtube.com/watch?v=123", false));
        assert!(is_audio_request("https://music.yandex.ru/track/123", "https://music.yandex.ru/track/123", false));
        assert!(is_audio_request("https://pornhub.com/view_video.php?viewkey=123", "https://pornhub.com/view_video.php?viewkey=123", true));
        assert!(!is_audio_request("https://pornhub.com/view_video.php?viewkey=123", "https://pornhub.com/view_video.php?viewkey=123", false));
    }

    #[test]
    fn test_format_progress_bar() {
        assert_eq!(
            format_progress_bar("📥 Скачивание", 0),
            "📥 Скачивание: 0%\n[░░░░░░░░░░]"
        );
        assert_eq!(
            format_progress_bar("📥 Скачивание", 50),
            "📥 Скачивание: 50%\n[█████░░░░░]"
        );
        assert_eq!(
            format_progress_bar("📤 Отправка", 36),
            "📤 Отправка: 36%\n[████░░░░░░]"
        );
        assert_eq!(
            format_progress_bar("📤 Отправка", 100),
            "📤 Отправка: 100%\n[██████████]"
        );
    }

    #[test]
    fn test_parse_timestamp() {
        assert_eq!(parse_timestamp("45"), Some(45));
        assert_eq!(parse_timestamp("01:15"), Some(75));
        assert_eq!(parse_timestamp("1:02:03"), Some(3723));
        assert_eq!(parse_timestamp("invalid"), None);
        assert_eq!(parse_timestamp("01:75"), None);
    }

    #[test]
    fn test_extract_time_range() {
        let r1 = extract_time_range("https://youtu.be/123 01:15-02:30").unwrap();
        assert_eq!(r1.start_secs, 75);
        assert_eq!(r1.end_secs, 150);

        let r2 = extract_time_range("/clip https://youtu.be/123 1:00 - 2:00").unwrap();
        assert_eq!(r2.start_secs, 60);
        assert_eq!(r2.end_secs, 120);

        let r3 = extract_time_range("https://youtu.be/123 00:01:15–00:02:30").unwrap();
        assert_eq!(r3.start_secs, 75);
        assert_eq!(r3.end_secs, 150);

        assert!(extract_time_range("https://youtu.be/123").is_none());
        assert!(extract_time_range("https://youtu.be/123 02:00-01:00").is_none());
    }

    #[test]
    fn test_extract_all_urls() {
        let text = "Check https://youtu.be/123 and https://vm.tiktok.com/456, plus http://site.com/789";
        let urls = extract_all_urls(text);
        assert_eq!(urls, vec!["https://youtu.be/123", "https://vm.tiktok.com/456", "http://site.com/789"]);

        let dups = "https://youtu.be/123 text https://youtu.be/123";
        assert_eq!(extract_all_urls(dups), vec!["https://youtu.be/123"]);

        assert!(extract_all_urls("no links here").is_empty());
    }

    #[test]
    fn test_find_subway_asset() {
        let asset = find_subway_asset();
        assert!(asset.is_some(), "assets/subway_surfers.mp4 should exist");
    }

    #[test]
    fn test_extract_speed_modifier() {
        assert_eq!(extract_speed_modifier("https://youtu.be/123 2x"), Some(2.0));
        assert_eq!(extract_speed_modifier("https://youtu.be/123 1.5x"), Some(1.5));
        assert_eq!(extract_speed_modifier("https://youtu.be/123 0.75x"), Some(0.75));
        assert_eq!(extract_speed_modifier("/speed 2x https://youtu.be/123"), Some(2.0));
        assert_eq!(extract_speed_modifier("/speed 1.25"), Some(1.25));
        assert_eq!(extract_speed_modifier("https://youtu.be/123 1.0x"), None);
        assert_eq!(extract_speed_modifier("https://youtu.be/123"), None);
    }

    #[test]
    fn test_build_atempo_filter() {
        assert_eq!(build_atempo_filter(1.5), "atempo=1.50");
        assert_eq!(build_atempo_filter(2.0), "atempo=2.00");
        assert_eq!(build_atempo_filter(0.75), "atempo=0.75");
        assert_eq!(build_atempo_filter(3.0), "atempo=2.0,atempo=1.50");
        assert_eq!(build_atempo_filter(0.25), "atempo=0.5,atempo=0.50");
    }

    #[test]
    fn test_parse_media_modifiers() {
        let (base, tr, spd, sub) = parse_media_modifiers("https://site.com/video#clip=10-30#speed=1.50#subway");
        assert_eq!(base, "https://site.com/video");
        assert_eq!(tr.unwrap().start_secs, 10);
        assert_eq!(spd, Some(1.5));
        assert!(sub);

        let (base2, tr2, spd2, sub2) = parse_media_modifiers("https://site.com/video#speed=2.00");
        assert_eq!(base2, "https://site.com/video");
        assert!(tr2.is_none());
        assert_eq!(spd2, Some(2.0));
        assert!(!sub2);
    }

    #[test]
    fn test_enhanced_time_range_formats() {
        let r1 = extract_time_range("https://youtu.be/123 15-45").unwrap();
        assert_eq!(r1.start_secs, 15);
        assert_eq!(r1.end_secs, 45);

        let r2 = extract_time_range("https://youtu.be/123 10..40").unwrap();
        assert_eq!(r2.start_secs, 10);
        assert_eq!(r2.end_secs, 40);

        let r3 = extract_time_range("https://youtu.be/123 00:10 00:50").unwrap();
        assert_eq!(r3.start_secs, 10);
        assert_eq!(r3.end_secs, 50);

        let r4 = extract_time_range("/clip https://youtu.be/123 30").unwrap();
        assert_eq!(r4.start_secs, 0);
        assert_eq!(r4.end_secs, 30);
    }
}

