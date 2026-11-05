//! Operator alerts (ADR 0017): incidents (the `notify_admins` response action), failed backups and integrity
//! failures are pushed to e-mail (SMTP) and/or a Telegram bot. Disabled unless configured; rate-limited per key and
//! globally; messages carry identifiers and categories only, never secrets, document content, user e-mails or IPs.
//!
//! Env (all optional; a channel is on only when its required keys are set):
//! - SMTP: `KIZUNA_ALERT_SMTP_HOST`, `KIZUNA_ALERT_SMTP_PORT` (587/465), `KIZUNA_ALERT_SMTP_TLS`
//!   (`starttls` default | `tls` | `none` — `none` only for localhost), `KIZUNA_ALERT_SMTP_USER`,
//!   `KIZUNA_ALERT_SMTP_PASSWORD`, `KIZUNA_ALERT_EMAIL_FROM`, `KIZUNA_ALERT_EMAIL_TO` (comma-separated).
//! - Telegram: `KIZUNA_ALERT_TELEGRAM_BOT_TOKEN`, `KIZUNA_ALERT_TELEGRAM_CHAT_ID`,
//!   `KIZUNA_ALERT_TELEGRAM_API_BASE` (default https://api.telegram.org; plain http only for localhost).
//! - `KIZUNA_ALERT_MIN_INTERVAL_SECS` (same key, default 900), `KIZUNA_ALERT_MAX_PER_HOUR` (default 20),
//!   `KIZUNA_ALERT_LABEL` (instance name in messages, default `kizuna`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::Secret;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    StartTls,
    Tls,
    None,
}

#[derive(Debug, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    pub user: Option<String>,
    pub password: Option<Secret<String>>,
    pub from: String,
    pub to: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub api_base: String,
    pub token: Secret<String>,
    pub chat_id: String,
}

#[derive(Debug, Clone)]
pub struct AlertConfig {
    pub smtp: Option<SmtpConfig>,
    pub telegram: Option<TelegramConfig>,
    pub min_interval_secs: i64,
    pub max_per_hour: usize,
    pub label: String,
}

