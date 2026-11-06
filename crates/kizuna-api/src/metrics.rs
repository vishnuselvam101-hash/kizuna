//! Prometheus text metrics (served on a separate loopback-only port), the per-request middleware that classifies,
//! admits and times every request, and the readiness probe (STABILITY_2026-11-06).
//!
//! Labels are bounded: route class (9), method (6), status class (5); no paths, tenants or users.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use serde_json::json;

use crate::admission::{self, RouteClass, RouteCtx};
use crate::app::AppState;

/// Latency histogram upper bounds in seconds.
pub const BUCKETS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 20.0, 30.0, 60.0,
];
const METHODS: [&str; 6] = ["GET", "POST", "PUT", "PATCH", "DELETE", "OTHER"];
const NCLASS: usize = RouteClass::ALL.len();

#[derive(Default)]
struct Histogram {
    buckets: [AtomicU64; BUCKETS.len()],
    count: AtomicU64,
    sum_micros: AtomicU64,
}

impl Histogram {
    fn observe(&self, d: Duration) {
        let s = d.as_secs_f64();
        for (i, b) in BUCKETS.iter().enumerate() {
            if s <= *b {
                self.buckets[i].fetch_add(1, Ordering::Relaxed);
            }
        }
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum_micros.fetch_add(d.as_micros() as u64, Ordering::Relaxed);
    }
}

pub struct Metrics {
    started: Instant,
    /// [class][method][status class 1xx..5xx]
    requests: Vec<AtomicU64>,
    latency: Vec<Histogram>,
    in_flight: AtomicI64,
    pub idem_replayed: AtomicU64,
    pub idem_mismatch: AtomicU64,
    pub idem_in_progress: AtomicU64,
    pub idem_stored: AtomicU64,
    /// (kind, outcome) -> count; outcomes ok / retry / quarantined / lease_lost.
    jobs: Mutex<BTreeMap<(String, &'static str), u64>>,
    job_latency: Histogram,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics {
            started: Instant::now(),
            requests: (0..NCLASS * METHODS.len() * 5).map(|_| AtomicU64::new(0)).collect(),
            latency: (0..NCLASS).map(|_| Histogram::default()).collect(),
            in_flight: AtomicI64::new(0),
            idem_replayed: AtomicU64::new(0),
            idem_mismatch: AtomicU64::new(0),
            idem_in_progress: AtomicU64::new(0),
            idem_stored: AtomicU64::new(0),
            jobs: Mutex::new(BTreeMap::new()),
            job_latency: Histogram::default(),
        }
    }
}

fn method_index(m: &Method) -> usize {
    METHODS.iter().position(|x| *x == m.as_str()).unwrap_or(5)
}

impl Metrics {
    pub fn observe_request(&self, class: RouteClass, method: &Method, status: StatusCode, d: Duration) {
        let sc = (status.as_u16() / 100).clamp(1, 5) as usize - 1;
        let i = (class.index() * METHODS.len() + method_index(method)) * 5 + sc;
        self.requests[i].fetch_add(1, Ordering::Relaxed);
        self.latency[class.index()].observe(d);
    }

    pub fn observe_job(&self, kind: &str, outcome: &'static str, d: Duration) {
        let mut g = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
        // Kinds are fixed in code; cap the map anyway so a bad row cannot grow it without bound.
        if g.len() < 256 || g.contains_key(&(kind.to_string(), outcome)) {
            *g.entry((kind.to_string(), outcome)).or_default() += 1;
        }
        drop(g);
        self.job_latency.observe(d);
    }

    pub fn requests_total(&self) -> u64 {
        self.requests.iter().map(|a| a.load(Ordering::Relaxed)).sum()
    }
}

