# Where we're heading: the architecture of a planet-scale video platform

This is the **target** shape. Each roadmap phase builds one slice of it.

```
                 ┌──────────── VOD (YouTube / Netflix) ─────────────┐
 creator ──► Upload API ──► Object store ──► Queue ──► Transcode workers
             (Rust, resumable)   (S3/MinIO)   (NATS)   (ffmpeg: ABR ladder,
                                                        HLS/DASH/CMAF)
                                                              │
                 ┌──────────── LIVE (JioHotstar) ───────────┐ │
 stadium ──► Ingest (RTMP/SRT) ──► Live transcoder ──► Packager (LL-HLS,
             camera feed            (1 input → 6+ ladders)   2–4s segments)
                                                              │
                                                              ▼
                                              Origin (+ origin shield tier)
                                                              │  1 request
                                    ┌─────────────────────────┼─────────────────┐
                                    ▼                         ▼                 ▼
                              Edge cache (city)         Edge cache          ISP-embedded cache
                              coalescing + LRU          ...                 (Netflix Open Connect,
                                    │                                       Google Global Cache)
                                    ▼  millions of requests
                              Players (hls.js / ExoPlayer / AVPlayer) — ABR picks quality

 Side services:  Catalog/metadata · Search · Recommendations · Auth + signed URLs/DRM
                 Playback API (which CDN? which ladder?) · Analytics (QoE, view counts)
```

## The five ideas that make it scale

1. **Video is just static files.** After transcoding, a movie is thousands of small
   `.ts`/`.m4s` chunks plus text playlists. Static files cache perfectly. (Lessons 03–04)
2. **The CDN does ~99% of the work.** Origin servers are small; the edges are huge
   and close to users. Netflix puts boxes *inside* ISPs; so does YouTube.
3. **Live sport is the easiest thing to cache and the hardest thing to survive.**
   Everyone wants the *same* segment in the *same* second → perfect cache hit ratio,
   but a thundering herd on every miss. Fix: request coalescing + tiered caches. (Lesson 05)
4. **ABR keeps it watchable.** Each player measures its own bandwidth and picks a rung
   of the ladder. Under extreme load the platform can also *cap* the ladder
   (e.g. no 4K during a final) to save bandwidth — graceful degradation.
5. **Scale before the spike, not during it.** A cricket match's peak is predictable
   (toss, last overs, a Kohli century). Platforms pre-warm capacity ahead of time and
   switch off non-essential features (recommendations, comments) when load spikes.

## Tool choices (and why)

| Need | Choice | Why |
|------|--------|-----|
| Services, edge cache | Rust: `tokio`, `axum`, `hyper` | no GC pauses, tiny memory per connection; Cloudflare's edge proxy (Pingora) is Rust |
| Encoding | ffmpeg (x264/x265/SVT-AV1) | the industry engine; Rust orchestrates it |
| Object store | MinIO locally, S3 in cloud | segments live here; origin reads from it |
| Job queue / events | NATS JetStream → Redpanda/Kafka | transcode jobs; view/QoE event streams |
| Metadata | Postgres → ScyllaDB | start simple; move to wide-column at huge scale |
| Analytics | ClickHouse | billions of playback events, fast aggregates |
| Run | Docker Compose → Kubernetes | autoscaling for the match-day surge |