fn is_local(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

impl AlertConfig {
    pub fn disabled() -> Self {
        AlertConfig {
            smtp: None,
            telegram: None,
            min_interval_secs: 900,
            max_per_hour: 20,
            label: "kizuna".into(),
        }
    }

    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let g = |k: &str| get(k).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let num = |k: &str, d: i64, lo: i64, hi: i64| -> Result<i64, String> {
            match g(k) {
                None => Ok(d),
                Some(v) => v
                    .parse::<i64>()
                    .ok()
                    .filter(|n| (lo..=hi).contains(n))
                    .ok_or(format!("invalid {k}")),
            }
        };
        let smtp = match g("KIZUNA_ALERT_SMTP_HOST") {
            None => None,
            Some(host) => {
                let tls = match g("KIZUNA_ALERT_SMTP_TLS").as_deref() {
                    None | Some("starttls") => SmtpTls::StartTls,
                    Some("tls") => SmtpTls::Tls,
                    Some("none") if is_local(&host) => SmtpTls::None,
                    Some(_) => return Err("invalid KIZUNA_ALERT_SMTP_TLS (plaintext only for localhost)".into()),
                };
                let to: Vec<String> = g("KIZUNA_ALERT_EMAIL_TO")
                    .ok_or("KIZUNA_ALERT_EMAIL_TO required with KIZUNA_ALERT_SMTP_HOST")?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let from = g("KIZUNA_ALERT_EMAIL_FROM").ok_or("KIZUNA_ALERT_EMAIL_FROM required")?;
                for a in to.iter().chain([&from]) {
                    a.parse::<lettre::message::Mailbox>()
                        .map_err(|_| "invalid alert e-mail address".to_string())?;
                }
                let default_port = if tls == SmtpTls::Tls { 465 } else { 587 };
                Some(SmtpConfig {
                    port: num("KIZUNA_ALERT_SMTP_PORT", default_port, 1, 65535)? as u16,
                    host,
                    tls,
                    user: g("KIZUNA_ALERT_SMTP_USER"),
                    password: g("KIZUNA_ALERT_SMTP_PASSWORD").map(Secret::new),
                    from,
                    to,
                })
            }
        };
        let telegram = match (g("KIZUNA_ALERT_TELEGRAM_BOT_TOKEN"), g("KIZUNA_ALERT_TELEGRAM_CHAT_ID")) {
            (Some(token), Some(chat_id)) => {
                let api_base = g("KIZUNA_ALERT_TELEGRAM_API_BASE").unwrap_or_else(|| "https://api.telegram.org".into());
                let host = api_base
                    .strip_prefix("http://")
                    .map(|r| r.split(['/', ':']).next().unwrap_or(""));
                if !(api_base.starts_with("https://") || host.is_some_and(is_local)) {
                    return Err("KIZUNA_ALERT_TELEGRAM_API_BASE must be https (http only for localhost)".into());
                }
                if !token.chars().all(|c| c.is_ascii_alphanumeric() || ":_-".contains(c)) {
                    return Err("invalid KIZUNA_ALERT_TELEGRAM_BOT_TOKEN".into());
                }
                Some(TelegramConfig {
                    api_base: api_base.trim_end_matches('/').to_string(),
                    token: Secret::new(token),
                    chat_id,
                })
            }
            (None, None) => None,
            _ => return Err("set both KIZUNA_ALERT_TELEGRAM_BOT_TOKEN and KIZUNA_ALERT_TELEGRAM_CHAT_ID".into()),
        };
        Ok(AlertConfig {
            smtp,
            telegram,
            min_interval_secs: num("KIZUNA_ALERT_MIN_INTERVAL_SECS", 900, 0, 7 * 86400)?,
            max_per_hour: num("KIZUNA_ALERT_MAX_PER_HOUR", 20, 1, 1000)? as usize,
            label: sanitize(&g("KIZUNA_ALERT_LABEL").unwrap_or_else(|| "kizuna".into()), 40),
        })
    }
}

/// One alert. `text` is composed by KIZUNA code from identifiers only.
#[derive(Debug, Clone)]
pub struct Alert {
    pub kind: String,
    pub severity: String,
    pub key: String,
    pub text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Disabled,
    Suppressed,
    Sent {
        ok: Vec<&'static str>,
        failed: Vec<(&'static str, String)>,
    },
}

/// Printable, single-line, length-capped.
pub fn sanitize(s: &str, max: usize) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GateState {
    last: HashMap<String, i64>,
    sent: Vec<i64>,
}

pub struct Alerter {
    cfg: AlertConfig,
    gate: Mutex<GateState>,
    /// Persists the rate-limit state between short-lived CLI runs (`kizuna-alert`).
    state_file: Option<PathBuf>,
}

impl Alerter {
    pub fn new(cfg: AlertConfig, state_file: Option<PathBuf>) -> Self {
        let st = state_file
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Alerter {
            cfg,
            gate: Mutex::new(st),
            state_file,
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.smtp.is_some() || self.cfg.telegram.is_some()
    }

    pub fn channels(&self) -> serde_json::Value {
        serde_json::json!({ "email": self.cfg.smtp.is_some(), "telegram": self.cfg.telegram.is_some() })
    }

    /// Rate gate: one alert per key per `min_interval_secs`, at most `max_per_hour` overall.
    fn admit(&self, key: &str, now: i64) -> bool {
        let mut g = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        g.sent.retain(|t| now - t < 3600);
        g.last.retain(|_, t| now - *t < self.cfg.min_interval_secs.max(3600));
        if g.sent.len() >= self.cfg.max_per_hour
            || g.last.get(key).is_some_and(|t| now - t < self.cfg.min_interval_secs)
        {
            return false;
        }
        g.sent.push(now);
        g.last.insert(key.to_string(), now);
        if let Some(p) = &self.state_file
            && let Ok(b) = serde_json::to_vec(&*g)
        {
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, b).is_ok() {
                let _ = std::fs::rename(tmp, p);
            }
        }
        true
    }