fn histogram(out: &mut String, name: &str, labels: &str, h: &Histogram) {
    let sep = if labels.is_empty() { "" } else { "," };
    for (i, b) in BUCKETS.iter().enumerate() {
        let _ = writeln!(
            out,
            "{name}_bucket{{{labels}{sep}le=\"{b}\"}} {}",
            h.buckets[i].load(Ordering::Relaxed)
        );
    }
    let n = h.count.load(Ordering::Relaxed);
    let _ = writeln!(out, "{name}_bucket{{{labels}{sep}le=\"+Inf\"}} {n}");
    let lb = if labels.is_empty() {
        String::new()
    } else {
        format!("{{{labels}}}")
    };
    let _ = writeln!(
        out,
        "{name}_sum{lb} {}",
        h.sum_micros.load(Ordering::Relaxed) as f64 / 1e6
    );
    let _ = writeln!(out, "{name}_count{lb} {n}");
}

/// (resident bytes, open fds) of this process, from /proc.
pub fn process_stats() -> (u64, u64) {
    let page = 4096u64;
    let rss = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()))
        .map_or(0, |p| p * page);
    let fds = std::fs::read_dir("/proc/self/fd").map_or(0, |d| d.count() as u64);
    (rss, fds)
}

/// Renders every metric. `job_pool` is the background-job pool, when the server runs one.
pub async fn render(st: &AppState, job_pool: Option<&sqlx::PgPool>) -> String {
    let m = &st.metrics;
    let mut o = String::with_capacity(16 * 1024);
    o.push_str("# HELP kizuna_http_requests_total HTTP requests by route class, method and status class.\n");
    o.push_str("# TYPE kizuna_http_requests_total counter\n");
    for c in RouteClass::ALL {
        for (mi, meth) in METHODS.iter().enumerate() {
            for sc in 0..5 {
                let v = m.requests[(c.index() * METHODS.len() + mi) * 5 + sc].load(Ordering::Relaxed);
                if v > 0 {
                    let _ = writeln!(
                        o,
                        "kizuna_http_requests_total{{class=\"{}\",method=\"{meth}\",status=\"{}xx\"}} {v}",
                        c.name(),
                        sc + 1
                    );
                }
            }
        }
    }
    o.push_str("# HELP kizuna_http_request_duration_seconds Request latency by route class (incl. admission wait).\n");
    o.push_str("# TYPE kizuna_http_request_duration_seconds histogram\n");
    for c in RouteClass::ALL {
        if c != RouteClass::Job {
            histogram(
                &mut o,
                "kizuna_http_request_duration_seconds",
                &format!("class=\"{}\"", c.name()),
                &m.latency[c.index()],
            );
        }
    }
    let _ = writeln!(
        o,
        "# TYPE kizuna_http_in_flight gauge\nkizuna_http_in_flight {}",
        m.in_flight.load(Ordering::Relaxed)
    );
    o.push_str("# HELP kizuna_admission_in_use Admission gate permits in use.\n# TYPE kizuna_admission_in_use gauge\n");
    for g in st.gates.all() {
        let _ = writeln!(o, "kizuna_admission_in_use{{gate=\"{}\"}} {}", g.name, g.in_use());
    }
    o.push_str("# TYPE kizuna_admission_waiting gauge\n");
    for g in st.gates.all() {
        let _ = writeln!(o, "kizuna_admission_waiting{{gate=\"{}\"}} {}", g.name, g.waiting());
    }
    o.push_str("# TYPE kizuna_admission_permits gauge\n");
    for g in st.gates.all() {
        let _ = writeln!(
            o,
            "kizuna_admission_permits{{gate=\"{}\"}} {}",
            g.name, g.limits.permits
        );
    }
    o.push_str("# HELP kizuna_admission_rejected_total Requests answered 429 server_busy.\n");
    o.push_str("# TYPE kizuna_admission_rejected_total counter\n");
    for g in st.gates.all() {
        let _ = writeln!(
            o,
            "kizuna_admission_rejected_total{{gate=\"{}\"}} {}",
            g.name,
            g.rejected.load(Ordering::Relaxed)
        );
    }
    o.push_str(
        "# HELP kizuna_db_pool_connections Pool connections by state.\n# TYPE kizuna_db_pool_connections gauge\n",
    );
    let mut pools = vec![("request", &st.pool)];
    if let Some(j) = job_pool {
        pools.push(("job", j));
    }
    for (name, p) in &pools {
        let (size, idle) = (p.size() as u64, p.num_idle() as u64);
        let _ = writeln!(o, "kizuna_db_pool_connections{{pool=\"{name}\",state=\"idle\"}} {idle}");
        let _ = writeln!(
            o,
            "kizuna_db_pool_connections{{pool=\"{name}\",state=\"in_use\"}} {}",
            size.saturating_sub(idle)
        );
        let _ = writeln!(
            o,
            "kizuna_db_pool_max_connections{{pool=\"{name}\"}} {}",
            p.options().get_max_connections()
        );
    }
    o.push_str("# HELP kizuna_jobs_queue_depth Background jobs not yet done, by state.\n");
    o.push_str("# TYPE kizuna_jobs_queue_depth gauge\n");
    let depth = crate::jobs::depth(job_pool.unwrap_or(&st.pool)).await;
    match depth {
        Ok(rows) => {
            for s in ["queued", "running", "quarantined"] {
                let n = rows.iter().find(|r| r.0 == s).map_or(0, |r| r.1);
                let _ = writeln!(o, "kizuna_jobs_queue_depth{{state=\"{s}\"}} {n}");
            }
            let _ = writeln!(o, "kizuna_jobs_depth_scrape_ok 1");
        }
        Err(e) => {
            tracing::warn!(error = %e, "job depth query failed");
            let _ = writeln!(o, "kizuna_jobs_depth_scrape_ok 0");
        }
    }
    o.push_str("# TYPE kizuna_job_runs_total counter\n");
    {
        let g = m.jobs.lock().unwrap_or_else(|e| e.into_inner());
        for ((kind, outcome), v) in g.iter() {
            let _ = writeln!(o, "kizuna_job_runs_total{{kind=\"{kind}\",outcome=\"{outcome}\"}} {v}");
        }
    }
    o.push_str("# TYPE kizuna_job_duration_seconds histogram\n");
    histogram(&mut o, "kizuna_job_duration_seconds", "", &m.job_latency);
    o.push_str("# TYPE kizuna_idempotency_total counter\n");
    for (k, a) in [
        ("stored", &m.idem_stored),
        ("replayed", &m.idem_replayed),
        ("mismatch", &m.idem_mismatch),
        ("in_progress", &m.idem_in_progress),
    ] {
        let _ = writeln!(
            o,
            "kizuna_idempotency_total{{outcome=\"{k}\"}} {}",
            a.load(Ordering::Relaxed)
        );
    }
    let (rss, fds) = process_stats();
    let _ = writeln!(
        o,
        "# TYPE process_resident_memory_bytes gauge\nprocess_resident_memory_bytes {rss}"
    );
    let _ = writeln!(o, "# TYPE process_open_fds gauge\nprocess_open_fds {fds}");
    let _ = writeln!(
        o,
        "# TYPE kizuna_uptime_seconds gauge\nkizuna_uptime_seconds {}",
        m.started.elapsed().as_secs()
    );
    let _ = writeln!(
        o,
        "# TYPE kizuna_draining gauge\nkizuna_draining {}",
        u8::from(st.draining.load(Ordering::Relaxed))
    );
    o
}

