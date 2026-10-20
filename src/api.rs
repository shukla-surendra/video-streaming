//! HTTP API. Public routes list and describe ready videos; /api/admin/* needs
//! `Authorization: Bearer <ADMIN_TOKEN>` and handles upload + delete.
use crate::{catalog::{Catalog, Status, Video}, live::{AuthRequest, Live, LiveStatus, LiveStream}, transcode};
use axum::{
    extract::{Multipart, Path, Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::{io::AsyncWriteExt, sync::mpsc};

#[derive(Clone)]
pub struct AppState {
    pub catalog: Catalog,
    pub live: Live,
    pub jobs: mpsc::UnboundedSender<String>,
    pub uploads: PathBuf,
    pub videos: PathBuf,
    pub admin_token: Arc<String>,
    pub max_upload_bytes: u64,
}

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

fn bad(msg: impl Into<String>) -> ApiError { ApiError(StatusCode::BAD_REQUEST, msg.into()) }
fn not_found() -> ApiError { ApiError(StatusCode::NOT_FOUND, "video not found".into()) }
fn internal(e: impl std::fmt::Display) -> ApiError { ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()) }

/// What viewers see: no source filenames, errors or other admin detail.
#[derive(Serialize)]
pub struct PublicVideo {
    id: String,
    title: String,
    description: String,
    duration_secs: f64,
    renditions: Vec<String>,
    created_at: u64,
    views: u64,
    playlist: String,
    thumbnail: String,
}

impl From<Video> for PublicVideo {
    fn from(v: Video) -> Self {
        PublicVideo {
            playlist: format!("/stream/{}/master.m3u8", v.id),
            thumbnail: format!("/stream/{}/thumb.jpg", v.id),
            id: v.id, title: v.title, description: v.description,
            duration_secs: v.duration_secs, renditions: v.renditions, created_at: v.created_at,
            views: v.views,
        }
    }
}

pub async fn list_public(State(s): State<AppState>) -> Json<Vec<PublicVideo>> {
    Json(s.catalog.list().await.into_iter().filter(|v| v.status == Status::Ready).map(Into::into).collect())
}

pub async fn get_public(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<PublicVideo>, ApiError> {
    match s.catalog.get(&id).await {
        Some(v) if v.status == Status::Ready => Ok(Json(v.into())),
        _ => Err(not_found()),
    }
}

/// The player calls this once playback actually starts.
pub async fn add_view(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let views = s.catalog.add_view(&id).await.ok_or_else(not_found)?;
    Ok(Json(serde_json::json!({ "views": views })))
}

/// PATCH body for videos and live streams: only the fields sent are changed.
#[derive(Deserialize)]
pub struct Edit {
    title: Option<String>,
    description: Option<String>,
    record: Option<bool>,
}

impl Edit {
    /// Trimmed, length-checked (title, description).
    fn validate(&self) -> Result<(Option<String>, Option<String>), ApiError> {
        let title = self.title.as_deref().map(str::trim).map(String::from);
        if title.as_deref().is_some_and(|t| t.is_empty() || t.chars().count() > 200) {
            return Err(bad("title must be 1–200 characters"));
        }
        let description = self.description.as_deref().map(str::trim).map(String::from);
        if description.as_deref().is_some_and(|d| d.chars().count() > 5000) {
            return Err(bad("description must be at most 5000 characters"));
        }
        Ok((title, description))
    }
}

pub async fn edit_video(State(s): State<AppState>, Path(id): Path<String>, Json(e): Json<Edit>) -> Result<Json<Video>, ApiError> {
    let (title, description) = e.validate()?;
    s.catalog.get(&id).await.ok_or_else(not_found)?;
    s.catalog.update(&id, true, |v| {
        if let Some(t) = title { v.title = t; }
        if let Some(d) = description { v.description = d; }
    }).await;
    Ok(Json(s.catalog.get(&id).await.ok_or_else(not_found)?))
}

pub async fn require_admin(State(s): State<AppState>, req: Request, next: Next) -> Result<Response, ApiError> {
    let ok = req.headers().get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|t| t == s.admin_token.as_str());
    if !ok {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "bad or missing admin token".into()));
    }
    Ok(next.run(req).await)
}

pub async fn list_admin(State(s): State<AppState>) -> Json<Vec<Video>> {
    Json(s.catalog.list().await)
}