    /// Blocking (network I/O); from async code use `spawn_blocking`.
    pub fn send(&self, a: &Alert) -> Delivery {
        if !self.enabled() {
            return Delivery::Disabled;
        }
        if !self.admit(&a.key, OffsetDateTime::now_utc().unix_timestamp()) {
            return Delivery::Suppressed;
        }
        let subject = sanitize(
            &format!("[{}] {} {}", self.cfg.label, a.severity.to_uppercase(), a.kind),
            120,
        );
        let body = sanitize(&a.text, 800);
        let (mut ok, mut failed) = (Vec::new(), Vec::new());
        if let Some(s) = &self.cfg.smtp {
            match send_smtp(s, &subject, &body) {
                Ok(()) => ok.push("email"),
                Err(e) => failed.push(("email", e)),
            }
        }
        if let Some(t) = &self.cfg.telegram {
            match send_telegram(t, &format!("{subject}\n{body}")) {
                Ok(()) => ok.push("telegram"),
                Err(e) => failed.push(("telegram", e)),
            }
        }
        for (ch, e) in &failed {
            tracing::warn!(channel = ch, error = %e, kind = %a.kind, "alert delivery failed");
        }
        Delivery::Sent { ok, failed }
    }
}

fn send_smtp(c: &SmtpConfig, subject: &str, body: &str) -> Result<(), String> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{Message, SmtpTransport, Transport};
    let mut m = Message::builder()
        .from(c.from.parse().map_err(|_| "invalid from")?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN);
    for t in &c.to {
        m = m.to(t.parse().map_err(|_| "invalid to")?);
    }
    let msg = m.body(body.to_string()).map_err(|_| "message build failed")?;
    let b = match c.tls {
        SmtpTls::StartTls => SmtpTransport::starttls_relay(&c.host).map_err(|_| "smtp tls setup failed")?,
        SmtpTls::Tls => SmtpTransport::relay(&c.host).map_err(|_| "smtp tls setup failed")?,
        SmtpTls::None => SmtpTransport::builder_dangerous(&c.host),
    };
    let mut b = b.port(c.port).timeout(Some(Duration::from_secs(15)));
    if let (Some(u), Some(p)) = (&c.user, &c.password) {
        b = b.credentials(Credentials::new(u.clone(), p.expose().clone()));
    }
    // Error text from lettre carries server replies, never our credentials; keep only the category.
    b.build().send(&msg).map(|_| ()).map_err(|e| {
        let code = e.status().map(|c| c.to_string()).unwrap_or_default();
        if e.is_permanent() {
            format!("smtp rejected {code}")
        } else if e.is_transient() {
            format!("smtp transient {code}")
        } else {
            "smtp connection/tls error".into()
        }
    })
}

fn send_telegram(c: &TelegramConfig, text: &str) -> Result<(), String> {
    let url = format!("{}/bot{}/sendMessage", c.api_base, c.token.expose());
    let body = serde_json::json!({ "chat_id": c.chat_id, "text": text, "disable_web_page_preview": true }).to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    // Never format the ureq error: it can contain the URL, which contains the bot token.
    match agent
        .post(&url)
        .header("content-type", "application/json")
        .send(body.as_bytes())
    {
        Ok(_) => Ok(()),
        Err(ureq::Error::StatusCode(code)) => Err(format!("telegram http {code}")),
        Err(_) => Err("telegram transport error".into()),
    }
}

