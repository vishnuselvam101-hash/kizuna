//! M6 benchmarks (DIRECTIVE §14). Run through `scripts/bench.sh`, which provides a throwaway database.
//!
//! kizuna-bench --out-dir reports/bench [--parts N] [--seed N]
//! Env: KIZUNA_BENCH_OWNER_URL, KIZUNA_BENCH_APP_URL (owner / runtime role URLs of an empty, throwaway database).
//!
//! Library-level: entity resolution, conflict detection. API-level (real TCP + HTTP against an in-process
//! kizuna-server on 127.0.0.1): claim writes, evidence lookup, decision trace, verification run, selective
//! invalidation, audit report + chain verification, concurrent reads. Every number written comes from this run.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use conflict_engine::{Ctx, Fact, Rule};
use kizuna_api::config::Config;
use kizuna_api::{AppState, db, router};
use kizuna_bench::{Conn, environment, memory_kib, summarize};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use uuid::Uuid;

type R<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Deterministic xorshift64* (no RNG dependency).
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

const PREFIXES: [&str; 8] = ["BRK", "SHF", "HSG", "FLG", "PIN", "GER", "BSH", "CVR"];

fn part_keys(rng: &mut Rng, n: usize) -> Vec<String> {
    let mut v: Vec<String> = Vec::with_capacity(n);
    let mut seen = std::collections::HashSet::new();
    while v.len() < n {
        let k = format!("{}-{:05}", PREFIXES[rng.below(8) as usize], rng.below(100_000));
        if seen.insert(k.clone()) {
            // ~5 % near-duplicate identifiers (separator/case variants), as found in real part lists.
            if rng.below(20) == 0 && v.len() + 1 < n {
                v.push(k.replace('-', "").to_lowercase());
            }
            v.push(k);
        }
    }
    v.truncate(n);
    v
}

fn timed<T>(f: impl FnOnce() -> T) -> (Duration, T) {
    let t = Instant::now();
    let r = f();
    (t.elapsed(), r)
}

fn bench_er(rng: &mut Rng, sizes: &[usize]) -> Value {
    let mut out = serde_json::Map::new();
    for &n in sizes {
        let keys = part_keys(rng, n);
        for _ in 0..2 {
            std::hint::black_box(entity_resolution::candidates(&keys));
        }
        let mut s = Vec::new();
        let start = Instant::now();
        let mut found = 0;
        for _ in 0..10 {
            let (d, c) = timed(|| entity_resolution::candidates(&keys));
            found = c.len();
            s.push(d);
        }
        let mut v = summarize(&s, start.elapsed());
        v["keys"] = json!(n);
        v["candidate_pairs"] = json!(found);
        v["pairs_compared_per_run"] = json!(n * (n - 1) / 2);
        out.insert(format!("candidates_{n}_keys"), v);
    }
    Value::Object(out)
}

fn facts_for(rng: &mut Rng, parts: usize) -> Vec<Fact> {
    let mut f = Vec::new();
    let mut id = 0;
    let mut push =
        |subject: &str, pred: &str, text: Option<&str>, num: Option<&str>, unit: Option<&str>, kind: &str| {
            id += 1;
            f.push(Fact {
                id: id.to_string(),
                subject: subject.into(),
                predicate: pred.into(),
                text: text.map(Into::into),
                num: num.and_then(|n| Decimal::from_str(n).ok()),
                unit: unit.map(Into::into),
                revision: Some("C".into()),
                scope: BTreeMap::from([("supplier".to_string(), "ACME".to_string())]),
                source_kind: kind.into(),
            });
        };
    for i in 0..parts {
        let s = format!("P-{i:06}");
        let fault = rng.below(10) == 0;
        push(&s, "mfg.revision", Some("C"), None, None, "spec_document");
        push(
            &s,
            "mfg.dimension.length",
            None,
            Some("120"),
            Some("mm"),
            "spec_document",
        );
        push(
            &s,
            "mfg.tolerance.length.plus",
            None,
            Some("0.2"),
            Some("mm"),
            "spec_document",
        );
        push(
            &s,
            "mfg.tolerance.length.minus",
            None,
            Some("0.2"),
            Some("mm"),
            "spec_document",
        );
        push(&s, "mfg.approval.status", Some("approved"), None, None, "spec_document");
        push(
            &s,
            "mfg.certificate.expiry",
            Some("2031-01-01"),
            None,
            None,
            "spec_document",
        );
        push(&s, "mfg.material", Some("S355JR"), None, None, "spec_document");
        let drawing_len = if fault { "125" } else { "120" };
        push(
            &s,
            "mfg.dimension.length",
            None,
            Some(drawing_len),
            Some("mm"),
            "drawing",
        );
        push(&s, "po.revision_ordered", Some("C"), None, None, "purchase_order");
    }
    f
}

