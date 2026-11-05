#!/usr/bin/env bash
# Fetches a backup from the off-host rclone remote and verifies its SHA-256 (first step of the restore drill,
# docs/deploy/OFFSITE_RESTORE.md). Prints the local path of the verified .tar.gpg.
# Usage: scripts/offsite-fetch.sh --remote REMOTE:PATH --db NAME --to DIR [--name kizuna-<db>-<UTC>.tar.gpg]
#   default: the newest backup of NAME on the remote (names sort by UTC timestamp).
set -euo pipefail
REMOTE=${KIZUNA_RCLONE_REMOTE:-} DB='' TO='' NAME=''
while (($#)); do
  case $1 in
    --remote) REMOTE=$2; shift 2 ;;
    --db) DB=$2; shift 2 ;;
    --to) TO=$2; shift 2 ;;
    --name) NAME=$2; shift 2 ;;
    *) echo "unknown arg $1" >&2; exit 64 ;;
  esac
done
RCLONE=${RCLONE:-rclone}
[[ $DB =~ ^kizuna_[a-z0-9_]+$ ]] || { echo "--db must be a kizuna_* database" >&2; exit 64; }
[[ -n $REMOTE && -n $TO ]] || { echo "--remote and --to required" >&2; exit 64; }
if [[ -z $NAME ]]; then
  NAME=$("$RCLONE" lsf "$REMOTE" --max-depth 1 --files-only --include "kizuna-${DB}-*.tar.gpg" | sort | tail -1)
  [[ -n $NAME ]] || { echo "no backup of $DB on $REMOTE" >&2; exit 1; }
fi
[[ $NAME =~ ^kizuna-${DB}-[0-9]{8}T[0-9]{6}Z\.tar\.gpg$ ]] || { echo "unexpected backup name $NAME" >&2; exit 1; }
umask 077
mkdir -p "$TO"
"$RCLONE" copyto "$REMOTE/$NAME" "$TO/$NAME"
"$RCLONE" copyto "$REMOTE/$NAME.sha256" "$TO/$NAME.sha256"
(cd "$TO" && sha256sum --quiet -c "$NAME.sha256") >&2 || { echo "checksum mismatch for $NAME" >&2; exit 1; }
echo "$TO/$NAME"
