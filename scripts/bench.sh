#!/usr/bin/env bash
# M6 benchmarks against a throwaway database (release build). Results: reports/bench/bench-<UTC>.{json,md}.
# Usage: scripts/bench.sh [--parts N] [--seed N]   (needs scripts/db-setup.sh roles; run as root for sudo -u postgres)
# Run on an otherwise idle host; the JSON records loadavg at start.
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/lib/throwaway.sh
kz_load_env
DB=kizuna_bench_$(date +%s)_$$
trap 'kz_drop_db "$DB" || true' EXIT
cargo build -q --release -p kizuna-bench --bin kizuna-bench
kz_create_db "$DB"
KIZUNA_BENCH_OWNER_URL=$(kz_owner_url "$DB") KIZUNA_BENCH_APP_URL=$(kz_app_url "$DB") \
  target/release/kizuna-bench --out-dir reports/bench "$@"
