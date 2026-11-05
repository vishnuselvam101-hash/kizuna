//! Malware scanning behind a small trait (ADR 0017). The only engine is ClamAV's `clamd` over its local Unix socket
//! (INSTREAM protocol); the static structural scan (`evidence_core::scan`, ADR 0011) always runs first.
//!
//! Policy: configured but unreachable, timed out, over the stream limit or answering garbage → the upload is
//! quarantined (fail closed), never accepted. Not configured → uploads keep the static verdict and every report and
//! the status endpoint say plainly that no antivirus ran.
//!
//! Env: `KIZUNA_CLAMD_SOCKET` (e.g. /run/clamav/clamd.ctl; unset = not configured), `KIZUNA_CLAMD_TIMEOUT_SECS`
//! (default 30, 1..=300), `KIZUNA_CLAMD_MAX_BYTES` (default 25 MiB = clamd's default StreamMaxLength).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Clean,
    Infected(String),
    /// Larger than the configured stream limit: not scanned.
    TooLarge,
    /// Scanner configured but not usable (connection, timeout, protocol or clamd error).
    Unavailable(String),
    NotConfigured,
}

impl Outcome {
    /// Finding added to the scan report; `None` means the static verdict stands.
    pub fn finding(&self) -> Option<String> {
        match self {
            Outcome::Clean | Outcome::NotConfigured => None,
            Outcome::Infected(sig) => Some(format!("av_infected:{sig}")),
            Outcome::TooLarge => Some("av_not_scanned_too_large".into()),
            Outcome::Unavailable(_) => Some("av_unavailable".into()),
        }
    }

    pub fn to_json(&self, engine: &str) -> Value {
        match self {
            Outcome::Clean => json!({ "engine": engine, "result": "clean" }),
            Outcome::Infected(sig) => json!({ "engine": engine, "result": "infected", "signature": sig }),
            Outcome::TooLarge => json!({ "engine": engine, "result": "not_scanned_too_large" }),
            Outcome::Unavailable(why) => json!({ "engine": engine, "result": "unavailable", "detail": why }),
            Outcome::NotConfigured => json!({ "engine": null, "result": "not_configured" }),
        }
    }
}

pub trait MalwareScanner: Send + Sync {
    fn engine(&self) -> &'static str;
    fn configured(&self) -> bool;
    /// Blocking; call from `spawn_blocking`.
    fn scan(&self, data: &[u8]) -> Outcome;
    /// Blocking health probe: engine version string, or why it is unusable.
    fn probe(&self) -> Result<String, String>;
}

pub struct NotConfigured;

impl MalwareScanner for NotConfigured {
    fn engine(&self) -> &'static str {
        "none"
    }
    fn configured(&self) -> bool {
        false
    }
    fn scan(&self, _: &[u8]) -> Outcome {
        Outcome::NotConfigured
    }
    fn probe(&self) -> Result<String, String> {
        Err("no antivirus configured (KIZUNA_CLAMD_SOCKET unset): uploads get the static scan only".into())
    }
}

#[derive(Debug, Clone)]
pub struct Clamd {
    pub socket: PathBuf,
    pub timeout: Duration,
    pub max_bytes: usize,
}

const CHUNK: usize = 64 * 1024;
const MAX_REPLY: u64 = 4096;

impl Clamd {
    fn connect(&self) -> Result<UnixStream, String> {
        let s = UnixStream::connect(&self.socket).map_err(|e| format!("connect: {}", e.kind()))?;
        s.set_read_timeout(Some(self.timeout)).map_err(|e| e.to_string())?;
        s.set_write_timeout(Some(self.timeout)).map_err(|e| e.to_string())?;
        Ok(s)
    }

    fn reply(s: &mut UnixStream) -> Result<String, String> {
        let mut buf = Vec::new();
        s.take(MAX_REPLY)
            .read_to_end(&mut buf)
            .map_err(|e| format!("read: {}", e.kind()))?;
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        Ok(String::from_utf8_lossy(&buf[..end]).trim().to_string())
    }

