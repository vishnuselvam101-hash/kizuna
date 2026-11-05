# Single-server deployment (pilot)

**Pilot on the founder's VPS: use `deploy/install-pilot.sh`** (idempotent; `--dry-run` first), which automates the
steps below with the values chosen for that host (tailnet-only URL via Tailscale Serve, ClamAV, seccomp, off-host
backups, alerts). Status and evidence: `docs/deploy/PILOT_STATUS.md`. This page remains the manual reference.

Status: instructions only. Nothing has been deployed. Written for M6; must be re-checked once M5 (defensive
security: migration 0005, `crates/security-detection`) is merged, because M5 adds a migration and may add settings.

Target: one Ubuntu 24.04+/26.04 host, PostgreSQL 16+ on the same host, Caddy for TLS, systemd for supervision.
One KIZUNA instance can host several tenants (row-level security), but a pilot normally gets its own host.

## 1. Packages and users
```sh
apt install postgresql caddy bubblewrap gnupg util-linux python3 # tesseract-ocr optional (image OCR)
useradd --system --home /var/lib/kizuna --shell /usr/sbin/nologin kizuna
install -d -o kizuna -g kizuna -m 700 /var/lib/kizuna /var/lib/kizuna/objects
install -d -o root -g kizuna -m 750 /etc/kizuna /opt/kizuna
```
Build on a build machine (same OS), not on the pilot host:
```sh
cargo build --release --bin kizuna-server --bin kizuna-admin --bin kizuna-restore-verify
(cd web && pnpm install --frozen-lockfile && pnpm build)
(cd services/extraction && uv sync --frozen --no-dev)
```
Copy `target/release/kizuna-{server,admin,restore-verify}` to `/opt/kizuna/bin/`, `scripts/` to `/opt/kizuna/scripts/`, `web/dist/` to `/opt/kizuna/web/`, and
`services/extraction/` (with its `.venv`, rebuilt on the host if the Python path differs) to `/opt/kizuna/extraction/`.
Record the deployed commit SHA in `/opt/kizuna/VERSION`.

## 2. Database roles
`scripts/db-setup.sh` is for development. In production create the same two roles by hand:

| Role | Purpose | Rights |
|---|---|---|
| `kizuna` | schema owner; runs migrations and `kizuna-admin` | owns tables; no superuser, no BYPASSRLS |
| `kizuna_app` | runtime role used by `kizuna-server` | DML via grants only; the server refuses to start if it is superuser, has BYPASSRLS or owns tables |
| `postgres` | backups only (`pg_dump` must bypass FORCE RLS) | local peer auth, never exposed |

```sql
CREATE ROLE kizuna LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD '...';
CREATE ROLE kizuna_app LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOBYPASSRLS PASSWORD '...';
CREATE DATABASE kizuna_prod OWNER kizuna;
\c kizuna_prod
REVOKE ALL ON DATABASE kizuna_prod FROM PUBLIC; GRANT CONNECT, TEMPORARY ON DATABASE kizuna_prod TO kizuna_app;
REVOKE CREATE ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO kizuna;
```
`pg_hba.conf`: `host kizuna_prod kizuna,kizuna_app 127.0.0.1/32 scram-sha-256`; PostgreSQL listens on localhost only.
Passwords: `openssl rand -hex 24`.