/// Alerts for incidents whose policy executed `notify_admins` after `since`. Returns the alerts and the new
/// watermark. Only identifiers and categories leave the system (rule, severity, tenant slug, incident id).
pub async fn incident_alerts(
    pool: &PgPool,
    label: &str,
    since: OffsetDateTime,
) -> sqlx::Result<(Vec<Alert>, OffsetDateTime)> {
    let tenants: Vec<(Uuid, String)> = sqlx::query_as("SELECT id, slug FROM tenants WHERE status = 'active'")
        .fetch_all(pool)
        .await?;
    let mut out = Vec::new();
    let mut mark = since;
    for (t, slug) in tenants {
        let mut tx = crate::db::tenant_tx(pool, t).await?;
        let rows: Vec<(OffsetDateTime, Uuid, String, String)> = sqlx::query_as(
            "SELECT a.created_at, i.id, i.rule, i.severity FROM response_actions a \
             JOIN incidents i ON i.tenant_id = a.tenant_id AND i.id = a.incident_id \
             WHERE a.kind = 'notify_admins' AND a.status = 'executed' AND a.created_at > $1 ORDER BY a.created_at LIMIT 100",
        )
        .bind(since)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        for (at, id, rule, sev) in rows {
            mark = mark.max(at);
            out.push(Alert {
                kind: "security_incident".into(),
                severity: sanitize(&sev, 16),
                key: format!("incident:{id}"),
                text: format!(
                    "{label}: security incident rule={} severity={} tenant={} incident={id}. Details only in the KIZUNA UI (Security > Incidents).",
                    sanitize(&rule, 64),
                    sanitize(&sev, 16),
                    sanitize(&slug, 64)
                ),
            });
        }
    }
    Ok((out, mark))
}

/// Server background task: poll every `every` and deliver incident alerts. Starts at "now", so restarts do not
/// replay old incidents.
pub fn spawn_incident_poller(pool: PgPool, alerter: std::sync::Arc<Alerter>, every: Duration) {
    if !alerter.enabled() {
        tracing::info!("alerts disabled (no SMTP or Telegram configured)");
        return;
    }
    tokio::spawn(async move {
        let mut since = OffsetDateTime::now_utc();
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            match incident_alerts(&pool, &alerter.cfg.label, since).await {
                Ok((alerts, mark)) => {
                    since = mark;
                    for a in alerts {
                        let al = std::sync::Arc::clone(&alerter);
                        let _ = tokio::task::spawn_blocking(move || al.send(&a)).await;
                    }
                }
                Err(e) => tracing::error!(error = %e, "alert poll failed"),
            }
        }
    });
}

/// Fixed texts for host-level events reported by `kizuna-alert` (systemd OnFailure hooks).
pub fn host_event(label: &str, kind: &str, source: &str) -> Option<Alert> {
    let src = sanitize(
        &source
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || "@._-".contains(*c))
            .collect::<String>(),
        80,
    );
    let (sev, what) = match kind {
        "backup_failed" => ("high", "off-host encrypted backup FAILED"),
        "integrity_failure" => ("critical", "integrity sweep FAILED (audit chain or stored object hash)"),
        "unit_failed" => ("high", "service unit failed"),
        "test" => ("info", "test alert (configuration check)"),
        _ => return None,
    };
    Some(Alert {
        kind: kind.into(),
        severity: sev.into(),
        key: format!("host:{kind}:{src}"),
        text: format!("{label}: {what}; source={src}. Check `journalctl -u {src}` on the host."),
    })
}

/// Test doubles: a minimal SMTP server and an HTTP endpoint that record what they receive.
#[doc(hidden)]
pub mod fake {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub type Inbox = Arc<Mutex<Vec<String>>>;

