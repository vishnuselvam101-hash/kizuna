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
        let webauthn_rp_id = get("KIZUNA_WEBAUTHN_RP_ID").unwrap_or_else(|| "localhost".into());
        let webauthn_origin = get("KIZUNA_WEBAUTHN_ORIGIN").unwrap_or_else(|| "http://localhost:8080".into());
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
                Some("none") => "none".into(),
                Some(_) => return Err(ConfigError::Invalid("KIZUNA_EXTRACTION_SANDBOX")),
            },
            extraction_timeout_secs: num("KIZUNA_EXTRACTION_TIMEOUT_SECS", 45, 5, 50)? as u64,
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