    fn instream(&self, data: &[u8]) -> Result<String, String> {
        let mut s = self.connect()?;
        let w = |s: &mut UnixStream, b: &[u8]| s.write_all(b).map_err(|e| format!("write: {}", e.kind()));
        w(&mut s, b"zINSTREAM\0")?;
        for c in data.chunks(CHUNK) {
            w(&mut s, &(c.len() as u32).to_be_bytes())?;
            w(&mut s, c)?;
        }
        w(&mut s, &0u32.to_be_bytes())?;
        Self::reply(&mut s)
    }
}

/// Parses a clamd INSTREAM reply (`stream: OK`, `stream: <sig> FOUND`, `<msg> ERROR`).
pub fn parse_reply(r: &str) -> Outcome {
    let body = r.strip_prefix("stream:").map(str::trim);
    match body {
        Some("OK") => Outcome::Clean,
        Some(b) if b.ends_with(" FOUND") => {
            let sig: String = b
                .trim_end_matches(" FOUND")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || "._-:/".contains(*c))
                .take(120)
                .collect();
            Outcome::Infected(if sig.is_empty() { "unknown".into() } else { sig })
        }
        _ if r.contains("size limit exceeded") => Outcome::TooLarge,
        _ if r.ends_with("ERROR") => Outcome::Unavailable("clamd_error".into()),
        _ => Outcome::Unavailable("unexpected_reply".into()),
    }
}

impl MalwareScanner for Clamd {
    fn engine(&self) -> &'static str {
        "clamav"
    }
    fn configured(&self) -> bool {
        true
    }
    fn scan(&self, data: &[u8]) -> Outcome {
        if data.len() > self.max_bytes {
            return Outcome::TooLarge;
        }
        match self.instream(data) {
            Ok(r) => parse_reply(&r),
            Err(e) => Outcome::Unavailable(e),
        }
    }
    fn probe(&self) -> Result<String, String> {
        let mut s = self.connect()?;
        s.write_all(b"zVERSION\0").map_err(|e| format!("write: {}", e.kind()))?;
        let v = Self::reply(&mut s)?;
        if v.starts_with("ClamAV") {
            Ok(v)
        } else {
            Err("unexpected_reply".into())
        }
    }
}

pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<std::sync::Arc<dyn MalwareScanner>, String> {
    let Some(sock) = get("KIZUNA_CLAMD_SOCKET").filter(|v| !v.is_empty()) else {
        return Ok(std::sync::Arc::new(NotConfigured));
    };
    if !sock.starts_with('/') {
        return Err("KIZUNA_CLAMD_SOCKET must be an absolute Unix socket path".into());
    }
    let num = |k: &str, d: u64, lo: u64, hi: u64| -> Result<u64, String> {
        match get(k) {
            None => Ok(d),
            Some(v) => v
                .parse::<u64>()
                .ok()
                .filter(|n| (lo..=hi).contains(n))
                .ok_or(format!("invalid {k}")),
        }
    };
    Ok(std::sync::Arc::new(Clamd {
        socket: sock.into(),
        timeout: Duration::from_secs(num("KIZUNA_CLAMD_TIMEOUT_SECS", 30, 1, 300)?),
        max_bytes: num("KIZUNA_CLAMD_MAX_BYTES", 25 * 1024 * 1024, 1024, 1024 * 1024 * 1024)? as usize,
    }))
}

/// Fake clamd for tests: answers INSTREAM like ClamAV (EICAR → FOUND) and VERSION.
#[doc(hidden)]
pub mod fake {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Mode {
        Normal,
        /// Accepts the connection and never answers (timeout path).
        Hang,
        /// Answers INSTREAM with clamd's size-limit error.
        SizeLimit,
    }

    /// The EICAR test string, assembled at runtime so this source file is not itself flagged.
    pub fn eicar() -> Vec<u8> {
        [
            "X5O!P%@AP[4\\PZX54(P^)7CC)7}$",
            "EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*",
        ]
        .concat()
        .into_bytes()
    }

