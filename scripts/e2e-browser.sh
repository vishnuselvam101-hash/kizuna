#!/usr/bin/env bash
# Browser acceptance run: release kizuna-server + throwaway PostgreSQL DB + built web app (vite preview, no mocks)
# + local mock AI provider, driven by Playwright/Chromium (web/e2e). EN and JA journeys run in separate tenants.
#
# Usage: scripts/e2e-browser.sh [--keep] [--up] [--down WORK] [-- <playwright args>]
#   (default)    up, run every Playwright project, down
#   --up         start everything, print WORK dir (env in $WORK/env.sh), leave it running
#   --down WORK  stop the processes started by --up and drop its database
#   --restart-server WORK  rebuild + restart kizuna-server of a --up environment (keeps DB and keys)
#   --keep       on a full run, keep DB, logs and processes' work dir (processes are still stopped)
# Needs the roles from scripts/db-setup.sh; run as root (sudo -u postgres for the throwaway DB).
# Results: web/e2e/results/ (screenshots, traces, report.json). Never touches any other database or service.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/lib/throwaway.sh
KEEP=0 MODE=full DOWN_WORK=
PW_ARGS=()
while (($#)); do
  case $1 in
    --keep) KEEP=1; shift ;;
    --up) MODE=up; shift ;;
    --down) MODE=down; DOWN_WORK=$2; shift 2 ;;
    --restart-server) MODE=restart; DOWN_WORK=$2; shift 2 ;;
    --) shift; PW_ARGS=("$@"); break ;;
    *) echo "unknown arg $1" >&2; exit 64 ;;
  esac
done
kz_load_env

down() {
  local work=$1
  [[ -f $work/server.pid ]] && kill "$(cat "$work/server.pid")" 2>/dev/null
  [[ -f $work/pids ]] && while read -r pid; do kill "$pid" 2>/dev/null || true; done <"$work/pids"
  sleep 0.5
  local db
  db=$(cat "$work/db" 2>/dev/null || true)
  [[ -n $db ]] && { kz_drop_db "$db" || true; }
  echo "stopped; dropped ${db:-<none>}"
}
if [[ $MODE == restart ]]; then
  # Rebuilds and restarts only kizuna-server of a --up environment (same DB, keys and storage).
  WORK=$DOWN_WORK BIN=${CARGO_TARGET_DIR:-target}/release
  [[ $(id -u) == 0 ]] && export KIZUNA_ALLOW_ROOT=1
  kz_extraction_env
  cargo build -q --release --bin kizuna-server
  kill "$(cat "$WORK/server.pid")" 2>/dev/null; sleep 1
  # shellcheck disable=SC1091
  (source "$WORK/server.env"; exec nohup "$BIN/kizuna-server" >>"$WORK/server.log" 2>&1 </dev/null) &
  echo $! >"$WORK/server.pid"
  # shellcheck disable=SC1091
  source "$WORK/server.env"; kz_wait_http "http://$KIZUNA_BIND/healthz" 30; echo restarted; exit 0
fi
if [[ $MODE == down ]]; then down "$DOWN_WORK"; rm -rf "$DOWN_WORK"; exit 0; fi

kz_extraction_env
[[ $(id -u) == 0 ]] && export KIZUNA_ALLOW_ROOT=1
BIN=${CARGO_TARGET_DIR:-target}/release
echo "== build (release)"
cargo build -q --release --bin kizuna-server --bin kizuna-admin

DB=kizuna_e2e_browser_$(date +%s)_$$
WORK=$(mktemp -d "${TMPDIR:-/tmp}/kizuna-browser.XXXXXX")
echo "$DB" >"$WORK/db"
: >"$WORK/pids"
if [[ $MODE == full ]]; then
  # shellcheck disable=SC2329  # invoked by the EXIT trap
  cleanup() {
    down "$WORK"
    if ((KEEP)); then echo "kept: $WORK"; else rm -rf "$WORK"; fi
  }
  trap cleanup EXIT
else
  # shellcheck disable=SC2329
  failed_up() {
    local rc=$?
    if ((rc)); then down "$WORK"; rm -rf "$WORK"; fi
    exit "$rc"
  }
  trap failed_up EXIT
fi

echo "== database $DB"
kz_create_db "$DB"
export KIZUNA_OWNER_DATABASE_URL; KIZUNA_OWNER_DATABASE_URL=$(kz_owner_url "$DB")
"$BIN/kizuna-admin" migrate >/dev/null