/// multipart/form-data: `title`, `description`, `file`. The file is streamed
/// to disk chunk by chunk — a 4 GB upload never sits in RAM (lesson 01!).
pub async fn upload(State(s): State<AppState>, mut form: Multipart) -> Result<(StatusCode, Json<Video>), ApiError> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let (mut title, mut description) = (String::new(), String::new());
    let mut saved: Option<(String, u64)> = None;

    let result = async {
        while let Some(mut field) = form.next_field().await.map_err(|e| bad(e.body_text()))? {
            match field.name() {
                Some("title") => title = field.text().await.map_err(|e| bad(e.body_text()))?.trim().to_string(),
                Some("description") => description = field.text().await.map_err(|e| bad(e.body_text()))?.trim().to_string(),
                Some("file") => {
                    if saved.is_some() { return Err(bad("only one file per upload")); }
                    let original = field.file_name().unwrap_or("upload").to_string();
                    let name = format!("{id}.{}", safe_ext(&original));
                    saved = Some((name.clone(), 0));
                    let mut file = tokio::fs::File::create(s.uploads.join(&name)).await.map_err(internal)?;
                    let mut bytes = 0u64;
                    while let Some(chunk) = field.chunk().await.map_err(|e| bad(e.body_text()))? {
                        bytes += chunk.len() as u64;
                        if bytes > s.max_upload_bytes {
                            return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "file too large".into()));
                        }
                        file.write_all(&chunk).await.map_err(internal)?;
                    }
                    file.flush().await.map_err(internal)?;
                    saved = Some((name, bytes));
                    if title.is_empty() { title = strip_ext(&original); }
                }
                _ => {}
            }
        }
        let (name, bytes) = saved.clone().ok_or_else(|| bad("missing file"))?;
        if bytes == 0 { return Err(bad("file is empty")); }
        // Reject non-videos now, while the uploader is still looking at the form.
        transcode::probe(s.uploads.join(&name), &[]).await.map_err(|e| bad(format!("not a playable video: {e}")))?;
        Ok((name, bytes))
    }.await;

    let (name, bytes) = match result {
        Ok(ok) => ok,
        Err(e) => {
            if let Some((name, _)) = saved { let _ = tokio::fs::remove_file(s.uploads.join(name)).await; }
            return Err(e);
        }
    };
    if title.is_empty() { title = "Untitled".into(); }
    let video = Video::new(id.clone(), title, description, name, bytes);
    s.catalog.insert(video.clone()).await;
    let _ = s.jobs.send(id);
    Ok((StatusCode::CREATED, Json(video)))
}

pub async fn retry(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<Video>, ApiError> {
    let v = s.catalog.get(&id).await.ok_or_else(not_found)?;
    if v.status != Status::Failed {
        return Err(ApiError(StatusCode::CONFLICT, "only failed videos can be retried".into()));
    }
    s.catalog.update(&id, true, |v| { v.status = Status::Queued; v.progress = 0; v.error = None; }).await;
    let _ = s.jobs.send(id.clone());
    Ok(Json(s.catalog.get(&id).await.ok_or_else(not_found)?))
}

pub async fn delete(State(s): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    let v = s.catalog.get(&id).await.ok_or_else(not_found)?;
    if v.status == Status::Processing {
        return Err(ApiError(StatusCode::CONFLICT, "video is transcoding; delete it when it finishes".into()));
    }
    s.catalog.remove(&id).await;
    let _ = tokio::fs::remove_file(s.uploads.join(&v.source_file)).await;
    let _ = tokio::fs::remove_dir_all(s.videos.join(&id)).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Keep only a short alphanumeric extension; never trust a client filename as a path.
fn safe_ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) if (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) => ext.to_ascii_lowercase(),
        _ => "bin".into(),
    }
}

fn strip_ext(name: &str) -> String {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem).trim().to_string()
}

// ---------------------------------------------------------------- live

#[derive(Serialize)]
pub struct PublicLive {
    id: String,
    title: String,
    description: String,
    status: LiveStatus,
    started_at: Option<u64>,
    viewers: usize,
    playlist: String,
}

impl PublicLive {
    fn new(s: LiveStream, live: &Live) -> Self {
        PublicLive {
            viewers: live.viewer_count(&s.id),
            playlist: format!("/livestream/{}/master.m3u8", s.id),
            id: s.id, title: s.title, description: s.description,
            status: s.status, started_at: s.started_at,
        }
    }
}

/// Admin view: everything, plus where to point OBS.
#[derive(Serialize)]
pub struct AdminLive {
    #[serde(flatten)]
    stream: LiveStream,
    viewers: usize,
    rtmp_url: String,
    srt_url: String,
}

