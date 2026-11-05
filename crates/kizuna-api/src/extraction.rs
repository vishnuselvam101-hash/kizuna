//! Runs the Python extraction worker on one document inside a sandbox (ADR 0012):
//! bubblewrap with all namespaces unshared (no network), a read-only minimal filesystem (no repository, no
//! configuration, no other documents), empty environment, tmpfs /tmp, plus prlimit (address space, CPU, files)
//! and a wall-clock timeout. Input: document bytes on stdin. Output: bounded JSON on stdout.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use domain_manufacturing::Record;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use crate::config::Config;

pub const MAX_OUTPUT: usize = 16 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct WorkerOutput {
    pub ok: bool,
    #[serde(default)]
    pub extractor: String,
    #[serde(default)]
    pub worker_version: String,
    #[serde(default)]
    pub records: Vec<Record>,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("sandbox unavailable: {0}")]
    Sandbox(String),
    #[error("timeout")]
    Timeout,
    #[error("worker crashed (status {0})")]
    Crashed(String),
    #[error("output too large")]
    OutputTooLarge,
    #[error("invalid output")]
    InvalidOutput,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl WorkerError {
    pub fn code(&self) -> &'static str {
        match self {
            WorkerError::Sandbox(_) => "sandbox_unavailable",
            WorkerError::Timeout => "timeout",
            WorkerError::Crashed(_) => "crashed",
            WorkerError::OutputTooLarge => "output_too_large",
            WorkerError::InvalidOutput => "invalid_output",
            WorkerError::Io(_) => "io_error",
        }
    }
}

/// Read-only binds needed for the interpreter: the venv and the real install root reached through symlinks.
fn interpreter_binds(py: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut binds = Vec::new();
    let Ok(canon) = std::fs::canonicalize(py) else {
        return binds;
    };
    let root = |p: &Path| p.parent().and_then(Path::parent).map(Path::to_path_buf);
    let Some(real_root) = root(&canon) else { return binds };
    binds.push((real_root.clone(), real_root.clone()));
    let mut p = py.to_path_buf();
    for _ in 0..8 {
        let Ok(t) = std::fs::read_link(&p) else { break };
        let t = if t.is_absolute() {
            t
        } else {
            p.parent().map(|d| d.join(&t)).unwrap_or(t)
        };
        if let Some(r) = root(&t)
            && r != real_root
            && std::fs::canonicalize(&r).ok().as_ref() == Some(&real_root)
            && !binds.iter().any(|(_, d)| d == &r)
        {
            binds.push((real_root.clone(), r));
        }
        p = t;
    }
    if let Some(venv) = root(py)
        && venv.join("pyvenv.cfg").exists()
        && !binds.iter().any(|(_, d)| d == &venv)
    {
        binds.push((venv.clone(), venv));
    }
    binds
}

fn running_as_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").map(|m| m.uid() == 0).unwrap_or(false)
}

const BOOT: &str = "import sys; sys.path.insert(0, sys.argv[1]); from kizuna_extraction.worker import main; main()";

