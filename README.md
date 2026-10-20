# vstream — learn how YouTube / Netflix / JioHotstar stream video, by building it

A learning repo. We build a video platform **one tiny idea at a time**, in Rust
(+ ffmpeg for the video maths), until the pieces add up to something shaped
like the real thing — including the hard part: **crores of people watching the
same live cricket match at once.**

The big picture of where we're heading lives in [docs/architecture.md](docs/architecture.md).

## Setup (once)

```bash
brew install ffmpeg            # rustup/cargo assumed
./scripts/00_make_sample.sh    # makes media/sample.mp4 — a 60s test movie (no downloads)
```

## Phase 1 — How video reaches your screen  ✅

Each lesson = one file, one idea, ~20–40 lines, with real output pasted at the bottom.

| # | Run | The one idea |
|---|-----|--------------|
| 01 | `cargo run --example 01_whole_file` | Sending the whole file is the DVD-rental model — RAM explodes, no seeking |
| 02 | `cargo run --example 02_range_requests` | `Range:` → `206 Partial Content` — how seeking works |
| 03 | `./scripts/03_make_hls.sh` then `cargo run --example 03_hls_server` | HLS: playlist + 4s chunks, all plain cacheable files |
| 04 | `./scripts/04_make_abr.sh`, same server, open `/?src=abr/master.m3u8` | ABR ladder: 1080p/720p/360p, player switches with your network |
| 05 | `cargo run --example 05_request_coalescing` | 10,000 viewers, 1 origin hit — why live sport doesn't melt servers |

Servers listen on http://localhost:3000. Stop with Ctrl-C.

## Roadmap

| Phase | Theme | You'll build |
|-------|-------|--------------|
| 1 ✅ | Playback basics | the 5 lessons above |
| 2 | Upload → transcode pipeline | resumable chunked upload (axum) → MinIO/S3 → job queue (NATS) → ffmpeg workers → HLS/CMAF |
| 3 | Delivery at scale | a Rust edge cache: LRU + coalescing + origin shield, signed URLs, load tests with `oha` |
| 4 | **Live** (the cricket match) | RTMP/SRT ingest → live transcode → sliding-window LL-HLS → edges; live viewer counter; surge pre-scaling |
| 5 | Platform services | catalog (Postgres → ScyllaDB), search, view analytics (Redpanda → ClickHouse), recommendations |
| 6 | Run it like prod | Kubernetes, autoscaling, QoE dashboards (startup time, rebuffer ratio), chaos tests |

Each phase follows the same format: small numbered lessons, run → see real output.

## Layout

```
examples/   one lesson per file (cargo run --example NN_name)
scripts/    ffmpeg helpers that prepare media/
static/     the browser player (hls.js)
docs/       architecture notes
media/      generated video (git-ignored)
```
