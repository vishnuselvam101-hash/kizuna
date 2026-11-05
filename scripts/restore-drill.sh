#!/usr/bin/env bash
# Restore drill: restores a backup from scripts/backup.sh into a THROWAWAY database and object directory, then
# verifies every tenant's audit hash chain and every artifact object hash with the server's own code
# (kizuna-restore-verify). Never touches the source database. Prints a JSON summary incl. measured restore time.
# Usage: scripts/restore-drill.sh BACKUP.tar.gpg [--tamper-test] [--report DIR]
#   --tamper-test  after a clean verification, alter one restored object and require verification to FAIL
#                  (negative control: proves the drill can detect damage).
# Env: KIZUNA_BACKUP_PASSPHRASE_FILE; KIZUNA_RESTORE_VERIFY_BIN (prebuilt verifier; else built with cargo); roles from scripts/db-setup.sh (passwords via KIZUNA_ENV_FILE/.env.local).
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/lib/throwaway.sh
BK=${1:?usage: restore-drill.sh BACKUP.tar.gpg [--tamper-test] [--report DIR]}; shift
TAMPER=0 REPORT=
while (($#)); do
  case $1 in
    --tamper-test) TAMPER=1; shift ;;
    --report) REPORT=$2; shift 2 ;;
    *) echo "unknown arg $1" >&2; exit 64 ;;
  esac
done
PASS=${KIZUNA_BACKUP_PASSPHRASE_FILE:?set KIZUNA_BACKUP_PASSPHRASE_FILE}
kz_load_env
VERIFY=${KIZUNA_RESTORE_VERIFY_BIN:-}
if [[ -z $VERIFY ]]; then
  cargo build -q --release -p kizuna-bench --bin kizuna-restore-verify
  VERIFY=target/release/kizuna-restore-verify
fi
T0=$(date +%s.%N)

echo "== checksum" >&2
(cd "$(dirname "$BK")" && sha256sum --quiet -c "$(basename "$BK").sha256") >&2

umask 077
WORK=$(mktemp -d "${TMPDIR:-/tmp}/kizuna-drill.XXXXXX")
DB=kizuna_drill_$(date +%s)_$$
trap 'kz_drop_db "$DB" >/dev/null 2>&1 || true; rm -rf "$WORK"' EXIT

echo "== decrypt" >&2
gpg --batch --quiet --no-tty --pinentry-mode loopback --passphrase-file "$PASS" -d "$BK" | tar -C "$WORK" -xf -
python3 -I - "$WORK" <<'PY' >&2
import hashlib, json, sys, pathlib
d = pathlib.Path(sys.argv[1]); m = json.load(open(d / "MANIFEST.json"))
assert m["format"] == "kizuna-backup@1", m["format"]
for p, meta in m["files"].items():
    got = hashlib.sha256((d / p).read_bytes()).hexdigest()
    if got != meta["sha256"]:
        sys.exit(f"manifest mismatch for {p}")
print(f"manifest ok: source={m['source_db']} migration={m['migration_version']} taken={m['utc']}")
PY

echo "== restore into $DB" >&2
kz_create_db "$DB"
chmod 711 "$WORK"; chown postgres "$WORK/db.dump"
sudo -u postgres pg_restore --exit-on-error -d "$DB" "$WORK/db.dump"
mkdir "$WORK/objects"
tar -C "$WORK/objects" -xf "$WORK/objects.tar"
T1=$(date +%s.%N)

echo "== verify (audit chains + artifact hashes)" >&2
set +e
KIZUNA_OWNER_DATABASE_URL=$(kz_owner_url "$DB") $VERIFY "$WORK/objects" >"$WORK/verify.json"
rc=$?
set -e
T2=$(date +%s.%N)
neg=null
if ((TAMPER)) && ((rc == 0)); then
  victim=$(find "$WORK/objects" -type f | sort | head -1)
  printf 'X' | dd of="$victim" bs=1 seek=0 conv=notrunc status=none
  set +e
  KIZUNA_OWNER_DATABASE_URL=$(kz_owner_url "$DB") $VERIFY "$WORK/objects" >/dev/null
  trc=$?
  set -e
  if ((trc == 2)); then neg='"detected"'; else neg='"NOT DETECTED"'; rc=3; fi
fi
SUMMARY=$(python3 -I - "$WORK/verify.json" "$BK" "$T0" "$T1" "$T2" "$rc" "$neg" <<'PY'
import json, sys, datetime
v = json.load(open(sys.argv[1]))
t0, t1, t2 = map(float, sys.argv[3:6])
print(json.dumps({"backup": sys.argv[2], "utc": datetime.datetime.now(datetime.UTC).isoformat(timespec="seconds"),
  "restore_seconds": round(t1 - t0, 2), "verify_seconds": round(t2 - t1, 2), "exit": int(sys.argv[6]),
  "tamper_test": json.loads(sys.argv[7]), "verification": v}, indent=1))
PY
)
echo "$SUMMARY"
if [[ -n $REPORT ]]; then mkdir -p "$REPORT"; echo "$SUMMARY" > "$REPORT/drill-$(date -u +%Y%m%dT%H%M%SZ).json"; fi
exit $rc