PW=$(openssl rand -hex 16)
USERS=(admin:tenant_admin admin2:tenant_admin reviewer:reviewer reviewer2:reviewer analyst:analyst auditor:auditor
  viewer:viewer keyuser:analyst victim:viewer)
for tenant in e2e-en e2e-ja; do
  "$BIN/kizuna-admin" create-tenant "$tenant" "Synthetic Manufacturing KK ($tenant)" >/dev/null
  for ur in "${USERS[@]}"; do
    echo "$PW" | "$BIN/kizuna-admin" create-user "$tenant" "${ur%%:*}@e2e.example" "${ur#*:}" "E2E ${ur%%:*}" >/dev/null
  done
done
(umask 077; printf '{"tenants":{"en":"e2e-en","ja":"e2e-ja"},"password":"%s","domain":"e2e.example"}' "$PW" >"$WORK/creds.json")

echo "== documents"
python3 -I scripts/e2e/gen_dataset.py --seed 7 --parts 7 --out "$WORK/data" >/dev/null
python3 -I scripts/e2e/gen_ja_docs.py "$WORK/data/ja"

echo "== mock AI provider"
AI_PORT=$(kz_free_port)
MOCK_AI_LOG=$WORK/ai.log nohup python3 -I scripts/e2e/mock_ai.py "$AI_PORT" >"$WORK/ai.out" 2>&1 </dev/null &
echo $! >>"$WORK/pids"

API_PORT=$(kz_free_port)
WEB_PORT=$(kz_free_port)
echo "== kizuna-server on 127.0.0.1:$API_PORT"
(umask 077; cat >"$WORK/server.env" <<SENV
export KIZUNA_DATABASE_URL=$(kz_app_url "$DB") KIZUNA_BIND=127.0.0.1:$API_PORT KIZUNA_STORAGE_DIR=$WORK/objects
export KIZUNA_DATA_KEY=$(openssl rand -base64 32) KIZUNA_AUDIT_SIGNING_KEY=$(openssl rand -base64 32)
export KIZUNA_WEBAUTHN_RP_ID=localhost KIZUNA_WEBAUTHN_ORIGIN=http://localhost:$WEB_PORT
export KIZUNA_AI_BASE_URL=http://127.0.0.1:$AI_PORT/v1 KIZUNA_AI_MODEL=mock-local
export KIZUNA_UPLOADS_PER_USER_PER_MIN=1000 KIZUNA_LOGIN_PER_IP_PER_MIN=1000 KIZUNA_LOG=warn
SENV
)
start_server() {
  # shellcheck disable=SC1091
  (source "$WORK/server.env"; exec nohup "$BIN/kizuna-server" >>"$WORK/server.log" 2>&1 </dev/null) &
  echo $! >"$WORK/server.pid"
}
start_server
kz_wait_http "http://127.0.0.1:$API_PORT/healthz" 30

echo "== web (vite build + preview on localhost:$WEB_PORT)"
(cd web && node_modules/.bin/tsc --noEmit && node_modules/.bin/vite build --logLevel warn)
(cd web && KIZUNA_API=http://127.0.0.1:$API_PORT WEB_PORT=$WEB_PORT exec nohup node_modules/.bin/vite preview \
  --config e2e/preview.config.mjs >"$WORK/web.log" 2>&1 </dev/null) &
echo $! >>"$WORK/pids"
kz_wait_http "http://127.0.0.1:$WEB_PORT/" 30

cat >"$WORK/env.sh" <<ENV
export E2E_BASE_URL=http://localhost:$WEB_PORT E2E_WORK=$WORK E2E_CREDS=$WORK/creds.json E2E_DATA=$WORK/data
export E2E_AI_LOG=$WORK/ai.log E2E_SERVER_LOG=$WORK/server.log
ENV
if [[ $MODE == up ]]; then echo "up: WORK=$WORK  (source \$WORK/env.sh; stop with --down \$WORK)"; exit 0; fi

echo "== playwright"
# shellcheck disable=SC1091
source "$WORK/env.sh"
set +e
(cd web && node_modules/.bin/playwright test -c e2e/playwright.config.ts "${PW_ARGS[@]}")
rc=$?
set -e
grep -iE 'panic' "$WORK/server.log" | tail -5 || true
echo "playwright exit $rc; report: web/e2e/results/"
exit $rc
