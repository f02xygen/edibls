use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::config::Config;
use crate::providers::pornhub::is_pornhub_url;

#[derive(Debug, Clone)]
pub struct DownloadedMedia {
    pub path: PathBuf,
    pub was_clipped_by_ytdlp: bool,
}

/// Downloads media via yt-dlp (video or audio-only) into the provided temp directory.
/// If a video format's filesize is unknown in metadata and exceeds `max_filesize_mb` during download,
/// automatically steps down to lower resolutions (720p -> 480p -> 240p).
pub async fn download_media(
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    time_range: Option<&crate::media::TimeRange>,
    temp_dir: &Path,
    config: &Config,
    progress: &Arc<AtomicU8>,
) -> Result<DownloadedMedia, String> {
    let mut candidate_heights: Vec<Option<u16>> = vec![max_height];
    if !audio_only {
        let start_h = max_height.unwrap_or(u16::MAX);
        for &fallback_h in &[720u16, 480, 240] {
            if fallback_h < start_h {
                candidate_heights.push(Some(fallback_h));
            }
        }
    }

    let is_yt = url.contains("youtube.com") || url.contains("youtu.be");
    let use_cookies = if is_yt { false } else { config.cookies_path.is_some() };

    let mut last_err = String::new();
    for (idx, height_opt) in candidate_heights.into_iter().enumerate() {
        if idx > 0 {
            log::info!(
                "Format exceeded max-filesize; retrying {} with lower resolution {:?}p",
                url,
                height_opt
            );
            progress.store(0, Ordering::Relaxed);
        }
        match run_ytdlp_once(url, audio_only, height_opt, time_range, temp_dir, config, progress, use_cookies).await {
            Ok(path) => return Ok(DownloadedMedia { path, was_clipped_by_ytdlp: time_range.is_some() }),
            Err((err_msg, is_filesize_exceeded, is_auth_required)) => {
                last_err = err_msg;
                if is_auth_required && is_yt && !use_cookies && config.cookies_path.is_some() {
                    log::info!("YouTube requested authentication for {}; retrying with cookies", url);
                    progress.store(0, Ordering::Relaxed);
                    match run_ytdlp_once(url, audio_only, height_opt, time_range, temp_dir, config, progress, true).await {
                        Ok(path) => return Ok(DownloadedMedia { path, was_clipped_by_ytdlp: time_range.is_some() }),
                        Err((retry_err, _, _)) => return Err(retry_err),
                    }
                }
                if is_filesize_exceeded {
                    continue;
                }
                // If download-sections failed on non-YouTube or seek-restricted platforms, fallback to full download
                if time_range.is_some() {
                    log::warn!("yt-dlp download-sections failed for {}; attempting full download + local trim: {}", url, last_err);
                    progress.store(0, Ordering::Relaxed);
                    if let Ok(full_path) = run_ytdlp_once(url, audio_only, height_opt, None, temp_dir, config, progress, use_cookies).await {
                        return Ok(DownloadedMedia { path: full_path, was_clipped_by_ytdlp: false });
                    }
                }
                return Err(last_err);
            }
        }
    }

    Err(last_err)
}

