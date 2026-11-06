//! Route classes, per-class statement timeouts and bounded admission queues (STABILITY_2026-11-06).
//!
//! Every request is classified once (method + path). The class picks the PostgreSQL `statement_timeout` applied to
//! every transaction opened through `db::*_tx` while the request runs (a task-local, so no handler signature
//! changes), and, for the expensive classes, an admission [`Gate`]: a semaphore with a bounded wait queue. A full
//! queue (or a wait past `max_wait`) answers 429 `server_busy` with Retry-After instead of letting work pile up on
//! the connection pool. The gates are also how the shared pool is partitioned: heavy work can hold at most
//! `heavy.permits` connections, so logins and reads keep getting connections while reports run.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::http::Method;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::config::ConfigError;
use crate::error::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteClass {
    /// Login, refresh, MFA ceremonies, portal login/invite.
    Auth,
    Read,
    Write,
    /// Whole-tenant work: verification run, reports, evidence packs, audit verification, simulations, detection.
    Heavy,
    Upload,
    /// Sandboxed extraction (`/artifacts/{id}/extract`, `/reextract`).
    Extract,
    /// Supplier portal (non-auth).
    Portal,
    /// Health, readiness and anything outside `/api/v1`.
    Ops,
    /// Background jobs (not an HTTP class; used for their statement timeout).
    Job,
}

impl RouteClass {
    pub const ALL: [RouteClass; 9] = [
        RouteClass::Auth,
        RouteClass::Read,
        RouteClass::Write,
        RouteClass::Heavy,
        RouteClass::Upload,
        RouteClass::Extract,
        RouteClass::Portal,
        RouteClass::Ops,
        RouteClass::Job,
    ];

    pub fn name(self) -> &'static str {
        match self {
            RouteClass::Auth => "auth",
            RouteClass::Read => "read",
            RouteClass::Write => "write",
            RouteClass::Heavy => "heavy",
            RouteClass::Upload => "upload",
            RouteClass::Extract => "extract",
            RouteClass::Portal => "portal",
            RouteClass::Ops => "ops",
            RouteClass::Job => "job",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == s)
    }
}

/// Classifies a request by method and full path (`/api/v1/...`).
pub fn classify(method: &Method, path: &str) -> RouteClass {
    let Some(p) = path.strip_prefix("/api/v1") else {
        return RouteClass::Ops;
    };
    let seg: Vec<&str> = p.trim_start_matches('/').split('/').collect();
    let post = method == Method::POST;
    let get = method == Method::GET;
    match seg.as_slice() {
        ["auth", ..] | ["portal", "login" | "logout"] | ["portal", "invite", ..] => RouteClass::Auth,
        ["artifacts"] if post => RouteClass::Upload,
        ["portal", "requests", _, "uploads"] if post => RouteClass::Upload,
        ["artifacts", _, "extract" | "reextract"] if post => RouteClass::Extract,
        ["verification", "run"]
        | ["reports"]
        | ["simulations"]
        | ["integrity", "run"]
        | ["security", "detect"]
        | ["capa", "overdue", "run"]
        | ["decisions", _, "evidence-pack"]
        | ["audit", "checkpoints", "verify-external"]
            if post =>
        {
            RouteClass::Heavy
        }
        ["audit", "verify"]
        | ["audit", "checkpoints", "verify"]
        | ["decisions", _, "report"]
        | ["dashboard", "summary"]
            if get =>
        {
            RouteClass::Heavy
        }
        ["portal", ..] => RouteClass::Portal,
        _ if get || method == Method::HEAD => RouteClass::Read,
        _ => RouteClass::Write,
    }
}

/// Bounded admission: `permits` run at once, at most `max_queue` wait (each at most `max_wait`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateLimits {
    pub permits: usize,
    pub max_queue: usize,
    pub max_wait: Duration,
}

pub struct Gate {
    pub name: &'static str,
    pub limits: GateLimits,
    sem: Arc<Semaphore>,
    waiting: AtomicUsize,
    pub rejected: AtomicU64,
    pub admitted: AtomicU64,
}

