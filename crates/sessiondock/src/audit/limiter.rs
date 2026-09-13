//! Per-client token bucket. Clients are loopback peers, so the key space is
//! tiny; the map is still bounded so a spoofed connect-info can never grow it.

use std::{
    collections::HashMap,
    net::IpAddr,
    time::{Duration, Instant},
};

const MAX_CLIENTS: usize = 64;

struct Bucket {
    tokens: f64,
    updated: Instant,
}

pub struct Limiter {
    buckets: HashMap<IpAddr, Bucket>,
    capacity: f64,
    refill_per_second: f64,
}

impl Limiter {
    pub fn new(burst: u32, refill_per_second: f64) -> Self {
        Self {
            buckets: HashMap::new(),
            capacity: f64::from(burst.max(1)),
            refill_per_second: if refill_per_second.is_finite() && refill_per_second > 0.0 {
                refill_per_second
            } else {
                1.0
            },
        }
    }

    /// Takes one token for `client`; returns how long until the next token
    /// when the bucket is empty.
    pub fn take(&mut self, client: IpAddr, now: Instant) -> Option<Duration> {
        if !self.buckets.contains_key(&client) && self.buckets.len() >= MAX_CLIENTS {
            // Evict the stalest bucket; a full bucket carries no state worth keeping.
            let stalest = self
                .buckets
                .iter()
                .min_by_key(|(_, bucket)| bucket.updated)
                .map(|(ip, _)| *ip);
            if let Some(ip) = stalest {
                self.buckets.remove(&ip);
            }
        }
        let bucket = self.buckets.entry(client).or_insert(Bucket {
            tokens: self.capacity,
            updated: now,
        });
        let elapsed = now.saturating_duration_since(bucket.updated).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * self.refill_per_second).min(self.capacity);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            return None;
        }
        let missing = 1.0 - bucket.tokens;
        Some(Duration::from_secs_f64(
            (missing / self.refill_per_second).max(0.001),
        ))
    }
}