## 3. Environment
`/etc/kizuna/server.env` (root:kizuna, 0640) — read by the server only:
```sh
KIZUNA_DATABASE_URL=postgres://kizuna_app:...@127.0.0.1:5432/kizuna_prod
KIZUNA_DATA_KEY=...              # openssl rand -base64 32; losing it loses TOTP enrolments — back it up separately
KIZUNA_BIND=127.0.0.1:8080       # never public; Caddy is the only client
KIZUNA_TRUSTED_PROXY_HOPS=1      # exactly one proxy (Caddy) — client IPs for rate limits and audit come from it
KIZUNA_STORAGE_DIR=/var/lib/kizuna/objects
KIZUNA_EXTRACTION_PYTHON=/opt/kizuna/extraction/.venv/bin/python
KIZUNA_EXTRACTION_DIR=/opt/kizuna/extraction/src
KIZUNA_EXTRACTION_SANDBOX=bwrap  # never `none` in production
KIZUNA_LOG=info
# M5: Ed25519 seed for signed audit checkpoints and signed reports (openssl rand -base64 32).
# Keep a copy OUTSIDE this host; export checkpoints regularly (POST /api/v1/audit/checkpoints) to a separate
# system so a database owner cannot silently re-chain the audit log.
KIZUNA_AUDIT_SIGNING_KEY=...
# Data-key rotation (lane-auth): KIZUNA_DATA_KEY_ID=<n>, KIZUNA_DATA_KEYS_PREVIOUS=<id:base64,...>, then
#   kizuna-admin rotate-data-key   (re-encrypts TOTP/WebAuthn secrets under the current key)
```
Run the server as the dedicated non-root `kizuna` user: the extraction sandbox then also applies a process limit
(`--nproc`), which is skipped when running as root. bubblewrap also installs a seccomp deny-list filter (ADR 0016).

Incident response (M5) runs inside the server (60 s sweep + inline triggers); thresholds are static defaults.
Emergency recovery if containment ever blocks administrators: `kizuna-admin emergency-unlock <tenant-slug>`
(re-enables admins, lifts address blocks and restrictions; audited). Keep `admin.env` on the server only.
`/etc/kizuna/admin.env` (root only, 0600): `KIZUNA_OWNER_DATABASE_URL=postgres://kizuna:...@127.0.0.1:5432/kizuna_prod`.
Remaining limits (`KIZUNA_ACCESS_TTL_SECS`, upload size, lockout, rate limits) keep the defaults in `.env.example`
unless the pilot needs otherwise.

## 4. Migrations, tenant, first admin
```sh
set -a; . /etc/kizuna/admin.env; set +a
/opt/kizuna/bin/kizuna-admin migrate
/opt/kizuna/bin/kizuna-admin create-tenant <slug> "<Company>"
/opt/kizuna/bin/kizuna-admin create-user <slug> admin@<company> tenant_admin "<Name>"   # password on stdin
```
The admin must enrol TOTP at first login (privileged actions require an MFA session). Other users are created by
the admin through the API/UI with least-privilege roles: `analyst` (upload, extract, run checks), `reviewer`
(+ review, decisions), `auditor` (read-only + audit verification), `viewer` (read-only).

