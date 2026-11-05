#!/usr/bin/env bash
# Minimal rclone stand-in for tests on hosts without rclone: remotes are "kztest:/abs/path" mapped to local paths.
# Supports exactly what scripts/offsite-{backup,fetch}.sh use: copyto, check --one-way, delete, lsf.
set -euo pipefail
p() { local a=$1; [[ $a == kztest:* ]] && a=${a#kztest:}; [[ $a == *:* ]] && { echo "stub: unknown remote $1" >&2; exit 3; }; echo "$a"; }
cmd=$1; shift
inc=() min_age='' args=()
while (($#)); do
  case $1 in
    --include) inc+=("$2"); shift 2 ;;
    --min-age) min_age=${2%d}; shift 2 ;;
    --max-depth) shift 2 ;;
    --one-way | --files-only) shift ;;
    *) args+=("$1"); shift ;;
  esac
done
# shellcheck disable=SC2053 # include patterns are globs on purpose
matches() { local f g; f=$(basename "$1"); for g in "${inc[@]}"; do [[ $f == $g ]] && return 0; done; return 1; }
case $cmd in
  copyto) src=$(p "${args[0]}"); dst=$(p "${args[1]}"); mkdir -p "$(dirname "$dst")"; cp -p "$src" "$dst" ;;
  check)
    s=$(p "${args[0]}"); d=$(p "${args[1]}")
    for f in "$s"/*; do matches "$f" || continue; cmp -s "$f" "$d/$(basename "$f")" || { echo "stub: differ $f" >&2; exit 1; }; done ;;
  delete)
    d=$(p "${args[0]}")
    for f in "$d"/*; do
      if [[ ! -f $f ]] || ! matches "$f"; then continue; fi
      if [[ -z $min_age ]] || (( $(date +%s) - $(stat -c %Y "$f") > min_age * 86400 )); then rm -f "$f"; fi
    done ;;
  lsf) d=$(p "${args[0]}"); for f in "$d"/*; do [[ -f $f ]] && matches "$f" && basename "$f"; done; true ;;
  *) echo "stub: unsupported $cmd" >&2; exit 2 ;;
esac
