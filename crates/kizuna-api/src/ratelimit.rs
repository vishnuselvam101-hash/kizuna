//! In-process sliding-window rate limiter keyed by string (ADR 0008).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Maximum number of tracked keys. Beyond it, unknown keys are refused (fail closed) until pruning frees space,
/// so attacker-chosen keys cannot grow memory or make each check O(n).
pub const MAX_KEYS: usize = 100_000;

struct Inner {
    map: HashMap<String, VecDeque<Instant>>,
    last_prune: Option<Instant>,
}

pub struct RateLimiter {
    window: Duration,
    cap: usize,
    inner: Mutex<Inner>,
}

impl RateLimiter {
    pub fn new(window: Duration) -> Self {
        Self::with_cap(window, MAX_KEYS)
    }

    pub fn with_cap(window: Duration, cap: usize) -> Self {
        Self {
            window,
            cap,
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                last_prune: None,
            }),
        }
    }

    /// Records a hit and returns false if `key` exceeded `limit` hits within the window.
    pub fn check(&self, key: &str, limit: u32) -> bool {
        self.check_at(key, limit, Instant::now())
    }

    pub fn check_at(&self, key: &str, limit: u32, now: Instant) -> bool {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let w = self.window;
        if g.map.len() >= self.cap && !g.map.contains_key(key) {
            // Prune at most once per tenth of a window to keep the hot path O(1) under attack.
            if g.last_prune.is_none_or(|t| now.duration_since(t) >= w / 10) {
                g.map
                    .retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < w));
                g.last_prune = Some(now);
            }
            if g.map.len() >= self.cap {
                return false;
            }
        }
        let q = g.map.entry(key.to_owned()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
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

    #[test]
    fn key_flood_is_bounded_and_fails_closed() {
        let rl = RateLimiter::with_cap(Duration::from_secs(60), 1000);
        let t0 = Instant::now();
        assert!(rl.check_at("known", 5, t0));
        for i in 0..999 {
            rl.check_at(&format!("flood{i}"), 5, t0);
        }
        assert!(!rl.check_at("new-key", 5, t0), "unknown keys refused when full");
        assert!(rl.check_at("known", 5, t0), "existing keys keep working");
        let start = std::time::Instant::now();
        for i in 0..10_000 {
            rl.check_at(&format!("more{i}"), 5, t0);
        }
        assert!(start.elapsed() < Duration::from_secs(2), "no O(n) rescans per call");
        assert!(
            rl.check_at("after", 5, t0 + Duration::from_secs(61)),
            "space freed after window"
        );
    }
}
