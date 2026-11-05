#!/usr/bin/env bash
# Installs the KIZUNA pilot on THIS host (idempotent; re-run to upgrade). Run as root from the repository:
#   sudo deploy/install-pilot.sh --dry-run     # prints every action and validates units/Caddyfile, changes nothing
#   sudo deploy/install-pilot.sh               # installs / upgrades
# Options: --skip-apt, --skip-build (reuse target/release + web/dist), --ts-host NAME (default: this node's
#          Tailscale DNS name), --reset-role-passwords (only if roles exist and no password source is known).
#
# Result: non-root user `kizuna`; /opt/kizuna (code, root-owned), /var/lib/kizuna (data), /etc/kizuna (secrets,
# 0600), /var/backups/kizuna; database kizuna_pilot (roles kizuna/kizuna_app, see scripts/lib/db.sh); hardened
# systemd units kizuna (API on 127.0.0.1:8460), kizuna-web (local Caddy on 127.0.0.1:8461), backup + integrity
# timers, alert hook; ClamAV daemon; tailnet-only HTTPS via `tailscale serve --https=8446`.
# It does not touch /etc/caddy, other services, the firewall or other databases. Existing secrets are kept.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/root/.cargo/bin:/root/.local/bin:$PATH

DRY=0 SKIP_APT=0 SKIP_BUILD=0 RESET_PW=0 TS_HOST=''
while (($#)); do
  case $1 in
    --dry-run) DRY=1; shift ;;
    --skip-apt) SKIP_APT=1; shift ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --reset-role-passwords) RESET_PW=1; shift ;;
    --ts-host) TS_HOST=$2; shift 2 ;;
    -h | --help) sed -n '2,15p' "$0"; exit 0 ;;
    *) echo "unknown option $1" >&2; exit 64 ;;
  esac
done

DB=kizuna_pilot
API_BIND=127.0.0.1:8460
WEB_HOST=127.0.0.1
WEB_BIND=$WEB_HOST:8461
TS_PORT=8446
PREFIX=/opt/kizuna STATE=/var/lib/kizuna ETC=/etc/kizuna BACKUPS=/var/backups/kizuna
UNIT_DIR=/etc/systemd/system
UNITS=(kizuna.service kizuna-web.service kizuna-backup.service kizuna-backup.timer kizuna-integrity.service
  kizuna-integrity.timer kizuna-alert@.service)
APT_PKGS=(bubblewrap gnupg clamav-daemon clamav-freshclam rclone util-linux python3)
CLAMD_SOCKET=/run/clamav/clamd.ctl

say() { printf '\n== %s\n' "$*"; }
note() { printf '   %s\n' "$*"; }
# run CMD... — executes, or in dry-run prints what would run.
run() {
  if ((DRY)); then printf '   would run: %s\n' "$*"; else "$@"; fi
}
# put PATH MODE OWNER:GROUP < content — writes atomically if the content changed. Secrets are never printed.
put() {
  local path=$1 mode=$2 owner=$3 tmp
  tmp=$(mktemp)
  cat > "$tmp"
  if [[ -f $path ]] && cmp -s "$tmp" "$path"; then
    note "unchanged: $path"
  elif ((DRY)); then
    note "would write $path ($mode $owner, $(wc -l < "$tmp") lines)"
  else
    install -m "$mode" -o "${owner%:*}" -g "${owner#*:}" "$tmp" "$path"
    note "wrote $path"
  fi
  rm -f "$tmp"
}
render() { # render TEMPLATE — substitutes @VARS@
  sed -e "s|@DB@|$DB|g" -e "s|@API_BIND@|$API_BIND|g" -e "s|@WEB_BIND@|$WEB_BIND|g" -e "s|@WEB_HOST@|$WEB_HOST|g" \
    -e "s|@TS_PORT@|$TS_PORT|g" "$1"
}
env_get() { # env_get FILE KEY — value of KEY in a KEY=VALUE file (no sourcing)
  [[ -r $1 ]] && sed -n "s/^$2=//p" "$1" | tail -1
}
gen_key() { openssl rand -base64 32; }
gen_pw() { openssl rand -hex 24; }

# ---------------------------------------------------------------------------------------------------------------
say "preflight"
((DRY)) && note "DRY RUN: nothing will be changed"
if ((EUID != 0)); then
  ((DRY)) || { echo "run as root (sudo)" >&2; exit 1; }
  note "not root: dry-run checks that need root are skipped"
