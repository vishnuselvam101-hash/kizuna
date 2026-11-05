//! Configuration from environment variables. Secrets are wrapped so they never appear in Debug output or logs.

use std::fmt;
use std::net::SocketAddr;

use crate::keyring::DataKeys;

/// A secret value whose Debug/Display output is redacted.
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(v: T) -> Self {
        Self(v)
    }
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for {0}")]
    Invalid(&'static str),
}

#[derive(Debug, Clone)]
pub struct RateLimits {
    pub login_per_ip_per_min: u32,
    pub login_per_account_per_min: u32,
    pub refresh_per_ip_per_min: u32,
    pub uploads_per_user_per_min: u32,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub database_url: Secret<String>,
    pub bind: SocketAddr,
    /// Versioned 32-byte keys for secrets at rest (TOTP seeds, WebAuthn credentials); see `keyring`.
    pub data_keys: DataKeys,
    /// WebAuthn relying party: id (registrable domain) and the exact browser origin.
    pub webauthn_rp_id: String,
    pub webauthn_origin: String,
    pub access_ttl_secs: i64,
    pub session_ttl_secs: i64,
    /// Inactivity timeout: a session unused for this long is revoked (ASVS V7.3.1; deep review KZ-A1).
    pub idle_ttl_secs: i64,
    /// Privileged operations need a login (password + second factor) at most this old (KZ-A3).
    pub reauth_ttl_secs: i64,
    pub max_body_bytes: usize,
    pub max_upload_bytes: usize,
    /// Number of trusted reverse proxies in front of the server (0 = ignore X-Forwarded-For).
    pub trusted_proxy_hops: usize,
    pub lockout_threshold: i32,
    pub lockout_secs: i64,
    pub rate: RateLimits,
    pub storage_dir: std::path::PathBuf,
    pub extraction_python: String,
    pub extraction_dir: std::path::PathBuf,
    /// `bwrap` (default: no network, read-only minimal filesystem) or `none` (rlimits only; development only).
    pub extraction_sandbox: String,
    pub extraction_timeout_secs: u64,
    /// Address-space limit per extraction worker in MiB (`KIZUNA_WORKER_MEMORY_MB`, default 1024).
    pub worker_memory_mb: u64,
    /// Entity-resolution matcher (KIZUNA_ER_VERSION=v1|v2, default v2); a request may override it with ?version=.
    pub er_version: entity_resolution::ErVersion,
    /// Ed25519 seed for signed audit checkpoints (keep outside the database).
    pub audit_signing_key: Option<Secret<[u8; 32]>>,
    /// Retired audit signing keys (comma-separated in `KIZUNA_AUDIT_SIGNING_KEYS_PREVIOUS`): old checkpoints stay
    /// verifiable by their stored key id after a rotation.
    pub audit_signing_keys_previous: Vec<Secret<[u8; 32]>>,
    /// External AI label mapping (OFF unless configured; tenants must still opt in). See ADR 0018.
    pub ai: Option<AiConfig>,
}

/// OpenAI-compatible chat-completions endpoint for AI-assisted extraction.
#[derive(Debug, Clone)]
pub struct AiConfig {
    /// Base URL ending before `/chat/completions`, e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: String,
    pub model: String,
    pub api_key: Option<Secret<String>>,
    pub timeout_secs: u64,
    /// Per-tenant budget: calls per UTC day and prompt characters per UTC day.
    pub daily_calls: i64,
    pub daily_chars: i64,
    /// Per-tenant rate limit: calls per minute.
    pub calls_per_min: i64,
}

