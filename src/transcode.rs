//! The transcode workers: take one uploaded file and turn it into an HLS
//! ABR ladder + a thumbnail — lessons 03/04, automated. Jobs arrive on a
//! channel (a stand-in for NATS in Phase 2); VSTREAM_WORKERS of them run at once.
use crate::catalog::{Catalog, Status};
use serde_json::Value;
use std::{ffi::OsStr, path::{Path, PathBuf}, process::Stdio, time::Duration};
use std::sync::Arc;
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, BufReader}, process::Command, sync::{mpsc, Mutex}};

pub type Jobs = Arc<Mutex<mpsc::UnboundedReceiver<String>>>;

/// (name, height, video kbps). We never upscale: a 720p upload gets 720p and below.
pub type Rung = (String, u32, u32);
const LADDER: [(&str, u32, u32); 4] = [
    ("1080p", 1080, 5000),
    ("720p", 720, 2800),
    ("480p", 480, 1400),
    ("360p", 360, 800),
];
const SEGMENT_SECS: u32 = 4;

pub struct Probe {
    pub duration: f64,
    pub height: u32,
    pub has_audio: bool,
}

/// Ask ffprobe what's inside a file or live URL. Errors if there's no video stream.
pub async fn probe(input: impl AsRef<OsStr>, input_args: &[&str]) -> Result<Probe, String> {
    let run = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "stream=codec_type,height:format=duration", "-of", "json"])
        .args(input_args)
        .arg(input)
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(Duration::from_secs(15), run).await
        .map_err(|_| "ffprobe timed out".to_string())?
        .map_err(|e| format!("could not run ffprobe: {e}"))?;
    let json: Value = serde_json::from_slice(&out.stdout).map_err(|_| "not a media file".to_string())?;
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video" && s["height"].as_u64().is_some())
        .ok_or("no video stream found")?;
    Ok(Probe {
        duration: json["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0),
        height: video["height"].as_u64().unwrap() as u32,
        has_audio: streams.iter().any(|s| s["codec_type"] == "audio"),
    })
}

/// Workers share one queue: whoever is free takes the next job.
pub async fn worker(catalog: Catalog, uploads: PathBuf, videos: PathBuf, jobs: Jobs) {
    loop {
        let Some(id) = jobs.lock().await.recv().await else { break };
        let Some(video) = catalog.get(&id).await else { continue }; // deleted while queued
        let src = uploads.join(&video.source_file);
        let out = videos.join(&id);
        println!("transcode {id}: start");
        catalog.update(&id, true, |v| { v.status = Status::Processing; v.progress = 0; v.error = None; }).await;
        match transcode(&catalog, &id, &src, &out).await {
            Ok((duration, renditions)) => {
                println!("transcode {id}: ready {renditions:?}");
                catalog.update(&id, true, |v| {
                    v.status = Status::Ready;
                    v.progress = 100;
                    v.duration_secs = duration;
                    v.renditions = renditions;
                }).await;
            }
            Err(e) => {
                eprintln!("transcode {id}: failed: {e}");
                let _ = tokio::fs::remove_dir_all(&out).await;
                catalog.update(&id, true, |v| { v.status = Status::Failed; v.error = Some(e); }).await;
            }
        }
    }
}

/// The rungs of `ladder` at or below the source height. A source smaller than
/// every rung gets one rung at its own size (even, for x264).
pub fn pick_rungs(ladder: &[(&str, u32, u32)], source_height: u32) -> Vec<Rung> {
    let rungs: Vec<Rung> = ladder.iter()
        .filter(|(_, h, _)| *h <= source_height)
        .map(|(n, h, k)| (n.to_string(), *h, *k))
        .collect();
    if rungs.is_empty() {
        let h = source_height & !1;
        return vec![(format!("{h}p"), h, 500)];
    }
    rungs
}