/// Decrements the waiting count however the wait ends (permit, timeout, or the request being dropped).
struct Waiting<'a>(&'a AtomicUsize);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Gate {
    pub fn new(name: &'static str, limits: GateLimits) -> Self {
        Gate {
            name,
            limits,
            sem: Arc::new(Semaphore::new(limits.permits)),
            waiting: AtomicUsize::new(0),
            rejected: AtomicU64::new(0),
            admitted: AtomicU64::new(0),
        }
    }

    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }

    pub fn in_use(&self) -> usize {
        self.limits.permits - self.sem.available_permits()
    }

    fn busy(&self) -> ApiError {
        self.rejected.fetch_add(1, Ordering::Relaxed);
        // Roughly how long a queue slot takes to free up; at least one second.
        ApiError::Busy(self.name, (self.limits.max_wait.as_secs() / 4).clamp(1, 30))
    }

    pub async fn enter(&self) -> Result<OwnedSemaphorePermit, ApiError> {
        if let Ok(p) = Arc::clone(&self.sem).try_acquire_owned() {
            self.admitted.fetch_add(1, Ordering::Relaxed);
            return Ok(p);
        }
        if self.waiting.fetch_add(1, Ordering::SeqCst) >= self.limits.max_queue {
            self.waiting.fetch_sub(1, Ordering::SeqCst);
            return Err(self.busy());
        }
        let guard = Waiting(&self.waiting);
        let r = tokio::time::timeout(self.limits.max_wait, Arc::clone(&self.sem).acquire_owned()).await;
        drop(guard);
        match r {
            Ok(Ok(p)) => {
                self.admitted.fetch_add(1, Ordering::Relaxed);
                Ok(p)
            }
            _ => Err(self.busy()),
        }
    }
}

pub struct Gates {
    pub upload: Gate,
    pub extract: Gate,
    pub heavy: Gate,
}

impl Gates {
    pub fn new(c: &StabilityConfig) -> Self {
        Gates {
            upload: Gate::new("upload", c.upload_gate),
            extract: Gate::new("extract", c.extract_gate),
            heavy: Gate::new("heavy", c.heavy_gate),
        }
    }

    pub fn for_class(&self, c: RouteClass) -> Option<&Gate> {
        match c {
            RouteClass::Upload => Some(&self.upload),
            RouteClass::Extract => Some(&self.extract),
            RouteClass::Heavy => Some(&self.heavy),
            _ => None,
        }
    }

    pub fn all(&self) -> [&Gate; 3] {
        [&self.upload, &self.extract, &self.heavy]
    }
}

/// Pool sizing, statement timeouts, admission limits and job workers.
#[derive(Debug, Clone)]
pub struct StabilityConfig {
    /// Request pool size (`KIZUNA_DB_POOL_SIZE`, 4..=256, default 16).
    pub db_pool_size: u32,
    /// Separate pool for background jobs (`KIZUNA_DB_JOB_POOL_SIZE`, 2..=64, default 6; workers, scheduler and sweeps): jobs never take request
    /// connections.
    pub job_pool_size: u32,
    /// Concurrent job workers (`KIZUNA_JOB_WORKERS`, 1..=16, default 2).
    pub job_workers: usize,
    /// Per-class statement timeouts in ms, indexed by `RouteClass::index` (`KIZUNA_STATEMENT_TIMEOUTS`, e.g.
    /// `read=15000,heavy=55000`; each 100..=600000).
    pub statement_timeout_ms: [u64; 9],
    pub upload_gate: GateLimits,
    pub extract_gate: GateLimits,
    pub heavy_gate: GateLimits,
}

impl Default for StabilityConfig {
    fn default() -> Self {
        let mut t = [0u64; 9];
        for c in RouteClass::ALL {
            t[c.index()] = match c {
                RouteClass::Auth => 5_000,
                RouteClass::Read | RouteClass::Write => 15_000,
                // Below the 60 s request timeout, above the measured worst verification run (22 s at 68 k claims).
                RouteClass::Heavy => 55_000,
                RouteClass::Upload | RouteClass::Extract => 30_000,
                RouteClass::Portal => 10_000,
                RouteClass::Ops => 2_000,
                RouteClass::Job => 120_000,
            };
        }
        let g = |permits, max_queue, secs| GateLimits {
            permits,
            max_queue,
            max_wait: Duration::from_secs(secs),
        };
        StabilityConfig {
            db_pool_size: 16,
            job_pool_size: 6,
            job_workers: 2,
            statement_timeout_ms: t,
            upload_gate: g(8, 16, 15),
            // Matches the two sandboxed extraction workers: queued extractions wait here, bounded.
            extract_gate: g(2, 6, 20),
            heavy_gate: g(3, 6, 20),
        }
    }
}

