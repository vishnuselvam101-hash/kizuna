//! In-process sliding-window rate limiter keyed by string (ADR 0008).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    window: Duration,
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl RateLimiter {
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Records a hit and returns false if `key` exceeded `limit` hits within the window.
    pub fn check(&self, key: &str, limit: u32) -> bool {
        self.check_at(key, limit, Instant::now())
    }

    pub fn check_at(&self, key: &str, limit: u32, now: Instant) -> bool {
        let mut map = self.hits.lock().unwrap_or_else(|p| p.into_inner());
        if map.len() > 100_000 {
            let w = self.window;
            map.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < w));
        }
        let q = map.entry(key.to_owned()).or_default();
        while q
            .front()
            .is_some_and(|t| now.duration_since(*t) >= self.window)
        {
            q.pop_front();
        }
        if q.len() >= limit as usize {
            return false;
        }
        q.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_per_key_and_recovers_after_window() {
        let rl = RateLimiter::new(Duration::from_secs(60));
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(rl.check_at("a", 3, t0));
        }
        assert!(!rl.check_at("a", 3, t0));
        assert!(rl.check_at("b", 3, t0));
        assert!(rl.check_at("a", 3, t0 + Duration::from_secs(61)));
    }
}