fi
missing=()
for c in systemctl tailscale caddy openssl psql install sed; do command -v "$c" >/dev/null || missing+=("$c"); done
if ((!SKIP_BUILD)); then for c in cargo pnpm uv; do command -v "$c" >/dev/null || missing+=("$c"); done; fi
((${#missing[@]} == 0)) || { echo "missing commands: ${missing[*]}" >&2; exit 1; }
systemctl is-active --quiet postgresql || { echo "PostgreSQL is not running" >&2; exit 1; }
[[ -z $TS_HOST ]] && TS_HOST=$(tailscale status --json | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["Self"]["DNSName"].rstrip("."))')
[[ $TS_HOST =~ ^[a-z0-9.-]+\.ts\.net$ ]] || { echo "could not determine the Tailscale host name (use --ts-host)" >&2; exit 1; }
ORIGIN=https://$TS_HOST:$TS_PORT
note "repository:  $REPO ($(git -C "$REPO" rev-parse --short HEAD 2>/dev/null || echo '?'))"
note "pilot URL:   $ORIGIN  (tailnet only)"
note "database:    $DB    API: $API_BIND    web: $WEB_BIND"
for hp in "$API_BIND" "$WEB_BIND"; do
  if ss -ltnH "sport = :${hp##*:}" | grep -q . && ! systemctl is-active --quiet kizuna kizuna-web; then
    echo "port ${hp##*:} is already in use by something else" >&2; exit 1
  fi
done
avail=$(df -BG --output=avail / | tail -1 | tr -dc 0-9)
((avail >= 5)) || { echo "less than 5 GB free on /" >&2; exit 1; }
note "disk free:   ${avail} GB; RAM available: $(free -g | awk '/Mem:/{print $7}') GB (clamd needs ~1.5 GB)"

# ---------------------------------------------------------------------------------------------------------------
say "packages"
if ((SKIP_APT)); then
  note "skipped (--skip-apt)"
else
  need=()
  for p in "${APT_PKGS[@]}"; do dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' || need+=("$p"); done
  if ((${#need[@]})); then
    run apt-get update -q
    run env DEBIAN_FRONTEND=noninteractive apt-get install -y -q --no-install-recommends "${need[@]}"
  else
    note "all present: ${APT_PKGS[*]}"
  fi
fi

# ---------------------------------------------------------------------------------------------------------------
say "user and directories"
if id kizuna >/dev/null 2>&1; then
  note "user kizuna exists"
else
  run useradd --system --user-group --home-dir "$STATE" --shell /usr/sbin/nologin kizuna
fi
run install -d -o root -g kizuna -m 750 "$PREFIX" "$PREFIX/bin" "$PREFIX/scripts" "$PREFIX/web" "$PREFIX/extraction"
run install -d -o root -g kizuna -m 750 "$ETC"
run install -d -o root -g root -m 700 "$ETC/rclone" "$BACKUPS"
run install -d -o kizuna -g kizuna -m 700 "$STATE" "$STATE/objects" "$STATE/caddy" "$STATE/alerts"

# ---------------------------------------------------------------------------------------------------------------
say "secrets (/etc/kizuna, 0600; existing values kept)"
# Role passwords: /etc/kizuna/drill.env (previous install) → the repo's .env.local (same roles on this cluster,
# created by scripts/db-setup.sh) → new random ones (only when the roles do not exist yet).
OWNER_PW=$(env_get "$ETC/drill.env" KIZUNA_OWNER_PASSWORD || true)
APP_PW=$(env_get "$ETC/drill.env" KIZUNA_APP_PASSWORD || true)
src="$ETC/drill.env"
if [[ -z $OWNER_PW || -z $APP_PW ]]; then
  OWNER_PW=$(env_get "$REPO/.env.local" KIZUNA_OWNER_PASSWORD || true)
  APP_PW=$(env_get "$REPO/.env.local" KIZUNA_APP_PASSWORD || true)
  src="$REPO/.env.local"
fi
roles_exist=0
if ((EUID == 0)); then
  # shellcheck source=scripts/lib/db.sh
  source "$REPO/scripts/lib/db.sh"
  kz_role_exists kizuna && kz_role_exists kizuna_app && roles_exist=1
fi
if [[ -z $OWNER_PW || -z $APP_PW ]]; then
  if ((roles_exist && !RESET_PW)); then
    echo "roles kizuna/kizuna_app exist but their passwords are unknown (no $ETC/drill.env, no $REPO/.env.local)." >&2
    echo "Re-run with --reset-role-passwords (this breaks other users of those roles, e.g. dev .env.local)." >&2
    exit 1
  fi
  OWNER_PW=$(gen_pw) APP_PW=$(gen_pw) src="generated"
fi
note "database role passwords: from $src"
put "$ETC/drill.env" 600 root:root <<EOF
KIZUNA_OWNER_PASSWORD=$OWNER_PW
KIZUNA_APP_PASSWORD=$APP_PW
EOF
put "$ETC/admin.env" 600 root:root <<EOF
KIZUNA_OWNER_DATABASE_URL=postgres://kizuna:$OWNER_PW@127.0.0.1:5432/$DB
EOF
DATA_KEY=$(env_get "$ETC/kizuna.env" KIZUNA_DATA_KEY || true)
SIGN_KEY=$(env_get "$ETC/kizuna.env" KIZUNA_AUDIT_SIGNING_KEY || true)
[[ -n $DATA_KEY ]] || DATA_KEY=$(gen_key)
[[ -n $SIGN_KEY ]] || SIGN_KEY=$(gen_key)
put "$ETC/kizuna.env" 600 root:root <<EOF
# KIZUNA pilot server environment (generated by deploy/install-pilot.sh; secrets kept across re-runs).
# Escrow KIZUNA_DATA_KEY and KIZUNA_AUDIT_SIGNING_KEY offline: they are NOT in the backups.
KIZUNA_DATABASE_URL=postgres://kizuna_app:$APP_PW@127.0.0.1:5432/$DB
KIZUNA_DATA_KEY=$DATA_KEY
KIZUNA_AUDIT_SIGNING_KEY=$SIGN_KEY
KIZUNA_BIND=$API_BIND
# tailscaled (sets X-Forwarded-For to the tailnet client) -> local Caddy (appends 127.0.0.1) -> API
KIZUNA_TRUSTED_PROXY_HOPS=2
KIZUNA_WEBAUTHN_RP_ID=$TS_HOST
KIZUNA_WEBAUTHN_ORIGIN=$ORIGIN
KIZUNA_STORAGE_DIR=$STATE/objects
KIZUNA_EXTRACTION_PYTHON=$PREFIX/extraction/.venv/bin/python
KIZUNA_EXTRACTION_DIR=$PREFIX/extraction/src
KIZUNA_EXTRACTION_SANDBOX=bwrap
KIZUNA_CLAMD_SOCKET=$CLAMD_SOCKET
KIZUNA_LOG=info
EOF
if [[ -f $ETC/alerts.env ]]; then
  note "kept: $ETC/alerts.env"
else
  put "$ETC/alerts.env" 600 root:root <<'EOF'
# Operator alerts (ADR 0017). Fill in one or both channels, then: systemctl restart kizuna
# and test with: sudo systemctl start kizuna-alert@test.service ; journalctl -u kizuna-alert@test -n 5
KIZUNA_ALERT_LABEL=kizuna-pilot
# Telegram: create a bot with @BotFather, send it a message, read chat id from getUpdates.
#KIZUNA_ALERT_TELEGRAM_BOT_TOKEN=
#KIZUNA_ALERT_TELEGRAM_CHAT_ID=
# E-mail (e.g. Gmail with an app password: smtp.gmail.com, 587, starttls)
#KIZUNA_ALERT_SMTP_HOST=
#KIZUNA_ALERT_SMTP_PORT=587
#KIZUNA_ALERT_SMTP_TLS=starttls
#KIZUNA_ALERT_SMTP_USER=
#KIZUNA_ALERT_SMTP_PASSWORD=
#KIZUNA_ALERT_EMAIL_FROM=
#KIZUNA_ALERT_EMAIL_TO=
EOF
fi
if [[ -s $ETC/backup.pass ]]; then
  note "kept: $ETC/backup.pass"
elif ((DRY)); then
  note "would generate $ETC/backup.pass (600 root) — escrow it offline"
else
  (umask 077 && openssl rand -base64 48 > "$ETC/backup.pass")
  note "generated $ETC/backup.pass — escrow it offline (sealed envelope / password manager)"
fi
REMOTE=$(env_get "$ETC/backup.env" KIZUNA_RCLONE_REMOTE || true)
put "$ETC/backup.env" 600 root:root <<EOF
KIZUNA_BACKUP_PASSPHRASE_FILE=$ETC/backup.pass
RCLONE_CONFIG=$ETC/rclone/rclone.conf
# Off-host destination, e.g. gdrive:kizuna-pilot (docs/deploy/OFFSITE_RESTORE.md). Empty = local only.
KIZUNA_RCLONE_REMOTE=$REMOTE
EOF

# ---------------------------------------------------------------------------------------------------------------
say "build (release)"
if ((SKIP_BUILD)); then
  note "skipped (--skip-build): using $REPO/target/release and $REPO/web/dist"
else
  run cargo build --release --locked --manifest-path "$REPO/Cargo.toml" -p kizuna-api -p kizuna-bench \
    --bin kizuna-server --bin kizuna-admin --bin kizuna-alert --bin kizuna-restore-verify
  run pnpm --dir "$REPO/web" install --frozen-lockfile
  run pnpm --dir "$REPO/web" build
fi

# ---------------------------------------------------------------------------------------------------------------
say "install code to $PREFIX"
for b in kizuna-server kizuna-admin kizuna-alert kizuna-restore-verify; do
  if ((DRY)) && [[ ! -x $REPO/target/release/$b ]]; then note "would install $PREFIX/bin/$b (built above)"; continue; fi
  run install -m 755 -o root -g root "$REPO/target/release/$b" "$PREFIX/bin/$b"
done
run rm -rf "$PREFIX/scripts.new" "$PREFIX/web.new"
run cp -r "$REPO/scripts" "$PREFIX/scripts.new"
run rm -rf "$PREFIX/scripts.new/tests" "$PREFIX/scripts.new/e2e"
run cp -r "$REPO/web/dist" "$PREFIX/web.new"
for d in scripts web; do
  run rm -rf "$PREFIX/$d.old"
  if [[ -e $PREFIX/$d ]]; then run mv "$PREFIX/$d" "$PREFIX/$d.old"; fi
  run mv "$PREFIX/$d.new" "$PREFIX/$d"
  run rm -rf "$PREFIX/$d.old"
done
run install -m 644 "$REPO/services/extraction/pyproject.toml" "$REPO/services/extraction/uv.lock" "$PREFIX/extraction/"
run rm -rf "$PREFIX/extraction/src"
run cp -r "$REPO/services/extraction/src" "$PREFIX/extraction/src"
# Venv on the system interpreter (readable by kizuna and bindable into the sandbox); no dev dependencies.
run env UV_PROJECT_ENVIRONMENT="$PREFIX/extraction/.venv" UV_PYTHON_DOWNLOADS=never \
  uv sync --directory "$PREFIX/extraction" --frozen --no-dev --python /usr/bin/python3 -q
run find "$PREFIX/extraction" -name __pycache__ -prune -exec rm -rf {} +
run chown -R root:kizuna "$PREFIX"
run chmod -R u=rwX,g=rX,o= "$PREFIX"
if ((DRY)); then note "would record $PREFIX/VERSION"; else git -C "$REPO" rev-parse HEAD > "$PREFIX/VERSION" || true; fi

# ---------------------------------------------------------------------------------------------------------------
say "database $DB"
if ((EUID != 0)); then
  note "skipped (not root)"
else
  if ((DRY)); then
    note "would ensure roles kizuna/kizuna_app (passwords from $src) and database $DB"
    if [[ $(kz_pg -c "SELECT 1 FROM pg_database WHERE datname = '$DB'") == 1 ]]; then note "database $DB exists"; fi
  else
    kz_ensure_roles "$OWNER_PW" "$APP_PW"
    kz_ensure_db "$DB"
    note "roles and $DB ready"
  fi
  if ((!DRY)); then
    KIZUNA_OWNER_DATABASE_URL="postgres://kizuna:$OWNER_PW@127.0.0.1:5432/$DB" "$PREFIX/bin/kizuna-admin" migrate
  else
    note "would run: kizuna-admin migrate (as owner role)"
  fi
fi

# ---------------------------------------------------------------------------------------------------------------
say "web front (Caddy config $ETC/Caddyfile) and systemd units"
VAL=$(mktemp -d)
trap 'rm -rf "$VAL"' EXIT
render "$REPO/deploy/Caddyfile.pilot" > "$VAL/Caddyfile"
if caddy validate --adapter caddyfile --config "$VAL/Caddyfile" >"$VAL/caddy.log" 2>&1; then
  note "Caddyfile valid"
else
  cat "$VAL/caddy.log" >&2; echo "Caddyfile invalid" >&2; exit 1
fi
put "$ETC/Caddyfile" 640 root:kizuna < "$VAL/Caddyfile"
for u in "${UNITS[@]}"; do render "$REPO/deploy/systemd/$u" > "$VAL/$u"; done
# Verify units; binaries under /opt/kizuna may not exist yet in a dry run, which systemd-analyze reports.
systemd-analyze verify "${UNITS[@]/#/$VAL/}" >"$VAL/verify.log" 2>&1 || true
# Only findings about our units count; binaries under /opt/kizuna do not exist yet on a first (dry) run.
if grep -E "^(kizuna|$VAL)" "$VAL/verify.log" | grep -v -E "Command $PREFIX/[^ ]+ is not executable" | grep -q .; then
  cat "$VAL/verify.log" >&2; echo "systemd unit verification failed" >&2; exit 1
fi
note "systemd units verified (systemd-analyze verify)"
for u in "${UNITS[@]}"; do put "$UNIT_DIR/$u" 644 root:root < "$VAL/$u"; done
run systemctl daemon-reload

# ---------------------------------------------------------------------------------------------------------------
say "ClamAV"
run systemctl enable --now clamav-freshclam.service
run systemctl enable --now clamav-daemon.service
note "clamd starts once freshclam has downloaded signatures (minutes); until then uploads are QUARANTINED (fail closed)"

say "services"
run systemctl enable kizuna.service kizuna-web.service kizuna-backup.timer kizuna-integrity.timer
run systemctl restart kizuna.service kizuna-web.service
run systemctl start kizuna-backup.timer kizuna-integrity.timer

say "Tailscale Serve (tailnet only) https://$TS_HOST:$TS_PORT -> http://$WEB_BIND"
if tailscale serve status 2>/dev/null | grep -A1 -F ":$TS_PORT" | grep -qF "http://$WEB_BIND"; then
  note "already configured"
else
  run tailscale serve --bg --https="$TS_PORT" "http://$WEB_BIND"
fi

# ---------------------------------------------------------------------------------------------------------------
say "checks"
if ((DRY)); then
  note "would check: http://$API_BIND/healthz, http://$WEB_BIND/healthz, $ORIGIN/healthz, GET /api/v1/security/status"
else
  for i in $(seq 1 30); do curl -fsS "http://$API_BIND/healthz" >/dev/null 2>&1 && break; sleep 1; [[ $i == 30 ]] && { journalctl -u kizuna -n 30 --no-pager >&2; exit 1; }; done
  note "API healthy"
  curl -fsS "http://$WEB_BIND/healthz" >/dev/null && note "web front healthy"
  if curl -fsS "$ORIGIN/healthz" >/dev/null; then note "tailnet URL healthy"; else note "tailnet URL not reachable yet (check: tailscale serve status)"; fi
  if systemctl is-active --quiet clamav-daemon; then note "clamd active"; else note "clamd not active yet (freshclam still downloading?)"; fi
fi

say "next steps"
cat <<EOF
   1. First tenant + admin (password on stdin; admin must enrol TOTP at first login):
        set -a; . $ETC/admin.env; set +a
        $PREFIX/bin/kizuna-admin create-tenant <slug> "<Company>"
        $PREFIX/bin/kizuna-admin create-user <slug> admin@<company> tenant_admin "<Name>"
   2. Open $ORIGIN from a device on the tailnet.
   3. Alerts: edit $ETC/alerts.env, then: systemctl restart kizuna; systemctl start kizuna-alert@test.service
   4. Off-host backups: configure rclone + KIZUNA_RCLONE_REMOTE (docs/deploy/OFFSITE_RESTORE.md), then
        systemctl start kizuna-backup.service; journalctl -u kizuna-backup -n 20
   5. Escrow offline: $ETC/backup.pass, KIZUNA_DATA_KEY and KIZUNA_AUDIT_SIGNING_KEY from $ETC/kizuna.env.
   6. Status: GET /api/v1/security/status (antivirus, seccomp, alerts); journalctl -u kizuna -u kizuna-web

   Later public domain: add to /etc/caddy/Caddyfile (founder decision; this script never edits it), set
   KIZUNA_WEBAUTHN_RP_ID/ORIGIN to the domain in $ETC/kizuna.env (existing passkeys are bound to the old origin):

        verify.example.com {
        	encode zstd gzip
        	request_body {
        		max_size 26MB
        	}
        	reverse_proxy $WEB_BIND
        	log {
        		output file /var/log/caddy/kizuna.log
        	}
        }
EOF
((DRY)) && say "dry run complete: no changes made"
exit 0
