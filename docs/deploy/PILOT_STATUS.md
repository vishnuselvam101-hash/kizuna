# Pilot ops status (branch `ops-pilot`)

Updated 2026-11-05. Founder decision: the pilot runs on this VPS (200.141.15.135), tailnet-only at
`https://vnx-hostinger.taild091df.ts.net:8446`. Nothing has been installed yet: the installer is ready for the
founder to run as root.

## Founder: install
```sh
cd /root/kizuna-ops            # or the main checkout once ops-pilot is merged
sudo deploy/install-pilot.sh --dry-run     # review: prints every action, validates units + Caddyfile
sudo deploy/install-pilot.sh               # install / upgrade (idempotent)
```
Then the "next steps" it prints: first tenant + admin, alerts in `/etc/kizuna/alerts.env`, rclone remote
(`docs/deploy/OFFSITE_RESTORE.md`), offline escrow of `/etc/kizuna/backup.pass`, `KIZUNA_DATA_KEY`,
`KIZUNA_AUDIT_SIGNING_KEY`. Remove: `sudo deploy/uninstall-pilot.sh [--dry-run] [--purge]` (keeps data unless purge).

## Built
| Item | Where | Evidence |
|---|---|---|
| Worker seccomp (bwrap `--seccomp 3`, seccompiler deny list) | `crates/kizuna-api/src/seccomp.rs`, `extraction.rs`, ADR 0016 | sandbox probe: socket/ptrace/mount → EPERM, `Seccomp: 2`; control without filter: socket/ptrace allowed; m3 extraction tests pass |
| ClamAV via clamd INSTREAM, fail-closed quarantine, honest status | `antivirus.rs`, `artifacts.rs`, `ops.rs` (`GET /api/v1/security/status`), ADR 0017 | fake clamd: EICAR quarantined, clean accepted, unreachable/hang/size-limit quarantined, not-configured reported |
| Alerts: SMTP + Telegram, rate-limited, identifiers only | `alerts.rs`, `bin/kizuna-alert.rs`, `kizuna-alert@.service` | fake SMTP/HTTP: delivery, token never in body/errors, per-key + hourly limits, persisted CLI state; incident → alert against PostgreSQL without e-mail/IP |
| Off-host backups (rclone), retention, restore-from-remote drill | `scripts/offsite-backup.sh`, `offsite-fetch.sh`, `docs/deploy/OFFSITE_RESTORE.md` | real rclone local remote + stub; real drill: 38/38 chains, 48/48 artifacts, tamper detected |
| Pilot installer / uninstaller (+ dry-run) | `deploy/` | dry-run run on this host: no changes; `caddy validate` + `systemd-analyze verify` pass |
| Integrity sweep timer | `scripts/integrity-sweep.sh`, `kizuna-integrity.timer` | against test DB: exit 0 intact; exit 2 when objects missing |
| Local CI | `scripts/ci-local.sh`; `check.sh` += shellcheck + ops script tests | ci-local green on d7bfab5 in 91 s: fmt, clippy, all Rust tests, cargo/pnpm/pip audit, Python 48, web 29, shellcheck, offsite tests (real + stub rclone), gitleaks |

## Host facts checked (not changed)
- bubblewrap 0.11.1 works for an unprivileged user despite `kernel.apparmor_restrict_unprivileged_userns=1`.
- systemd 259: `ProtectKernelTunables`, `ProtectKernelLogs`, `ProtectHostname` make bwrap's procfs mount fail; the
  API unit's final hardening set was run with `systemd-run` as `nobody` and bwrap works. `sudo` refuses under
  `NoNewPrivileges`; backup uses `runuser` (tested under the backup unit's hardening).
- Kernel: IA32 emulation on (filter kills foreign-arch syscalls), x32 ABI off.
- Ports 8460/8461 and Tailscale 8446 free; rclone 1.60, Caddy 2.6.2, tailscale 1.104 present; clamd NOT installed
  (installer adds `clamav-daemon`, `clamav-freshclam`); ~7 GB RAM available, clamd needs ~1.5 GB.

## Not verified / residual risk
- Installer not executed for real (by instruction); first real run may surface host specifics.
- Real ClamAV, Google Drive, SMTP server and Telegram not exercised (fakes and a local rclone remote only).
- Client IPs: `KIZUNA_TRUSTED_PROXY_HOPS=2` assumes tailscaled sets `X-Forwarded-For` to the tailnet peer. The
  installer verifies it: a failed login (unknown tenant) through the tailnet URL must be recorded with this node's
  `tailscale ip -4`; otherwise it prints a WARNING and the check command.
- Database roles: the pilot has its own `kizuna_pilot_owner` / `kizuna_pilot_app` with freshly generated passwords
  (in `/etc/kizuna/drill.env`); dev roles and `.env.local` are not used or changed. Migrations keep the names
  `kizuna`/`kizuna_app` and `db::migrator` maps them at migrate time (`KIZUNA_DB_OWNER_ROLE`/`KIZUNA_DB_APP_ROLE`;
  checksums unchanged). `tests/custom_roles.rs` (run by ci-local/CI) migrates under other names and checks grants,
  ownership, FORCE RLS, tenant isolation, refused cross-tenant writes, the owner policy, and that the dev roles have
  no rights there. Both roles and the database live on the same PostgreSQL cluster as dev (superuser shared).
- Seccomp is a deny list (default allow); see ADR 0016.
- Alerts are best-effort (no retry queue).
