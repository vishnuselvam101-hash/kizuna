#!/usr/bin/env bash
# End-to-end run: real kizuna-server + throwaway PostgreSQL DB + synthetic manufacturing data with injected faults.
# Usage: scripts/e2e.sh [--seed N] [--parts N] [--keep]   (needs scripts/db-setup.sh roles; run as root for sudo -u postgres)
# Writes reports/e2e/e2e-seed<N>.json (real run only). Drops the database and storage on exit unless --keep.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/lib/throwaway.sh
SEED=7 PARTS=24 KEEP=0
while (($#)); do
  case $1 in
    --seed) SEED=$2; shift 2 ;;
    --parts) PARTS=$2; shift 2 ;;
    --keep) KEEP=1; shift ;;
    *) echo "unknown arg $1" >&2; exit 64 ;;
  esac
done
kz_load_env
kz_extraction_env

DB=kizuna_e2e_$(date +%s)_$$
WORK=$(mktemp -d "${TMPDIR:-/tmp}/kizuna-e2e.XXXXXX")
SERVER_PID=
# shellcheck disable=SC2329  # invoked by the EXIT trap
cleanup() {
  [[ -n $SERVER_PID ]] && kill "$SERVER_PID" 2>/dev/null && wait "$SERVER_PID" 2>/dev/null || true
  if ((KEEP)); then echo "kept: db=$DB work=$WORK"; else kz_drop_db "$DB" || true; rm -rf "$WORK"; fi
}
trap cleanup EXIT

echo "== build"
cargo build -q --bin kizuna-server --bin kizuna-admin
BIN=target/debug

echo "== database $DB"
kz_create_db "$DB"
export KIZUNA_OWNER_DATABASE_URL; KIZUNA_OWNER_DATABASE_URL=$(kz_owner_url "$DB")
$BIN/kizuna-admin migrate

PW=$(openssl rand -hex 16)
$BIN/kizuna-admin create-tenant e2e-mfg "Synthetic Manufacturing KK" >/dev/null
$BIN/kizuna-admin create-tenant e2e-other "Other Tenant" >/dev/null
declare -A USERS=([analyst]=analyst [reviewer]=reviewer [reviewer2]=reviewer [viewer]=viewer [auditor]=auditor)
users_json=""
for u in "${!USERS[@]}"; do
  echo "$PW" | $BIN/kizuna-admin create-user e2e-mfg "$u@e2e.example" "${USERS[$u]}" "E2E $u" >/dev/null
  users_json+="\"$u\":\"$u@e2e.example\","
done
echo "$PW" | $BIN/kizuna-admin create-user e2e-other "outsider@e2e.example" reviewer "Outsider" >/dev/null
users_json+="\"outsider\":\"outsider@e2e.example\""
(umask 077; printf '{"tenant":"e2e-mfg","other_tenant":"e2e-other","password":"%s","users":{%s}}' "$PW" "$users_json" > "$WORK/creds.json")

echo "== dataset (seed $SEED, $PARTS parts)"
python3 -I scripts/e2e/gen_dataset.py --seed "$SEED" --parts "$PARTS" --out "$WORK/data"

PORT=$(kz_free_port)
echo "== kizuna-server on 127.0.0.1:$PORT"
KIZUNA_DATABASE_URL=$(kz_app_url "$DB") KIZUNA_BIND=127.0.0.1:$PORT KIZUNA_STORAGE_DIR=$WORK/objects \
  KIZUNA_DATA_KEY=$(openssl rand -base64 32) KIZUNA_UPLOADS_PER_USER_PER_MIN=1000 KIZUNA_LOG=warn \
  $BIN/kizuna-server >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
kz_wait_http "http://127.0.0.1:$PORT/healthz" 30

echo "== workflow"
mkdir -p reports/e2e
OUT=reports/e2e/e2e-seed$SEED.json
set +e
python3 -I scripts/e2e/driver.py --base "http://127.0.0.1:$PORT" --data "$WORK/data" --creds "$WORK/creds.json" --out "$WORK/result.json"
rc=$?
set -e
python3 -I - "$WORK/result.json" "$OUT" "$SEED" "$PARTS" <<'PY'
import json, platform, subprocess, sys, datetime
r = json.load(open(sys.argv[1]))
r["provenance"] = {
    "command": f"scripts/e2e.sh --seed {sys.argv[3]} --parts {sys.argv[4]}",
    "commit": subprocess.run(["git", "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip(),
    "dirty": bool(subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True).stdout.strip()),
    "utc": datetime.datetime.now(datetime.UTC).isoformat(timespec="seconds"),
    "host": platform.platform(), "build": "debug",
}
json.dump(r, open(sys.argv[2], "w"), indent=1)
PY
echo "result: $OUT (exit $rc)"
grep -iE 'error|panic' "$WORK/server.log" | tail -5 || true
exit $rc
