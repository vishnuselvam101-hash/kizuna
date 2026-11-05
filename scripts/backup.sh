#!/usr/bin/env bash
# Encrypted backup of one KIZUNA database + its object store.
# Usage: scripts/backup.sh --db NAME --storage DIR --out DIR
# Env:   KIZUNA_BACKUP_PASSPHRASE_FILE  file (mode 600) holding the backup passphrase — keep a copy OFF this host.
#
# Output: DIR/kizuna-<db>-<UTC>.tar.gpg and DIR/kizuna-<db>-<UTC>.tar.gpg.sha256
#   inside the (GnuPG symmetric, AES-256, integrity-protected) archive: db.dump (pg_dump custom format),
#   objects.tar (object store), MANIFEST.json (SHA-256 of both, source db, migration version, time).
# pg_dump runs as the postgres superuser: KIZUNA tables use FORCE ROW LEVEL SECURITY, so neither the owner nor the
# runtime role can dump every tenant's rows.
set -euo pipefail
cd "$(dirname "$0")/.."
DB='' STORAGE='' OUT=''
while (($#)); do
  case $1 in
    --db) DB=$2; shift 2 ;;
    --storage) STORAGE=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    *) echo "unknown arg $1" >&2; exit 64 ;;
  esac
done
[[ $DB =~ ^kizuna_[a-z0-9_]+$ ]] || { echo "--db must be a kizuna_* database" >&2; exit 64; }
[[ -d $STORAGE ]] || { echo "--storage must be the object store directory" >&2; exit 64; }
[[ -n $OUT ]] || { echo "--out required" >&2; exit 64; }
PASS=${KIZUNA_BACKUP_PASSPHRASE_FILE:?set KIZUNA_BACKUP_PASSPHRASE_FILE}
[[ $(stat -c %a "$PASS") == 600 || $(stat -c %a "$PASS") == 400 ]] || { echo "$PASS must be mode 600/400" >&2; exit 1; }

umask 077
mkdir -p "$OUT"
STAGE=$(mktemp -d "${TMPDIR:-/tmp}/kizuna-backup.XXXXXX")
trap 'rm -rf "$STAGE"' EXIT
TS=$(date -u +%Y%m%dT%H%M%SZ)
NAME=kizuna-$DB-$TS
# runuser when root (works under systemd NoNewPrivileges=yes, which makes sudo refuse); sudo otherwise.
as_postgres() {
  if ((EUID == 0)) && command -v runuser >/dev/null; then runuser -u postgres -- "$@"; else sudo -u postgres "$@"; fi
}

# postgres writes into a pre-created 600 file it owns; the directory only needs traverse permission.
chmod 711 "$STAGE"
touch "$STAGE/db.dump"; chown postgres "$STAGE/db.dump"
as_postgres pg_dump -Fc --no-password -d "$DB" -f "$STAGE/db.dump"
chown "$(id -u):$(id -g)" "$STAGE/db.dump"
tar -C "$STORAGE" -cf "$STAGE/objects.tar" .
MIG=$(as_postgres psql -qAt -d "$DB" -c "SELECT coalesce(max(version),0) FROM _sqlx_migrations WHERE success")
python3 -I - "$STAGE" "$DB" "$MIG" "$TS" <<'PY'
import hashlib, json, sys, pathlib
d = pathlib.Path(sys.argv[1])
h = lambda p: hashlib.sha256((d / p).read_bytes()).hexdigest()
json.dump({"format": "kizuna-backup@1", "source_db": sys.argv[2], "migration_version": int(sys.argv[3]), "utc": sys.argv[4],
           "files": {p: {"sha256": h(p), "bytes": (d / p).stat().st_size} for p in ("db.dump", "objects.tar")}},
          open(d / "MANIFEST.json", "w"), indent=1)
PY
chmod 700 "$STAGE"
tar -C "$STAGE" -cf - MANIFEST.json db.dump objects.tar \
  | gpg --batch --quiet --no-tty --pinentry-mode loopback --passphrase-file "$PASS" \
        --symmetric --cipher-algo AES256 --s2k-digest-algo SHA512 --s2k-count 65011712 --compress-algo none \
        -o "$OUT/$NAME.tar.gpg"
(cd "$OUT" && sha256sum "$NAME.tar.gpg" > "$NAME.tar.gpg.sha256")
echo "$OUT/$NAME.tar.gpg"