fn bench_conflicts(rng: &mut Rng, sizes: &[usize]) -> Value {
    let rules = domain_manufacturing::rules::all_rules();
    let refs: Vec<&dyn Rule> = rules.iter().map(|r| r.as_ref()).collect();
    let ctx = Ctx {
        today: "2026-11-05".into(),
    };
    let mut out = serde_json::Map::new();
    for &n in sizes {
        let facts = facts_for(rng, n);
        for _ in 0..2 {
            std::hint::black_box(conflict_engine::run(&refs, &facts, &ctx));
        }
        let mut s = Vec::new();
        let mut findings = 0;
        let start = Instant::now();
        for _ in 0..20 {
            let (d, r) = timed(|| conflict_engine::run(&refs, &facts, &ctx));
            findings = r.len();
            s.push(d);
        }
        let mut v = summarize(&s, start.elapsed());
        v["parts"] = json!(n);
        v["facts"] = json!(facts.len());
        v["findings"] = json!(findings);
        out.insert(format!("run_{n}_parts"), v);
    }
    Value::Object(out)
}

struct Api {
    addr: SocketAddr,
    tok: String,
    auditor: String,
}

async fn series(
    conn: &mut Conn,
    n: usize,
    mut req: impl FnMut(usize) -> (String, String, Option<Value>),
    tok: &str,
) -> R<(Value, Vec<Value>)> {
    let mut s = Vec::with_capacity(n);
    let mut bodies = Vec::new();
    let mut errors = 0;
    let start = Instant::now();
    for i in 0..n {
        let (m, p, b) = req(i);
        let t = Instant::now();
        let (st, body) = conn.request(&m, &p, Some(tok), b.as_ref()).await?;
        s.push(t.elapsed());
        if st != 200 {
            errors += 1;
            if errors == 1 {
                eprintln!("{m} {p} -> {st} {body}");
            }
        }
        bodies.push(body);
    }
    let mut v = summarize(&s, start.elapsed());
    v["errors"] = json!(errors);
    Ok((v, bodies))
}

async fn concurrent(api: &Api, conc: usize, total: usize, keys: Arc<Vec<String>>) -> R<Value> {
    let start = Instant::now();
    let per = total / conc;
    let mut hs = Vec::new();
    for w in 0..conc {
        let (addr, tok, keys) = (api.addr, api.tok.clone(), Arc::clone(&keys));
        hs.push(tokio::spawn(async move {
            let mut c = Conn::connect(addr).await?;
            let mut s = Vec::with_capacity(per);
            let mut errors = 0usize;
            for i in 0..per {
                let k = &keys[(w * 7919 + i * 31) % keys.len()];
                let t = Instant::now();
                let (st, _) = c
                    .request("GET", &format!("/api/v1/evidence/part/{k}"), Some(&tok), None)
                    .await?;
                s.push(t.elapsed());
                errors += usize::from(st != 200);
            }
            Ok::<_, std::io::Error>((s, errors))
        }));
    }
    let mut all = Vec::new();
    let mut errors = 0;
    for h in hs {
        let (s, e) = h.await??;
        all.extend(s);
        errors += e;
    }
    let mut v = summarize(&all, start.elapsed());
    v["concurrency"] = json!(conc);
    v["errors"] = json!(errors);
    Ok(v)
}