    pub fn spawn(dir: &Path, mode: Mode) -> PathBuf {
        let path = dir.join(format!("clamd-{}.sock", uuid::Uuid::now_v7()));
        let l = UnixListener::bind(&path).expect("bind fake clamd");
        std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(mut c) = conn else { break };
                std::thread::spawn(move || {
                    let mut cmd = Vec::new();
                    let mut b = [0u8; 1];
                    while c.read_exact(&mut b).is_ok() && b[0] != 0 {
                        cmd.push(b[0]);
                    }
                    if mode == Mode::Hang {
                        std::thread::sleep(std::time::Duration::from_secs(30));
                        return;
                    }
                    let reply: &[u8] = match cmd.as_slice() {
                        b"zVERSION" => b"ClamAV 1.4.3/27800/fake\0",
                        b"zINSTREAM" => {
                            let mut data = Vec::new();
                            loop {
                                let mut len = [0u8; 4];
                                if c.read_exact(&mut len).is_err() {
                                    return;
                                }
                                let n = u32::from_be_bytes(len) as usize;
                                if n == 0 {
                                    break;
                                }
                                let mut chunk = vec![0u8; n];
                                if c.read_exact(&mut chunk).is_err() {
                                    return;
                                }
                                data.extend_from_slice(&chunk);
                            }
                            let e = eicar();
                            if mode == Mode::SizeLimit {
                                b"INSTREAM size limit exceeded. ERROR\0"
                            } else if data.windows(e.len()).any(|w| w == e.as_slice()) {
                                b"stream: Win.Test.EICAR_HDB-1 FOUND\0"
                            } else {
                                b"stream: OK\0"
                            }
                        }
                        _ => b"UNKNOWN COMMAND\0",
                    };
                    let _ = c.write_all(reply);
                });
            }
        });
        path
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{Mode, eicar, spawn};
    use super::*;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("kz-av-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn clamd(sock: PathBuf) -> Clamd {
        Clamd {
            socket: sock,
            timeout: Duration::from_secs(2),
            max_bytes: 1024 * 1024,
        }
    }

    #[test]
    fn eicar_found_clean_ok_and_version() {
        let d = tmp();
        let c = clamd(spawn(&d, Mode::Normal));
        assert_eq!(c.scan(&eicar()), Outcome::Infected("Win.Test.EICAR_HDB-1".into()));
        let mut wrapped = b"part,qty\n".to_vec();
        wrapped.extend(eicar());
        assert!(matches!(c.scan(&wrapped), Outcome::Infected(_)));
        // Multi-chunk stream.
        assert_eq!(c.scan(&vec![b'a'; 200_000]), Outcome::Clean);
        assert!(c.probe().unwrap().starts_with("ClamAV"));
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn fails_closed_when_unreachable_hanging_or_over_limit() {
        let d = tmp();
        let gone = clamd(d.join("missing.sock"));
        assert!(matches!(gone.scan(b"x"), Outcome::Unavailable(_)));
        assert!(gone.probe().is_err());
        let hang = clamd(spawn(&d, Mode::Hang));
        assert!(matches!(hang.scan(b"x"), Outcome::Unavailable(_)), "timeout");
        let limit = clamd(spawn(&d, Mode::SizeLimit));
        assert_eq!(limit.scan(b"x"), Outcome::TooLarge);
        let mut small = clamd(spawn(&d, Mode::Normal));
        small.max_bytes = 4;
        assert_eq!(small.scan(b"12345"), Outcome::TooLarge, "not even sent");
        for o in [
            Outcome::TooLarge,
            Outcome::Unavailable("x".into()),
            Outcome::Infected("s".into()),
        ] {
            assert!(o.finding().is_some(), "{o:?} must quarantine");
        }
        assert!(Outcome::NotConfigured.finding().is_none());
        std::fs::remove_dir_all(d).ok();
    }

    #[test]
    fn reply_parsing_and_config() {
        assert_eq!(parse_reply("stream: OK"), Outcome::Clean);
        assert_eq!(
            parse_reply("stream: Evil;rm -rf FOUND"),
            Outcome::Infected("Evilrm-rf".into()),
            "signature sanitised"
        );
        assert!(matches!(parse_reply("garbage"), Outcome::Unavailable(_)));
        let none = from_lookup(|_| None).unwrap();
        assert!(!none.configured());
        assert!(none.probe().unwrap_err().contains("no antivirus configured"));
        assert!(from_lookup(|k| (k == "KIZUNA_CLAMD_SOCKET").then(|| "relative.sock".into())).is_err());
        let c = from_lookup(|k| (k == "KIZUNA_CLAMD_SOCKET").then(|| "/run/clamav/clamd.ctl".into())).unwrap();
        assert_eq!(c.engine(), "clamav");
    }
}
