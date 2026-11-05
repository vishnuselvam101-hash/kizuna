//! Injectable wall clock. Expiry decisions (sessions, refresh tokens, lockout, TOTP, WebAuthn challenges) read time
//! from `AppState::clock` and pass it to SQL as a parameter, so tests can move time without sleeping.

use std::sync::atomic::{AtomicI64, Ordering};

use time::{Duration, OffsetDateTime};

pub trait Clock: Send + Sync {
    fn now(&self) -> OffsetDateTime;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

/// System time plus a manually advanced offset (tests; the server binary always uses `SystemClock`).
#[derive(Default)]
pub struct OffsetClock(AtomicI64);

impl OffsetClock {
    pub fn advance(&self, d: Duration) {
        self.0.fetch_add(d.whole_seconds(), Ordering::SeqCst);
    }
}

impl Clock for OffsetClock {
    fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc() + Duration::seconds(self.0.load(Ordering::SeqCst))
    }
}
