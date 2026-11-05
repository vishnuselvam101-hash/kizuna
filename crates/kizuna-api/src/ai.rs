//! AI-assisted extraction (ADR 0018). Runs in the API process, outside the extraction sandbox, only on records the
//! sandboxed worker already extracted. OFF unless the operator configures `KIZUNA_AI_*` AND the tenant's admin
//! opted in under the current data-processing notice. Any failure returns an error code; the caller then keeps the
//! deterministic `mfg-map@1` result only (fallback).

use std::time::{Duration, Instant};

use domain_manufacturing::Record;
use domain_manufacturing::ai::{self as aimap, AI_MAPPER_VERSION};
use evidence_core::claim::ClaimInput;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::config::AiConfig;
use crate::db::{self, Tx};
use crate::error::ApiResult;

const MAX_RESPONSE_BYTES: usize = 256 * 1024;
/// Concurrent AI calls per process, and at most one per tenant: a slow free provider used by one tenant cannot
/// occupy capacity other tenants need (review M5-M7 L6). Busy → immediate fallback to rules (`ai_busy`).
const MAX_CONCURRENT_AI_CALLS: usize = 8;
static AI_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(MAX_CONCURRENT_AI_CALLS);
static AI_TENANTS: std::sync::Mutex<Vec<Uuid>> = std::sync::Mutex::new(Vec::new());

struct TenantSlot(Uuid);

impl TenantSlot {
    fn take(t: Uuid) -> Option<Self> {
        let mut g = AI_TENANTS.lock().ok()?;
        if g.contains(&t) {
            return None;
        }
        g.push(t);
        Some(TenantSlot(t))
    }
}

impl Drop for TenantSlot {
    fn drop(&mut self) {
        if let Ok(mut g) = AI_TENANTS.lock() {
            g.retain(|x| *x != self.0);
        }
    }
}

pub struct Suggested {
    pub claims: Vec<ClaimInput>,
    pub model: String,
    pub prompt_sha256: [u8; 32],
    pub labels_sent: usize,
    pub mappings: usize,
    pub tokens: Value,
    pub duration_ms: u128,
    /// Mappings refused by the guards (`governance:<label>`, `implausible:<label>`).
    pub dropped: Vec<String>,
}

/// Result of one AI attempt plus the labels refused before sending (instruction-like text).
pub struct Outcome {
    pub result: Result<Option<Suggested>, &'static str>,
    pub refused: Vec<String>,
}

/// `Ok(None)`: AI not used (off, not opted in, nothing to map). `Err(code)`: AI was attempted or refused by budget;
/// the caller falls back to the rules result.
pub async fn suggest(
    pool: &sqlx::PgPool,
    cfg: Option<&AiConfig>,
    tenant: Uuid,
    artifact: Uuid,
    kind: &str,
    records: &[Record],
) -> ApiResult<Outcome> {
    let done = |result, refused| Ok(Outcome { result, refused });
    let Some(cfg) = cfg else { return done(Ok(None), vec![]) };
    let mut tx = db::tenant_tx(pool, tenant).await?;
    if !crate::tenant_settings::ai_opted_in(&mut tx).await? {
        tx.commit().await?;
        return done(Ok(None), vec![]);
    }
    let (cands, refused) = aimap::candidates(records);
    if cands.is_empty() {
        tx.commit().await?;
        return done(Ok(None), refused);
    }
    let Some(_tenant_slot) = TenantSlot::take(tenant) else {
        tx.commit().await?;
        return done(Err("ai_busy"), refused);
    };
    let Ok(_slot) = AI_SLOTS.try_acquire() else {
        tx.commit().await?;
        return done(Err("ai_busy"), refused);
    };
    let system = aimap::system_prompt();
    let user = aimap::user_prompt(kind, &cands);
    let prompt_hash = crate::crypto::sha256(format!("{system}\n{user}").as_bytes());
    let chars = (system.chars().count() + user.chars().count()) as i64;
    if let Err(code) = reserve(&mut tx, cfg, tenant, artifact, &prompt_hash, chars).await? {
        tx.commit().await?;
        return done(Err(code), refused);
    }
    tx.commit().await?;

    let started = Instant::now();
    let content = match call(cfg, &system, &user).await {
        Ok(c) => c,
        Err(code) => return done(Err(code), refused),
    };
    let duration_ms = started.elapsed().as_millis();
    let (sugg, dropped) = match aimap::parse_response(&content.0, &cands) {
        Ok(s) => s,
        Err(code) => return done(Err(code), refused),
    };
    done(
        Ok(Some(Suggested {
            claims: aimap::suggested_claims(kind, records, &sugg),
            model: cfg.model.clone(),
            prompt_sha256: prompt_hash,
            labels_sent: cands.len(),
            mappings: sugg.len(),
            tokens: content.1,
            duration_ms,
            dropped,
        })),
        refused,
    )
}