    pub fn smtp() -> (u16, Inbox) {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = l.local_addr().expect("addr").port();
        let inbox: Inbox = Arc::default();
        let ib = Arc::clone(&inbox);
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { break };
                let ib = Arc::clone(&ib);
                std::thread::spawn(move || {
                    let mut r = BufReader::new(c.try_clone().expect("clone"));
                    let _ = c.write_all(b"220 fake ESMTP\r\n");
                    let mut line = String::new();
                    let mut data = String::new();
                    while r.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                        let cmd = line.to_ascii_uppercase();
                        let reply: &[u8] = if cmd.starts_with("EHLO") || cmd.starts_with("HELO") {
                            b"250-fake\r\n250 8BITMIME\r\n"
                        } else if cmd.starts_with("DATA") {
                            let _ = c.write_all(b"354 go\r\n");
                            let mut l2 = String::new();
                            while r.read_line(&mut l2).map(|n| n > 0).unwrap_or(false) && l2 != ".\r\n" {
                                data.push_str(&l2);
                                l2.clear();
                            }
                            ib.lock().expect("lock").push(std::mem::take(&mut data));
                            b"250 queued\r\n"
                        } else if cmd.starts_with("QUIT") {
                            let _ = c.write_all(b"221 bye\r\n");
                            return;
                        } else {
                            b"250 ok\r\n"
                        };
                        let _ = c.write_all(reply);
                        line.clear();
                    }
                });
            }
        });
        (port, inbox)
    }

    /// Records request line + body; replies with `status`.
    pub fn http(status: u16) -> (u16, Inbox) {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = l.local_addr().expect("addr").port();
        let inbox: Inbox = Arc::default();
        let ib = Arc::clone(&inbox);
        std::thread::spawn(move || {
            for c in l.incoming() {
                let Ok(mut c) = c else { break };
                let mut r = BufReader::new(c.try_clone().expect("clone"));
                let mut first = String::new();
                let _ = r.read_line(&mut first);
                let mut len = 0usize;
                let mut h = String::new();
                while r.read_line(&mut h).map(|n| n > 2).unwrap_or(false) {
                    if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    h.clear();
                }
                let mut body = vec![0u8; len];
                let _ = r.read_exact(&mut body);
                ib.lock()
                    .expect("lock")
                    .push(format!("{}\n{}", first.trim(), String::from_utf8_lossy(&body)));
                let _ = write!(
                    c,
                    "HTTP/1.1 {status} X\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
                );
            }
        });
        (port, inbox)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(smtp_port: Option<u16>, tg_port: Option<u16>) -> AlertConfig {
        let mut m: HashMap<&str, String> = HashMap::new();
        if let Some(p) = smtp_port {
            m.insert("KIZUNA_ALERT_SMTP_HOST", "127.0.0.1".into());
            m.insert("KIZUNA_ALERT_SMTP_PORT", p.to_string());
            m.insert("KIZUNA_ALERT_SMTP_TLS", "none".into());
            m.insert("KIZUNA_ALERT_EMAIL_FROM", "kizuna@pilot.example".into());
            m.insert("KIZUNA_ALERT_EMAIL_TO", "ops@pilot.example, cto@pilot.example".into());
        }
        if let Some(p) = tg_port {
            m.insert("KIZUNA_ALERT_TELEGRAM_BOT_TOKEN", "123456:SECRET-token_x".into());
            m.insert("KIZUNA_ALERT_TELEGRAM_CHAT_ID", "-1001".into());
            m.insert("KIZUNA_ALERT_TELEGRAM_API_BASE", format!("http://127.0.0.1:{p}"));
        }
        m.insert("KIZUNA_ALERT_LABEL", "pilot".into());
        AlertConfig::from_lookup(|k| m.get(k).cloned()).unwrap()
    }

    #[test]
    fn disabled_when_unset() {
        let a = Alerter::new(AlertConfig::from_lookup(|_| None).unwrap(), None);
        assert!(!a.enabled());
        let al = host_event("x", "test", "kizuna-backup.service").unwrap();
        assert_eq!(a.send(&al), Delivery::Disabled);
    }

    #[test]
    fn email_and_telegram_delivered_without_secrets() {
        let (sp, mail) = fake::smtp();
        let (hp, hooks) = fake::http(200);
        let a = Alerter::new(cfg(Some(sp), Some(hp)), None);
        let al = host_event("pilot", "backup_failed", "kizuna-backup.service; rm -rf /").unwrap();
        assert_eq!(
            a.send(&al),
            Delivery::Sent {
                ok: vec!["email", "telegram"],
                failed: vec![]
            }
        );
        let m = mail.lock().unwrap().join("");
        assert!(m.contains("[pilot] HIGH backup_failed"), "{m}");
        assert!(m.contains("kizuna-backup.servicerm-rf"), "source sanitised: {m}");
        let h = hooks.lock().unwrap().join("");
        assert!(h.starts_with("POST /bot123456:SECRET-token_x/sendMessage"), "{h}");
        assert!(h.contains("\"chat_id\":\"-1001\""));
        assert!(
            !h.split_once('\n').unwrap().1.contains("SECRET"),
            "token only in the URL path"
        );
        assert!(!m.contains("SECRET"));
    }

    #[test]
    fn rate_limited_per_key_and_globally() {
        let (hp, hooks) = fake::http(200);
        let mut c = cfg(None, Some(hp));
        c.max_per_hour = 3;
        let a = Alerter::new(c, None);
        let ev = |k: &str| host_event("pilot", "unit_failed", k).unwrap();
        assert!(matches!(a.send(&ev("a")), Delivery::Sent { .. }));
        assert_eq!(a.send(&ev("a")), Delivery::Suppressed, "same key within interval");
        assert!(matches!(a.send(&ev("b")), Delivery::Sent { .. }));
        assert!(matches!(a.send(&ev("c")), Delivery::Sent { .. }));
        assert_eq!(a.send(&ev("d")), Delivery::Suppressed, "hourly cap");
        assert_eq!(hooks.lock().unwrap().len(), 3);
    }

    #[test]
    fn rate_state_persists_across_cli_runs() {
        let (hp, _) = fake::http(200);
        let f = std::env::temp_dir().join(format!("kz-alert-{}.json", Uuid::now_v7()));
        let ev = host_event("pilot", "integrity_failure", "kizuna-integrity.service").unwrap();
        assert!(matches!(
            Alerter::new(cfg(None, Some(hp)), Some(f.clone())).send(&ev),
            Delivery::Sent { .. }
        ));
        assert_eq!(
            Alerter::new(cfg(None, Some(hp)), Some(f.clone())).send(&ev),
            Delivery::Suppressed
        );
        std::fs::remove_file(f).ok();
    }

    #[test]
    fn delivery_failures_reported_without_token() {
        let (hp, _) = fake::http(500);
        let a = Alerter::new(cfg(Some(1), Some(hp)), None);
        let Delivery::Sent { ok, failed } = a.send(&host_event("p", "test", "x").unwrap()) else {
            panic!()
        };
        assert!(ok.is_empty());
        assert_eq!(failed.len(), 2);
        assert!(failed.iter().all(|(_, e)| !e.contains("SECRET")), "{failed:?}");
        assert!(failed.iter().any(|(_, e)| e == "telegram http 500"));
    }

    #[test]
    fn config_validation() {
        let bad = |pairs: &[(&str, &str)]| {
            let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            AlertConfig::from_lookup(|k| m.get(k).cloned()).is_err()
        };
        assert!(bad(&[("KIZUNA_ALERT_TELEGRAM_BOT_TOKEN", "1:x")]), "chat id missing");
        assert!(bad(&[
            ("KIZUNA_ALERT_TELEGRAM_BOT_TOKEN", "1:x"),
            ("KIZUNA_ALERT_TELEGRAM_CHAT_ID", "1"),
            ("KIZUNA_ALERT_TELEGRAM_API_BASE", "http://evil.example")
        ]));
        assert!(
            bad(&[
                ("KIZUNA_ALERT_SMTP_HOST", "smtp.gmail.com"),
                ("KIZUNA_ALERT_SMTP_TLS", "none"),
                ("KIZUNA_ALERT_EMAIL_FROM", "a@b.c"),
                ("KIZUNA_ALERT_EMAIL_TO", "d@e.f")
            ]),
            "plaintext SMTP only to localhost"
        );
        assert!(
            bad(&[("KIZUNA_ALERT_SMTP_HOST", "smtp.gmail.com")]),
            "recipients required"
        );
        let c = cfg(Some(25), Some(1));
        assert!(!format!("{c:?}").contains("SECRET"), "Debug redacts the token");
        assert!(host_event("p", "nonsense", "x").is_none());
    }
}
