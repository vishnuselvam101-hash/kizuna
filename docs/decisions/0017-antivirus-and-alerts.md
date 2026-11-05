# ADR 0017: ClamAV behind a scanner trait; operator alerts

Status: accepted (2026-11-05). Supersedes the "not adopted" note on ClamAV in ADR 0011.

## Antivirus
- `antivirus::MalwareScanner` (engine, configured, blocking `scan`, `probe`). Implementations: `NotConfigured`
  (default) and `Clamd` — ClamAV's daemon over its local Unix socket with the `zINSTREAM` protocol (64 KiB chunks,
  length-prefixed, zero terminator), read/write timeout, and our own size cap checked before sending.
- Order at upload: static structural scan (ADR 0011) first; rejected types never reach the engine. Then the engine:
  `stream: OK` keeps the static verdict; `FOUND` → quarantine with finding `av_infected:<signature>` (signature
  sanitised); over the size cap, clamd size-limit error, unreachable socket, timeout, clamd `ERROR` or an unexpected
  reply → quarantine with `av_not_scanned_too_large` / `av_unavailable` (fail closed). A quarantined file is kept,
  never extracted or served, and released only by an MFA'd incident manager (existing flow).
- `scan_result.antivirus` records engine and result on every upload, including `not_configured`.
  `GET /api/v1/security/status` (SecurityEventRead) states whether antivirus is configured and reachable, whether
  the seccomp filter is active and which alert channels are on. Startup logs a warning when no scanner is configured.
- Env: `KIZUNA_CLAMD_SOCKET` (absolute path; unset = not configured), `KIZUNA_CLAMD_TIMEOUT_SECS` (30),
  `KIZUNA_CLAMD_MAX_BYTES` (25 MiB = clamd's default StreamMaxLength, matching the upload limit).
- ClamAV (GPL-2.0) runs as a separate daemon installed from the distro (`clamav-daemon`, `clamav-freshclam`); we only
  speak its socket protocol, no linking. Signature updates via freshclam. clamd needs ~1–1.5 GB RAM.
- Tests: fake clamd (`antivirus::fake`) answering like ClamAV; EICAR assembled at runtime. Unit tests cover found,
  clean (multi-chunk), unreachable, hanging (timeout), size limit, local cap, reply parsing; `tests/ops_pilot.rs`
  covers the upload path and status endpoint.

## Alerts
- `alerts::Alerter` with optional SMTP (`lettre`, STARTTLS/TLS via rustls; plaintext only to localhost) and Telegram
  Bot API (`ureq` + rustls; https only, http only to localhost for tests). Both off unless configured.
- Sources: (1) the server polls every 60 s for incidents whose policy executed `notify_admins` (covers detection
  rules incl. audit-chain/artifact integrity failures) — watermark starts at server start, no replay; (2)
  `kizuna-alert <kind> <unit>` for host events: systemd `OnFailure=` of the backup and integrity-sweep units.
- Rate limits: one alert per key (incident id / host event + unit) per `KIZUNA_ALERT_MIN_INTERVAL_SECS` (900) and
  at most `KIZUNA_ALERT_MAX_PER_HOUR` (20) overall; the CLI persists the gate in `/var/lib/kizuna/alerts/state.json`.
- Content: fixed texts with rule, severity, tenant slug, incident id or unit name. Never document content, user
  e-mails, IPs, tokens or secrets. Transport errors are reduced to categories (ureq errors can contain the URL, which
  contains the bot token).
- Tests: fake SMTP and HTTP servers (`alerts::fake`): delivery on both channels, token only in the request path,
  per-key and hourly limits, persisted state, failure reporting without the token, config validation, and an
  incident → alert test against PostgreSQL without personal data.

## Consequences
Alerts are best-effort (no queue): a failed delivery is logged and not retried until the next distinct event or the
interval expires. The status endpoint probes clamd on each call (cheap `VERSION`).
