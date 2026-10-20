//! Lesson 01 — Serve the whole movie in one go.
//!
//! Story: renting a DVD. You carry the whole disc home before you can watch,
//! and you can't jump to the climax until it's in your hands.
//! Here the server reads the ENTIRE file into memory and sends it all.
//!
//! Run:   cargo run --example 01_whole_file
//! Try:   curl -sI http://localhost:3000/movie.mp4
//! Open:  http://localhost:3000/movie.mp4 in a browser
use axum::{http::header, response::IntoResponse, routing::get, Router};

async fn movie() -> impl IntoResponse {
    let bytes = tokio::fs::read("media/sample.mp4").await.unwrap(); // whole file in RAM!
    println!("sent {} bytes in one go", bytes.len());
    ([(header::CONTENT_TYPE, "video/mp4")], bytes)
}

#[tokio::main]
async fn main() {
    let app = Router::new().route("/movie.mp4", get(movie));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    println!("listening on http://localhost:3000/movie.mp4");
    axum::serve(listener, app).await.unwrap();
}

// Output (curl -sI):
// HTTP/1.1 200 OK
// content-type: video/mp4
// content-length: 60692797
//
// Server log:
// sent 60692797 bytes in one go
//
// Notice: NO `accept-ranges` header. Even a HEAD request made us read all
// 60 MB into RAM. A 4 GB movie x 1,000 viewers = 4 TB of RAM. Lesson 02 fixes this.