/// ffmpeg output args for an HLS ABR ladder, shared by VOD and live. The
/// caller adds the playlist flags, segment filename and output path.
pub fn ladder_args(rungs: &[Rung], has_audio: bool, segment_secs: u32) -> Vec<String> {
    // One decode, N scaled outputs: [0:v]split=3[s0][s1][s2];[s0]scale=-2:1080[v0];...
    let n = rungs.len();
    let mut filter = format!("[0:v]split={n}{}", (0..n).map(|i| format!("[s{i}]")).collect::<String>());
    for (i, (_, h, _)) in rungs.iter().enumerate() {
        filter += &format!(";[s{i}]scale=-2:{h}[v{i}]");
    }

    let mut args: Vec<String> = vec!["-filter_complex".into(), filter];
    let mut stream_map = vec![];
    for (i, (name, _, kbps)) in rungs.iter().enumerate() {
        args.extend([
            "-map".into(), format!("[v{i}]"), format!("-c:v:{i}"), "libx264".into(),
            format!("-b:v:{i}"), format!("{kbps}k"),
            format!("-maxrate:v:{i}"), format!("{}k", kbps * 107 / 100),
            format!("-bufsize:v:{i}"), format!("{}k", kbps * 3 / 2),
        ]);
        stream_map.push(if has_audio { format!("v:{i},a:{i},name:{name}") } else { format!("v:{i},name:{name}") });
    }
    if has_audio {
        for _ in 0..n { args.extend(["-map".into(), "0:a:0".into()]); }
        args.extend(["-c:a", "aac", "-b:a", "128k", "-ac", "2"].map(String::from));
    }
    // A keyframe exactly every segment, whatever the frame rate, so every
    // rung's segments line up and the player can switch quality at any boundary.
    args.extend([
        "-preset".into(), "veryfast".into(), "-sc_threshold".into(), "0".into(),
        "-force_key_frames".into(), format!("expr:gte(t,n_forced*{segment_secs})"),
        "-f".into(), "hls".into(), "-hls_time".into(), segment_secs.to_string(),
        "-master_pl_name".into(), "master.m3u8".into(),
        "-var_stream_map".into(), stream_map.join(" "),
    ]);
    args
}

async fn transcode(catalog: &Catalog, id: &str, src: &Path, out: &Path) -> Result<(f64, Vec<String>), String> {
    let p = probe(src, &[]).await?;
    let rungs = pick_rungs(&LADDER, p.height);

    let _ = tokio::fs::remove_dir_all(out).await; // clean slate on retries
    tokio::fs::create_dir_all(out).await.map_err(|e| e.to_string())?;

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostats", "-y", "-i"]).arg(src)
        .args(ladder_args(&rungs, p.has_audio, SEGMENT_SECS))
        .args(["-hls_playlist_type", "vod", "-hls_flags", "independent_segments", "-hls_segment_filename"])
        .arg(out.join("%v/seg_%04d.ts")).arg(out.join("%v/index.m3u8"))
        .args(["-progress", "pipe:1"])
        .stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| format!("could not run ffmpeg: {e}"))?;
    let mut stderr = child.stderr.take().unwrap();
    let errors = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    // -progress prints key=value lines; out_time_us tells us how far along we are.
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut last = 0u8;
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
            if p.duration > 0.0 {
                let pct = ((us / 1e6 / p.duration) * 100.0).clamp(0.0, 99.0) as u8;
                if pct != last {
                    last = pct;
                    catalog.update(id, false, |v| v.progress = pct).await;
                }
            }
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if !status.success() {
        let msg = errors.await.unwrap_or_default();
        return Err(format!("ffmpeg failed: {}", msg.lines().last().unwrap_or("unknown error")));
    }

    // Poster frame from ~10% in (capped at 5s), so it's not a black intro frame.
    let at = (p.duration * 0.1).min(5.0);
    let thumb = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-ss", &format!("{at:.2}"), "-i"]).arg(src)
        .args(["-frames:v", "1", "-vf", "scale=640:-2", "-q:v", "4"]).arg(out.join("thumb.jpg"))
        .status().await.map_err(|e| e.to_string())?;
    if !thumb.success() {
        eprintln!("transcode {id}: thumbnail failed (video still playable)");
    }

    Ok((p.duration, rungs.into_iter().map(|(n, _, _)| n).collect()))
}