/// Outermost request middleware: classify, admit (bounded queue → 429), scope the statement timeout, time.
pub async fn http_layer(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let class = admission::classify(req.method(), req.uri().path());
    let method = req.method().clone();
    let t0 = Instant::now();
    st.metrics.in_flight.fetch_add(1, Ordering::Relaxed);
    let resp = async {
        let _permit = match st.gates.for_class(class) {
            Some(g) => match g.enter().await {
                Ok(p) => Some(p),
                Err(e) => return e.into_response(),
            },
            None => None,
        };
        let ctx = RouteCtx {
            class,
            statement_timeout_ms: st.cfg.stability.statement_timeout(class),
        };
        admission::scope(ctx, next.run(req)).await
    }
    .await;
    st.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
    st.metrics.observe_request(class, &method, resp.status(), t0.elapsed());
    resp
}

/// GET /readyz: ready to take traffic (not draining, database answers within 2 s, object store present).
/// `/healthz` stays a pure liveness probe.
pub async fn readyz(State(st): State<AppState>) -> Response {
    let draining = st.draining.load(Ordering::Relaxed);
    let db_ok = matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&st.pool)
        )
        .await,
        Ok(Ok(1))
    );
    // The store directory is created on first write; creating it here is what that write would do.
    let store_ok = std::fs::create_dir_all(&st.cfg.storage_dir).is_ok() && st.cfg.storage_dir.is_dir();
    let ok = !draining && db_ok && store_ok;
    let body = json!({
        "status": if ok { "ready" } else { "not_ready" },
        "checks": { "draining": draining, "database": db_ok, "storage": store_ok },
    });
    let code = if ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(body)).into_response()
}

