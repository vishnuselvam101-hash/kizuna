#!/usr/bin/env bash
# Stability soak (STABILITY_2026-11-06): release kizuna-server against an EMPTY database you provide (a private
# PostgreSQL instance; never the system one), mixed load incl. idempotency retries, background jobs, supplier portal
# and CAPA 8D; metrics scraped from the loopback metrics port; afterwards integrity checks and a SIGTERM drain.
# Usage: scripts/chaos/stability-soak.sh WORKDIR MINUTES
# Env: KIZUNA_OWNER_DATABASE_URL, KIZUNA_DATABASE_URL (runtime role), SOAK_SEEDS (default 200), SOAK_WORKERS (8).
set -euo pipefail
cd "$(dirname "$0")/../.."
WORK=${1:?usage: stability-soak.sh WORKDIR MINUTES}
MINUTES=${2:?usage: stability-soak.sh WORKDIR MINUTES}
: "${KIZUNA_OWNER_DATABASE_URL:?}" "${KIZUNA_DATABASE_URL:?}"
SEEDS=${SOAK_SEEDS:-200}
WORKERS=${SOAK_WORKERS:-8}
mkdir -p "$WORK"
BIN=target/release
SERVER_PID=
# shellcheck disable=SC2329  # invoked by the EXIT trap
cleanup() { [[ -n $SERVER_PID ]] && kill "$SERVER_PID" 2>/dev/null || true; }
trap cleanup EXIT

echo "== build (release)"
cargo build -q --release --bin kizuna-server --bin kizuna-admin --bin kizuna-restore-verify

echo "== migrate + tenants/users"
$BIN/kizuna-admin migrate
PW=$(openssl rand -hex 16)
$BIN/kizuna-admin create-tenant soak-mfg "Soak Manufacturing KK" >/dev/null
$BIN/kizuna-admin create-tenant soak-other "Other Tenant" >/dev/null
declare -A USERS=([analyst]=analyst [reviewer]=reviewer [reviewer2]=reviewer [viewer]=viewer [auditor]=auditor
                  [admin]=tenant_admin)
users_json=""
for u in "${!USERS[@]}"; do
  echo "$PW" | $BIN/kizuna-admin create-user soak-mfg "$u@soak.example" "${USERS[$u]}" "Soak $u" >/dev/null
  users_json+="\"$u\":\"$u@soak.example\","
done
echo "$PW" | $BIN/kizuna-admin create-user soak-other "outsider@soak.example" reviewer "Outsider" >/dev/null
users_json+="\"outsider\":\"outsider@soak.example\""
rm -f "$WORK/creds.json.totp.json"
(umask 077; printf '{"tenant":"soak-mfg","other_tenant":"soak-other","password":"%s","users":{%s}}' "$PW" "$users_json" \
  > "$WORK/creds.json")

echo "== datasets ($SEEDS seeds)"
if [[ ! -d $WORK/data/s$SEEDS ]]; then
  for s in $(seq 1 "$SEEDS"); do python3 -I scripts/e2e/gen_dataset.py --seed "$s" --parts 24 --out "$WORK/data/s$s" >/dev/null; done
fi

port() { python3 -I -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
PORT=$(port) MPORT=$(port)
echo "== kizuna-server on 127.0.0.1:$PORT (metrics 127.0.0.1:$MPORT)"
[[ $(id -u) == 0 ]] && export KIZUNA_ALLOW_ROOT=1
KIZUNA_BIND=127.0.0.1:$PORT KIZUNA_METRICS_BIND=127.0.0.1:$MPORT KIZUNA_STORAGE_DIR=$WORK/objects \
  KIZUNA_DATA_KEY=$(openssl rand -base64 32) KIZUNA_UPLOADS_PER_USER_PER_MIN=1000 KIZUNA_LOG=warn KIZUNA_DEV=1 \
  KIZUNA_EXTRACTION_PYTHON=$PWD/services/extraction/.venv/bin/python KIZUNA_EXTRACTION_DIR=$PWD/services/extraction/src \
  $BIN/kizuna-server >"$WORK/server.log" 2>&1 &
SERVER_PID=$!
for _ in $(seq 60); do curl -fsS "http://127.0.0.1:$PORT/readyz" >/dev/null 2>&1 && break; sleep 0.5; done
curl -fsS "http://127.0.0.1:$PORT/readyz"; echo

echo "== soak $MINUTES min"
python3 -I scripts/chaos/soak.py --base "http://127.0.0.1:$PORT" --creds "$WORK/creds.json" --datasets "$WORK/data" \
  --minutes "$MINUTES" --workers "$WORKERS" --pid "$SERVER_PID" --out "$WORK/result.json" --stability \
  --metrics "http://127.0.0.1:$MPORT/metrics" --psql-cmd "psql $KIZUNA_DATABASE_URL" \
  --owner-psql-cmd "psql $KIZUNA_OWNER_DATABASE_URL"
curl -fsS "http://127.0.0.1:$MPORT/metrics" > "$WORK/metrics-end.txt"

echo "== jobs drain (60 s), then SIGTERM"
for _ in $(seq 60); do
  q=$(awk '/^kizuna_jobs_queue_depth\{state="(queued|running)"\}/ {s+=$2} END {print s+0}' \
      <(curl -fsS "http://127.0.0.1:$MPORT/metrics"))
  [[ $q == 0 ]] && break; sleep 1
done
curl -fsS "http://127.0.0.1:$MPORT/metrics" > "$WORK/metrics-drained.txt"
kill -TERM "$SERVER_PID"; set +e; wait "$SERVER_PID"; echo "server exit: $?" | tee "$WORK/exit.txt"; set -e; SERVER_PID=

echo "== integrity"
$BIN/kizuna-restore-verify "$WORK/objects" > "$WORK/restore-verify.json" || true
python3 -I -c '
import json, sys
d = json.load(open(sys.argv[1])); t = d["tenants"]
print(json.dumps({"ok": d["ok"], "chains_intact": all(x["audit_chain"]["intact"] for x in t),
  "artifacts": sum(x["artifacts"] for x in t), "artifacts_intact": sum(x["artifacts_intact"] for x in t)}))' \
  "$WORK/restore-verify.json" | tee "$WORK/integrity.json"
psql "$KIZUNA_OWNER_DATABASE_URL" -qAt -F' ' > "$WORK/jobs-states.txt" <<'SQL'
SELECT kind, state, count(*), max(attempts) FROM jobs GROUP BY 1, 2 ORDER BY 1, 2;
SQL
cat "$WORK/jobs-states.txt"
