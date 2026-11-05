#!/usr/bin/env bash
# Local quality gate; CI runs the same steps. Usage: scripts/check.sh [--no-audit]
set -euo pipefail
cd "$(dirname "$0")/.."
ENV_FILE=${KIZUNA_ENV_FILE:-.env.local}   # scripts/ci-local.sh points this at a throwaway database
# shellcheck disable=SC1090
if [[ -f $ENV_FILE ]]; then set -a; source "$ENV_FILE"; set +a; fi
: "${KIZUNA_TEST_DATABASE_URL:?run scripts/db-setup.sh first}"
step() { printf '\n== %s\n' "$*"; }
JOBS=${CARGO_BUILD_JOBS:-4}

step "rust fmt";     cargo fmt --all -- --check
step "rust clippy";  cargo clippy -q -j"$JOBS" --all-targets -- -D warnings
step "rust tests";   cargo test -q -j"$JOBS" --workspace
if [[ "${1:-}" != "--no-audit" ]]; then
  step "cargo audit"; cargo audit -q
  step "cargo deny"; cargo deny --log-level error check
  step "pnpm audit"; (cd web && pnpm audit)
  step "pip-audit"
  req=$(mktemp); trap 'rm -f "$req"' EXIT
  (cd services/extraction && uv export -q --frozen --all-groups -o "$req" && uvx -q pip-audit==2.10.1 --strict --require-hashes -r "$req")
fi
step "python lint+tests"
(cd services/extraction && uv lock --check -q && uv run -q --locked ruff check . \
  && uv run -q --locked ruff format --check . && uv run -q --locked pytest -q)
step "web typecheck+tests"
(cd web && pnpm -s lint && pnpm -s test)
step "shell lint";    shellcheck -x scripts/*.sh scripts/lib/*.sh scripts/tests/*.sh deploy/*.sh
step "ops script tests"
for t in scripts/tests/*.test.sh; do "$t"; done
# The secret scan is mandatory in CI (deep review KZ-SC4); locally it runs when gitleaks is installed.
if command -v gitleaks >/dev/null; then
  step "secret scan"; gitleaks git --no-banner --redact -l warn .
elif [[ "${CI:-}" == "true" ]]; then
  echo "gitleaks missing in CI" >&2; exit 1
fi
# Keep the dev target dir from eating the disk: delete target/debug when it exceeds 8 GB.
sz=$(du -sm target/debug 2>/dev/null | cut -f1 || echo 0)
if (( ${sz:-0} > 8192 )); then echo "target/debug is ${sz} MB > 8 GB: removing"; rm -rf target/debug; fi
step "all checks passed"
