use std::path::Path;
use crate::ffmpeg::probe::has_audio_stream;
use crate::parsing::timestamps::TimeRange;

/// Builds cascaded atempo filters for ffmpeg (atempo only accepts 0.5 to 2.0).
pub fn build_atempo_filter(speed: f32) -> String {
    let mut current = speed;
    let mut filters = Vec::new();
    while current > 2.0 {
        filters.push("atempo=2.0".to_string());
        current /= 2.0;
    }
    while current < 0.5 {
        filters.push("atempo=0.5".to_string());
        current /= 0.5;
    }
    filters.push(format!("atempo={:.2}", current));
    filters.join(",")
}

/// Trims and/or adjusts playback speed of a media file (video or audio) using ffmpeg.
/// When only trimming without speed change, it tries stream copy first, falling back to re-encoding.
pub async fn process_media_cut_or_speed(
    input_path: &Path,
    time_range: Option<&TimeRange>,
    speed: Option<f32>,
    is_audio: bool,
    output_path: &Path,
) -> Result<(), String> {
    if !input_path.exists() {
        return Err(format!("Исходный файл {:?} не существует", input_path));
    }

    let speed_mult = speed.unwrap_or(1.0);
    let has_speed = (speed_mult - 1.0).abs() > 0.01;
    let has_cut = time_range.is_some();

    if !has_speed && !has_cut {
        if input_path != output_path {
            tokio::fs::copy(input_path, output_path)
                .await
                .map_err(|e| format!("Ошибка копирования файла: {e}"))?;
        }
        return Ok(());
    }

    // Fast path: only cut without speed change -> try stream copy first
    if has_cut && !has_speed {
        let tr = time_range.unwrap();
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.arg("-y")
            .arg("-ss")
            .arg(tr.start_secs.to_string())
            .arg("-to")
            .arg(tr.end_secs.to_string())
            .arg("-i")
            .arg(input_path)
            .args(["-c", "copy"]);

        if !is_audio {
            cmd.args(["-avoid_negative_ts", "make_zero"]);
        }
        cmd.arg(output_path);

        if let Ok(status) = cmd.kill_on_drop(true).status().await {
            if status.success() && output_path.exists() {
                if let Ok(meta) = tokio::fs::metadata(output_path).await {
                    if meta.len() > 1024 {
                        return Ok(());
                    }
                }
            }
        }
        let _ = tokio::fs::remove_file(output_path).await;
    }

    // Encoding path (handles speed and/or fallback for cuts)
    let atempo_filter = build_atempo_filter(speed_mult);

    if is_audio {
        let mut cmd = tokio::process::Command::new("ffmpeg");
        cmd.arg("-y");
        if let Some(tr) = time_range {
            cmd.arg("-ss").arg(tr.start_secs.to_string())
               .arg("-to").arg(tr.end_secs.to_string());
        }
        cmd.arg("-i").arg(input_path);
        if has_speed {
            cmd.args(["-filter:a", &atempo_filter]);
        }
        let ext = output_path.extension().and_then(|s| s.to_str()).unwrap_or("mp3").to_lowercase();
        if ext == "mp3" {
            cmd.args(["-c:a", "libmp3lame", "-q:a", "2"]);
        } else if ext == "flac" {
            cmd.args(["-c:a", "flac"]);
        } else {
            cmd.args(["-c:a", "aac", "-b:a", "192k"]);
        }
        cmd.args(["-map", "0:a", "-map", "0:v?", "-c:v", "copy"]);
        cmd.arg(output_path);

        let status = cmd
            .kill_on_drop(true)
            .status()
            .await
            .map_err(|e| format!("Ошибка запуска ffmpeg: {e}"))?;

        if !status.success() || !output_path.exists() {
            return Err("Не удалось обработать аудиофайл через ffmpeg".to_string());
        }
        return Ok(());
    }

    // Video path
    let has_audio = has_audio_stream(input_path).await;
    let v_pts = 1.0 / speed_mult;

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y");
    if let Some(tr) = time_range {
        cmd.arg("-ss").arg(tr.start_secs.to_string())
           .arg("-to").arg(tr.end_secs.to_string());
    }
    cmd.arg("-i").arg(input_path);

    if has_audio {
        if has_speed {
            let filter = format!("[0:v]setpts={v_pts:.4}*PTS[v];[0:a]{atempo_filter}[a]");
            cmd.args([
                "-filter_complex", &filter,
                "-map", "[v]",
                "-map", "[a]",
                "-c:v", "libx264",
                "-preset", "veryfast",
                "-crf", "23",
                "-c:a", "aac",
                "-b:a", "192k",
            ]);
        } else {
            // Cut re-encode fallback
            cmd.args([
                "-c:v", "libx264",
                "-preset", "veryfast",
                "-crf", "23",
                "-c:a", "copy",
            ]);
        }
    } else {
        if has_speed {
            let filter = format!("setpts={v_pts:.4}*PTS");
            cmd.args([
                "-filter:v", &filter,
                "-c:v", "libx264",
                "-preset", "veryfast",
                "-crf", "23",
            ]);
        } else {
            // Cut re-encode fallback for silent video
            cmd.args([
                "-c:v", "libx264",
                "-preset", "veryfast",
                "-crf", "23",
            ]);
        }
    }

    cmd.arg(output_path);

    let status = cmd
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| format!("Ошибка запуска ffmpeg: {e}"))?;

    if !status.success() || !output_path.exists() {
        return Err("Не удалось обработать видео через ffmpeg".to_string());
    }

    Ok(())
}