## 5. systemd
`/etc/systemd/system/kizuna.service`:
```ini
[Unit]
Description=KIZUNA Verify API
After=postgresql.service
Requires=postgresql.service

[Service]
User=kizuna
Group=kizuna
EnvironmentFile=/etc/kizuna/server.env
WorkingDirectory=/var/lib/kizuna
ExecStart=/opt/kizuna/bin/kizuna-server
Restart=on-failure
RestartSec=2
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/kizuna
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelModules=yes
ProtectControlGroups=yes
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK
LockPersonality=yes
MemoryMax=2G
TasksMax=512

[Install]
WantedBy=multi-user.target
```
Do not add `ProtectKernelTunables`, `ProtectKernelLogs` or `ProtectHostname`: they over-mount parts of /proc and the
kernel then refuses bubblewrap's procfs mount (tested on systemd 259, `deploy/systemd/kizuna.service`).
bubblewrap needs unprivileged user namespaces; do not add `RestrictNamespaces=yes` or `PrivateUsers=yes` without
re-running the extraction tests on the host (`AF_NETLINK` is for bwrap's loopback setup). Verify with
`systemctl start kizuna && curl -fsS http://127.0.0.1:8080/healthz`.

## 6. Caddy (TLS)
`/etc/caddy/Caddyfile`:
```caddy
verify.example.co.jp {
	encode zstd gzip
	request_body {
		max_size 26MB
	}
	handle /api/* {
		reverse_proxy 127.0.0.1:8080
	}
	handle /healthz {
		reverse_proxy 127.0.0.1:8080
	}
	handle {
		root * /opt/kizuna/web
		try_files {path} /index.html
		file_server
	}
	header {
		Strict-Transport-Security "max-age=63072000; includeSubDomains"
		X-Content-Type-Options nosniff
		Referrer-Policy no-referrer
		-Server
	}
	log {
		output file /var/log/caddy/kizuna.log
	}
}
```
Caddy obtains and renews the certificate automatically (ports 80/443 open, DNS pointing at the host). Firewall:
allow 22 (restricted source), 80, 443 only. The API sets its own security headers; the SPA's CSP should be added
here once the web UI's asset list is final.

## 7. Backups
Daily encrypted backup with `scripts/backup.sh` (pg_dump custom format + object store tar, GnuPG AES-256,
SHA-256 manifest and checksum file):
```sh
# /etc/kizuna/backup.pass: root 0600, passphrase also stored offline (sealed envelope / company password manager)
KIZUNA_BACKUP_PASSPHRASE_FILE=/etc/kizuna/backup.pass \
  /opt/kizuna/scripts/backup.sh --db kizuna_prod --storage /var/lib/kizuna/objects --out /var/backups/kizuna
```
As a systemd timer (`kizuna-backup.service` `Type=oneshot` running the line above as root; `kizuna-backup.timer`
`OnCalendar=*-*-* 02:30`, `Persistent=true`). Off-host copy, retention and the restore-from-remote drill:
`scripts/offsite-backup.sh`, `docs/deploy/OFFSITE_RESTORE.md`. `KIZUNA_DATA_KEY` and the backup passphrase are NOT in the
backup and must be escrowed separately.

Restore drill (monthly and after every upgrade) — restores into a throwaway database and verifies every tenant's
audit hash chain and every artifact hash with the server's own code; `--tamper-test` proves the check can fail:
```sh
KIZUNA_BACKUP_PASSPHRASE_FILE=/etc/kizuna/backup.pass KIZUNA_RESTORE_VERIFY_BIN=/opt/kizuna/bin/kizuna-restore-verify \
  KIZUNA_ENV_FILE=/etc/kizuna/drill.env /opt/kizuna/scripts/restore-drill.sh /var/backups/kizuna/<file>.tar.gpg \
  --tamper-test --report /var/backups/kizuna/drills
```
`drill.env` (root 0600) holds `KIZUNA_OWNER_PASSWORD` and `KIZUNA_APP_PASSWORD`; the drill creates and drops a
`kizuna_drill_*` database via `sudo -u postgres` and never touches `kizuna_prod`.

Real restore (incident): stop `kizuna`, restore into a new database with `pg_restore`, extract `objects.tar` into a
new storage directory, run `kizuna-restore-verify` against both, then point `server.env` at them and start.
Measured on the dev host for the E2E data set (54 artifacts, 125 audit events): restore 2.2 s, verify 0.1 s
(`reports/drill/`). Production volumes will be larger; re-measure on the pilot host.

## 8. Upgrades
1. Backup + restore drill. 2. Stop service. 3. Replace binaries/web/extraction. 4. `kizuna-admin migrate`.
5. Start; `curl /healthz`; `kizuna-admin verify-audit <slug>`. Rollback = previous binaries + restore the
pre-upgrade backup (migrations are forward-only).

## 9. Operational checks
- `kizuna-admin verify-audit <slug>` daily (exit 2 = chain broken → incident).
- Watch `security-events` (auditor role) for lockouts, rejected/quarantined uploads, integrity failures.
- Disk: object store grows with uploads; PostgreSQL WAL; `/var/backups`.
- Logs: `journalctl -u kizuna`; Caddy access log. Neither contains secrets (config secrets are redacted).
