#!/usr/bin/env bash
# Local quality gate; CI runs the same steps. Usage: scripts/check.sh [--no-audit]
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -f .env.local ]]; then set -a; source .env.local; set +a; fi
: "${KIZUNA_TEST_DATABASE_URL:?run scripts/db-setup.sh first}"
step() { printf '\n== %s\n' "$*"; }
JOBS=${CARGO_BUILD_JOBS:-4}

step "rust fmt";     cargo fmt --all -- --check
step "rust clippy";  cargo clippy -q -j"$JOBS" --all-targets -- -D warnings
step "rust tests";   cargo test -q -j"$JOBS" --workspace
if [[ "${1:-}" != "--no-audit" ]]; then
  step "cargo audit"; cargo audit -q
  step "pnpm audit"; (cd web && pnpm audit)
fi
step "python lint+tests"
(cd services/extraction && uv run -q ruff check . && uv run -q ruff format --check . && uv run -q pytest -q)
step "web typecheck+tests"
(cd web && pnpm -s lint && pnpm -s test)
if command -v gitleaks >/dev/null; then
  step "secret scan"; gitleaks git --no-banner --redact -l warn .
fi
# Keep the dev target dir from eating the disk.
sz=$(du -sm target 2>/dev/null | cut -f1 || echo 0)
if (( sz > 10240 )); then echo "target is ${sz} MB > 10 GB: run 'rm -rf target/debug'"; fi
step "all checks passed"
