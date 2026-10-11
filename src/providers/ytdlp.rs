use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use crate::config::Config;
use crate::providers::pornhub::is_pornhub_url;

/// Downloads media via yt-dlp (video or audio-only) into the provided temp directory.
/// If a video format's filesize is unknown in metadata and exceeds `max_filesize_mb` during download,
/// automatically steps down to lower resolutions (720p -> 480p -> 240p).
pub async fn download_media(
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    temp_dir: &Path,
    config: &Config,
    progress: &Arc<AtomicU8>,
) -> Result<PathBuf, String> {
    let mut candidate_heights: Vec<Option<u16>> = vec![max_height];
    if !audio_only {
        let start_h = max_height.unwrap_or(u16::MAX);
        for &fallback_h in &[720u16, 480, 240] {
            if fallback_h < start_h {
                candidate_heights.push(Some(fallback_h));
            }
        }
    }

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
        match run_ytdlp_once(url, audio_only, height_opt, temp_dir, config, progress).await {
            Ok(path) => return Ok(path),
            Err((err_msg, is_filesize_exceeded)) => {
                last_err = err_msg;
                if !is_filesize_exceeded {
                    return Err(last_err);
                }
            }
        }
    }

    Err(last_err)
}

async fn run_ytdlp_once(
    url: &str,
    audio_only: bool,
    max_height: Option<u16>,
    temp_dir: &Path,
    config: &Config,
    progress: &Arc<AtomicU8>,
) -> Result<PathBuf, (String, bool)> {
    log::info!(
        "Processing URL (audio_only={}, max_height={:?}): {}",
        audio_only,
        max_height,
        url
    );

    let output_template = temp_dir.join("%(id)s.%(ext)s");
    let output_template_str = output_template.to_string_lossy();

    let max_filesize_str = format!("{}M", config.max_filesize_mb);
    let match_filter = format!("duration <= {}", config.max_duration_secs);

    // For Pornhub CDN (ev.phncdn.com), rate=500k&burst=1600k/3800k throttles large single chunks.
    // Setting chunk size to 1M lets each range request download at unthrottled burst speed.
    let http_chunk_size = if is_pornhub_url(url) { "1M" } else { "10M" };

    let mut cmd = Command::new("yt-dlp");

    cmd.args([
        "--max-filesize",
        &max_filesize_str,
        "--match-filter",
        &match_filter,
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
        "--newline",
        "--progress-template",
        "download:EDIBLS_PROG:%(progress._percent_str)s",
    ]);

    // Build combined youtube extractor args so flags do not overwrite each other
    let yt_extractor_args = if let Some(pot_url) = &config.youtube_pot_url {
        format!("youtube:player_client=android,ios,web;po_token=web+{pot_url}")
    } else {
        "youtube:player_client=android,ios,web".to_string()
    };
    cmd.arg("--extractor-args").arg(yt_extractor_args);

    if audio_only {
        cmd.args([
            "-x",
            "--audio-format",
            "mp3",
            "--embed-metadata",
            "--postprocessor-args",
            "ffmpeg:-threads 0",
        ]);
    } else if let Some(height) = max_height {
        let format_selector = format!(
            "bestvideo[height<={height}][filesize<={max_filesize_str}]+bestaudio/best[height<={height}][filesize<={max_filesize_str}]/bestvideo[height<={height}]+bestaudio/best[height<={height}]"
        );
        let sort_criteria = format!("res:{height},vcodec:h264,fps,acodec:m4a");
        cmd.args([
            "-f",
            &format_selector,
            "-S",
            &sort_criteria,
            "--merge-output-format",
            "mp4",
        ]);
    } else {
        let format_selector = format!(
            "b[filesize<={max_filesize_str}]/bestvideo[filesize<={max_filesize_str}]+bestaudio/best[filesize<={max_filesize_str}]/best"
        );
        cmd.args([
            "-f",
            &format_selector,
            "-S",
            "vcodec:h264,fps,res,acodec:m4a",
            "--merge-output-format",
            "mp4",
        ]);
    }

    if let Some(cookies) = &config.cookies_path {
        log::info!("Using cookies from: {:?}", cookies);
        cmd.arg("--cookies").arg(cookies);
    }

    let filepath_record = temp_dir.join("filepath.txt");
    let _ = tokio::fs::remove_file(&filepath_record).await;
    let filepath_record_str = filepath_record.to_string_lossy();

    let mut child = cmd
        .arg("-o")
        .arg(&*output_template_str)
        .arg("--print-to-file")
        .arg("after_move:filepath")
        .arg(&*filepath_record_str)
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| (format!("Failed to execute yt-dlp process: {e}"), false))?;

    let stdout_pipe = child.stdout.take().expect("stdout piped");
    let stderr_pipe = child.stderr.take().expect("stderr piped");

    let prog_clone = Arc::clone(progress);
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout_pipe).lines();
        let mut collected = String::new();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(rest) = line.strip_prefix("EDIBLS_PROG:") {
                let cleaned = rest.trim().trim_end_matches('%').trim();
                if let Ok(val) = cleaned.parse::<f32>() {
                    let pct = val.round().clamp(0.0, 100.0) as u8;
                    let prev = prog_clone.load(Ordering::Relaxed);
                    if pct > prev {
                        prog_clone.store(pct, Ordering::Relaxed);
                    }
                }
            } else {
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
        .map_err(|e| (format!("Failed waiting for yt-dlp process: {e}"), false))?;

    let stdout = stdout_task.await.unwrap_or_default();
    let stderr = stderr_task.await.unwrap_or_default();

    if status.success() {
        if let Ok(recorded) = tokio::fs::read_to_string(&filepath_record).await {
            let file_path_str = recorded.lines().last().unwrap_or("").trim();
            if !file_path_str.is_empty() {
                let path = PathBuf::from(file_path_str);
                if path.exists() {
                    progress.store(100, Ordering::Relaxed);
                    log::info!("Downloaded file saved at: {:?}", path);
                    return Ok(path);
                }
            }
        }
    }

    let combined = format!("{stdout}\n{stderr}");
    log::error!("yt-dlp execution failed or skipped: {}", combined.trim());

    if combined.contains("does not pass filter duration") {
        Err((
            format!(
                "Видео слишком длинное (максимум {} минут)",
                config.max_duration_secs / 60
            ),
            false,
        ))
    } else if combined.contains("File is larger than max-filesize") {
        Err((
            format!(
                "Размер файла превышает лимит в {} МБ",
                config.max_filesize_mb
            ),
            true,
        ))
    } else {
        Err((
            "Не удалось скачать медиафайл по указанной ссылке".to_string(),
            false,
        ))
    }
}
