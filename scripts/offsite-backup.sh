#!/usr/bin/env bash
# Encrypted backup (scripts/backup.sh) + off-host copy with rclone + verification + retention.
# Usage: scripts/offsite-backup.sh --db NAME --storage DIR --out DIR [--remote REMOTE:PATH]
#          [--keep-local-days N] [--keep-remote-days N] [--file EXISTING.tar.gpg]
#   --remote          rclone destination (default $KIZUNA_RCLONE_REMOTE; none = local backup only, says so)
#   --keep-local-days delete local kizuna-<db>-*.tar.gpg(.sha256) older than N days (default 14)
#   --keep-remote-days same on the remote (default 60); runs only after this run's upload was verified
#   --file            upload an existing backup instead of taking a new one
# Env: KIZUNA_BACKUP_PASSPHRASE_FILE (backup.sh); RCLONE_CONFIG (founder-configured remote, e.g. Google Drive);
#      RCLONE (binary, default rclone). Exit non-zero on any failure (systemd OnFailure= then sends an alert).
# Prints one JSON line. The archive is already encrypted (GnuPG AES-256) before it leaves the host; the passphrase
# and KIZUNA_DATA_KEY are never uploaded.
set -euo pipefail
cd "$(dirname "$0")/.."
DB='' STORAGE='' OUT='' REMOTE=${KIZUNA_RCLONE_REMOTE:-} KEEP_LOCAL=14 KEEP_REMOTE=60 FILE=''
while (($#)); do
  case $1 in
    --db) DB=$2; shift 2 ;;
    --storage) STORAGE=$2; shift 2 ;;
    --out) OUT=$2; shift 2 ;;
    --remote) REMOTE=$2; shift 2 ;;
    --keep-local-days) KEEP_LOCAL=$2; shift 2 ;;
    --keep-remote-days) KEEP_REMOTE=$2; shift 2 ;;
    --file) FILE=$2; shift 2 ;;
    *) echo "unknown arg $1" >&2; exit 64 ;;
  esac
done
RCLONE=${RCLONE:-rclone}
[[ $DB =~ ^kizuna_[a-z0-9_]+$ ]] || { echo "--db must be a kizuna_* database" >&2; exit 64; }
[[ -n $OUT ]] || { echo "--out required" >&2; exit 64; }
[[ $KEEP_LOCAL =~ ^[0-9]+$ && $KEEP_REMOTE =~ ^[0-9]+$ && $KEEP_LOCAL -ge 1 && $KEEP_REMOTE -ge 1 ]] \
  || { echo "retention days must be integers >= 1" >&2; exit 64; }
if [[ -n $REMOTE && ! $REMOTE =~ ^[A-Za-z0-9_.-]+:[^[:space:]]*$ ]]; then
  echo "--remote must look like name:path" >&2; exit 64
fi

if [[ -z $FILE ]]; then
  [[ -d $STORAGE ]] || { echo "--storage must be the object store directory" >&2; exit 64; }
  FILE=$(scripts/backup.sh --db "$DB" --storage "$STORAGE" --out "$OUT" | tail -1)
fi
NAME=$(basename "$FILE")
[[ $NAME =~ ^kizuna-${DB}-[0-9]{8}T[0-9]{6}Z\.tar\.gpg$ ]] || { echo "unexpected backup name $NAME" >&2; exit 1; }
(cd "$(dirname "$FILE")" && sha256sum --quiet -c "$NAME.sha256") || { echo "local checksum failed" >&2; exit 1; }

uploaded=false
if [[ -n $REMOTE ]]; then
  "$RCLONE" copyto "$FILE" "$REMOTE/$NAME"
  "$RCLONE" copyto "$FILE.sha256" "$REMOTE/$NAME.sha256"
  # Size + hash comparison where the backend has a common hash (Drive: MD5); otherwise size only.
  "$RCLONE" check "$(dirname "$FILE")" "$REMOTE" --one-way --include "$NAME" --include "$NAME.sha256"
  uploaded=true
  "$RCLONE" delete "$REMOTE" --max-depth 1 --min-age "${KEEP_REMOTE}d" \
    --include "kizuna-${DB}-*.tar.gpg" --include "kizuna-${DB}-*.tar.gpg.sha256"
else
  echo "WARNING: no off-host remote configured (KIZUNA_RCLONE_REMOTE); backup kept on this host only" >&2
fi
# Local retention never removes the backup just taken.
find "$(dirname "$FILE")" -maxdepth 1 -type f \( -name "kizuna-${DB}-*.tar.gpg" -o -name "kizuna-${DB}-*.tar.gpg.sha256" \) \
  ! -name "$NAME" ! -name "$NAME.sha256" -mtime "+$((KEEP_LOCAL - 1))" -delete
printf '{"backup":"%s","uploaded":%s,"remote":"%s","utc":"%s"}\n' "$NAME" "$uploaded" "${REMOTE%%:*}" "$(date -u +%FT%TZ)"
