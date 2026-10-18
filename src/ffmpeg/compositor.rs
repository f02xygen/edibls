use std::path::{Path, PathBuf};
use crate::ffmpeg::probe::probe_video_metadata;
use crate::media::VideoFile;

/// Locates the subway surfers gameplay asset if available.
pub fn find_subway_asset() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("/app/assets/subway_surfers.mp4"),
        PathBuf::from("assets/subway_surfers.mp4"),
        std::env::current_dir().unwrap_or_default().join("assets/subway_surfers.mp4"),
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("assets/subway_surfers.mp4")))
            .unwrap_or_default(),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Combines a video with looped vertical Subway Surfers gameplay placed on the right side.
pub async fn attach_subway_surfers(
    video_path: &Path,
    subway_asset_path: &Path,
    output_path: &Path,
) -> Result<VideoFile, String> {
    if !subway_asset_path.exists() {
        return Err("Файл геймплея Subway Surfers не найден в assets/subway_surfers.mp4".to_string());
    }

    let (_w, h, dur, _) = probe_video_metadata(video_path).await;
    let target_h = h.unwrap_or(720);
    let target_h = if target_h % 2 != 0 { target_h + 1 } else { target_h };

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
        ])
        .arg(video_path)
        .args([
            "-stream_loop",
            "-1",
            "-i",
        ])
        .arg(subway_asset_path)
        .args([
            "-filter_complex",
            &format!("[1:v]fps=30,scale=-2:{target_h},setsar=1[subway];[0:v]fps=30,scale=-2:{target_h},setsar=1[main];[main][subway]hstack=inputs=2[outv]"),
            "-map",
            "[outv]",
            "-map",
            "0:a?",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "23",
            "-c:a",
            "copy",
            "-shortest",
        ])
        .arg(output_path)
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| format!("Ошибка запуска ffmpeg: {e}"))?;

    if !status.success() || !output_path.exists() {
        return Err("Не удалось сгенерировать видео с Subway Surfers".to_string());
    }

    let (out_w, out_h, out_dur, out_thumb) = probe_video_metadata(output_path).await;
    Ok(VideoFile {
        path: output_path.to_path_buf(),
        title: Some("Subway Surfers Edition".to_string()),
        width: out_w,
        height: out_h,
        duration_secs: out_dur.or(dur),
        thumbnail_path: out_thumb,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_subway_asset() {
        let asset = find_subway_asset();
        assert!(asset.is_some(), "assets/subway_surfers.mp4 should exist");
    }
}
