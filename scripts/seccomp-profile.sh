#!/usr/bin/env bash
# Derives the extraction worker's seccomp allowlist (ADR 0016) from real runs. Usage: scripts/seccomp-profile.sh [OUT]
# 1. The extraction pytest suite records every distinct worker request it builds (KIZUNA_REQUEST_DUMP, see
#    services/extraction/tests/conftest.py): CSV, XLSX, PDF, scanned-PDF OCR, images, DXF, hostile inputs.
# 2. Each request is replayed through the real worker in the real bubblewrap sandbox WITHOUT the filter under
#    `strace -f`; the union of syscalls made from the sandboxed exec on is written to OUT (JSON, default
#    reports/seccomp/profile-<arch>.json) and the run fails if any of them is missing from `seccomp::allowed()`.
# 3. Each request is replayed again WITH the filter; the worker must give the identical answer.
# Needs strace and bwrap. Touches no database.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:-reports/seccomp/profile-$(uname -m).json}
command -v strace >/dev/null || { echo "strace is required" >&2; exit 1; }
corpus=$(mktemp -d); trap 'rm -rf "$corpus"' EXIT
echo "== record worker requests (extraction pytest)"
# The isolation test asserts that extraction opens no files; recording the corpus does, so it is skipped here.
(cd services/extraction && KIZUNA_REQUEST_DUMP=$corpus uv run -q --locked pytest -q -p no:cacheprovider \
  --deselect tests/test_isolation.py::test_extraction_touches_no_files_sockets_or_processes)
echo "== $(find "$corpus" -type f | wc -l) distinct requests; replay under strace in the sandbox"
mkdir -p "$(dirname "$OUT")"
KIZUNA_SECCOMP_CORPUS=$corpus KIZUNA_SECCOMP_PROFILE_OUT=$PWD/$OUT \
  cargo test -q -p kizuna-api --lib -- --ignored --exact extraction::tests::seccomp_profile_corpus
echo "== profile written to $OUT"
