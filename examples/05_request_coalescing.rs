//! Lesson 05 — Request coalescing: why 5 crore viewers don't melt the origin.
//!
//! Story: a cricket match, 4th ball of the 20th over. 10,000 viewers on the
//! SAME edge server all ask for `segment_42.ts` in the same instant.
//! A naive cache sees 10,000 misses and phones the origin 10,000 times.
//! A smart cache says "someone already went to fetch it — wait for them",
//! like one friend fetching chai for the whole group instead of everyone going.
//!
//! Run: cargo run --example 05_request_coalescing
use std::{collections::HashMap, sync::{atomic::{AtomicU32, Ordering}, Arc, Mutex}, time::Duration};
use tokio::sync::OnceCell;

static ORIGIN_HITS: AtomicU32 = AtomicU32::new(0);

async fn fetch_from_origin(name: &str) -> Arc<String> {
    ORIGIN_HITS.fetch_add(1, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(100)).await; // origin is far away
    Arc::new(format!("<bytes of {name}>"))
}

type Cache = Arc<Mutex<HashMap<String, Arc<OnceCell<Arc<String>>>>>>;

async fn coalesced_get(cache: Cache, name: &str) -> Arc<String> {
    // Everyone asking for the same segment gets the same OnceCell...
    let cell = cache.lock().unwrap().entry(name.into()).or_default().clone();
    // ...and only the first caller actually runs the fetch; the rest wait.
    cell.get_or_init(|| fetch_from_origin(name)).await.clone()
}

async fn ten_thousand_viewers(coalesce: bool) {
    ORIGIN_HITS.store(0, Ordering::SeqCst);
    let cache: Cache = Default::default();
    let viewers = (0..10_000).map(|_| {
        let cache = cache.clone();
        tokio::spawn(async move {
            if coalesce { coalesced_get(cache, "segment_42.ts").await }
            else { fetch_from_origin("segment_42.ts").await }
        })
    });
    futures::future::join_all(viewers).await;
    println!("coalescing={coalesce:<5} -> origin hits: {}", ORIGIN_HITS.load(Ordering::SeqCst));
}

#[tokio::main]
async fn main() {
    ten_thousand_viewers(false).await;
    ten_thousand_viewers(true).await;
}

// Output:
// coalescing=false -> origin hits: 10000
// coalescing=true  -> origin hits: 1
//
// Same 10,000 viewers, 10,000x less origin load. Real CDNs call this
// "request collapsing" (Varnish, Nginx `proxy_cache_lock`, CloudFront, Akamai).
// It is THE reason live sports can scale: every viewer of a match wants the
// exact same few segments at the exact same moment.
