//! Verification-run benchmark at soak scale (PERF_VERIFY_2026-11-05). Seeds a tenant with synthetic manufacturing
//! claims (spec / drawing / purchase order / inspection report per part, ~half the parts with one injected fault)
//! straight into the database, then times `POST /verification/run`'s work (`verification::detect_in_tx` or
//! `verification::detect`) in a runtime-role tenant transaction:
//!   full_first      first full run (inserts every finding)
//!   full_rerun      steady-state full re-runs
//!   change+run      one "typical change" (a new inspection document for one part: 4 claims, or a supersede) followed
//!                   by a run in the given mode
//!
//! kizuna-verify-bench [--parts 3900] [--seed 7] [--runs 7] [--mode full|incremental] [--slug vbench]
//! Env: KIZUNA_BENCH_OWNER_URL, KIZUNA_BENCH_APP_URL. An existing tenant with the same slug is reused (no reseed).

use std::time::{Duration, Instant};

use kizuna_api::db;
use kizuna_bench::summarize;
use serde_json::json;
use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

type R<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// One claim row: (subject, predicate, text, num, unit, revision, scope).
type Row = (
    String,
    &'static str,
    Option<String>,
    Option<String>,
    Option<&'static str>,
    Option<String>,
    String,
);

#[derive(Default)]
struct Batch {
    id: Vec<Uuid>,
    subject: Vec<String>,
    predicate: Vec<String>,
    text: Vec<Option<String>>,
    num: Vec<Option<String>>,
    unit: Vec<Option<String>>,
    revision: Vec<Option<String>>,
    scope: Vec<String>,
    artifact: Vec<Uuid>,
}

const PREFIXES: [&str; 8] = ["BRK", "SHF", "HSG", "FLG", "PIN", "GER", "BSH", "CVR"];

/// Documents for one part. `fault` 0..6 injects one fault (0 = clean).
fn part_docs(p: &str, fault: u64, lot: u64) -> Vec<(&'static str, Vec<Row>)> {
    let sc = |s: &str| format!("{{\"supplier\":\"{s}\"}}");
    let rev = || Some("C".to_string());
    let t = |s: &str| Some(s.to_string());
    let len = if fault == 3 { "125" } else { "120" };
    let expiry = if fault == 5 { "2024-03-31" } else { "2031-01-01" };
    let po_rev = if fault == 1 { "B" } else { "C" };
    let unit = if fault == 4 { "in" } else { "mm" };
    let measured = if fault == 6 { "120.9" } else { "120.05" };
    let mut spec = vec![
        (p.into(), "mfg.revision", t("C"), None, None, rev(), sc("ACME")),
        (
            p.into(),
            "mfg.dimension.length",
            None,
            Some("120".into()),
            Some("mm"),
            rev(),
            sc("ACME"),
        ),
        (
            p.into(),
            "mfg.tolerance.length.plus",
            None,
            Some("0.2".into()),
            Some("mm"),
            rev(),
            sc("ACME"),
        ),
        (
            p.into(),
            "mfg.tolerance.length.minus",
            None,
            Some("0.2".into()),
            Some("mm"),
            rev(),
            sc("ACME"),
        ),
        (
            p.into(),
            "mfg.certificate.expiry",
            t(expiry),
            None,
            None,
            rev(),
            sc("ACME"),
        ),
        (p.into(), "mfg.material", t("S355JR"), None, None, rev(), sc("ACME")),
    ];
    if fault != 2 {
        spec.push((
            p.into(),
            "mfg.approval.status",
            t("approved"),
            None,
            None,
            rev(),
            sc("ACME"),
        ));
    }
    let drawing = vec![
        (p.into(), "mfg.revision", t("C"), None, None, rev(), sc("ACME")),
        (
            p.into(),
            "mfg.dimension.length",
            None,
            Some(len.into()),
            Some(if fault == 4 { "in" } else { unit }),
            rev(),
            sc("ACME"),
        ),
        (p.into(), "mfg.material", t("S355JR"), None, None, rev(), sc("ACME")),
    ];
    let po = vec![
        (p.into(), "po.revision_ordered", t(po_rev), None, None, None, sc("ACME")),
        (p.into(), "po.supplier", t("ACME"), None, None, None, sc("ACME")),
        (
            p.into(),
            "po.quantity",
            None,
            Some("500".into()),
            Some("pcs"),
            None,
            sc("ACME"),
        ),
    ];
    let lot_sc = format!("{{\"supplier\":\"ACME\",\"lot\":\"L{lot}\"}}");
    let insp = vec![
        (
            p.into(),
            "qa.revision_inspected",
            t("C"),
            None,
            None,
            None,
            lot_sc.clone(),
        ),
        (
            p.into(),
            "qa.measured.length",
            None,
            Some(measured.into()),
            Some("mm"),
            rev(),
            lot_sc.clone(),
        ),
        (p.into(), "qa.result", t("pass"), None, None, None, lot_sc.clone()),
        (p.into(), "qa.sample_size", None, Some("32".into()), None, None, lot_sc),
    ];
    vec![
        ("spec_document", spec),
        ("drawing", drawing),
        ("purchase_order", po),
        ("inspection_report", insp),
    ]
}

