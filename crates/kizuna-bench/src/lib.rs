//! Shared pieces for the M6 operator tools: latency statistics, a minimal HTTP/1.1 keep-alive client (so API
//! benchmarks go through the real TCP + HTTP stack without adding an HTTP-client dependency) and environment capture.

use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Latency summary (milliseconds) over one measured series.
pub fn summarize(samples: &[Duration], wall: Duration) -> Value {
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    ms.sort_by(f64::total_cmp);
    let pct = |p: f64| -> f64 {
        if ms.is_empty() {
            return 0.0;
        }
        // Nearest-rank percentile.
        let rank = ((p / 100.0) * ms.len() as f64).ceil().max(1.0) as usize;
        ms[rank.min(ms.len()) - 1]
    };
    let n = ms.len();
    let mean = if n == 0 { 0.0 } else { ms.iter().sum::<f64>() / n as f64 };
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    json!({
        "n": n,
        "p50_ms": r3(pct(50.0)),
        "p95_ms": r3(pct(95.0)),
        "p99_ms": r3(pct(99.0)),
        "mean_ms": r3(mean),
        "min_ms": r3(ms.first().copied().unwrap_or(0.0)),
        "max_ms": r3(ms.last().copied().unwrap_or(0.0)),
        "wall_s": r3(wall.as_secs_f64()),
        "throughput_per_s": if wall.is_zero() { 0.0 } else { r3(n as f64 / wall.as_secs_f64()) },
    })
}

/// `VmHWM` (peak RSS) and `VmRSS` of this process in KiB, from /proc (Linux only; null elsewhere).
pub fn memory_kib() -> Value {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |k: &str| -> Option<u64> {
        s.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
    };
    json!({ "peak_rss_kib": get("VmHWM:"), "rss_kib": get("VmRSS:") })
}

fn cmd(c: &str, args: &[&str]) -> String {
    std::process::Command::new(c)
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Hardware/software environment of the run (recorded with every result).
pub fn environment() -> Value {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with("model name"))
        .and_then(|l| l.split(':').nth(1))
        .map(|s| s.trim().to_string());
    let mem = std::fs::read_to_string("/proc/meminfo")
        .unwrap_or_default()
        .lines()
        .next()
        .map(str::to_string);
    json!({
        "cpu": cpu,
        "logical_cpus": std::thread::available_parallelism().map(|n| n.get()).ok(),
        "memory": mem,
        "kernel": cmd("uname", &["-r"]),
        "loadavg_at_start": std::fs::read_to_string("/proc/loadavg").ok().map(|s| s.trim().to_string()),
        "rustc": cmd("rustc", &["-V"]),
        "commit": cmd("git", &["rev-parse", "--short", "HEAD"]),
        "worktree_dirty": !cmd("git", &["status", "--porcelain"]).is_empty(),
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
    })
}

/// One keep-alive HTTP/1.1 connection. Supports Content-Length and chunked responses; nothing else.
pub struct Conn {
    io: BufReader<TcpStream>,
}

impl Conn {
    pub async fn connect(addr: std::net::SocketAddr) -> std::io::Result<Self> {
        let s = TcpStream::connect(addr).await?;
        s.set_nodelay(true)?;
        Ok(Self { io: BufReader::new(s) })
    }

    pub async fn request(
        &mut self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> std::io::Result<(u16, Value)> {
        let body = body.map(|b| b.to_string()).unwrap_or_default();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nhost: bench\r\ncontent-length: {}\r\n",
            body.len()
        );
        if let Some(t) = token {
            head.push_str(&format!("authorization: Bearer {t}\r\n"));
        }
        if !body.is_empty() {
            head.push_str("content-type: application/json\r\n");
        }
        head.push_str("\r\n");
        head.push_str(&body);
        self.io.get_mut().write_all(head.as_bytes()).await?;

        let bad = |m: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, m.to_string());
        let mut line = String::new();
        self.io.read_line(&mut line).await?;
        let status: u16 = line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| bad("status line"))?;
        let (mut len, mut chunked) = (0usize, false);
        loop {
            line.clear();
            self.io.read_line(&mut line).await?;
            let l = line.trim_end();
            if l.is_empty() {
                break;
            }
            if let Some((k, v)) = l.split_once(':') {
                match k.trim().to_ascii_lowercase().as_str() {
                    "content-length" => len = v.trim().parse().map_err(|_| bad("content-length"))?,
                    "transfer-encoding" => chunked = v.to_ascii_lowercase().contains("chunked"),
                    _ => {}
                }
            }
        }
        let mut buf = Vec::new();
        if chunked {
            loop {
                line.clear();
                self.io.read_line(&mut line).await?;
                let n = usize::from_str_radix(line.trim(), 16).map_err(|_| bad("chunk size"))?;
                let mut chunk = vec![0; n + 2];
                self.io.read_exact(&mut chunk).await?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        } else {
            buf.resize(len, 0);
            self.io.read_exact(&mut buf).await?;
        }
        let v = serde_json::from_slice(&buf).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&buf).into()));
        Ok((status, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let s: Vec<Duration> = (1..=100).map(Duration::from_millis).collect();
        let v = summarize(&s, Duration::from_secs(1));
        assert_eq!(v["p50_ms"], 50.0);
        assert_eq!(v["p95_ms"], 95.0);
        assert_eq!(v["p99_ms"], 99.0);
        assert_eq!(v["throughput_per_s"], 100.0);
        assert_eq!(summarize(&[], Duration::ZERO)["n"], 0);
    }
}
