//! Runs the Python extraction worker on one document inside a sandbox (ADR 0012):
//! bubblewrap with all namespaces unshared (no network), a read-only minimal filesystem (no repository, no
//! configuration, no other documents), empty environment, tmpfs /tmp, plus prlimit (address space, CPU, files)
//! and a wall-clock timeout. Input: document bytes on stdin. Output: bounded JSON on stdout.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use domain_manufacturing::{ColumnMapping, Record};
use serde::Deserialize;
use serde_json::{Value, json};
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

/// The tenant's saved import settings (`import_settings`, API gap #1) translated into what the worker and the
/// mapper consume. Values outside the whitelist are ignored (and reported), never passed on.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ImportOptions {
    pub version: i32,
    /// Worker limits (the worker only ever lowers its own defaults).
    pub limits: serde_json::Map<String, Value>,
    /// Worker options (validated again by the worker).
    pub options: serde_json::Map<String, Value>,
    /// Long-table column mapping for CSV/XLSX.
    pub mapping: Option<ColumnMapping>,
    pub ignored: Vec<String>,
}

const XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

pub fn import_options(version: i32, s: &Value, media_type: &str) -> ImportOptions {
    let mut o = ImportOptions {
        version,
        ..Default::default()
    };
    let num = |o: &mut ImportOptions, path: &str, key: &str, lo: u64, hi: u64, limit: bool| {
        let (a, b) = path.split_once('.').unwrap_or((path, ""));
        let v = &s[a][b];
        if v.is_null() {
            return;
        }
        match v.as_u64().filter(|n| (lo..=hi).contains(n)) {
            Some(n) if limit => {
                o.limits.insert(key.into(), json!(n));
            }
            Some(n) => {
                o.options.insert(key.into(), json!(n));
            }
            None => o.ignored.push(path.into()),
        }
    };
    let pick = |o: &mut ImportOptions, path: &str, key: &str, allowed: &[&str]| {
        let (a, b) = path.split_once('.').unwrap_or((path, ""));
        match &s[a][b] {
            Value::Null => {}
            Value::String(x) if allowed.contains(&x.as_str()) => {
                o.options.insert(key.into(), json!(x));
            }
            _ => o.ignored.push(path.into()),
        }
    };
    match media_type {
        "text/csv" => {
            pick(&mut o, "csv.delimiter", "csv_delimiter", &[",", ";", "\t"]);
            pick(&mut o, "csv.encoding", "csv_encoding", &["utf-8", "shift_jis"]);
            num(&mut o, "csv.header_row", "header_row", 0, 100, false);
            num(&mut o, "csv.max_records", "max_records", 1, 20_000, true);
        }
        XLSX => {
            match &s["xlsx"]["sheet"] {
                Value::Null => {}
                Value::String(x) if x.chars().count() <= 100 => {
                    o.options.insert("xlsx_sheet".into(), json!(x));
                }
                _ => o.ignored.push("xlsx.sheet".into()),
            }
            num(&mut o, "xlsx.header_row", "header_row", 0, 100, false);
            num(&mut o, "xlsx.max_sheets", "max_sheets", 1, 50, true);
            num(&mut o, "xlsx.max_records", "max_records", 1, 20_000, true);
        }
        "application/pdf" | "image/png" | "image/jpeg" | "image/tiff" => {
            match &s["pdf"]["ocr"] {
                Value::Null => {}
                Value::Bool(b) => {
                    o.options.insert("ocr".into(), json!(b));
                }
                _ => o.ignored.push("pdf.ocr".into()),
            }
            pick(
                &mut o,
                "pdf.ocr_language",
                "ocr_language",
                &["jpn+eng", "eng+jpn", "eng", "jpn"],
            );
            if media_type == "application/pdf" {
                num(&mut o, "pdf.max_pages", "max_pages", 1, 500, true);
            }
        }
        _ => {}
    }
    if matches!(media_type, "text/csv" | XLSX)
        && let Some(m) = s["mapping"].as_object()
    {
        let f = |k: &str| {
            m.get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .chars()
                .take(100)
                .collect::<String>()
        };
        let cm = ColumnMapping {
            subject_key: f("subject_key"),
            predicate: f("predicate"),
            value: f("value"),
            unit: f("unit"),
            revision: f("revision"),
        };
        if !cm.predicate.is_empty() && !cm.value.is_empty() {
            o.mapping = Some(cm);
        }
    }
    o
}

pub async fn run_worker(
    cfg: &Config,
    media_type: &str,
    data: &[u8],
    import: &ImportOptions,
) -> Result<(WorkerOutput, [u8; 32]), WorkerError> {
    let argv = command_argv(cfg);
    if !Path::new(&argv[0]).exists() {
        return Err(WorkerError::Sandbox(argv[0].clone()));
    }
    let req = serde_json::json!({
        "media_type": media_type,
        "data_b64": base64::engine::general_purpose::STANDARD.encode(data),
        "limits": import.limits,
        "options": import.options,
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
    fn import_settings_become_whitelisted_worker_options() {
        let s = json!({
            "csv": { "delimiter": ";", "encoding": "shift_jis", "header_row": 3, "max_records": 500 },
            "xlsx": { "sheet": "BOM", "header_row": 2, "max_sheets": 999, "max_records": 10 },
            "pdf": { "ocr": false, "ocr_language": "eng; rm -rf /", "max_pages": 20 },
            "mapping": { "subject_key": "part_number", "predicate": "attribute", "value": "value", "unit": "unit" },
        });
        let c = import_options(4, &s, "text/csv");
        assert_eq!(c.version, 4);
        assert_eq!(
            json!(c.options),
            json!({"csv_delimiter": ";", "csv_encoding": "shift_jis", "header_row": 3})
        );
        assert_eq!(json!(c.limits), json!({"max_records": 500}));
        assert_eq!(c.mapping.as_ref().map(|m| m.predicate.as_str()), Some("attribute"));
        let x = import_options(4, &s, XLSX);
        assert_eq!(json!(x.options), json!({"xlsx_sheet": "BOM", "header_row": 2}));
        assert_eq!(json!(x.limits), json!({"max_records": 10}));
        assert_eq!(x.ignored, vec!["xlsx.max_sheets"]);
        let p = import_options(4, &s, "application/pdf");
        assert_eq!(json!(p.options), json!({"ocr": false}));
        assert_eq!(json!(p.limits), json!({"max_pages": 20}));
        assert_eq!(p.ignored, vec!["pdf.ocr_language"]);
        assert!(p.mapping.is_none());
        assert_eq!(import_options(0, &json!({}), "text/csv"), ImportOptions::default());
    }

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