async fn insert_docs(owner: &PgPool, tenant: Uuid, docs: &[(&'static str, Vec<Row>)]) -> R<Vec<Uuid>> {
    let mut tx = db::tenant_tx(owner, tenant).await?;
    let mut b = Batch::default();
    let (mut aid, mut akind, mut asha, mut aname) = (vec![], vec![], vec![], vec![]);
    for (kind, rows) in docs {
        let a = Uuid::now_v7();
        aid.push(a);
        akind.push(kind.to_string());
        asha.push(kizuna_api::crypto::random_bytes::<32>().to_vec());
        aname.push(format!("{kind}-{a}.pdf"));
        for r in rows {
            b.id.push(Uuid::now_v7());
            b.subject.push(r.0.clone());
            b.predicate.push(r.1.to_string());
            b.text.push(r.2.clone());
            b.num.push(r.3.clone());
            b.unit.push(r.4.map(str::to_string));
            b.revision.push(r.5.clone());
            b.scope.push(r.6.clone());
            b.artifact.push(a);
        }
    }
    sqlx::query(
        "INSERT INTO artifacts (id, tenant_id, sha256, size_bytes, media_type, kind, status, storage_key, display_name, created_by) \
         SELECT u.id, $1, u.sha, 1000, 'application/pdf', u.kind, 'accepted', 'bench/' || u.id, u.name, $1 \
         FROM unnest($2::uuid[], $3::text[], $4::bytea[], $5::text[]) AS u(id, kind, sha, name)",
    )
    .bind(tenant)
    .bind(&aid)
    .bind(&akind)
    .bind(&asha)
    .bind(&aname)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO claims (id, tenant_id, subject_type, subject_key, predicate, value_text, value_num, value_unit, revision, scope, artifact_id, extraction_method, confidence_bps) \
         SELECT u.id, $1, 'part', u.s, u.p, u.t, u.n::numeric, u.u, u.r, u.sc::jsonb, u.a, 'rules:v1', 9000 \
         FROM unnest($2::uuid[], $3::text[], $4::text[], $5::text[], $6::text[], $7::text[], $8::text[], $9::text[], $10::uuid[]) \
              AS u(id, s, p, t, n, u, r, sc, a)",
    )
    .bind(tenant)
    .bind(&b.id)
    .bind(&b.subject)
    .bind(&b.predicate)
    .bind(&b.text)
    .bind(&b.num)
    .bind(&b.unit)
    .bind(&b.revision)
    .bind(&b.scope)
    .bind(&b.artifact)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(b.id)
}

fn part_keys(rng: &mut Rng, n: usize) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut v = Vec::with_capacity(n);
    while v.len() < n {
        let k = format!("{}-{:05}", PREFIXES[rng.below(8) as usize], rng.below(100_000));
        if seen.insert(k.clone()) {
            v.push(k);
        }
    }
    v
}