impl StabilityConfig {
    pub fn statement_timeout(&self, c: RouteClass) -> u64 {
        self.statement_timeout_ms[c.index()]
    }

    pub fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let mut c = StabilityConfig::default();
        let num = |k: &'static str, d: u64, lo: u64, hi: u64| -> Result<u64, ConfigError> {
            match get(k) {
                None => Ok(d),
                Some(v) => v
                    .parse::<u64>()
                    .ok()
                    .filter(|n| (lo..=hi).contains(n))
                    .ok_or(ConfigError::Invalid(k)),
            }
        };
        c.db_pool_size = num("KIZUNA_DB_POOL_SIZE", 16, 4, 256)? as u32;
        c.job_pool_size = num("KIZUNA_DB_JOB_POOL_SIZE", 6, 2, 64)? as u32;
        c.job_workers = num("KIZUNA_JOB_WORKERS", 2, 1, 16)? as usize;
        if let Some(v) = get("KIZUNA_STATEMENT_TIMEOUTS").filter(|v| !v.trim().is_empty()) {
            for part in v.split(',') {
                let (k, ms) = part
                    .trim()
                    .split_once('=')
                    .ok_or(ConfigError::Invalid("KIZUNA_STATEMENT_TIMEOUTS"))?;
                let class = RouteClass::parse(k.trim()).ok_or(ConfigError::Invalid("KIZUNA_STATEMENT_TIMEOUTS"))?;
                let ms: u64 = ms
                    .trim()
                    .parse()
                    .ok()
                    .filter(|n| (100..=600_000).contains(n))
                    .ok_or(ConfigError::Invalid("KIZUNA_STATEMENT_TIMEOUTS"))?;
                c.statement_timeout_ms[class.index()] = ms;
            }
        }
        let gate = |k: &'static str, d: GateLimits| -> Result<GateLimits, ConfigError> {
            let Some(v) = get(k).filter(|v| !v.trim().is_empty()) else {
                return Ok(d);
            };
            let n: Vec<u64> = v
                .split(',')
                .map(|x| x.trim().parse::<u64>())
                .collect::<Result<_, _>>()
                .map_err(|_| ConfigError::Invalid(k))?;
            match n.as_slice() {
                [p, q, w] if (1..=1024).contains(p) && *q <= 4096 && (1..=120).contains(w) => Ok(GateLimits {
                    permits: *p as usize,
                    max_queue: *q as usize,
                    max_wait: Duration::from_secs(*w),
                }),
                _ => Err(ConfigError::Invalid(k)),
            }
        };
        c.upload_gate = gate("KIZUNA_GATE_UPLOAD", c.upload_gate)?;
        c.extract_gate = gate("KIZUNA_GATE_EXTRACT", c.extract_gate)?;
        c.heavy_gate = gate("KIZUNA_GATE_HEAVY", c.heavy_gate)?;
        // Heavy work must leave at least half of the request pool to everything else.
        if c.heavy_gate.permits as u32 * 2 > c.db_pool_size {
            return Err(ConfigError::Invalid("KIZUNA_GATE_HEAVY"));
        }
        Ok(c)
    }
}

/// Request (or job) context visible to the database layer.
#[derive(Debug, Clone, Copy)]
pub struct RouteCtx {
    pub class: RouteClass,
    pub statement_timeout_ms: u64,
}

tokio::task_local! {
    static ROUTE: RouteCtx;
}

/// Runs `f` with `ctx` as the current route context.
pub async fn scope<F: std::future::Future>(ctx: RouteCtx, f: F) -> F::Output {
    ROUTE.scope(ctx, f).await
}

