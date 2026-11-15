//! A small in-memory rate limiter: one token bucket per key, refilled continuously.
//!
//! It lives in the server process, which is right for a single-node deployment. Keys are
//! short strings such as `auth:<address>` or `write:<user id>`.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

/// Buckets kept before stale ones are swept.
const SWEEP_AT: usize = 50_000;

#[derive(Debug, Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Limiter {
    /// Take one token from `key`'s bucket, which holds `per_minute` tokens and refills at
    /// `per_minute` per minute. Returns false when it is empty.
    pub fn allow(&self, key: &str, per_minute: u32) -> bool {
        if per_minute == 0 {
            return false;
        }
        let capacity = f64::from(per_minute);
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() > SWEEP_AT {
            buckets.retain(|_, (tokens, at)| {
                *tokens + now.duration_since(*at).as_secs_f64() * capacity / 60.0 < capacity
            });
        }
        let (tokens, at) = buckets.entry(key.to_owned()).or_insert((capacity, now));
        let refill = now.duration_since(*at).as_secs_f64() * capacity / 60.0;
        *tokens = (*tokens + refill).min(capacity);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucket_empties_and_keys_are_independent() {
        let l = Limiter::default();
        for _ in 0..3 {
            assert!(l.allow("a", 3));
        }
        assert!(!l.allow("a", 3));
        assert!(l.allow("b", 3));
        assert!(!l.allow("c", 0));
    }
}
