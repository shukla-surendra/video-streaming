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

## The app — upload, transcode, watch

```bash
ADMIN_TOKEN=change-me cargo run --release
```

| URL | What |
|-----|------|
| http://localhost:3000/ | browse: live now, search, sort by newest / most viewed |
| http://localhost:3000/watch?v=ID | player — ABR "Auto" or pin a quality, view count, resume where you left off, up next |
| http://localhost:3000/admin | sign in with `ADMIN_TOKEN`: upload, progress, edit, retry, delete; live streams |

What happens on upload: the file streams to `data/uploads/` (never held in RAM) →
ffprobe rejects non-videos → a background worker runs ffmpeg to build an HLS ladder
(1080p/720p/480p/360p, never upscaled) + a thumbnail into `data/videos/<id>/` →
the video shows up on the home page. Jobs interrupted by a restart re-run on startup.

| Env var | Default | |
|---------|---------|---|
| `ADMIN_TOKEN` | random, printed at startup | admin password |
| `VSTREAM_DATA` | `data` | uploads, segments, `catalog.json` |
| `VSTREAM_MAX_UPLOAD_MB` | `4096` | |
| `VSTREAM_WORKERS` | `1` | transcodes at once; each ffmpeg already uses several cores |
| `PORT` | `3000` | |

### With Docker Compose

```bash
cp .env.example .env        # set ADMIN_TOKEN
docker compose up --build -d
```

| Service | URL | What |
|---------|-----|------|
| `frontend` | http://localhost:8080 | viewer site (nginx); caches segments with `proxy_cache_lock` — check the `X-Cache` header |
| `admin` | http://localhost:8081 | admin panel (nginx); streams uploads to the backend |
| `backend` | http://localhost:3000 | Rust API + ffmpeg worker; data in the `vstream-data` volume |
| `mediamtx` | rtmp://localhost:1935, srt://localhost:8890 | live ingest; checks stream keys with the backend |

The frontend refuses `/api/admin/*`, so the admin API is only reachable via the admin
site (or the backend port directly). Behind real domains, set `SITE_URL` / `ADMIN_URL` in `.env`.
`docker compose down -v` deletes all uploaded videos.

### Going live

1. Admin panel → **Live streams** → *Create stream*. You get a Server URL and a Stream key.
2. OBS → Settings → Stream → Service **Custom** → paste both → *Start Streaming*.
   (Or test without OBS: `ffmpeg -re -f lavfi -i testsrc2=size=1280x720:rate=30 -f lavfi -i sine -c:v libx264 -preset veryfast -g 60 -c:a aac -f flv rtmp://localhost:1935/live/<stream key>`)
3. ~8 s later it's on the home page under **Live now**; viewers get 720p/480p/360p ABR,
   ~4 s behind real time (the watch page shows the exact figure), with a live viewer count.

```
OBS ──RTMP/SRT──► MediaMTX ──"key ok?"──► backend /internal/mediamtx/auth
                     │
                     └──RTSP (internal)──► backend ffmpeg: 1 input → 3 rungs, 1 s segments,
                                           20 s sliding window ──► /livestream/<id>/master.m3u8
                                                                        │
                                           frontend nginx: playlists cached 1 s, segments 1 h,
                                           proxy_cache_lock → 1 backend hit per segment
```

**Recording:** each broadcast is also copied as-is to disk (no re-encode) and, when it
ends, goes through the upload pipeline as "<title> (live recording)". Turn it off per
stream with *Edit*. Broadcasts under 5 s aren't kept; the first ~4 s (while the backend
waits for the feed) aren't in the recording.

**Latency:** a frame is in a finished segment ~1 s after OBS sends it; the player sits
3 s behind the newest segment. After a stall (Wi-Fi hiccup, paused tab) it catches up by
playing at up to 1.2x, or jumps if it's more than 6 s behind — so delay doesn't pile up.

*New key* stops the current broadcast immediately. If OBS drops and reconnects within
10 s, the stream picks back up on its own. OBS on another machine or a phone: set
`LIVE_RTMP_URL` in `.env` to your LAN IP so the admin panel shows the right address.
Without Docker, `cargo run` expects MediaMTX at `rtsp://localhost:8554` (`MEDIAMTX_RTSP`),
configured with `authHTTPAddress: http://localhost:3000/internal/mediamtx/auth`.

API: `GET /api/videos`, `GET /api/videos/{id}`, `POST /api/videos/{id}/view`; with `Authorization: Bearer <token>`:
`GET|POST /api/admin/videos`, `PATCH|DELETE /api/admin/videos/{id}`, `POST /api/admin/videos/{id}/retry`.
Live: `GET /api/live` (on air now), `GET /api/live/{id}`, `POST /api/live/{id}/heartbeat`; admin:
`GET|POST /api/admin/live`, `PATCH|DELETE /api/admin/live/{id}` (`title`, `description`, `record`), `POST /api/admin/live/{id}/key`.
Code: `src/main.rs` (routes), `src/api.rs`, `src/catalog.rs`, `src/transcode.rs`, `src/live.rs`, pages in `static/`.

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
| 4 | **Live** (the cricket match) | ✅ in the app: RTMP/SRT ingest → live ABR transcode → sliding-window HLS, viewer counter. Next: LL-HLS (~2 s latency), DVR rewind, record-to-VOD, surge pre-scaling |
| 5 | Platform services | catalog (Postgres → ScyllaDB), search, view analytics (Redpanda → ClickHouse), recommendations |
| 6 | Run it like prod | Kubernetes, autoscaling, QoE dashboards (startup time, rebuffer ratio), chaos tests |

Each phase follows the same format: small numbered lessons, run → see real output.

## Layout

```
src/        the app: routes, API, catalog, transcode worker
examples/   one lesson per file (cargo run --example NN_name)
scripts/    ffmpeg helpers that prepare media/
static/     app pages (index, watch, admin) + the lesson player (player.html)
docker/     Dockerfiles + nginx configs for docker-compose.yml
docs/       architecture notes
media/      lesson video (git-ignored)
data/       app uploads, renditions, catalog (git-ignored)
```
