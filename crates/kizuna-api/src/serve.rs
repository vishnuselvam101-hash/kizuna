//! HTTP/1.1 accept loop with slow-client bounds (red-team 2026-11-05 F3).
//!
//! `axum::serve` sets no timer, so hyper never times out a connection that sends its request headers slowly or not
//! at all (slowloris), nor an idle keep-alive connection: each one held a socket and a task indefinitely. Here every
//! connection gets a header-read deadline (which also bounds keep-alive idling between requests), and the number of
//! concurrent connections is capped; the accept loop waits for a free slot instead of accepting without limit.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use axum::http::Request;
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tower::ServiceExt;

#[derive(Debug, Clone, Copy)]
pub struct ServeLimits {
    /// Time a client has to send a complete request header block (also the keep-alive idle limit).
    pub header_read_timeout: Duration,
    /// Concurrent connections; further clients wait in the listen backlog.
    pub max_connections: usize,
    /// How long shutdown waits for in-flight requests.
    pub shutdown_grace: Duration,
}

impl Default for ServeLimits {
    fn default() -> Self {
        Self {
            header_read_timeout: Duration::from_secs(15),
            max_connections: 1024,
            shutdown_grace: Duration::from_secs(30),
        }
    }
}

impl ServeLimits {
    /// `KIZUNA_HEADER_READ_TIMEOUT_SECS` (2..=120, default 15) and `KIZUNA_MAX_CONNECTIONS` (16..=65536, default 1024).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let num = |k: &str, d: u64, lo: u64, hi: u64| -> Result<u64, String> {
            match get(k) {
                None => Ok(d),
                Some(v) => v
                    .parse::<u64>()
                    .ok()
                    .filter(|n| (lo..=hi).contains(n))
                    .ok_or_else(|| format!("{k} must be {lo}..={hi}")),
            }
        };
        let d = Self::default();
        Ok(Self {
            header_read_timeout: Duration::from_secs(num("KIZUNA_HEADER_READ_TIMEOUT_SECS", 15, 2, 120)?),
            max_connections: num("KIZUNA_MAX_CONNECTIONS", 1024, 16, 65_536)? as usize,
            shutdown_grace: d.shutdown_grace,
        })
    }
}

/// Serves `app` on `listener` until `shutdown` resolves, then drains in-flight connections (bounded by
/// `shutdown_grace`). Each request carries `ConnectInfo<SocketAddr>` like `into_make_service_with_connect_info`.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    limits: ServeLimits,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let slots = Arc::new(tokio::sync::Semaphore::new(limits.max_connections));
    let graceful = GracefulShutdown::new();
    let mut http = hyper::server::conn::http1::Builder::new();
    http.timer(TokioTimer::new())
        .header_read_timeout(limits.header_read_timeout);
    tokio::pin!(shutdown);
    loop {
        let slot = tokio::select! {
            s = Arc::clone(&slots).acquire_owned() => match s {
                Ok(s) => s,
                Err(_) => break,
            },
            () = &mut shutdown => break,
        };
        let (stream, peer) = tokio::select! {
            r = listener.accept() => match r {
                Ok(x) => x,
                Err(e) => {
                    // EMFILE and friends: back off instead of spinning.
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let _ = stream.set_nodelay(true);
        let svc = app.clone().map_request(move |mut req: Request<Incoming>| {
            req.extensions_mut().insert(ConnectInfo::<SocketAddr>(peer));
            req
        });
        let conn = graceful.watch(http.serve_connection(TokioIo::new(stream), TowerToHyperService::new(svc)));
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(error = %e, %peer, "connection ended with error");
            }
            drop(slot);
        });
    }
    drop(listener);
    if tokio::time::timeout(limits.shutdown_grace, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("shutdown grace period elapsed with connections still open");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_bounded() {
        let get = |m: &'static [(&'static str, &'static str)]| {
            move |k: &str| m.iter().find(|(a, _)| *a == k).map(|(_, v)| (*v).to_string())
        };
        let l = ServeLimits::from_lookup(get(&[])).unwrap();
        assert_eq!(l.header_read_timeout, Duration::from_secs(15));
        assert_eq!(l.max_connections, 1024);
        assert!(ServeLimits::from_lookup(get(&[("KIZUNA_HEADER_READ_TIMEOUT_SECS", "0")])).is_err());
        assert!(ServeLimits::from_lookup(get(&[("KIZUNA_MAX_CONNECTIONS", "1")])).is_err());
        let l = ServeLimits::from_lookup(get(&[("KIZUNA_HEADER_READ_TIMEOUT_SECS", "5")])).unwrap();
        assert_eq!(l.header_read_timeout, Duration::from_secs(5));
    }
}
