//! The catalog: every video's metadata, kept in memory and saved to one JSON
//! file. Phase 5 swaps this for Postgres; the API around it stays the same.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::{Path, PathBuf}, sync::{atomic::{AtomicBool, Ordering}, Arc}, time::SystemTime};
use tokio::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Processing,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: Status,
    pub progress: u8, // 0–100 while transcoding
    pub duration_secs: f64,
    pub renditions: Vec<String>, // e.g. ["1080p", "720p", "360p"]
    pub error: Option<String>,
    pub created_at: u64, // unix seconds
    pub source_file: String, // original upload, under data/uploads/
    pub source_bytes: u64,
    #[serde(default)]
    pub views: u64,
}

impl Video {
    pub fn new(id: String, title: String, description: String, source_file: String, source_bytes: u64) -> Self {
        Video {
            id, title, description, source_file, source_bytes, created_at: now_secs(),
            status: Status::Queued, progress: 0, duration_secs: 0.0,
            renditions: vec![], error: None, views: 0,
        }
    }
}

#[derive(Clone)]
pub struct Catalog {
    videos: Arc<Mutex<HashMap<String, Video>>>,
    path: Arc<PathBuf>,
    dirty: Arc<AtomicBool>, // view counts changed since the last save
}

impl Catalog {
    pub async fn load(path: PathBuf) -> Self {
        let videos = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<Vec<Video>>(&bytes)
                .expect("catalog.json is corrupt")
                .into_iter()
                .map(|v| (v.id.clone(), v))
                .collect(),
            Err(_) => HashMap::new(),
        };
        Catalog { videos: Arc::new(Mutex::new(videos)), path: Arc::new(path), dirty: Default::default() }
    }

    /// Newest first.
    pub async fn list(&self) -> Vec<Video> {
        let mut all: Vec<Video> = self.videos.lock().await.values().cloned().collect();
        all.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        all
    }

    pub async fn get(&self, id: &str) -> Option<Video> {
        self.videos.lock().await.get(id).cloned()
    }

    pub async fn insert(&self, video: Video) {
        let mut videos = self.videos.lock().await;
        videos.insert(video.id.clone(), video);
        self.save(&videos).await;
    }

    pub async fn remove(&self, id: &str) -> Option<Video> {
        let mut videos = self.videos.lock().await;
        let removed = videos.remove(id);
        self.save(&videos).await;
        removed
    }

    /// Change one video. `persist = false` for chatty updates like progress ticks.
    pub async fn update(&self, id: &str, persist: bool, f: impl FnOnce(&mut Video)) {
        let mut videos = self.videos.lock().await;
        if let Some(v) = videos.get_mut(id) {
            f(v);
            if persist {
                self.save(&videos).await;
            }
        }
    }

    /// Count a view. Rewriting the whole catalog on every view would be silly,
    /// so this only marks it dirty; `flush` (every few seconds) saves.
    pub async fn add_view(&self, id: &str) -> Option<u64> {
        let mut videos = self.videos.lock().await;
        let v = videos.get_mut(id).filter(|v| v.status == Status::Ready)?;
        v.views += 1;
        self.dirty.store(true, Ordering::Relaxed);
        Some(v.views)
    }

    pub async fn flush(&self) {
        if self.dirty.swap(false, Ordering::Relaxed) {
            let videos = self.videos.lock().await;
            self.save(&videos).await;
        }
    }

    async fn save(&self, videos: &HashMap<String, Video>) {
        self.dirty.store(false, Ordering::Relaxed);
        let all: Vec<&Video> = videos.values().collect();
        save_json(&self.path, &all).await;
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Write to a temp file then rename, so a crash never leaves half a file.
pub async fn save_json(path: &Path, value: &impl Serialize) {
    let json = serde_json::to_vec_pretty(value).unwrap();
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = async {
        tokio::fs::write(&tmp, json).await?;
        tokio::fs::rename(&tmp, path).await
    }
    .await
    {
        eprintln!("saving {} failed: {e}", path.display());
    }
}