#[tokio::main]
async fn main() -> R<()> {
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let out_dir = arg("--out-dir").unwrap_or_else(|| "reports/bench".into());
    let parts: usize = arg("--parts").map(|v| v.parse()).transpose()?.unwrap_or(300);
    let seed: u64 = arg("--seed").map(|v| v.parse()).transpose()?.unwrap_or(7);
    let owner_url = std::env::var("KIZUNA_BENCH_OWNER_URL")?;
    let app_url = std::env::var("KIZUNA_BENCH_APP_URL")?;
    let env = environment();
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut res = serde_json::Map::new();
    let mut mem = serde_json::Map::new();

    eprintln!("[1/9] entity resolution");
    res.insert("entity_resolution".into(), bench_er(&mut rng, &[500, 2000, 5000]));
    eprintln!("[2/9] conflict detection (library)");
    res.insert(
        "conflict_detection_library".into(),
        bench_conflicts(&mut rng, &[1000, 10_000]),
    );
    mem.insert("after_library".into(), memory_kib());

    // Server against the throwaway database.
    let owner = db::connect(&owner_url, 2).await?;
    db::migrator(&db::Roles::from_env()?).run(&owner).await?;
    let pg_version: String = sqlx::query_scalar("SELECT version()").fetch_one(&owner).await?;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1,'bench','Bench')")
        .bind(tenant)
        .execute(&owner)
        .await?;
    let pw = hex::encode(kizuna_api::crypto::random_bytes::<16>());
    let mut tx = db::tenant_tx(&owner, tenant).await?;
    for (email, role) in [("rev@bench.test", "reviewer"), ("aud@bench.test", "auditor")] {
        kizuna_api::users::insert_user(
            &mut tx,
            tenant,
            email,
            "Bench",
            policy_engine::Role::parse(role).map_err(|_| "role")?,
            &pw,
        )
        .await
        .map_err(|e| e.to_string())?;
    }
    tx.commit().await?;
    let storage = std::env::temp_dir().join(format!("kizuna-bench-{}", Uuid::new_v4()));
    let key = base64::engine::general_purpose::STANDARD.encode(kizuna_api::crypto::random_bytes::<32>());
    let cfg = Config::from_lookup(|k| match k {
        "KIZUNA_DATABASE_URL" => Some(app_url.clone()),
        "KIZUNA_DEV" => Some("1".into()),
        "KIZUNA_DATA_KEY" => Some(key.clone()),
        "KIZUNA_STORAGE_DIR" => Some(storage.to_string_lossy().into_owned()),
        "KIZUNA_ACCESS_TTL_SECS" => Some("3600".into()),
        _ => None,
    })?;
    let pool = db::connect(&app_url, 16).await?;
    db::assert_runtime_role(&pool).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let app = router(AppState::new(pool, cfg));
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await });
    let mut c = Conn::connect(addr).await?;
    let login = |email: &'static str| json!({ "tenant": "bench", "email": email, "password": pw.clone() });
    let (_, b) = c
        .request("POST", "/api/v1/auth/login", None, Some(&login("rev@bench.test")))
        .await?;
    let tok = b["access_token"].as_str().ok_or("login failed")?.to_string();
    let (_, b) = c
        .request("POST", "/api/v1/auth/login", None, Some(&login("aud@bench.test")))
        .await?;
    let auditor = b["access_token"].as_str().ok_or("login failed")?.to_string();
    let api = Api { addr, tok, auditor };

    eprintln!("[3/9] seeding {parts} parts via the API (claim writes)");
    let keys = part_keys(&mut rng, parts);
    let mut claims: Vec<(String, String)> = Vec::new(); // (length claim, approval claim) per part
    let mut writes = Vec::new();
    let start = Instant::now();
    for (i, k) in keys.iter().enumerate() {
        let len = 40 + (i % 300);
        let bodies = [
            json!({"subject_type":"part","subject_key":k,"predicate":"mfg.revision","value_text":"C","revision":"C","scope":{"supplier":"ACME"}}),
            json!({"subject_type":"part","subject_key":k,"predicate":"mfg.dimension.length","value_num":len.to_string(),"value_unit":"mm","revision":"C","scope":{"supplier":"ACME"}}),
            json!({"subject_type":"part","subject_key":k,"predicate":"mfg.approval.status","value_text":"approved","revision":"C","scope":{"supplier":"ACME"}}),
            json!({"subject_type":"part","subject_key":k,"predicate":"mfg.material","value_text":if i % 10 == 0 {"S275JR"} else {"S355JR"},"revision":"C","scope":{"supplier":"ACME"}}),
            json!({"subject_type":"part","subject_key":k,"predicate":"mfg.material","value_text":"S355JR","revision":"C","scope":{"supplier":"ACME"}}),
        ];
        let mut ids = Vec::new();
        for b in &bodies {
            let t = Instant::now();
            let (st, r) = c.request("POST", "/api/v1/claims", Some(&api.tok), Some(b)).await?;
            writes.push(t.elapsed());
            if st != 200 {
                return Err(format!("claim create {st}: {r}").into());
            }
            ids.push(r["id"].as_str().unwrap_or_default().to_string());
        }
        claims.push((ids[1].clone(), ids[2].clone()));
    }
    res.insert("api_claim_write".into(), summarize(&writes, start.elapsed()));

    // Decisions: one per part; every 10th also depends on the previous part's decision (propagation chains).
    let mut decisions: Vec<String> = Vec::new();
    let mut dw = Vec::new();
    let start = Instant::now();
    for (i, k) in keys.iter().enumerate() {
        let deps = if i % 10 == 9 {
            json!([{"decision_id": decisions[i - 1]}])
        } else {
            json!([])
        };
        let body = json!({"kind":"supplier_part_release","title":format!("Release {k}"),"subject_type":"part","subject_key":k,
            "outcome":"approved","rationale":"bench","depends_on":deps,
            "evidence":[{"claim_id":claims[i].0,"materiality":{"type":"numeric_abs","threshold":"0.05"}},{"claim_id":claims[i].1}]});
        let t = Instant::now();
        let (st, r) = c
            .request("POST", "/api/v1/decisions", Some(&api.tok), Some(&body))
            .await?;
        dw.push(t.elapsed());
        if st != 200 {
            return Err(format!("decision create {st}: {r}").into());
        }
        decisions.push(r["id"].as_str().unwrap_or_default().to_string());
    }
    res.insert("api_decision_create".into(), summarize(&dw, start.elapsed()));
    mem.insert("after_seed".into(), memory_kib());

    eprintln!("[4/9] evidence lookup");
    let ks = keys.clone();
    let (v, _) = series(
        &mut c,
        1000,
        |i| {
            (
                "GET".into(),
                format!("/api/v1/evidence/part/{}", ks[(i * 37) % ks.len()]),
                None,
            )
        },
        &api.tok,
    )
    .await?;
    res.insert("evidence_lookup".into(), v);

    eprintln!("[5/9] conflict detection (API, whole tenant)");
    let (first, b) = series(
        &mut c,
        1,
        |_| ("POST".into(), "/api/v1/verification/run".into(), Some(json!({}))),
        &api.tok,
    )
    .await?;
    let (steady, _) = series(
        &mut c,
        10,
        |_| ("POST".into(), "/api/v1/verification/run".into(), Some(json!({}))),
        &api.tok,
    )
    .await?;
    res.insert("conflict_detection_api".into(), json!({"claims_in_tenant": parts * 5, "first_run": first, "first_run_result": b[0], "rerun_idempotent": steady}));

    eprintln!("[6/9] decision trace");
    let ds = decisions.clone();
    let (v, _) = series(
        &mut c,
        500,
        |i| {
            (
                "GET".into(),
                format!("/api/v1/decisions/{}/trace", ds[(i * 13) % ds.len()]),
                None,
            )
        },
        &api.tok,
    )
    .await?;
    res.insert("decision_trace".into(), v);

    eprintln!("[7/9] selective invalidation (material supersede of a claim with dependants)");
    let n_inv = parts.min(200);
    let (v, _) = series(&mut c, n_inv, |i| {
        let body = json!({"subject_type":"part","subject_key":keys[i],"predicate":"mfg.dimension.length","value_num":(40 + (i % 300) + 1).to_string(),
            "value_unit":"mm","revision":"C","scope":{"supplier":"ACME"},"reason":"bench correction"});
        ("POST".into(), format!("/api/v1/claims/{}/supersede", claims[i].0), Some(body))
    }, &api.tok).await?;
    let mut affected = 0;
    for d in decisions.iter().take(n_inv) {
        let (_, b) = c
            .request("GET", &format!("/api/v1/decisions/{d}"), Some(&api.tok), None)
            .await?;
        affected += usize::from(b["decision"]["status"] == "potentially_affected");
    }
    let mut v = v;
    v["decisions_flagged"] = json!(affected);
    v["decisions_expected_flagged"] = json!(n_inv);
    res.insert("selective_invalidation".into(), v);

    eprintln!("[8/9] audit generation");
    let (rep, _) = series(
        &mut c,
        300,
        |i| {
            (
                "GET".into(),
                format!("/api/v1/decisions/{}/report", ds[(i * 7) % ds.len()]),
                None,
            )
        },
        &api.auditor,
    )
    .await?;
    let (ver, b) = series(
        &mut c,
        10,
        |_| ("GET".into(), "/api/v1/audit/verify".into(), None),
        &api.auditor,
    )
    .await?;
    res.insert(
        "audit_generation".into(),
        json!({"verification_report": rep, "audit_chain_verify": ver, "chain": b[0]}),
    );
    mem.insert("after_api_series".into(), memory_kib());

    eprintln!("[9/9] concurrent API (GET evidence, keep-alive connections)");
    let keys = Arc::new(keys);
    let mut conc = serde_json::Map::new();
    for n in [1usize, 8, 32, 64] {
        conc.insert(format!("c{n}"), concurrent(&api, n, 3200, Arc::clone(&keys)).await?);
    }
    res.insert("concurrent_api".into(), Value::Object(conc));
    mem.insert("end".into(), memory_kib());
    let _ = std::fs::remove_dir_all(&storage);

    let utc = time::OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}T{:02}{:02}Z",
        utc.year(),
        utc.month() as u8,
        utc.day(),
        utc.hour(),
        utc.minute()
    );
    let mut env = env;
    env["postgres"] = json!(pg_version);
    let doc = json!({
        "command": format!("scripts/bench.sh --parts {parts} --seed {seed}"),
        "utc": stamp, "environment": env, "parameters": {"parts": parts, "seed": seed},
        "notes": [
            "MEASURED on this host; client and server share one process and one tokio runtime (loopback TCP, real HTTP/1.1).",
            "Memory is the bench process (server + client + library benches), not the server alone.",
            "PostgreSQL runs on the same host; numbers include its latency.",
        ],
        "results": res, "memory_kib": mem,
    });
    std::fs::create_dir_all(&out_dir)?;
    let base = format!("{out_dir}/bench-{stamp}");
    std::fs::write(format!("{base}.json"), serde_json::to_string_pretty(&doc)?)?;
    std::fs::write(format!("{base}.md"), markdown(&doc))?;
    println!("{base}.json\n{base}.md");
    Ok(())
}

