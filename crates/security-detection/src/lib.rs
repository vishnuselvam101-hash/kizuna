//! Detection rules over security events and the response policy (directive §10, "security-events" module).
//! Defensive only: every action targets KIZUNA's own sessions, accounts, uploads, workers or documents.
//! Nothing here contacts, scans or acts against external systems.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

pub const RULES_VERSION: &str = "sec-rules@1";

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: String,
    pub kind: String,
    pub user: Option<String>,
    pub ip: Option<String>,
    /// Unix seconds.
    pub at: i64,
    /// Selected detail fields (e.g. `reason`, `artifact_id`).
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    RevokeUserSessions,
    LockAccount,
    BlockIp,
    QuarantineArtifact,
    RestrictUploads,
    SuspendExtraction,
    NotifyAdmins,
}

impl ActionKind {
    pub fn code(self) -> &'static str {
        match self {
            ActionKind::RevokeUserSessions => "revoke_user_sessions",
            ActionKind::LockAccount => "lock_account",
            ActionKind::BlockIp => "block_ip",
            ActionKind::QuarantineArtifact => "quarantine_artifact",
            ActionKind::RestrictUploads => "restrict_uploads",
            ActionKind::SuspendExtraction => "suspend_extraction",
            ActionKind::NotifyAdmins => "notify_admins",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// Low-impact, reversible or time-limited: executed automatically.
    PreAuthorised,
    /// High-impact: proposed, executed only after an MFA-verified administrator approves.
    RequiresApproval,
}

impl Policy {
    pub fn code(self) -> &'static str {
        match self {
            Policy::PreAuthorised => "pre_authorised",
            Policy::RequiresApproval => "requires_approval",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposedAction {
    pub kind: ActionKind,
    pub target: String,
    pub policy: Policy,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Detection {
    pub rule: &'static str,
    pub severity: &'static str,
    pub subject: String,
    pub event_ids: Vec<String>,
    pub summary: String,
    pub actions: Vec<ProposedAction>,
}

#[derive(Debug, Clone)]
pub struct Thresholds {
    pub brute_force: usize,
    pub brute_force_lock: usize,
    pub spray_failures: usize,
    pub spray_users: usize,
    pub authz_denials: usize,
    pub privileged_ops: usize,
    pub upload_abuse: usize,
    pub extraction_failures: usize,
    pub exports: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            brute_force: 5,
            brute_force_lock: 10,
            spray_failures: 8,
            spray_users: 3,
            authz_denials: 10,
            privileged_ops: 5,
            upload_abuse: 5,
            extraction_failures: 5,
            exports: 20,
        }
    }
}

fn act(kind: ActionKind, target: &str, policy: Policy) -> ProposedAction {
    ProposedAction {
        kind,
        target: target.to_string(),
        policy,
    }
}

fn window<'a>(events: &'a [Event], now: i64, secs: i64, kinds: &[&str]) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|e| e.at > now - secs && e.at <= now && kinds.contains(&e.kind.as_str()))
        .collect()
}

fn group_by<'a, F: Fn(&Event) -> Option<String>>(es: &[&'a Event], key: F) -> BTreeMap<String, Vec<&'a Event>> {
    let mut m: BTreeMap<String, Vec<&Event>> = BTreeMap::new();
    for e in es {
        if let Some(k) = key(e) {
            m.entry(k).or_default().push(e);
        }
    }
    m
}

fn ids(es: &[&Event]) -> Vec<String> {
    es.iter().map(|e| e.id.clone()).collect()
}

