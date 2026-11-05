#!/usr/bin/env bash
# Integrity sweep of the LIVE pilot (read-only): every tenant's audit hash chain + every stored artifact's SHA-256,
# using the server's own verifier (kizuna-restore-verify). Run by kizuna-integrity.timer.
# Usage: scripts/integrity-sweep.sh STORAGE_DIR   Env: KIZUNA_OWNER_DATABASE_URL; KIZUNA_RESTORE_VERIFY_BIN.
# Exit 0 = intact (or no tenants yet); 2 = damage found (systemd OnFailure= then alerts); other = could not run.
# The full JSON report goes to the journal only (artifact ids, no document content).
set -euo pipefail
STORAGE=${1:?usage: integrity-sweep.sh STORAGE_DIR}
BIN=${KIZUNA_RESTORE_VERIFY_BIN:-/opt/kizuna/bin/kizuna-restore-verify}
: "${KIZUNA_OWNER_DATABASE_URL:?KIZUNA_OWNER_DATABASE_URL not set}"
set +e
out=$("$BIN" "$STORAGE")
rc=$?
set -e
summary=$(python3 -I -c '
import json, sys
d = json.loads(sys.stdin.read())
t = d["tenants"]
print(json.dumps({"ok": d["ok"], "tenants": len(t),
  "chains_intact": sum(1 for x in t if x["audit_chain"]["intact"]),
  "artifacts": sum(x["artifacts"] for x in t), "artifacts_intact": sum(x["artifacts_intact"] for x in t)}))
' <<<"$out" 2>/dev/null) || { echo "integrity sweep: verifier failed (exit $rc)" >&2; exit 1; }
echo "$summary"
if ((rc == 2)) && [[ $summary == *'"tenants": 0'* ]]; then
  echo "integrity sweep: no tenants yet, nothing to verify"; exit 0
fi
((rc == 0)) || { echo "$out" >&2; exit "$rc"; }