fn row(name: &str, v: &Value) -> String {
    format!(
        "| {name} | {} | {} | {} | {} | {} | {} |\n",
        v["n"],
        v["p50_ms"],
        v["p95_ms"],
        v["p99_ms"],
        v["throughput_per_s"],
        v.get("errors").map_or("-".into(), |e| e.to_string())
    )
}

fn markdown(d: &Value) -> String {
    let r = &d["results"];
    let mut s = format!(
        "# KIZUNA benchmark run {}\n\nCommand: `{}`  \nEnvironment: `{}`\n\nAll numbers MEASURED in this run (see JSON for full data).\n\n",
        d["utc"].as_str().unwrap_or(""),
        d["command"].as_str().unwrap_or(""),
        d["environment"]
    );
    s.push_str(
        "| benchmark | n | P50 ms | P95 ms | P99 ms | throughput /s | errors |\n|---|---|---|---|---|---|---|\n",
    );
    for (k, v) in r["entity_resolution"].as_object().into_iter().flatten() {
        s.push_str(&row(&format!("ER {k} ({} pairs)", v["candidate_pairs"]), v));
    }
    for (k, v) in r["conflict_detection_library"].as_object().into_iter().flatten() {
        s.push_str(&row(&format!("conflict rules {k} ({} facts)", v["facts"]), v));
    }
    s.push_str(&row("API claim write", &r["api_claim_write"]));
    s.push_str(&row("API decision create", &r["api_decision_create"]));
    s.push_str(&row("API evidence lookup", &r["evidence_lookup"]));
    s.push_str(&row(
        "API verification run (first)",
        &r["conflict_detection_api"]["first_run"],
    ));
    s.push_str(&row(
        "API verification run (re-run)",
        &r["conflict_detection_api"]["rerun_idempotent"],
    ));
    s.push_str(&row("API decision trace", &r["decision_trace"]));
    s.push_str(&row(
        "API selective invalidation (supersede)",
        &r["selective_invalidation"],
    ));
    s.push_str(&row(
        "API verification report",
        &r["audit_generation"]["verification_report"],
    ));
    s.push_str(&row(
        "API audit chain verify",
        &r["audit_generation"]["audit_chain_verify"],
    ));
    for (k, v) in r["concurrent_api"].as_object().into_iter().flatten() {
        s.push_str(&row(&format!("concurrent evidence GET {k}"), v));
    }
    s.push_str(&format!(
        "\nSelective invalidation correctness: {} of {} dependent decisions flagged.  \nAudit chain: {}  \nMemory (KiB): {}\n\nNotes:\n",
        r["selective_invalidation"]["decisions_flagged"],
        r["selective_invalidation"]["decisions_expected_flagged"],
        r["audit_generation"]["chain"],
        d["memory_kib"]
    ));
    for n in d["notes"].as_array().into_iter().flatten() {
        s.push_str(&format!("- {}\n", n.as_str().unwrap_or("")));
    }
    s
}
