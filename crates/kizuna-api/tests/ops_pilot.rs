#![allow(clippy::unwrap_used)]
//! Ops pilot (ADR 0016/0017): antivirus at upload with a fake clamd, honest status, incident alerts.
mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use common::*;
use kizuna_api::alerts::{AlertConfig, Alerter, Delivery, fake as fake_alert};
use kizuna_api::antivirus::fake::{Mode, eicar, spawn};
use kizuna_api::antivirus::{Clamd, MalwareScanner};
use kizuna_api::{db, router};

fn sock_dir() -> std::path::PathBuf {
    // Unix socket paths are limited to ~108 bytes: use the system temp dir, not the deep target dir.
    let d = std::env::temp_dir().join(format!("kz-clamd-{}", uuid::Uuid::now_v7().simple()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn with_av(c: &mut Ctx, av: Arc<dyn MalwareScanner>) {
    c.st.av = av;
    c.app = router(c.st.clone());
}

fn clamd(sock: std::path::PathBuf) -> Arc<dyn MalwareScanner> {
    Arc::new(Clamd {
        socket: sock,
        timeout: Duration::from_secs(2),
        max_bytes: 25 * 1024 * 1024,
    })
}

async fn admin_token(c: &Ctx) -> String {
    let (t, slug) = c.tenant().await;
    let (_, email) = c.user(t, "tenant_admin").await;
    c.token(&slug, &email).await
}

#[tokio::test]
async fn eicar_upload_is_quarantined_and_clean_upload_accepted() {
    let mut c = ctx().await;
    with_av(&mut c, clamd(spawn(&sock_dir(), Mode::Normal)));
    let tok = admin_token(&c).await;
    let mut csv = b"part,qty\n".to_vec();
    csv.extend(eicar());
    csv.extend(b"\n");
    let r = c.upload(&tok, "x.csv", &csv, None).await;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let a = &r.body["artifact"];
    assert_eq!(a["status"], "quarantined", "{a}");
    assert_eq!(a["scan_result"]["antivirus"]["result"], "infected");
    assert!(
        a["scan_result"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f.as_str().unwrap().starts_with("av_infected:"))
    );
    let ok = c.upload(&tok, "ok.csv", b"part,qty\nBRK-100,4\n", None).await;
    assert_eq!(ok.body["artifact"]["status"], "accepted", "{:?}", ok.body);
    assert_eq!(ok.body["artifact"]["scan_result"]["antivirus"]["result"], "clean");
    let s = c.get("/api/v1/security/status", &tok).await;
    assert_eq!(s.status, StatusCode::OK);
    assert_eq!(s.body["antivirus"]["status"], "ok", "{:?}", s.body);
    assert_eq!(s.body["extraction_sandbox"]["seccomp"]["status"], "enabled");
}

#[tokio::test]
async fn configured_but_unavailable_scanner_fails_closed() {
    let mut c = ctx().await;
    with_av(&mut c, clamd(sock_dir().join("nothing-listens.sock")));
    let tok = admin_token(&c).await;
    let r = c.upload(&tok, "ok.csv", b"part,qty\nBRK-200,1\n", None).await;
    assert_eq!(r.body["artifact"]["status"], "quarantined", "{:?}", r.body);
    assert_eq!(r.body["artifact"]["scan_result"]["antivirus"]["result"], "unavailable");
    let s = c.get("/api/v1/security/status", &tok).await;
    assert_eq!(s.body["antivirus"]["status"], "unavailable", "{:?}", s.body);
}

#[tokio::test]
async fn not_configured_is_reported_honestly() {
    let c = ctx().await;
    let tok = admin_token(&c).await;
    let r = c.upload(&tok, "ok.csv", b"part,qty\nBRK-300,1\n", None).await;
    assert_eq!(r.body["artifact"]["status"], "accepted");
    assert_eq!(
        r.body["artifact"]["scan_result"]["antivirus"]["result"],
        "not_configured"
    );
    let s = c.get("/api/v1/security/status", &tok).await;
    assert_eq!(s.body["antivirus"]["configured"], false);
    assert_eq!(s.body["antivirus"]["status"], "not_configured");
    assert_eq!(s.body["alerts"]["email"], false);
    let (t, slug) = c.tenant().await;
    let (_, viewer) = c.user(t, "viewer").await;
    let vt = c.token(&slug, &viewer).await;
    assert_eq!(
        c.get("/api/v1/security/status", &vt).await.status,
        StatusCode::FORBIDDEN
    );
}

/// An incident whose policy executes `notify_admins` produces exactly one alert with identifiers only.
#[tokio::test]
async fn incident_notify_admins_becomes_alert_without_personal_data() {
    let c = ctx().await;
    let (t, slug) = c.tenant().await;
    let (uid, email) = c.user(t, "analyst").await;
    let since = time::OffsetDateTime::now_utc() - time::Duration::seconds(1);
    let mut tx = db::tenant_tx(&c.owner, t).await.unwrap();
    sqlx::query("INSERT INTO security_events (id, tenant_id, kind, severity, user_id, client_ip) VALUES ($1,$2,'auth.refresh_token_reuse','high',$3,'203.0.113.77')")
        .bind(uuid::Uuid::now_v7())
        .bind(t)
        .bind(uid)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    kizuna_api::incidents::run_detection(&c.st.pool, t).await.unwrap();
    let (alerts, mark) = kizuna_api::alerts::incident_alerts(&c.st.pool, "pilot", since)
        .await
        .unwrap();
    let mine: Vec<_> = alerts.iter().filter(|a| a.text.contains(&slug)).collect();
    assert!(!mine.is_empty(), "{alerts:?}");
    assert!(mark > since);
    let (port, inbox) = fake_alert::http(200);
    let al = Alerter::new(
        AlertConfig::from_lookup(|k| match k {
            "KIZUNA_ALERT_TELEGRAM_BOT_TOKEN" => Some("1:abc".into()),
            "KIZUNA_ALERT_TELEGRAM_CHAT_ID" => Some("42".into()),
            "KIZUNA_ALERT_TELEGRAM_API_BASE" => Some(format!("http://127.0.0.1:{port}")),
            _ => None,
        })
        .unwrap(),
        None,
    );
    assert!(matches!(al.send(mine[0]), Delivery::Sent { .. }));
    let got = inbox.lock().unwrap().join("\n");
    assert!(got.contains("security incident") && got.contains(&slug), "{got}");
    assert!(
        !got.contains(&email) && !got.contains("203.0.113.77"),
        "no personal data: {got}"
    );
    // Watermark: the same actions are not reported twice.
    let (again, _) = kizuna_api::alerts::incident_alerts(&c.st.pool, "pilot", mark)
        .await
        .unwrap();
    assert!(!again.iter().any(|a| a.text.contains(&slug)));
}
