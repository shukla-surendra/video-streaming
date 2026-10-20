//! vstream app: admin uploads a video → worker transcodes it to an HLS ABR
//! ladder → viewers browse and watch. Everything lessons 01–05 taught, wired up.
//!
//! Run:   cargo run
//! Open:  http://localhost:3000/        (viewers)
//!        http://localhost:3000/admin   (uploads; token printed at startup)
//!
//! Env:   ADMIN_TOKEN        admin password (default: random, printed at startup)
//!        VSTREAM_DATA       where uploads, segments and the catalog live (default: data)
//!        VSTREAM_MAX_UPLOAD_MB  default 4096
//!        VSTREAM_WORKERS    transcodes running at once (default 1; each uses several cores)
//!        PORT               default 3000
//!        MEDIAMTX_RTSP      where to pull live feeds from (default: rtsp://localhost:8554)
//!        LIVE_RTMP_URL / LIVE_SRT_URL  ingest addresses shown to broadcasters in the admin panel
mod api;
mod catalog;
mod live;
mod transcode;

use api::AppState;
use axum::{
    extract::DefaultBodyLimit,
    middleware,
    routing::{delete, get, post},
    Router,
};
use catalog::{Catalog, Status};
use std::{path::PathBuf, sync::Arc};
use tower_http::services::{ServeDir, ServeFile};

#[tokio::main]
async fn main() {
    let data = PathBuf::from(std::env::var("VSTREAM_DATA").unwrap_or_else(|_| "data".into()));
    let (uploads, videos, live_dir) = (data.join("uploads"), data.join("videos"), data.join("live"));
    for dir in [&uploads, &videos] {
        tokio::fs::create_dir_all(dir).await.expect("cannot create data dir");
    }
    let max_upload_mb: u64 = std::env::var("VSTREAM_MAX_UPLOAD_MB").ok().and_then(|v| v.parse().ok()).unwrap_or(4096);
    let admin_token = std::env::var("ADMIN_TOKEN").unwrap_or_else(|_| {
        let t = uuid::Uuid::new_v4().simple().to_string();
        println!("ADMIN_TOKEN not set; generated one for this run: {t}");
        t
    });

    let catalog = Catalog::load(data.join("catalog.json")).await;
    let (jobs, rx) = tokio::sync::mpsc::unbounded_channel();
    let rx: transcode::Jobs = Arc::new(tokio::sync::Mutex::new(rx));
    let workers: usize = std::env::var("VSTREAM_WORKERS").ok().and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    for _ in 0..workers {
        tokio::spawn(transcode::worker(catalog.clone(), uploads.clone(), videos.clone(), rx.clone()));
    }
    // View counts are saved in batches, not on every view.
    let flusher = catalog.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            flusher.flush().await;
        }
    });

    // Jobs that were queued or mid-transcode when the server stopped: run them again.
    for v in catalog.list().await.into_iter().rev() {
        if matches!(v.status, Status::Queued | Status::Processing) {
            catalog.update(&v.id, true, |v| { v.status = Status::Queued; v.progress = 0; }).await;
            let _ = jobs.send(v.id);
        }
    }

    let env = |k: &str, default: &str| std::env::var(k).unwrap_or_else(|_| default.into());
    let ingest = live::Ingest {
        rtsp_base: env("MEDIAMTX_RTSP", "rtsp://localhost:8554"),
        rtmp_url: env("LIVE_RTMP_URL", "rtmp://localhost:1935/live"),
        srt_url: env("LIVE_SRT_URL", "srt://localhost:8890"),
    };
    let live = live::Live::load(
        data.join("live.json"), live_dir.clone(), ingest,
        catalog.clone(), jobs.clone(), uploads.clone(),
    ).await;
    live.resume_all().await;

    let state = AppState {
        catalog, live, jobs, uploads, videos: videos.clone(),
        admin_token: Arc::new(admin_token),
        max_upload_bytes: max_upload_mb * 1024 * 1024,
    };

    let admin = Router::new()
        .route("/videos", get(api::list_admin).post(api::upload))
        .route("/videos/{id}", delete(api::delete).patch(api::edit_video))
        .route("/videos/{id}/retry", post(api::retry))
        .route("/live", get(api::list_live_admin).post(api::create_live))
        .route("/live/{id}", delete(api::delete_live).patch(api::edit_live))
        .route("/live/{id}/key", post(api::reset_live_key))
        .layer(DefaultBodyLimit::max((state.max_upload_bytes + 1024 * 1024) as usize))
        .route_layer(middleware::from_fn_with_state(state.clone(), api::require_admin));

    let app = Router::new()
        .route("/api/videos", get(api::list_public))
        .route("/api/videos/{id}", get(api::get_public))
        .route("/api/videos/{id}/view", post(api::add_view))
        .route("/api/live", get(api::list_live))
        .route("/api/live/{id}", get(api::get_live))
        .route("/api/live/{id}/heartbeat", post(api::heartbeat))
        .route("/internal/mediamtx/auth", post(api::mediamtx_auth))
        .nest("/api/admin", admin)
        .with_state(state)
        // After transcoding a video is just static files (lesson 03) — any CDN could serve this.
        .nest_service("/stream", ServeDir::new(&videos))
        .nest(
            "/livestream",
            Router::new()
                .fallback_service(ServeDir::new(&live_dir))
                .layer(middleware::from_fn(api::live_cache_headers)),
        )
        .nest_service("/static", ServeDir::new("static"))
        .route_service("/watch", ServeFile::new("static/watch.html"))
        .route_service("/admin", ServeFile::new("static/admin.html"))
        .route_service("/", ServeFile::new("static/index.html"));

    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();
    println!("viewers: http://localhost:{port}/   admin: http://localhost:{port}/admin");
    axum::serve(listener, app).await.unwrap();
}