/// Per-tenant budget (calls and prompt characters per UTC day) and rate limit (calls per minute). The call is
/// recorded before it is made (under a per-tenant lock), so concurrent requests cannot overspend.
async fn reserve(
    tx: &mut Tx,
    cfg: &AiConfig,
    tenant: Uuid,
    artifact: Uuid,
    prompt_hash: &[u8; 32],
    chars: i64,
) -> ApiResult<Result<(), &'static str>> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 31))")
        .bind(tenant.to_string())
        .execute(&mut **tx)
        .await?;
    let (day_calls, day_chars, min_calls): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE occurred_at >= date_trunc('day', now())), \
                coalesce(sum(prompt_chars) FILTER (WHERE occurred_at >= date_trunc('day', now())), 0)::bigint, \
                count(*) FILTER (WHERE occurred_at > now() - interval '1 minute') \
         FROM ai_calls WHERE occurred_at > now() - interval '1 day'",
    )
    .fetch_one(&mut **tx)
    .await?;
    if min_calls >= cfg.calls_per_min {
        return Ok(Err("ai_rate_limited"));
    }
    if day_calls >= cfg.daily_calls || day_chars + chars > cfg.daily_chars {
        return Ok(Err("ai_budget_exceeded"));
    }
    sqlx::query(
        "INSERT INTO ai_calls (id, tenant_id, artifact_id, model, prompt_sha256, prompt_chars, status) \
         VALUES ($1,$2,$3,$4,$5,$6,'sent')",
    )
    .bind(Uuid::now_v7())
    .bind(tenant)
    .bind(artifact)
    .bind(&cfg.model)
    .bind(prompt_hash.to_vec())
    .bind(chars as i32)
    .execute(&mut **tx)
    .await?;
    Ok(Ok(()))
}

/// One chat-completions request. No tools are offered, redirects are not followed, proxies from the environment
/// are ignored, and the answer is size-capped. Returns the message content and the provider's token usage.
pub async fn call(cfg: &AiConfig, system: &str, user: &str) -> Result<(String, Value), &'static str> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(cfg.timeout_secs))
        .connect_timeout(Duration::from_secs(cfg.timeout_secs.min(5)))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| "ai_client")?;
    let body = json!({
        "model": cfg.model,
        "temperature": 0,
        "max_tokens": 4000,
        "response_format": { "type": "json_object" },
        "messages": [ { "role": "system", "content": system }, { "role": "user", "content": user } ],
    });
    let mut req = client.post(format!("{}/chat/completions", cfg.base_url)).json(&body);
    if let Some(k) = &cfg.api_key {
        req = req.bearer_auth(k.expose());
    }
    let mut resp = req
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "ai_timeout" } else { "ai_unreachable" })?;
    if !resp.status().is_success() {
        return Err(match resp.status().as_u16() {
            429 => "ai_provider_rate_limited",
            401 | 403 => "ai_provider_auth",
            _ => "ai_provider_error",
        });
    }
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| if e.is_timeout() { "ai_timeout" } else { "ai_unreachable" })?
    {
        buf.extend_from_slice(&chunk);
        if buf.len() > MAX_RESPONSE_BYTES {
            return Err("ai_response_too_large");
        }
    }
    let v: Value = serde_json::from_slice(&buf).map_err(|_| "ai_response_not_json")?;
    let content = v["choices"][0]["message"]["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or("ai_no_content")?;
    let usage = json!({
        "prompt_tokens": v["usage"]["prompt_tokens"].as_i64(),
        "completion_tokens": v["usage"]["completion_tokens"].as_i64(),
    });
    Ok((content.to_string(), usage))
}

pub fn method(model: &str) -> String {
    let m: String = model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "@-_.:/".contains(*c))
        .take(60)
        .collect();
    format!("ai:{m}+{AI_MAPPER_VERSION}")
}