async fn run_ytdlp_once(
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    time_range: Option<&crate::media::TimeRange>,
    temp_dir: &Path,
    config: &Config,
    progress: &Arc<AtomicU8>,
    use_cookies: bool,
) -> Result<PathBuf, (String, bool, bool)> {
    log::info!(
        "Processing URL (audio_only={}, max_height={:?}, time_range={:?}): {}",
        audio_only,
        max_height,
        time_range,
        url
    );

    let output_template = temp_dir.join("%(id)s.%(ext)s");
    let output_template_str = output_template.to_string_lossy();

    let max_filesize_str = format!("{}M", config.max_filesize_mb);
    let match_filter = format!("duration <=? {}", config.max_duration_secs);

    // For Pornhub CDN (ev.phncdn.com), rate=500k&burst=1600k/3800k throttles large single chunks.
    // Setting chunk size to 1M lets each range request download at unthrottled burst speed,
    // and provides finer-grained progress updates for small audio files (like SoundCloud tracks).
    let is_ph = is_pornhub_url(url);
    let is_sc = crate::providers::soundcloud::is_soundcloud_url(url);
    let http_chunk_size = if is_ph || is_sc || audio_only { "1M" } else { "10M" };

    let mut cmd = Command::new("yt-dlp");

    cmd.args([
        "--max-filesize",
        &max_filesize_str,
        "--concurrent-fragments",
        "8",
        "--http-chunk-size",
        http_chunk_size,
        "--no-mtime",
        "--no-part",
        "--js-runtimes",
        "deno",
        "--js-runtimes",
        "node",
        "--no-playlist",
        "--progress",
        "--newline",
        "--progress-template",
        "download:EDIBLS_PROG:%(progress._percent_str)s",
    ]);

    if let Some(tr) = time_range {
        cmd.args([
            "--download-sections",
            &format!("*{}-{}", tr.start_secs, tr.end_secs),
            "--force-keyframes-at-cuts",
        ]);
    } else {
        cmd.args(["--match-filter", &match_filter]);
    }

    // Build combined youtube extractor args
    if let Some(pot_url) = &config.youtube_pot_url {
        let clean_pot = pot_url.trim_end_matches("/get_pot").trim_end_matches('/');
        cmd.arg("--extractor-args")
            .arg(format!("youtubepot-bgutilhttp:base_url={clean_pot}"));
    }
    cmd.arg("--extractor-args")
        .arg("youtube:player_client=mweb,web_safari,web");

    if audio_only {
        if is_sc {
            // SoundCloud has native MP3 and AAC streams: prefer direct progressive HTTP MP3 before HLS, without ffmpeg re-encoding
            cmd.args([
                "-f",
                "ba[ext=mp3][protocol^=http]/ba[ext=mp3]/ba[ext=m4a]/ba",
                "--embed-metadata",
            ]);
        } else {
            // For YouTube & other sites: prefer native m4a (AAC) or mp3 stream without CPU re-encoding.
            // Telegram natively plays m4a (AAC) and mp3 with full album art & metadata.
            cmd.args([
                "-f",
                "ba[ext=m4a]/ba[ext=mp3]/ba",
                "-x",
                "--audio-format",
                "m4a",
                "--embed-metadata",
                "--postprocessor-args",
                "ffmpeg:-threads 0",
            ]);
        }
    } else if let Some(height) = max_height {
        let format_selector = format!(
            "bestvideo*[height<=?{height}][filesize<=?{max_filesize_str}]+bestaudio/best[height<=?{height}][filesize<=?{max_filesize_str}]/best[height<=?{height}]/best"
        );
        let sort_criteria = if is_ph {
            format!("proto:https,res:{height},vcodec:h264,fps,acodec:m4a")
        } else {
            format!("res:{height},vcodec:h264,fps,acodec:m4a")
        };
        cmd.args([
            "-f",
            &format_selector,
            "-S",
            &sort_criteria,
            "--merge-output-format",
            "mp4",
        ]);
    } else {
        // Prioritize highest resolution bestvideo*+bestaudio or best with unknown/checked filesize
        let format_selector = format!(
            "bestvideo*[filesize<=?{max_filesize_str}]+bestaudio/best[filesize<=?{max_filesize_str}]/best"
        );
        let sort_criteria = if is_ph {
            "proto:https,res,fps,vcodec:h264,acodec:m4a"
        } else {
            "res,fps,vcodec:h264,acodec:m4a"
        };
        cmd.args([
            "-f",
            &format_selector,
            "-S",
            sort_criteria,
            "--merge-output-format",
            "mp4",
        ]);
    }

    if use_cookies {
        if let Some(cookies) = &config.cookies_path {
            log::info!("Using cookies from: {:?}", cookies);
            cmd.arg("--cookies").arg(cookies);
        }
    }

    let filepath_record = temp_dir.join("filepath.txt");
    let _ = tokio::fs::remove_file(&filepath_record).await;
    let filepath_record_str = filepath_record.to_string_lossy();

    let title_record = temp_dir.join("title.txt");
    let _ = tokio::fs::remove_file(&title_record).await;
    let title_record_str = title_record.to_string_lossy();

    let mut child = cmd
        .arg("-o")
        .arg(&*output_template_str)
        .arg("--print-to-file")
        .arg("after_move:filepath")
        .arg(&*filepath_record_str)
        .arg("--print-to-file")
        .arg("after_move:title")
        .arg(&*title_record_str)
        .arg(url)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| (format!("Failed to execute yt-dlp process: {e}"), false, false))?;

    let stdout_pipe = child.stdout.take().expect("stdout piped");
    let stderr_pipe = child.stderr.take().expect("stderr piped");

    // Start at 10% immediately so the user sees immediate progress during yt-dlp startup & extraction
    progress.store(10, Ordering::Relaxed);

    let prog_ticker = Arc::clone(progress);
    let ticker_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(350));
        interval.tick().await;
        loop {
            interval.tick().await;
            let cur = prog_ticker.load(Ordering::Relaxed);
            let cap = if audio_only { 85 } else { 35 };
            if cur < cap {
                let step = if audio_only { 12 } else { 6 };
                prog_ticker.store(cur.saturating_add(step).min(cap), Ordering::Relaxed);
            }
        }
    });

    let prog_clone = Arc::clone(progress);
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout_pipe).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(rest) = line.strip_prefix("EDIBLS_PROG:") {
                let cleaned = rest.trim().trim_end_matches('%').trim();
                if let Ok(val) = cleaned.parse::<f32>() {
                    let base = if audio_only { 35.0 } else { 20.0 };
                    let span = if audio_only { 57.0 } else { 72.0 };
                    let mapped = base + (val.clamp(0.0, 100.0) * (span / 100.0));
                    let pct = mapped.round().clamp(base, 92.0) as u8;
                    let prev = prog_clone.load(Ordering::Relaxed);
                    if pct > prev {
                        prog_clone.store(pct, Ordering::Relaxed);
                    }
                }
            } else {
                let milestone = if line.contains("[Metadata]")
                    || line.contains("[ExtractAudio]")
                    || line.contains("[Merger]")
                {
                    Some(95u8)
                } else if line.contains("Downloading m3u8 manifest")
                    || line.contains("Checking thumbnail")
                    || line.contains("Downloading 1 format")
                {
                    Some(40u8)
                } else if line.contains("format info JSON") {
                    Some(28u8)
                } else if line.contains("Downloading info JSON")
                    || line.contains("Extracting URL")
                {
                    Some(18u8)
                } else {
                    None
                };

                if let Some(m) = milestone {
                    let prev = prog_clone.load(Ordering::Relaxed);
                    if m > prev {
                        prog_clone.store(m, Ordering::Relaxed);
                    }
                }

                collected.push_str(&line);
                collected.push('\n');
            }
        }
        collected
    });

    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr_pipe).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            collected.push_str(&line);
            collected.push('\n');
        }
        collected
    });

    let status = child
        .wait()
        .await
        .map_err(|e| (format!("Failed waiting for yt-dlp process: {e}"), false, false))?;

    ticker_task.abort();
    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();

    if status.success() {
        if let Ok(recorded) = tokio::fs::read_to_string(&filepath_record).await {
            let file_path_str = recorded.lines().last().unwrap_or("").trim();
            if !file_path_str.is_empty() {
                let path = PathBuf::from(file_path_str);
                if path.exists() {
                    let max_bytes = config.max_filesize_mb * 1024 * 1024;
                    if let Ok(meta) = tokio::fs::metadata(&path).await {
                        if meta.len() > max_bytes {
                            log::warn!(
                                "Downloaded file {:?} ({} bytes) exceeds max_filesize ({} bytes); removing",
                                path,
                                meta.len(),
                                max_bytes
                            );
                            let _ = tokio::fs::remove_file(&path).await;
                            return Err((
                                format!(
                                    "Размер файла превышает лимит в {} МБ",
                                    config.max_filesize_mb
                                ),
                                true,
                                false,
                            ));
                        }
                    }
                    progress.store(100, Ordering::Relaxed);
                    log::info!("Downloaded file saved at: {:?}", path);
                    return Ok(path);
                }
            }
        }
    }

    let combined = format!("{stdout}\n{stderr}");
    log::error!("yt-dlp execution failed or skipped: {}", combined.trim());

    if combined.contains("This video is DRM protected") || combined.contains("has_drm") {
        Err((
            "Этот трек защищён DRM (SoundCloud Go+) и недоступен для бесплатного скачивания.".to_string(),
            false,
            false,
        ))
    } else if combined.contains("HTTP Error 404") && combined.contains("soundcloud") {
        Err((
            "Трек не найден на SoundCloud или был удалён.".to_string(),
            false,
            false,
        ))
    } else if combined.contains("does not pass filter") && combined.contains("duration") {
        Err((
            format!(
                "Видео слишком длинное (максимум {} минут)",
                config.max_duration_secs / 60
            ),
            false,
            false,
        ))
    } else if combined.contains("File is larger than max-filesize") {
        Err((
            format!(
                "Размер файла превышает лимит в {} МБ",
                config.max_filesize_mb
            ),
            true,
            false,
        ))
    } else {
        let is_auth_required = combined.contains("Sign in to confirm")
            || combined.contains("Sign in")
            || combined.contains("confirm your age")
            || combined.contains("age-restricted")
            || combined.contains("Private video")
            || combined.contains("login")
            || combined.contains("bot");

        Err((
            "Не удалось скачать медиафайл по указанной ссылке".to_string(),
            false,
            is_auth_required,
        ))
    }
}