pub fn detect(events: &[Event], now: i64, t: &Thresholds) -> Vec<Detection> {
    let mut out = Vec::new();
    use ActionKind::*;
    use Policy::*;

    // R1 brute force against one account.
    let fails = window(events, now, 900, &["auth.login_failed"]);
    let credential_fail = |e: &&Event| {
        matches!(
            e.details.get("reason").map(String::as_str),
            Some("bad_password" | "bad_totp" | "reauth_bad_password")
        )
    };
    let cf: Vec<&Event> = fails.iter().copied().filter(credential_fail).collect();
    for (user, es) in group_by(&cf, |e| e.user.clone()) {
        if es.len() >= t.brute_force {
            let mut actions = vec![act(NotifyAdmins, &user, PreAuthorised)];
            if es.len() >= t.brute_force_lock {
                actions.push(act(LockAccount, &user, RequiresApproval));
            }
            out.push(Detection {
                rule: "brute_force_account",
                severity: "high",
                subject: format!("user:{user}"),
                event_ids: ids(&es),
                summary: format!(
                    "{} failed credential checks for one account within 15 minutes",
                    es.len()
                ),
                actions,
            });
        }
    }
    // R2 password spraying / credential stuffing from one IP across accounts.
    for (ip, es) in group_by(&fails, |e| e.ip.clone()) {
        let users: BTreeSet<String> = es
            .iter()
            .map(|e| e.user.clone().unwrap_or_else(|| format!("unknown:{}", e.id)))
            .collect();
        if es.len() >= t.spray_failures && users.len() >= t.spray_users {
            out.push(Detection {
                rule: "password_spraying",
                severity: "high",
                subject: format!("ip:{ip}"),
                event_ids: ids(&es),
                summary: format!(
                    "{} failed logins for {} accounts from one address within 15 minutes",
                    es.len(),
                    users.len()
                ),
                actions: vec![act(BlockIp, &ip, PreAuthorised), act(NotifyAdmins, &ip, PreAuthorised)],
            });
        }
    }
    // R3 stolen refresh token.
    let reuse = window(events, now, 3600, &["auth.refresh_token_reuse"]);
    for (user, es) in group_by(&reuse, |e| e.user.clone()) {
        out.push(Detection {
            rule: "token_theft",
            severity: "critical",
            subject: format!("user:{user}"),
            event_ids: ids(&es),
            summary: "a rotated refresh token was replayed (likely theft)".into(),
            actions: vec![
                act(RevokeUserSessions, &user, PreAuthorised),
                act(LockAccount, &user, RequiresApproval),
                act(NotifyAdmins, &user, PreAuthorised),
            ],
        });
    }
    // R4 authorisation probing (incl. cross-tenant attempts surfacing as denials).
    let den = window(events, now, 600, &["authz.denied"]);
    for (user, es) in group_by(&den, |e| e.user.clone()) {
        if es.len() >= t.authz_denials {
            out.push(Detection {
                rule: "authorization_probing",
                severity: "medium",
                subject: format!("user:{user}"),
                event_ids: ids(&es),
                summary: format!("{} denied requests within 10 minutes", es.len()),
                actions: vec![
                    act(RevokeUserSessions, &user, PreAuthorised),
                    act(NotifyAdmins, &user, PreAuthorised),
                ],
            });
        }
    }
    // R5 burst of privileged operations by one administrator.
    let priv_ops = window(
        events,
        now,
        600,
        &[
            "admin.role_changed",
            "admin.user_disabled",
            "auth.mfa_reset",
            "auth.sessions_revoked",
        ],
    );
    for (actor, es) in group_by(&priv_ops, |e| e.user.clone()) {
        if es.len() >= t.privileged_ops {
            out.push(Detection {
                rule: "privileged_operation_burst",
                severity: "high",
                subject: format!("user:{actor}"),
                event_ids: ids(&es),
                summary: format!(
                    "{} privileged account changes within 10 minutes by one administrator",
                    es.len()
                ),
                actions: vec![
                    act(NotifyAdmins, &actor, PreAuthorised),
                    act(RevokeUserSessions, &actor, RequiresApproval),
                ],
            });
        }
    }
    // R6 integrity failures.
    for e in window(events, now, 3600, &["integrity.artifact_mismatch"]) {
        if let Some(a) = e.details.get("artifact_id") {
            out.push(Detection {
                rule: "artifact_integrity_failure",
                severity: "critical",
                subject: format!("artifact:{a}"),
                event_ids: vec![e.id.clone()],
                summary: "stored document bytes no longer match their recorded SHA-256".into(),
                actions: vec![
                    act(QuarantineArtifact, a, PreAuthorised),
                    act(NotifyAdmins, a, PreAuthorised),
                ],
            });
        }
    }
    let chain = window(events, now, 3600, &["integrity.audit_chain_broken"]);
    if !chain.is_empty() {
        out.push(Detection {
            rule: "audit_chain_broken",
            severity: "critical",
            subject: "audit_log".into(),
            event_ids: ids(&chain),
            summary: "audit hash chain verification failed; preserve the database for forensics".into(),
            actions: vec![act(NotifyAdmins, "audit_log", PreAuthorised)],
        });
    }
    // R7 abusive uploads.
    let ups = window(
        events,
        now,
        1800,
        &["ingest.upload_rejected", "ingest.artifact_quarantined"],
    );
    for (user, es) in group_by(&ups, |e| e.user.clone()) {
        if es.len() >= t.upload_abuse {
            out.push(Detection {
                rule: "upload_abuse",
                severity: "medium",
                subject: format!("user:{user}"),
                event_ids: ids(&es),
                summary: format!("{} rejected or quarantined uploads within 30 minutes", es.len()),
                actions: vec![
                    act(RestrictUploads, &user, PreAuthorised),
                    act(NotifyAdmins, &user, PreAuthorised),
                ],
            });
        }
    }
    // R8 abnormal ingestion: repeated extraction failures.
    let ex = window(events, now, 1800, &["ingest.extraction_failed"]);
    if ex.len() >= t.extraction_failures {
        out.push(Detection {
            rule: "extraction_failures",
            severity: "medium",
            subject: "tenant".into(),
            event_ids: ids(&ex),
            summary: format!("{} extraction failures within 30 minutes", ex.len()),
            actions: vec![
                act(SuspendExtraction, "tenant", PreAuthorised),
                act(NotifyAdmins, "tenant", PreAuthorised),
            ],
        });
    }
    // R9 unusual exports.
    let exp = window(events, now, 3600, &["export.report"]);
    for (user, es) in group_by(&exp, |e| e.user.clone()) {
        if es.len() >= t.exports {
            out.push(Detection {
                rule: "unusual_exports",
                severity: "medium",
                subject: format!("user:{user}"),
                event_ids: ids(&es),
                summary: format!("{} report exports within one hour", es.len()),
                actions: vec![act(NotifyAdmins, &user, PreAuthorised)],
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(i: usize, kind: &str, user: Option<&str>, ip: &str, at: i64, reason: &str) -> Event {
        Event {
            id: format!("e{i}"),
            kind: kind.into(),
            user: user.map(Into::into),
            ip: Some(ip.into()),
            at,
            details: BTreeMap::from([("reason".to_string(), reason.to_string())]),
        }
    }

    #[test]
    fn brute_force_and_spraying() {
        let now = 10_000;
        let mut es: Vec<Event> = (0..10)
            .map(|i| ev(i, "auth.login_failed", Some("u1"), "1.1.1.1", now - 10, "bad_password"))
            .collect();
        let d = detect(&es, now, &Thresholds::default());
        let bf = d.iter().find(|d| d.rule == "brute_force_account").unwrap();
        assert!(
            bf.actions
                .iter()
                .any(|a| a.kind == ActionKind::LockAccount && a.policy == Policy::RequiresApproval)
        );
        assert!(
            !d.iter().any(|d| d.rule == "password_spraying"),
            "one account is not spraying"
        );
        es = (0..9)
            .map(|i| {
                ev(
                    i,
                    "auth.login_failed",
                    Some(&format!("u{}", i % 4)),
                    "6.6.6.6",
                    now - 10,
                    "bad_password",
                )
            })
            .collect();
        let d = detect(&es, now, &Thresholds::default());
        let sp = d.iter().find(|d| d.rule == "password_spraying").unwrap();
        assert_eq!(
            sp.actions[0],
            ProposedAction {
                kind: ActionKind::BlockIp,
                target: "6.6.6.6".into(),
                policy: Policy::PreAuthorised
            }
        );
    }

    #[test]
    fn windows_and_thresholds() {
        let now = 10_000;
        let old: Vec<Event> = (0..20)
            .map(|i| {
                ev(
                    i,
                    "auth.login_failed",
                    Some("u1"),
                    "1.1.1.1",
                    now - 2000,
                    "bad_password",
                )
            })
            .collect();
        assert!(detect(&old, now, &Thresholds::default()).is_empty(), "outside window");
        let few: Vec<Event> = (0..4)
            .map(|i| ev(i, "auth.login_failed", Some("u1"), "1.1.1.1", now, "bad_password"))
            .collect();
        assert!(detect(&few, now, &Thresholds::default()).is_empty());
        let unknown: Vec<Event> = (0..6)
            .map(|i| ev(i, "auth.login_failed", Some("u1"), "1.1.1.1", now, "unknown_user"))
            .collect();
        assert!(
            detect(&unknown, now, &Thresholds::default())
                .iter()
                .all(|d| d.rule != "brute_force_account")
        );
    }

    #[test]
    fn token_theft_and_integrity() {
        let now = 50;
        let mut e = ev(1, "auth.refresh_token_reuse", Some("u9"), "2.2.2.2", now, "");
        let d = detect(std::slice::from_ref(&e), now, &Thresholds::default());
        assert_eq!(d[0].severity, "critical");
        assert_eq!(d[0].actions[0].kind, ActionKind::RevokeUserSessions);
        e.kind = "integrity.artifact_mismatch".into();
        e.details.insert("artifact_id".into(), "a1".into());
        let d = detect(&[e], now, &Thresholds::default());
        assert_eq!(
            d[0].actions[0],
            ProposedAction {
                kind: ActionKind::QuarantineArtifact,
                target: "a1".into(),
                policy: Policy::PreAuthorised
            }
        );
    }
}
