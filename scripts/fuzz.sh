#!/usr/bin/env bash
# Property-based fuzzing of every parser / untrusted-input path (cargo-fuzz is not installed on this host).
# Usage: scripts/fuzz.sh [cases]   — runs all proptest properties in release mode with many cases, then the
# Python worker's hypothesis/malformed-input tests. Time-boxed to 15 minutes overall.
set -euo pipefail
cd "$(dirname "$0")/.."
CASES=${1:-20000}
start=$(date +%s)
PROPTEST_CASES=$CASES timeout 600 cargo test -q --release -j4 \
  -p audit-engine -p evidence-core -p entity-resolution -p conflict-engine -p domain-manufacturing -p dependency-engine \
  -p kizuna-verify-pack -p kizuna-api --lib
# Evidence pack: manifest parser and whole-pack mutation properties (integration-test target).
PROPTEST_CASES=$CASES timeout 600 cargo test -q --release -j4 -p kizuna-verify-pack --test pack fuzz_
(cd services/extraction && timeout 300 uv run -q pytest -q)
echo "fuzz: PROPTEST_CASES=$CASES, $(( $(date +%s) - start ))s"