/// `KIZUNA_METRICS_BIND`: default `127.0.0.1:9464`; `off` disables. Only loopback addresses are accepted: the
/// endpoint is unauthenticated and meant for a local scraper or an SSH tunnel.
pub fn metrics_bind(get: impl Fn(&str) -> Option<String>) -> Result<Option<SocketAddr>, String> {
    match get("KIZUNA_METRICS_BIND").as_deref().map(str::trim) {
        Some("off") => Ok(None),
        None | Some("") => Ok(Some(SocketAddr::from(([127, 0, 0, 1], 9464)))),
        Some(v) => {
            let a: SocketAddr = v
                .parse()
                .map_err(|_| "KIZUNA_METRICS_BIND must be ip:port or off".to_string())?;
            if !a.ip().is_loopback() {
                return Err("KIZUNA_METRICS_BIND must be a loopback address".into());
            }
            Ok(Some(a))
        }
    }
}

/// The metrics router (`GET /metrics`), for the separate loopback listener.
pub fn router(st: AppState, job_pool: Option<sqlx::PgPool>) -> Router {
    Router::new().route(
        "/metrics",
        axum::routing::get(move || {
            let (st, jp) = (st.clone(), job_pool.clone());
            async move {
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        "text/plain; version=0.0.4; charset=utf-8",
                    )],
                    render(&st, jp.as_ref()).await,
                )
            }
        }),
    )
}

/// Marks the server as draining (readiness fails; in-flight requests finish).
pub fn set_draining(flag: &AtomicBool) {
    flag.store(true, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_bind_is_loopback_only() {
        let get = |v: Option<&'static str>| move |_: &str| v.map(String::from);
        assert_eq!(
            metrics_bind(get(None)).unwrap(),
            Some(SocketAddr::from(([127, 0, 0, 1], 9464)))
        );
        assert_eq!(metrics_bind(get(Some("off"))).unwrap(), None);
        assert!(metrics_bind(get(Some("[::1]:9000"))).unwrap().is_some());
        assert!(metrics_bind(get(Some("0.0.0.0:9464"))).is_err());
        assert!(metrics_bind(get(Some("10.0.0.1:9464"))).is_err());
        assert!(metrics_bind(get(Some("nonsense"))).is_err());
    }

    #[test]
    fn histogram_is_cumulative() {
        let h = Histogram::default();
        h.observe(Duration::from_millis(3));
        h.observe(Duration::from_millis(70));
        h.observe(Duration::from_secs(100));
        let mut o = String::new();
        histogram(&mut o, "x", "class=\"read\"", &h);
        assert!(o.contains("x_bucket{class=\"read\",le=\"0.005\"} 1\n"));
        assert!(o.contains("x_bucket{class=\"read\",le=\"0.1\"} 2\n"));
        assert!(o.contains("x_bucket{class=\"read\",le=\"60\"} 2\n"));
        assert!(o.contains("x_bucket{class=\"read\",le=\"+Inf\"} 3\n"));
        assert!(o.contains("x_count{class=\"read\"} 3\n"));
    }
}
