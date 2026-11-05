//! Sends one host-level operator alert (ADR 0017), for systemd `OnFailure=` hooks and the integrity sweep.
//!
//! kizuna-alert <backup_failed|integrity_failure|unit_failed|test> <source-unit>
//!
//! Channels and limits come from the same `KIZUNA_ALERT_*` environment as the server. Rate-limit state persists in
//! `KIZUNA_ALERT_STATE_FILE` (default /var/lib/kizuna/alerts/state.json when that directory exists).
//! Exit 0 when delivered, disabled or rate-limited; 1 when every configured channel failed; 64 on usage errors.

use kizuna_api::alerts::{AlertConfig, Alerter, Delivery, host_event};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(kind), Some(source)) = (args.first(), args.get(1)) else {
        eprintln!("usage: kizuna-alert <backup_failed|integrity_failure|unit_failed|test> <source>");
        std::process::exit(64);
    };
    let cfg = match AlertConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("alert configuration invalid: {e}");
            std::process::exit(1);
        }
    };
    let state = std::env::var_os("KIZUNA_ALERT_STATE_FILE").map(Into::into).or_else(|| {
        let d = std::path::Path::new("/var/lib/kizuna/alerts");
        d.is_dir().then(|| d.join("state.json"))
    });
    let Some(alert) = host_event(&cfg.label, kind, source) else {
        eprintln!("unknown alert kind");
        std::process::exit(64);
    };
    match Alerter::new(cfg, state).send(&alert) {
        Delivery::Disabled => println!("alerts disabled (no channel configured)"),
        Delivery::Suppressed => println!("rate-limited (already alerted recently)"),
        Delivery::Sent { ok, failed } => {
            println!("delivered: {ok:?}; failed: {failed:?}");
            if ok.is_empty() {
                std::process::exit(1);
            }
        }
    }
}
