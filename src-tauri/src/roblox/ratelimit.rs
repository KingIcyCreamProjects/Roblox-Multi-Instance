use std::{collections::HashMap, time::Duration};
use tokio::{sync::Mutex, time::Instant};

const MAX_KEYS: usize = 256;

/// Token bucket per key: `per_min` burst, refills `per_min`/60 per second.
#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl RateLimiter {
    pub async fn acquire(&self, key: &str, per_min: u32) {
        let cap = per_min as f64;
        loop {
            let wait = {
                let mut g = self.buckets.lock().await;
                let now = Instant::now();
                // Per-cookie keys pile up: drop buckets idle long enough to be full again
                // (a missing key starts full, so this changes nothing).
                if g.len() > MAX_KEYS {
                    g.retain(|_, (_, last)| now.duration_since(*last) < Duration::from_secs(60));
                }
                let (tokens, last) = g.entry(key.to_string()).or_insert((cap, now));
                *tokens = (*tokens + now.duration_since(*last).as_secs_f64() * cap / 60.0).min(cap);
                *last = now;
                if *tokens >= 1.0 {
                    *tokens -= 1.0;
                    return;
                }
                Duration::from_secs_f64((1.0 - *tokens) * 60.0 / cap)
            };
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn blocks_after_burst() {
        let r = RateLimiter::default();
        let t = tokio::time::Instant::now();
        for _ in 0..60 {
            r.acquire("users", 60).await;
        }
        r.acquire("presence", 60).await; // other host unaffected
        assert!(t.elapsed() < Duration::from_millis(10));
        r.acquire("users", 60).await;
        assert!(t.elapsed() >= Duration::from_millis(900));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_keys_are_dropped() {
        let r = RateLimiter::default();
        for i in 0..=MAX_KEYS {
            r.acquire(&i.to_string(), 20).await;
        }
        tokio::time::advance(Duration::from_secs(61)).await;
        r.acquire("new", 20).await;
        assert_eq!(r.buckets.lock().await.len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn refill_is_capped_at_burst() {
        let r = RateLimiter::default();
        r.acquire("k", 1).await;
        // uncapped, ten minutes would refill ten tokens
        tokio::time::advance(Duration::from_secs(600)).await;
        let t = tokio::time::Instant::now();
        r.acquire("k", 1).await;
        assert!(t.elapsed() < Duration::from_millis(10));
        r.acquire("k", 1).await;
        assert!(t.elapsed() >= Duration::from_secs(59));
    }

    #[tokio::test(start_paused = true)]
    async fn pruning_keeps_recently_used_keys() {
        let r = RateLimiter::default();
        r.acquire("old", 20).await;
        tokio::time::advance(Duration::from_secs(10)).await;
        for i in 0..MAX_KEYS {
            r.acquire(&i.to_string(), 20).await;
        }
        tokio::time::advance(Duration::from_secs(51)).await; // "old" 61 s idle, the rest 51 s
        r.acquire("new", 20).await; // len 257 > MAX_KEYS: prune runs
        let g = r.buckets.lock().await;
        assert!(!g.contains_key("old"));
        assert!(g.contains_key("0") && g.contains_key("new"));
    }
}