/// Builds the sandboxed command line (exposed for tests).
pub fn command_argv(cfg: &Config) -> Vec<String> {
    let py = cfg.extraction_python.clone();
    let src = cfg.extraction_dir.to_string_lossy().into_owned();
    let limits = [
        "/usr/bin/prlimit".to_string(),
        "--as=3221225472".into(),
        format!("--cpu={}", cfg.extraction_timeout_secs),
        "--nofile=64".into(),
        "--core=0".into(),
        "--fsize=0".into(),
        "--".into(),
    ];
    // RLIMIT_NPROC counts processes per real UID; it is only meaningful when the server runs as a dedicated
    // non-root user (as documented for deployment). Under root it would count every root process on the host.
    let mut limits = limits.to_vec();
    if !running_as_root() {
        limits.insert(limits.len() - 1, "--nproc=64".into());
    }
    let tail = [py.clone(), "-I".into(), "-c".into(), BOOT.into(), src.clone()];
    if cfg.extraction_sandbox == "none" {
        return limits.into_iter().chain(tail).collect();
    }
    let mut a: Vec<String> = [
        "/usr/bin/bwrap",
        "--unshare-all",
        "--unshare-user",
        "--disable-userns",
        "--die-with-parent",
        "--new-session",
        "--clearenv",
        "--cap-drop",
        "ALL",
        "--ro-bind",
        "/usr",
        "/usr",
        "--symlink",
        "usr/lib",
        "/lib",
        "--symlink",
        "usr/lib64",
        "/lib64",
        "--symlink",
        "usr/bin",
        "/bin",
        "--ro-bind",
        "/etc/ld.so.cache",
        "/etc/ld.so.cache",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
        "--chdir",
        "/tmp",
        "--setenv",
        "PATH",
        "/usr/bin",
        "--setenv",
        "OMP_THREAD_LIMIT",
        "1",
        "--setenv",
        "PYTHONDONTWRITEBYTECODE",
        "1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for (s, d) in interpreter_binds(Path::new(&py)) {
        a.extend([
            "--ro-bind".into(),
            s.to_string_lossy().into_owned(),
            d.to_string_lossy().into_owned(),
        ]);
    }
    a.extend(["--ro-bind".into(), src.clone(), src]);
    a.push("--".into());
    a.extend(limits);
    a.extend(tail);
    a
}

pub async fn run_worker(cfg: &Config, media_type: &str, data: &[u8]) -> Result<(WorkerOutput, [u8; 32]), WorkerError> {
    let argv = command_argv(cfg);
    if !Path::new(&argv[0]).exists() {
        return Err(WorkerError::Sandbox(argv[0].clone()));
    }
    let req = serde_json::json!({
        "media_type": media_type,
        "data_b64": base64::engine::general_purpose::STANDARD.encode(data),
    })
    .to_string();
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or(WorkerError::InvalidOutput)?;
    let mut stdout = child.stdout.take().ok_or(WorkerError::InvalidOutput)?;
    let work = async move {
        let writer = async move {
            let r = stdin.write_all(req.as_bytes()).await;
            drop(stdin);
            r
        };
        let reader = async move {
            let mut buf = Vec::new();
            (&mut stdout)
                .take(MAX_OUTPUT as u64 + 1)
                .read_to_end(&mut buf)
                .await
                .map(|_| buf)
        };
        let (w, r) = tokio::join!(writer, reader);
        w?;
        r
    };
    let out = match tokio::time::timeout(Duration::from_secs(cfg.extraction_timeout_secs), work).await {
        Ok(r) => r?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(WorkerError::Timeout);
        }
    };
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .map_err(|_| WorkerError::Timeout)??;
    if out.len() > MAX_OUTPUT {
        return Err(WorkerError::OutputTooLarge);
    }
    if !status.success() {
        return Err(WorkerError::Crashed(status.to_string()));
    }
    let parsed: WorkerOutput = serde_json::from_slice(&out).map_err(|_| WorkerError::InvalidOutput)?;
    Ok((parsed, crate::crypto::sha256(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_argv_isolates_network_and_filesystem() {
        let mut cfg = crate::config::Config::from_lookup(|k| match k {
            "KIZUNA_DATABASE_URL" => Some("postgres://x".into()),
            "KIZUNA_DATA_KEY" => Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into()),
            _ => None,
        })
        .unwrap();
        cfg.extraction_python = "/usr/bin/python3".into();
        let a = command_argv(&cfg);
        assert_eq!(a[0], "/usr/bin/bwrap");
        for flag in [
            "--unshare-all",
            "--unshare-user",
            "--disable-userns",
            "--clearenv",
            "--die-with-parent",
            "--cap-drop",
            "--fsize=0",
        ] {
            assert!(a.iter().any(|x| x == flag), "{flag}");
        }
        assert!(!a.windows(2).any(|w| w[0] == "--bind"), "no writable binds");
        assert!(
            !a.iter().any(|x| x == "/" || x == "/root" || x == "/etc"),
            "no broad binds: {a:?}"
        );
        assert!(a.iter().any(|x| x.starts_with("--as=")));
    }
}
