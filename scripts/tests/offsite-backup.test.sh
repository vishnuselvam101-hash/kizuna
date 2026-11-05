#!/usr/bin/env bash
# Tests scripts/offsite-{backup,fetch}.sh against a local rclone remote (real rclone when installed, plus the stub;
# KZ_RCLONE_MODES="stub" to force the stub only). No database, no network.
set -euo pipefail
cd "$(dirname "$0")/../.."
modes=${KZ_RCLONE_MODES:-$(command -v rclone >/dev/null && echo "real stub" || echo stub)}
fail() { echo "FAIL: $*" >&2; exit 1; }
for mode in $modes; do
  T=$(mktemp -d "${TMPDIR:-/tmp}/kz-offsite.XXXXXX")
  trap 'rm -rf "$T"' EXIT
  if [[ $mode == real ]]; then
    : > "$T/rclone.conf"
    export RCLONE=rclone RCLONE_CONFIG=$T/rclone.conf RCLONE_CONFIG_KZTEST_TYPE=local
  else
    export RCLONE=$PWD/scripts/tests/rclone-stub.sh
  fi
  R=kztest:$T/remote L=$T/local DB=kizuna_t
  mkdir -p "$L" "$T/remote"
  mk() { # mk DIR UTC AGE_DAYS
    local f=$1/kizuna-$DB-$2.tar.gpg
    head -c 4096 /dev/urandom > "$f"; (cd "$1" && sha256sum "$(basename "$f")" > "$(basename "$f").sha256")
    touch -d "$3 days ago" "$f" "$f.sha256"
  }
  mk "$L" 20260901T023000Z 40; mk "$L" 20261101T023000Z 4; mk "$L" 20261105T023000Z 0
  mk "$T/remote" 20260801T023000Z 90; mk "$T/remote" 20261001T023000Z 35
  NEW=$L/kizuna-$DB-20261105T023000Z.tar.gpg
  out=$(scripts/offsite-backup.sh --db $DB --out "$L" --remote "$R" --file "$NEW" --keep-local-days 14 --keep-remote-days 60)
  [[ $out == *'"uploaded":true'* ]] || fail "$mode: summary $out"
  cmp -s "$NEW" "$T/remote/$(basename "$NEW")" || fail "$mode: not uploaded"
  [[ -f $T/remote/$(basename "$NEW").sha256 ]] || fail "$mode: checksum not uploaded"
  [[ ! -e $T/remote/kizuna-$DB-20260801T023000Z.tar.gpg ]] || fail "$mode: remote retention"
  [[ -e $T/remote/kizuna-$DB-20261001T023000Z.tar.gpg ]] || fail "$mode: remote kept too little"
  [[ ! -e $L/kizuna-$DB-20260901T023000Z.tar.gpg && ! -e $L/kizuna-$DB-20260901T023000Z.tar.gpg.sha256 ]] || fail "$mode: local retention"
  [[ -e $L/kizuna-$DB-20261101T023000Z.tar.gpg ]] || fail "$mode: local kept too little"
  # Restore side: newest backup fetched and verified.
  got=$(scripts/offsite-fetch.sh --remote "$R" --db $DB --to "$T/fetch")
  if [[ $got != "$T/fetch/$(basename "$NEW")" ]] || ! cmp -s "$got" "$NEW"; then fail "$mode: fetch $got"; fi
  # Tampered remote copy is refused.
  printf 'X' | dd of="$T/remote/$(basename "$NEW")" bs=1 seek=10 conv=notrunc status=none
  if scripts/offsite-fetch.sh --remote "$R" --db $DB --to "$T/fetch2" >/dev/null 2>&1; then fail "$mode: tamper not detected"; fi
  # Unreachable remote: non-zero exit and no retention run.
  mk "$L" 20260902T023000Z 40
  if scripts/offsite-backup.sh --db $DB --out "$L" --remote "nosuchremote:x" --file "$NEW" >/dev/null 2>&1; then
    fail "$mode: bad remote must fail"
  fi
  [[ -e $L/kizuna-$DB-20260902T023000Z.tar.gpg ]] || fail "$mode: retention ran after failed upload"
  # Local-only mode warns and succeeds.
  out=$(scripts/offsite-backup.sh --db $DB --out "$L" --remote "" --file "$NEW" 2>&1)
  [[ $out == *"kept on this host only"* && $out == *'"uploaded":false'* ]] || fail "$mode: local-only $out"
  rm -rf "$T"; trap - EXIT
  echo "offsite backup tests ($mode rclone): ok"
done
