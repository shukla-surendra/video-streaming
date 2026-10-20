//! Lesson 03 — HLS: a playlist of small chunks instead of one big file.
//!
//! Story: a web series vs a 3-hour film. Nobody downloads the whole film;
//! you get episode 1 (4 seconds), then episode 2, ... The `.m3u8` playlist is
//! the episode list. The player reads it and fetches chunks one by one.
//! Each chunk is a plain static file, so ANY web server or CDN can cache it.
//!
//! Prep: ./scripts/03_make_hls.sh   (and ./scripts/04_make_abr.sh for lesson 04)
//! Run:  cargo run --example 03_hls_server
//! Open: http://localhost:3000/                      (single quality)
//!       http://localhost:3000/?src=abr/master.m3u8  (lesson 04: 3 qualities)
use axum::{extract::Request, middleware::{self, Next}, response::Response, Router};
use tower_http::services::{ServeDir, ServeFile};

async fn log(req: Request, next: Next) -> Response {
    println!("GET {}", req.uri().path()); // watch the player walk the playlist
    next.run(req).await
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .nest_service("/media", ServeDir::new("media")) // .m3u8 + .ts are just files
        .fallback_service(ServeFile::new("static/player.html"))
        .layer(middleware::from_fn(log));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("open http://localhost:3000/");
    axum::serve(listener, app).await.unwrap();
}

// Output (server log, fetching like a player would with curl):
// GET /
// GET /media/hls/index.m3u8      <- 1. read the episode list
// GET /media/hls/seg_000.ts      <- 2. fetch chunk 0 (2,401,700 bytes = 4 seconds)
// GET /media/abr/360p/seg_000.ts <- same chunk at 360p: 440,108 bytes
// In a browser you'll see seg_001, seg_002, ... keep coming, just ahead of playback.
//
// The playlist (curl /media/hls/index.m3u8):
// #EXTM3U
// #EXT-X-TARGETDURATION:4
// #EXT-X-PLAYLIST-TYPE:VOD
// #EXTINF:4.000000,
// seg_000.ts
// #EXTINF:4.000000,
// seg_001.ts
//
// Lesson 04 (?src=abr/master.m3u8): same server, but the master playlist lists
// 1080p / 720p / 360p. In Chrome DevTools > Network set "Slow 4G" and watch
// the log switch from /abr/1080p/seg_... to /abr/360p/seg_... — that's ABR.
