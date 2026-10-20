//! Live streaming. A broadcaster (OBS, a phone app, ffmpeg) pushes RTMP or SRT
//! to MediaMTX. MediaMTX asks us "is this stream key valid?" (`authorize`);
//! if yes, we pull the feed back over RTSP and run ONE ffmpeg that turns it
//! into a sliding-window HLS ladder — 1 input → N renditions, the shape of
//! Phase 4's cricket match, minus the low-latency tricks. The same ffmpeg also
//! copies the untouched feed to disk; when the broadcast ends that recording
//! goes through the normal upload pipeline and becomes a library video.
use crate::catalog::{now_secs, save_json, Catalog, Video};
use crate::transcode;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    process::Stdio,
    sync::{Arc, Mutex as SyncMutex},
    time::{Duration, Instant},
};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, process::Command, sync::{mpsc, Mutex, Notify}};

/// Live encoding has to keep up with real time, so the ladder tops out at 720p.
const LADDER: [(&str, u32, u32); 3] = [("720p", 720, 2800), ("480p", 480, 1400), ("360p", 360, 800)];
const SEGMENT_SECS: u32 = 2;
const WINDOW: u32 = 10; // segments in the playlist (~20 s); older ones are deleted
const VIEWER_TTL: Duration = Duration::from_secs(30); // players heartbeat every 10 s
const RECONNECT_SECS: u32 = 10; // after a broadcast ends, keep watching for the encoder to come back
const MIN_RECORDING_SECS: f64 = 5.0; // shorter broadcasts (tests, false starts) aren't kept

