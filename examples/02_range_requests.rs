//! Lesson 02 — Range requests: "just give me bytes 5,000,000 to 6,000,000".
//!
//! Story: a library book. You don't photocopy the whole book to read
//! chapter 7 — you ask for pages 140-160. Browsers do the same with video:
//! they send `Range: bytes=START-END` and the server answers
//! `206 Partial Content` with only that slice. That's what makes seeking work.
//!
//! Run:  cargo run --example 02_range_requests
//! Try:  curl -s -D - -o /dev/null -r 0-99 http://localhost:3000/movie.mp4
use axum::{http::{header, HeaderMap, StatusCode}, response::IntoResponse, routing::get, Router};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

async fn movie(headers: HeaderMap) -> impl IntoResponse {
    let mut file = tokio::fs::File::open("media/sample.mp4").await.unwrap();
    let size = file.metadata().await.unwrap().len();

    // "bytes=100-199" -> (100, 199). Open-ended "bytes=100-"? We send at most 1 MB.
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).unwrap_or("bytes=0-");
    let (a, b) = range.trim_start_matches("bytes=").split_once('-').unwrap();
    let start: u64 = a.parse().unwrap_or(0);
    let end: u64 = b.parse().unwrap_or(start + 1_000_000).min(size - 1);

    file.seek(std::io::SeekFrom::Start(start)).await.unwrap();
    let mut slice = vec![0; (end - start + 1) as usize];
    file.read_exact(&mut slice).await.unwrap();
    println!("{range:<20} -> sent {} of {size} bytes", slice.len());

    (StatusCode::PARTIAL_CONTENT, [
        (header::CONTENT_TYPE, "video/mp4".to_string()),
        (header::ACCEPT_RANGES, "bytes".to_string()),
        (header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}")),
    ], slice)
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/movie.mp4", get(movie));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on http://localhost:3000/movie.mp4");
    axum::serve(listener, app).await.unwrap();
}

// Output (curl -r 0-99):
// HTTP/1.1 206 Partial Content
// accept-ranges: bytes
// content-range: bytes 0-99/60692797
// content-length: 100
//
// Server log:
// bytes=0-99           -> sent 100 of 60692797 bytes
// bytes=30000000-      -> sent 1000001 of 60692797 bytes   <- capped at ~1 MB
//
// Open the URL in a browser and drag the seek bar: watch the log print new
// ranges. But one file = one quality. A viewer on 2G gets the same 1080p
// bytes as a fibre user and buffers forever. Lesson 03 chops the video up.