/// Extracts audio track from a video file into an audio container (e.g. mp3),
/// optionally applying timecode clipping and/or tempo adjustment.
pub async fn extract_audio_from_video(
    input_path: &Path,
    time_range: Option<&TimeRange>,
    speed: Option<f32>,
    output_path: &Path,
) -> Result<(), String> {
    if !input_path.exists() {
        return Err(format!("Исходный файл {:?} не существует", input_path));
    }

    if !has_audio_stream(input_path).await {
        return Err("В этом видео нет аудиодорожки.".to_string());
    }

    let speed_mult = speed.unwrap_or(1.0);
    let has_speed = (speed_mult - 1.0).abs() > 0.01;
    let atempo_filter = build_atempo_filter(speed_mult);

    let mut cmd = tokio::process::Command::new("ffmpeg");
    cmd.arg("-y");
    if let Some(tr) = time_range {
        cmd.arg("-ss")
            .arg(tr.start_secs.to_string())
            .arg("-to")
            .arg(tr.end_secs.to_string());
    }
    cmd.arg("-i").arg(input_path);
    cmd.arg("-vn");

    if has_speed {
        cmd.args(["-filter:a", &atempo_filter]);
    }

    let ext = output_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("mp3")
        .to_lowercase();
    if ext == "mp3" {
        cmd.args(["-c:a", "libmp3lame", "-q:a", "2"]);
    } else if ext == "flac" {
        cmd.args(["-c:a", "flac"]);
    } else {
        cmd.args(["-c:a", "aac", "-b:a", "192k"]);
    }
    cmd.arg(output_path);

    let status = cmd
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| format!("Ошибка запуска ffmpeg при извлечении аудио: {e}"))?;

    if !status.success() || !output_path.exists() {
        return Err("Не удалось извлечь аудио из видео через ffmpeg".to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_atempo_filter() {
        assert_eq!(build_atempo_filter(1.5), "atempo=1.50");
        assert_eq!(build_atempo_filter(2.0), "atempo=2.00");
        assert_eq!(build_atempo_filter(0.75), "atempo=0.75");
        assert_eq!(build_atempo_filter(3.0), "atempo=2.0,atempo=1.50");
        assert_eq!(build_atempo_filter(0.25), "atempo=0.5,atempo=0.50");
    }

    #[tokio::test]
    async fn test_process_audio_speed_and_cut() {
        let td = tempfile::tempdir().unwrap();
        let input_path = td.path().join("in.mp3");
        let output_path = td.path().join("out.mp3");

        // Generate 3 seconds of audio using lavfi sine
        let status = tokio::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi", "-i", "sine=frequency=1000:duration=3", input_path.to_str().unwrap()])
            .status()
            .await;
        if status.is_err() || !status.unwrap().success() {
            // ffmpeg not available in environment, skip gracefully
            return;
        }

        let tr = crate::media::TimeRange {
            start_secs: 1,
            end_secs: 2,
            raw_label: "1-2".to_string(),
        };

        let res = process_media_cut_or_speed(&input_path, Some(&tr), Some(1.25), true, &output_path).await;
        assert!(res.is_ok(), "Expected cut and speed adjustment to succeed on audio: {:?}", res);
        assert!(output_path.exists());
    }

    #[tokio::test]
    async fn test_extract_audio_from_video() {
        let td = tempfile::tempdir().unwrap();
        let video_path = td.path().join("in.mp4");
        let out_audio = td.path().join("out.mp3");

        // Generate 2 seconds of test video with audio
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f", "lavfi", "-i", "testsrc=size=320x240:rate=10:duration=2",
                "-f", "lavfi", "-i", "sine=frequency=1000:duration=2",
                "-c:v", "libx264", "-c:a", "aac",
                video_path.to_str().unwrap()
            ])
            .status()
            .await;
        if status.is_err() || !status.unwrap().success() {
            return;
        }

        let res = extract_audio_from_video(&video_path, None, Some(1.5), &out_audio).await;
        assert!(res.is_ok(), "Expected extract audio to succeed: {:?}", res);
        assert!(out_audio.exists());
    }
}