enum SessionEnd { NoFeed, Ended, Stopped }

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveStatus {
    Offline,  // waiting for the broadcaster
    Starting, // feed received, first segments being made
    Live,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LiveStream {
    pub id: String,
    pub title: String,
    pub description: String,
    pub stream_key: String, // secret: whoever has it can broadcast on this stream
    pub status: LiveStatus,
    pub renditions: Vec<String>,
    pub started_at: Option<u64>,
    pub created_at: u64,
    #[serde(default = "yes")]
    pub record: bool, // save each broadcast to the video library
}

fn yes() -> bool { true }

/// Where broadcasters connect, and where we pull feeds from.
pub struct Ingest {
    pub rtsp_base: String, // e.g. rtsp://mediamtx:8554
    pub rtmp_url: String,  // shown to broadcasters
    pub srt_url: String,
}

/// What MediaMTX POSTs to its auth URL for every publish/read attempt.
#[derive(Deserialize)]
pub struct AuthRequest {
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub path: String,
}

#[derive(Clone)]
pub struct Live(Arc<Inner>);

struct Inner {
    streams: Mutex<HashMap<String, LiveStream>>,
    sessions: SyncMutex<HashMap<String, Arc<Notify>>>, // running transcoders; notify = stop
    viewers: SyncMutex<HashMap<String, HashMap<String, Instant>>>,
    path: PathBuf,
    out_dir: PathBuf,
    ingest: Ingest,
    // Our own ffmpeg reads from MediaMTX with these; nobody else may read raw feeds.
    pull_user: String,
    pull_pass: String,
    // Recordings become ordinary uploads: a file in uploads/ + a job on the queue.
    catalog: Catalog,
    jobs: mpsc::UnboundedSender<String>,
    uploads: PathBuf,
}

impl Live {
    pub async fn load(
        path: PathBuf, out_dir: PathBuf, ingest: Ingest,
        catalog: Catalog, jobs: mpsc::UnboundedSender<String>, uploads: PathBuf,
    ) -> Self {
        let mut streams: HashMap<String, LiveStream> = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<Vec<LiveStream>>(&bytes)
                .expect("live.json is corrupt")
                .into_iter()
                .map(|s| (s.id.clone(), s))
                .collect(),
            Err(_) => HashMap::new(),
        };
        for s in streams.values_mut() {
            s.status = LiveStatus::Offline; // nothing is running yet; resume_all() checks
            s.started_at = None;
        }
        let _ = tokio::fs::remove_dir_all(&out_dir).await; // stale segments from last run
        tokio::fs::create_dir_all(&out_dir).await.expect("cannot create live dir");
        Live(Arc::new(Inner {
            streams: Mutex::new(streams),
            sessions: Default::default(),
            viewers: Default::default(),
            path, out_dir, ingest, catalog, jobs, uploads,
            pull_user: "vstream".into(),
            pull_pass: uuid::Uuid::new_v4().simple().to_string(),
        }))
    }

    pub fn rtmp_url(&self) -> &str { &self.0.ingest.rtmp_url }
    pub fn srt_url(&self) -> &str { &self.0.ingest.srt_url }

    pub async fn list(&self) -> Vec<LiveStream> {
        let mut all: Vec<LiveStream> = self.0.streams.lock().await.values().cloned().collect();
        all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        all
    }

    pub async fn get(&self, id: &str) -> Option<LiveStream> {
        self.0.streams.lock().await.get(id).cloned()
    }

    pub async fn create(&self, title: String, description: String) -> LiveStream {
        let s = LiveStream {
            id: uuid::Uuid::new_v4().simple().to_string(),
            stream_key: new_key(),
            title, description,
            status: LiveStatus::Offline,
            renditions: vec![],
            started_at: None,
            created_at: now_secs(),
            record: true,
        };
        let mut streams = self.0.streams.lock().await;
        streams.insert(s.id.clone(), s.clone());
        self.save(&streams).await;
        s
    }

    /// Change title/description/record. Takes effect for the next broadcast's
    /// recording; viewers see the new title right away.
    pub async fn edit(&self, id: &str, f: impl FnOnce(&mut LiveStream)) -> Option<LiveStream> {
        let mut streams = self.0.streams.lock().await;
        let s = streams.get_mut(id)?;
        f(s);
        let s = s.clone();
        self.save(&streams).await;
        Some(s)
    }

    /// New key; also cuts off whoever is broadcasting with the old one.
    pub async fn reset_key(&self, id: &str) -> Option<LiveStream> {
        let s = {
            let mut streams = self.0.streams.lock().await;
            let s = streams.get_mut(id)?;
            s.stream_key = new_key();
            let s = s.clone();
            self.save(&streams).await;
            s
        };
        self.stop(id);
        Some(s)
    }

    pub async fn delete(&self, id: &str) -> Option<LiveStream> {
        let removed = {
            let mut streams = self.0.streams.lock().await;
            let removed = streams.remove(id);
            self.save(&streams).await;
            removed
        };
        self.stop(id);
        removed
    }

    async fn save(&self, streams: &HashMap<String, LiveStream>) {
        let all: Vec<&LiveStream> = streams.values().collect();
        save_json(&self.0.path, &all).await;
    }

    async fn set(&self, id: &str, f: impl FnOnce(&mut LiveStream)) {
        if let Some(s) = self.0.streams.lock().await.get_mut(id) {
            f(s);
        }
    }

    fn stop(&self, id: &str) {
        if let Some(stop) = self.0.sessions.lock().unwrap().get(id) {
            stop.notify_one();
        }
    }

    /// MediaMTX's auth hook. Publishing needs a valid stream key in the path
    /// (`live/<key>`) and kicks off our transcoder; reading raw feeds is ours only.
    pub async fn authorize(&self, req: &AuthRequest) -> bool {
        match req.action.as_str() {
            "publish" => {
                let Some(key) = req.path.strip_prefix("live/") else { return false };
                let found = self.0.streams.lock().await.values().find(|s| s.stream_key == key).map(|s| s.id.clone());
                match found {
                    Some(id) => {
                        println!("live {id}: broadcaster connected");
                        self.start_session(id, key.to_string(), 15);
                        true
                    }
                    None => false,
                }
            }
            "read" => req.user == self.0.pull_user && req.password == self.0.pull_pass,
            _ => false,
        }
    }

    /// After a backend restart, encoders still connected to MediaMTX never
    /// re-authenticate, so look for each stream's feed once.
    pub async fn resume_all(&self) {
        for s in self.list().await {
            self.start_session(s.id, s.stream_key, 1);
        }
    }

    fn start_session(&self, id: String, key: String, wait_secs: u32) {
        let stop = Arc::new(Notify::new());
        {
            let mut sessions = self.0.sessions.lock().unwrap();
            if sessions.contains_key(&id) { return; } // already transcoding (or about to)
            sessions.insert(id.clone(), stop.clone());
        }
        let live = self.clone();
        tokio::spawn(async move {
            // Keep going while broadcasts keep coming: OBS reconnecting after a
            // network blip lands here instead of being missed.
            let mut wait = wait_secs;
            loop {
                let end = live.session(&id, &key, &stop, wait).await;
                live.0.viewers.lock().unwrap().remove(&id);
                live.set(&id, |s| { s.status = LiveStatus::Offline; s.started_at = None; }).await;
                let _ = tokio::fs::remove_dir_all(live.0.out_dir.join(&id)).await;
                match end {
                    Ok(SessionEnd::Ended) => { println!("live {id}: ended"); wait = RECONNECT_SECS; }
                    Ok(_) => break,
                    Err(e) => { eprintln!("live {id}: {e}"); break; }
                }
            }
            live.0.sessions.lock().unwrap().remove(&id);
        });
    }

    async fn session(&self, id: &str, key: &str, stop: &Notify, wait_secs: u32) -> Result<SessionEnd, String> {
        let base = self.0.ingest.rtsp_base.replacen("rtsp://", &format!("rtsp://{}:{}@", self.0.pull_user, self.0.pull_pass), 1);
        let url = format!("{base}/live/{key}");
        let rtsp = ["-rtsp_transport", "tcp"];

        // The publish was authorised a moment before the feed becomes readable.
        let mut probe = None;
        for _ in 0..wait_secs {
            tokio::select! {
                _ = stop.notified() => return Ok(SessionEnd::Stopped),
                r = transcode::probe(&url, &rtsp) => if let Ok(p) = r { probe = Some(p); break; },
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let Some(p) = probe else { return Ok(SessionEnd::NoFeed) };

        let rungs = transcode::pick_rungs(&LADDER, p.height);
        let dir = self.0.out_dir.join(id);
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.map_err(|e| e.to_string())?;
        let started = now_secs();
        let Some(stream) = self.get(id).await else { return Ok(SessionEnd::Stopped) };
        self.set(id, |s| {
            s.status = LiveStatus::Starting;
            s.renditions = rungs.iter().map(|(n, _, _)| n.clone()).collect();
            s.started_at = Some(started);
        }).await;
        println!("live {id}: transcoding {:?}", rungs.iter().map(|r| &r.0).collect::<Vec<_>>());

        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostats"]).args(rtsp).arg("-i").arg(&url)
            .args(transcode::ladder_args(&rungs, p.has_audio, SEGMENT_SECS))
            .args(["-tune", "zerolatency", "-hls_list_size", &WINDOW.to_string(),
                "-hls_flags", "delete_segments+independent_segments", "-hls_segment_filename"])
            // Unique names per broadcast, so caches can keep segments for an hour
            // without ever serving last week's seg_00001.ts.
            .arg(dir.join(format!("%v/seg_{started}_%05d.ts"))).arg(dir.join("%v/index.m3u8"));
        // Second output: the original feed, copied as-is (no re-encode, ~no CPU).
        // Matroska takes any codec a broadcaster might send.
        let recording = stream.record.then(|| {
            let vid = uuid::Uuid::new_v4().simple().to_string();
            (vid.clone(), format!("{vid}.mkv"))
        });
        if let Some((_, name)) = &recording {
            cmd.args(["-map", "0:v:0", "-map", "0:a:0?", "-c", "copy", "-f", "matroska"]).arg(self.0.uploads.join(name));
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| format!("could not run ffmpeg: {e}"))?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let errors = tokio::spawn(async move {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s).await;
            s
        });

        // "Live" only once the master playlist exists, so players never get a 404.
        let live = self.clone();
        let (id2, master) = (id.to_string(), dir.join("master.m3u8"));
        let went_live = tokio::spawn(async move {
            for _ in 0..60 {
                if tokio::fs::try_exists(&master).await.unwrap_or(false) {
                    live.set(&id2, |s| if s.status == LiveStatus::Starting { s.status = LiveStatus::Live }).await;
                    println!("live {id2}: on air");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });

        let result = tokio::select! {
            _ = stop.notified() => {
                // "q" asks ffmpeg to finish cleanly, so the recording is a valid file.
                let _ = stdin.write_all(b"q").await;
                if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
                    let _ = child.kill().await;
                }
                Ok(SessionEnd::Stopped)
            }
            status = child.wait() => match status {
                Ok(st) if st.success() => Ok(SessionEnd::Ended),
                Ok(_) => {
                    let msg = errors.await.unwrap_or_default();
                    // A broadcaster hanging up looks like a read error; that's a normal end.
                    println!("live {id}: ffmpeg exited: {}", msg.lines().last().unwrap_or("no output"));
                    Ok(SessionEnd::Ended)
                }
                Err(e) => Err(e.to_string()),
            },
        };
        went_live.abort();
        if let Some((vid, name)) = recording {
            self.save_recording(&stream, vid, name).await;
        }
        result
    }

    /// Hand a finished recording to the VOD pipeline, exactly like an upload.
    async fn save_recording(&self, stream: &LiveStream, vid: String, name: String) {
        let path = self.0.uploads.join(&name);
        let long_enough = transcode::probe(&path, &[]).await.is_ok_and(|p| p.duration >= MIN_RECORDING_SECS);
        let bytes = tokio::fs::metadata(&path).await.map_or(0, |m| m.len());
        if !long_enough || bytes == 0 {
            println!("live {}: recording too short, not kept", stream.id);
            let _ = tokio::fs::remove_file(&path).await;
            return;
        }
        let title = format!("{} (live recording)", stream.title);
        self.0.catalog.insert(Video::new(vid.clone(), title, stream.description.clone(), name, bytes)).await;
        let _ = self.0.jobs.send(vid.clone());
        println!("live {}: recording saved as video {vid}", stream.id);
    }

    /// A player says "still watching"; returns how many are watching now.
    pub fn heartbeat(&self, id: &str, viewer: &str) -> usize {
        let mut viewers = self.0.viewers.lock().unwrap();
        let v = viewers.entry(id.to_string()).or_default();
        let now = Instant::now();
        v.insert(viewer.to_string(), now);
        v.retain(|_, seen| now.duration_since(*seen) < VIEWER_TTL);
        v.len()
    }

    pub fn viewer_count(&self, id: &str) -> usize {
        let now = Instant::now();
        self.0.viewers.lock().unwrap().get(id)
            .map_or(0, |v| v.values().filter(|seen| now.duration_since(**seen) < VIEWER_TTL).count())
    }
}

fn new_key() -> String {
    format!("live_{}", uuid::Uuid::new_v4().simple())
}