/// The statement timeout of the current request or job, if any (none outside a scope: e.g. admin CLI, tests).
pub fn current_statement_timeout_ms() -> Option<u64> {
    ROUTE.try_with(|r| r.statement_timeout_ms).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        let c = |m: Method, p: &str| classify(&m, p);
        assert_eq!(c(Method::POST, "/api/v1/auth/login"), RouteClass::Auth);
        assert_eq!(c(Method::POST, "/api/v1/portal/login"), RouteClass::Auth);
        assert_eq!(c(Method::POST, "/api/v1/artifacts"), RouteClass::Upload);
        assert_eq!(c(Method::GET, "/api/v1/artifacts"), RouteClass::Read);
        assert_eq!(c(Method::POST, "/api/v1/portal/requests/x/uploads"), RouteClass::Upload);
        assert_eq!(c(Method::GET, "/api/v1/portal/requests/x/uploads"), RouteClass::Portal);
        assert_eq!(c(Method::POST, "/api/v1/artifacts/x/extract"), RouteClass::Extract);
        assert_eq!(c(Method::POST, "/api/v1/verification/run"), RouteClass::Heavy);
        assert_eq!(c(Method::GET, "/api/v1/decisions/x/report"), RouteClass::Heavy);
        assert_eq!(c(Method::POST, "/api/v1/claims"), RouteClass::Write);
        assert_eq!(c(Method::PUT, "/api/v1/capa/cases/x/steps/1"), RouteClass::Write);
        assert_eq!(c(Method::GET, "/healthz"), RouteClass::Ops);
    }

    #[test]
    fn config_parses_and_bounds() {
        let get = |m: &'static [(&'static str, &'static str)]| {
            move |k: &str| m.iter().find(|(a, _)| *a == k).map(|(_, v)| (*v).to_string())
        };
        let d = StabilityConfig::from_lookup(&get(&[])).unwrap();
        assert_eq!(d.statement_timeout(RouteClass::Heavy), 55_000);
        let c = StabilityConfig::from_lookup(&get(&[
            ("KIZUNA_STATEMENT_TIMEOUTS", "read=1000, heavy=9000"),
            ("KIZUNA_GATE_UPLOAD", "2,3,4"),
        ]))
        .unwrap();
        assert_eq!(c.statement_timeout(RouteClass::Read), 1000);
        assert_eq!(c.statement_timeout(RouteClass::Heavy), 9000);
        assert_eq!(c.upload_gate.permits, 2);
        for bad in [
            ("KIZUNA_STATEMENT_TIMEOUTS", "nope=5"),
            ("KIZUNA_STATEMENT_TIMEOUTS", "read=1"),
            ("KIZUNA_GATE_UPLOAD", "0,1,1"),
            ("KIZUNA_GATE_HEAVY", "12,1,1"),
            ("KIZUNA_DB_POOL_SIZE", "1"),
        ] {
            let m: &'static [(&str, &str)] = Box::leak(Box::new([bad]));
            assert!(StabilityConfig::from_lookup(&get(m)).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn gate_bounds_queue_and_wait() {
        let g = Arc::new(Gate::new(
            "t",
            GateLimits {
                permits: 1,
                max_queue: 1,
                max_wait: Duration::from_millis(300),
            },
        ));
        let p1 = g.enter().await.unwrap();
        // One waiter is allowed...
        let g2 = Arc::clone(&g);
        let waiter = tokio::spawn(async move { g2.enter().await.map(drop) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(g.waiting(), 1);
        // ...a second one is turned away at once with Retry-After.
        let t = std::time::Instant::now();
        assert!(matches!(g.enter().await, Err(ApiError::Busy("t", _))));
        assert!(t.elapsed() < Duration::from_millis(100));
        drop(p1);
        assert!(waiter.await.unwrap().is_ok());
        assert_eq!(g.waiting(), 0);
        // A waiter past max_wait is rejected; a cancelled waiter does not leak its queue slot.
        let _p = g.enter().await.unwrap();
        assert!(g.enter().await.is_err());
        let g3 = Arc::clone(&g);
        let h = tokio::spawn(async move { g3.enter().await.map(drop) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        h.abort();
        let _ = h.await;
        assert_eq!(g.waiting(), 0);
        assert_eq!(g.rejected.load(Ordering::Relaxed), 2);
    }
}
