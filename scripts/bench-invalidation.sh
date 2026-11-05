#!/usr/bin/env bash
# Old (frozen v1) vs new selective invalidation on synthetic graphs; one process per case for peak RSS.
# Usage: scripts/bench-invalidation.sh [max_edges] [old_timeout_s]   Prints JSON lines with max_rss_kb added.
set -euo pipefail
cd "$(dirname "$0")/.."
MAX=${1:-1000000}
TMO=${2:-300}
cargo build -q --release -p dependency-engine --example inval_bench
BIN=target/release/examples/inval_bench
for scenario in fanout narrow evidence; do
  for edges in 1000 10000 100000 1000000; do
    (( edges > MAX )) && continue
    reps=$(( edges <= 10000 ? 21 : (edges <= 100000 ? 5 : 3) ))
    for algo in build new bounded old; do
      out=$(mktemp)
      r=$reps; [[ $algo == old ]] && (( edges >= 100000 )) && r=1
      if timeout "$TMO" /usr/bin/time -f '%M' -o "$out.rss" "$BIN" "$algo" "$scenario" "$edges" "$r" >"$out"; then
        sed "s/}\$/,\"max_rss_kb\":$(tail -1 "$out.rss")}/" "$out"
      else
        echo "{\"algo\":\"$algo\",\"scenario\":\"$scenario\",\"edges\":$edges,\"timeout_s\":$TMO}"
      fi
      rm -f "$out" "$out.rss"
    done
  done
done