/// Accepts `https://host[:port]/path`, or `http://` only for loopback hosts. No credentials, query or fragment.
pub fn valid_ai_base_url(u: &str) -> bool {
    let (scheme, rest) = match u.split_once("://") {
        Some(x) => x,
        None => return false,
    };
    if rest.is_empty() || rest.contains(['@', '?', '#', ' ', '\\']) || !u.is_ascii() {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or("");
    let host = if let Some(h) = authority.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return false;
    }
    let loopback = host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback());
    scheme == "https" || (scheme == "http" && loopback)
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let req = |k: &'static str| get(k).filter(|v| !v.is_empty()).ok_or(ConfigError::Missing(k));
        let num = |k: &'static str, d: i64, lo: i64, hi: i64| -> Result<i64, ConfigError> {
            match get(k) {
                None => Ok(d),
                Some(v) => v
                    .parse::<i64>()
                    .ok()
                    .filter(|n| (lo..=hi).contains(n))
                    .ok_or(ConfigError::Invalid(k)),
            }
        };
        let bind = get("KIZUNA_BIND")
            .unwrap_or_else(|| "127.0.0.1:8080".into())
            .parse()
            .map_err(|_| ConfigError::Invalid("KIZUNA_BIND"))?;
        let access_ttl_secs = num("KIZUNA_ACCESS_TTL_SECS", 900, 60, 3600)?;
        let session_ttl_secs = num("KIZUNA_SESSION_TTL_SECS", 12 * 3600, 900, 7 * 24 * 3600)?;
        if session_ttl_secs < access_ttl_secs {
            return Err(ConfigError::Invalid("KIZUNA_SESSION_TTL_SECS"));
        }
        // Outside development both must be set explicitly: a silent localhost default fails every ceremony in
        // production without saying why (deep review KZ-I2).
        let dev = get("KIZUNA_DEV").as_deref() == Some("1");
        let webauthn_rp_id = match get("KIZUNA_WEBAUTHN_RP_ID").filter(|v| !v.is_empty()) {
            Some(v) => v,
            None if dev => "localhost".into(),
            None => return Err(ConfigError::Missing("KIZUNA_WEBAUTHN_RP_ID")),
        };
        let webauthn_origin = match get("KIZUNA_WEBAUTHN_ORIGIN").filter(|v| !v.is_empty()) {
            Some(v) => v,
            None if dev => "http://localhost:8080".into(),
            None => return Err(ConfigError::Missing("KIZUNA_WEBAUTHN_ORIGIN")),
        };
        crate::webauthn::build(&webauthn_rp_id, &webauthn_origin)
            .map_err(|_| ConfigError::Invalid("KIZUNA_WEBAUTHN_ORIGIN"))?;
        let here = std::env::current_dir().unwrap_or_default();
        Ok(Config {
            database_url: Secret::new(req("KIZUNA_DATABASE_URL")?),
            bind,
            data_keys: DataKeys::from_lookup(&get)?,
            webauthn_rp_id,
            webauthn_origin,
            access_ttl_secs,
            session_ttl_secs,
            idle_ttl_secs: num("KIZUNA_IDLE_TTL_SECS", 1800, 300, 24 * 3600)?,
            reauth_ttl_secs: num("KIZUNA_REAUTH_TTL_SECS", 900, 60, 24 * 3600)?,
            max_body_bytes: num("KIZUNA_MAX_BODY_BYTES", 64 * 1024, 1024, 1024 * 1024)? as usize,
            max_upload_bytes: num("KIZUNA_MAX_UPLOAD_BYTES", 25 * 1024 * 1024, 1024, 200 * 1024 * 1024)? as usize,
            trusted_proxy_hops: num("KIZUNA_TRUSTED_PROXY_HOPS", 0, 0, 5)? as usize,
            lockout_threshold: num("KIZUNA_LOCKOUT_THRESHOLD", 5, 3, 20)? as i32,
            lockout_secs: num("KIZUNA_LOCKOUT_SECS", 900, 60, 86400)?,
            rate: RateLimits {
                login_per_ip_per_min: num("KIZUNA_LOGIN_PER_IP_PER_MIN", 20, 1, 10_000)? as u32,
                login_per_account_per_min: num("KIZUNA_LOGIN_PER_ACCOUNT_PER_MIN", 10, 1, 10_000)? as u32,
                refresh_per_ip_per_min: num("KIZUNA_REFRESH_PER_IP_PER_MIN", 60, 1, 10_000)? as u32,
                uploads_per_user_per_min: num("KIZUNA_UPLOADS_PER_USER_PER_MIN", 30, 1, 10_000)? as u32,
            },
            storage_dir: get("KIZUNA_STORAGE_DIR")
                .map(Into::into)
                .unwrap_or_else(|| here.join("var/objects")),
            extraction_python: get("KIZUNA_EXTRACTION_PYTHON").unwrap_or_else(|| {
                let venv = here.join("services/extraction/.venv/bin/python");
                if venv.exists() {
                    venv.to_string_lossy().into_owned()
                } else {
                    "/usr/bin/python3".into()
                }
            }),
            extraction_dir: get("KIZUNA_EXTRACTION_DIR")
                .map(Into::into)
                .unwrap_or_else(|| here.join("services/extraction/src")),
            extraction_sandbox: match get("KIZUNA_EXTRACTION_SANDBOX").as_deref() {
                None | Some("bwrap") => "bwrap".into(),
                Some("none") if get("KIZUNA_DEV").as_deref() == Some("1") => "none".into(),
                Some(_) => return Err(ConfigError::Invalid("KIZUNA_EXTRACTION_SANDBOX")),
            },
            audit_signing_key: match get("KIZUNA_AUDIT_SIGNING_KEY").filter(|v| !v.is_empty()) {
                None => None,
                Some(v) => Some(Secret::new(crate::keyring::parse_key(&v, "KIZUNA_AUDIT_SIGNING_KEY")?)),
            },
            audit_signing_keys_previous: get("KIZUNA_AUDIT_SIGNING_KEYS_PREVIOUS")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(|v| crate::keyring::parse_key(v, "KIZUNA_AUDIT_SIGNING_KEYS_PREVIOUS").map(Secret::new))
                .collect::<Result<_, _>>()?,
            extraction_timeout_secs: num("KIZUNA_EXTRACTION_TIMEOUT_SECS", 45, 5, 50)? as u64,
            worker_memory_mb: num("KIZUNA_WORKER_MEMORY_MB", 1024, 256, 4096)? as u64,
            er_version: match get("KIZUNA_ER_VERSION") {
                None => entity_resolution::ErVersion::default(),
                Some(v) => entity_resolution::ErVersion::parse(&v).ok_or(ConfigError::Invalid("KIZUNA_ER_VERSION"))?,
            },
            ai: match (
                get("KIZUNA_AI_BASE_URL").filter(|v| !v.is_empty()),
                get("KIZUNA_AI_MODEL"),
            ) {
                (None, _) => None,
                (Some(base), model) => {
                    let base = base.trim_end_matches('/').to_string();
                    if !valid_ai_base_url(&base) {
                        return Err(ConfigError::Invalid("KIZUNA_AI_BASE_URL"));
                    }
                    let model = model
                        .filter(|m| !m.is_empty() && m.len() <= 100 && m.chars().all(|c| c.is_ascii_graphic()))
                        .ok_or(ConfigError::Invalid("KIZUNA_AI_MODEL"))?;
                    let timeout_secs = num("KIZUNA_AI_TIMEOUT_SECS", 12, 1, 30)? as u64;
                    // The whole extract request (worker + AI) must finish inside the 60 s request timeout.
                    if timeout_secs + num("KIZUNA_EXTRACTION_TIMEOUT_SECS", 45, 5, 50)? as u64 > 58 {
                        return Err(ConfigError::Invalid("KIZUNA_AI_TIMEOUT_SECS"));
                    }
                    Some(AiConfig {
                        base_url: base,
                        model,
                        api_key: get("KIZUNA_AI_API_KEY").filter(|v| !v.is_empty()).map(Secret::new),
                        timeout_secs,
                        daily_calls: num("KIZUNA_AI_DAILY_CALLS", 50, 1, 100_000)?,
                        daily_chars: num("KIZUNA_AI_DAILY_CHARS", 200_000, 1000, 100_000_000)?,
                        calls_per_min: num("KIZUNA_AI_CALLS_PER_MIN", 5, 1, 1000)?,
                    })
                }
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use std::collections::HashMap;

    fn base() -> HashMap<&'static str, String> {
        HashMap::from([
            ("KIZUNA_WEBAUTHN_RP_ID", "localhost".to_string()),
            ("KIZUNA_WEBAUTHN_ORIGIN", "http://localhost:8080".to_string()),
            ("KIZUNA_DATABASE_URL", "postgres://u:supersecretpw@h/db".to_string()),
            (
                "KIZUNA_DATA_KEY",
                base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
            ),
        ])
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let m = base();
        let c = Config::from_lookup(|k| m.get(k).cloned()).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("supersecretpw"));
        assert!(dbg.contains("REDACTED"));
        assert_eq!(c.bind.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn regression_review_l3_sandbox_none_needs_dev_flag() {
        let mut m = base();
        m.insert("KIZUNA_EXTRACTION_SANDBOX", "none".into());
        assert!(Config::from_lookup(|k| m.get(k).cloned()).is_err());
        m.insert("KIZUNA_DEV", "1".into());
        assert_eq!(
            Config::from_lookup(|k| m.get(k).cloned()).unwrap().extraction_sandbox,
            "none"
        );
    }

    #[test]
    fn webauthn_relying_party_required_outside_dev() {
        let mut m = base();
        m.remove("KIZUNA_WEBAUTHN_RP_ID");
        assert!(matches!(
            Config::from_lookup(|k| m.get(k).cloned()),
            Err(ConfigError::Missing("KIZUNA_WEBAUTHN_RP_ID"))
        ));
        m.insert("KIZUNA_DEV", "1".into());
        assert_eq!(
            Config::from_lookup(|k| m.get(k).cloned()).unwrap().webauthn_rp_id,
            "localhost"
        );
    }

    #[test]
    fn ai_is_off_by_default_and_validated() {
        let mut m = base();
        let c = Config::from_lookup(|k| m.get(k).cloned()).unwrap();
        assert!(c.ai.is_none());
        m.insert("KIZUNA_AI_BASE_URL", "http://127.0.0.1:11434/v1/".into());
        assert!(Config::from_lookup(|k| m.get(k).cloned()).is_err(), "model required");
        m.insert("KIZUNA_AI_MODEL", "gemma4:cloud".into());
        m.insert("KIZUNA_AI_API_KEY", "sk-topsecretkey".into());
        let c = Config::from_lookup(|k| m.get(k).cloned()).unwrap();
        let ai = c.ai.clone().unwrap();
        assert_eq!(ai.base_url, "http://127.0.0.1:11434/v1");
        assert!(!format!("{c:?}").contains("topsecretkey"));
        for bad in [
            "http://example.com/v1",
            "ftp://127.0.0.1/",
            "https://user:pw@example.com/v1",
            "https://example.com/v1?x=1",
            "https://",
            "http://127.0.0.1.evil.com/v1",
        ] {
            assert!(!valid_ai_base_url(bad), "{bad}");
        }
        assert!(valid_ai_base_url("https://integrate.api.nvidia.com/v1"));
        assert!(valid_ai_base_url("http://[::1]:8080/v1"));
        m.insert("KIZUNA_AI_TIMEOUT_SECS", "20".into());
        assert!(Config::from_lookup(|k| m.get(k).cloned()).is_err(), "45 + 20 > 58");
    }

    #[test]
    fn rejects_missing_and_invalid() {
        let mut m = base();
        m.remove("KIZUNA_DATA_KEY");
        assert!(matches!(
            Config::from_lookup(|k| m.get(k).cloned()),
            Err(ConfigError::Missing("KIZUNA_DATA_KEY"))
        ));
        let mut m = base();
        m.insert("KIZUNA_DATA_KEY", "c2hvcnQ=".into());
        assert!(Config::from_lookup(|k| m.get(k).cloned()).is_err());
        let mut m = base();
        m.insert("KIZUNA_ACCESS_TTL_SECS", "999999".into());
        assert!(Config::from_lookup(|k| m.get(k).cloned()).is_err());
        let mut m = base();
        m.insert("KIZUNA_WEBAUTHN_ORIGIN", "not a url".into());
        assert!(Config::from_lookup(|k| m.get(k).cloned()).is_err());
        let mut m = base();
        m.insert("KIZUNA_WEBAUTHN_ORIGIN", "https://evil.example".into());
        assert!(
            Config::from_lookup(|k| m.get(k).cloned()).is_err(),
            "origin must match rp id"
        );
    }
}
