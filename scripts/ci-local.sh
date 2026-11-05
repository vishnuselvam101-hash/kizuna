#!/usr/bin/env bash
# Runs the GitHub workflow (.github/workflows/ci.yml) on this host: same toolchain, a fresh database owned by the
# same two roles (throwaway kizuna_ci_*, dropped afterwards), frozen pnpm install, then scripts/check.sh with
# everything (fmt, clippy, tests, cargo/pnpm/pip audits, Python, web, shell lint + ops script tests, secret scan).
# Usage: scripts/ci-local.sh [--keep-db]       Needs: roles from scripts/db-setup.sh, root (sudo -u postgres).
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=scripts/lib/throwaway.sh
source scripts/lib/throwaway.sh
KEEP=0
[[ ${1:-} == --keep-db ]] && KEEP=1
step() { printf '\n== [ci-local] %s\n' "$*"; }

step "toolchain (workflow: dtolnay/rust-toolchain@1.95, cargo-audit 0.22.1, node 24, pnpm 9)"
want=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
have=$(rustc --version | awk '{print $2}')
[[ $have == "$want"* ]] || { echo "rustc $have, workflow uses $want" >&2; exit 1; }
echo "rustc $have; $(cargo audit --version 2>/dev/null || echo 'cargo-audit MISSING'); node $(node --version); pnpm $(pnpm --version)"
command -v cargo-audit >/dev/null || { echo "cargo install cargo-audit --locked --version 0.22.1" >&2; exit 1; }
if [[ -n $(git status --porcelain --untracked-files=no) ]]; then
  echo "note: uncommitted changes are included (CI checks the committed tree)"
fi

step "throwaway database"
kz_load_env
DB=kizuna_ci_$(date +%s)_$$
kz_create_db "$DB"
ENVF=$(mktemp)
cleanup() {
  rm -f "$ENVF"
  if ((KEEP)); then echo "kept database $DB"; else kz_drop_db "$DB" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT
chmod 600 "$ENVF"
cat > "$ENVF" <<ENV
KIZUNA_OWNER_PASSWORD=$KIZUNA_OWNER_PASSWORD
KIZUNA_APP_PASSWORD=$KIZUNA_APP_PASSWORD
KIZUNA_TEST_DATABASE_URL=$(kz_app_url "$DB")
KIZUNA_TEST_OWNER_DATABASE_URL=$(kz_owner_url "$DB")
ENV
echo "$DB"

step "pnpm install --frozen-lockfile"
(cd web && pnpm install --frozen-lockfile --silent)

step "scripts/check.sh"
t0=$(date +%s)
KIZUNA_ENV_FILE=$ENVF scripts/check.sh
echo "[ci-local] green in $(($(date +%s) - t0)) s on $(git rev-parse --short HEAD)"
