use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoMetadata {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub duration_secs: Option<u32>,
    pub thumbnail_path: Option<PathBuf>,
}

/// Checks whether a media file contains at least one audio stream.
pub async fn has_audio_stream(path: &Path) -> bool {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "a:0",
            "-show_entries", "stream=codec_type",
            "-of", "csv=p=0",
        ])
        .arg(path)
        .kill_on_drop(true)
        .output()
        .await;
    match output {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.trim() == "audio"
        }
        Err(_) => false,
    }
}

/// Probes video dimensions, duration, and extracts a thumbnail frame (max 320x320) via ffprobe and ffmpeg.
pub async fn probe_video_metadata(
    path: &Path,
) -> (Option<u32>, Option<u32>, Option<u32>, Option<PathBuf>) {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,duration:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .await
        .ok();

    let mut width = None;
    let mut height = None;
    let mut duration_secs = None;

    if let Some(out) = output {
        if out.status.success() {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                if let Some(stream) = val.pointer("/streams/0") {
                    width = stream.get("width").and_then(|v| v.as_u64()).map(|v| v as u32);
                    height = stream.get("height").and_then(|v| v.as_u64()).map(|v| v as u32);
                    if let Some(d_str) = stream.get("duration").and_then(|v| v.as_str()) {
                        if let Ok(d) = d_str.parse::<f64>() {
                            if d > 0.0 {
                                duration_secs = Some(d.round() as u32);
                            }
                        }
                    }
                }
                if duration_secs.is_none() {
                    if let Some(d_str) = val.pointer("/format/duration").and_then(|v| v.as_str()) {
                        if let Ok(d) = d_str.parse::<f64>() {
                            if d > 0.0 {
                                duration_secs = Some(d.round() as u32);
                            }
                        }
                    }
                }
            }
        }
    }

    let thumb_path = path.with_extension("thumb.jpg");
    let ss_time = match duration_secs {
        Some(d) if d > 3 => "1",
        Some(_) => "0",
        None => "1",
    };

    let thumb_res = tokio::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-ss",
            ss_time,
            "-i",
        ])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='min(320,iw)':'min(320,ih)':force_original_aspect_ratio=decrease",
            "-q:v",
            "3",
        ])
        .arg(&thumb_path)
        .kill_on_drop(true)
        .status()
        .await;

    let final_thumb = if thumb_res.map(|s| s.success()).unwrap_or(false) && thumb_path.exists() {
        Some(thumb_path)
    } else {
        None
    };

    (width, height, duration_secs, final_thumb)
}

/// Probes video file dimensions (width, height) using ffprobe.
#[allow(dead_code)]
pub async fn get_video_dimensions(path: &Path) -> Option<(u32, u32)> {
    let (w, h, _, _) = probe_video_metadata(path).await;
    match (w, h) {
        (Some(width), Some(height)) if width > 0 && height > 0 => Some((width, height)),
        _ => None,
    }
}