impl AdminLive {
    fn new(s: LiveStream, live: &Live) -> Self {
        AdminLive {
            viewers: live.viewer_count(&s.id),
            rtmp_url: live.rtmp_url().to_string(),
            srt_url: format!("{}?streamid=publish:live/{}", live.srt_url(), s.stream_key),
            stream: s,
        }
    }
}

/// Streams on air right now.
pub async fn list_live(State(s): State<AppState>) -> Json<Vec<PublicLive>> {
    Json(s.live.list().await.into_iter()
        .filter(|l| l.status == LiveStatus::Live)
        .map(|l| PublicLive::new(l, &s.live))
        .collect())
}

/// Any stream, on air or not, so a viewer can wait on the page until it starts.
pub async fn get_live(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<PublicLive>, ApiError> {
    let l = s.live.get(&id).await.ok_or_else(not_found)?;
    Ok(Json(PublicLive::new(l, &s.live)))
}

#[derive(Deserialize)]
pub struct Heartbeat {
    viewer: String,
}

pub async fn heartbeat(State(s): State<AppState>, Path(id): Path<String>, Json(hb): Json<Heartbeat>) -> Result<Json<serde_json::Value>, ApiError> {
    if hb.viewer.is_empty() || hb.viewer.len() > 64 {
        return Err(bad("bad viewer id"));
    }
    match s.live.get(&id).await {
        Some(l) if l.status == LiveStatus::Live => Ok(Json(serde_json::json!({ "viewers": s.live.heartbeat(&id, &hb.viewer) }))),
        Some(_) => Ok(Json(serde_json::json!({ "viewers": 0 }))),
        None => Err(not_found()),
    }
}

pub async fn list_live_admin(State(s): State<AppState>) -> Json<Vec<AdminLive>> {
    Json(s.live.list().await.into_iter().map(|l| AdminLive::new(l, &s.live)).collect())
}

#[derive(Deserialize)]
pub struct NewLive {
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
}

pub async fn create_live(State(s): State<AppState>, Json(req): Json<NewLive>) -> Result<(StatusCode, Json<AdminLive>), ApiError> {
    let title = req.title.trim();
    if title.is_empty() { return Err(bad("title is required")); }
    let l = s.live.create(title.to_string(), req.description.trim().to_string()).await;
    Ok((StatusCode::CREATED, Json(AdminLive::new(l, &s.live))))
}

pub async fn edit_live(State(s): State<AppState>, Path(id): Path<String>, Json(e): Json<Edit>) -> Result<Json<AdminLive>, ApiError> {
    let (title, description) = e.validate()?;
    let l = s.live.edit(&id, |l| {
        if let Some(t) = title { l.title = t; }
        if let Some(d) = description { l.description = d; }
        if let Some(r) = e.record { l.record = r; }
    }).await.ok_or_else(not_found)?;
    Ok(Json(AdminLive::new(l, &s.live)))
}

pub async fn reset_live_key(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<AdminLive>, ApiError> {
    let l = s.live.reset_key(&id).await.ok_or_else(not_found)?;
    Ok(Json(AdminLive::new(l, &s.live)))
}

pub async fn delete_live(State(s): State<AppState>, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    s.live.delete(&id).await.ok_or_else(not_found)?;
    Ok(StatusCode::NO_CONTENT)
}

/// MediaMTX calls this for every publish/read attempt; any 2xx means "allowed".
pub async fn mediamtx_auth(State(s): State<AppState>, Json(req): Json<AuthRequest>) -> StatusCode {
    if s.live.authorize(&req).await { StatusCode::OK } else { StatusCode::UNAUTHORIZED }
}

/// Live playlists change every segment, so caches may hold them only for a
/// second (and serve them at most 2 s stale while refetching); segments have
/// unique names and never change, so cache them long.
pub async fn live_cache_headers(req: Request, next: Next) -> Response {
    let is_playlist = req.uri().path().ends_with(".m3u8");
    let mut res = next.run(req).await;
    let value = match (res.status().is_success(), is_playlist) {
        (true, true) => "public, max-age=1, stale-while-revalidate=2",
        (true, false) => "public, max-age=3600",
        (false, _) => "no-store", // a stream that isn't up yet must not stay "missing" in caches
    };
    res.headers_mut().insert(header::CACHE_CONTROL, header::HeaderValue::from_static(value));
    res
}