async fn run_once(app: &PgPool, tenant: Uuid, mode: &str) -> R<(Duration, serde_json::Value)> {
    let t0 = Instant::now();
    let mut tx = db::tenant_tx(app, tenant).await?;
    let now = OffsetDateTime::now_utc();
    let r = match mode {
        "full" => kizuna_api::verification::detect_in_tx(&mut tx, tenant, now).await,
        _ => kizuna_api::verification::detect(&mut tx, tenant, now, kizuna_api::verification::RunMode::Auto).await,
    }
    .map_err(|e| format!("{e:?}"))?;
    tx.commit().await?;
    Ok((t0.elapsed(), serde_json::to_value(&r)?))
}

#[tokio::main]
async fn main() -> R<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let parts: usize = arg("--parts").map(|v| v.parse()).transpose()?.unwrap_or(3900);
    let seed: u64 = arg("--seed").map(|v| v.parse()).transpose()?.unwrap_or(7);
    let runs: usize = arg("--runs").map(|v| v.parse()).transpose()?.unwrap_or(7);
    let mode = arg("--mode").unwrap_or_else(|| "full".into());
    let slug = arg("--slug").unwrap_or_else(|| "vbench".into());
    let owner = db::connect(&std::env::var("KIZUNA_BENCH_OWNER_URL")?, 2).await?;
    let app = db::connect(&std::env::var("KIZUNA_BENCH_APP_URL")?, 2).await?;
    db::migrator(&db::Roles::from_env()?).run(&owner).await?;
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let keys = part_keys(&mut rng, parts);
    let existing: Option<Uuid> = sqlx::query_scalar("SELECT id FROM tenants WHERE slug = $1")
        .bind(&slug)
        .fetch_optional(&owner)
        .await?;
    let tenant = match existing {
        Some(t) => t,
        None => {
            let t = Uuid::now_v7();
            sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1,$2,'Bench')")
                .bind(t)
                .bind(&slug)
                .execute(&owner)
                .await?;
            let t0 = Instant::now();
            for chunk in keys.chunks(100) {
                let mut docs = Vec::new();
                for k in chunk {
                    let fault = if rng.below(2) == 0 { 0 } else { 1 + rng.below(6) };
                    docs.extend(part_docs(k, fault, rng.below(1000)));
                }
                insert_docs(&owner, t, &docs).await?;
            }
            eprintln!("seeded {parts} parts in {:?}", t0.elapsed());
            t
        }
    };
    let mut tx = db::tenant_tx(&owner, tenant).await?;
    let claims: i64 = sqlx::query_scalar("SELECT count(*) FROM claims")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    let mut out = serde_json::Map::new();
    out.insert("claims".into(), json!(claims));
    out.insert("mode".into(), json!(mode));
    let (d, r) = run_once(&app, tenant, &mode).await?;
    eprintln!("first run ({mode}): {d:?} {r}");
    out.insert("first_run_ms".into(), json!(d.as_millis()));
    out.insert("first_run".into(), r);
    let mut steady = vec![];
    for _ in 0..runs {
        steady.push(run_once(&app, tenant, &mode).await?.0);
    }
    out.insert("rerun_no_change".into(), summarize(&steady, steady.iter().sum()));
    // Typical change: one new inspection document for a random part (4 claims), then a run.
    let mut changed = vec![];
    let mut last = json!(null);
    for i in 0..runs {
        let k = &keys[rng.below(keys.len() as u64) as usize];
        let docs = part_docs(k, if i % 2 == 0 { 6 } else { 0 }, 5000 + i as u64);
        insert_docs(&owner, tenant, &docs[3..]).await?;
        let (d, r) = run_once(&app, tenant, &mode).await?;
        changed.push(d);
        last = r;
    }
    out.insert("change_then_run".into(), summarize(&changed, changed.iter().sum()));
    out.insert("change_then_run_last".into(), last);
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
