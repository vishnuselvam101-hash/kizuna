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
ALT=${DB}_alt
kz_create_db "$DB"
# Second database under different role names (custom-role migrations test, tests/custom_roles.rs).
# shellcheck source=scripts/lib/db.sh
source scripts/lib/db.sh
ALT_OWNER=kizuna_cialt_owner ALT_APP=kizuna_cialt_app ALT_OPW=$(openssl rand -hex 16) ALT_APW=$(openssl rand -hex 16)
kz_ensure_roles "$ALT_OPW" "$ALT_APW" "$ALT_OWNER" "$ALT_APP"
kz_create_db "$ALT" "$ALT_OWNER" "$ALT_APP"
ENVF=$(mktemp)
cleanup() {
  rm -f "$ENVF"
  if ((KEEP)); then echo "kept databases $DB $ALT"; else
    kz_drop_db "$DB" >/dev/null 2>&1 || true
    kz_drop_db "$ALT" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT
chmod 600 "$ENVF"
cat > "$ENVF" <<ENV
KIZUNA_OWNER_PASSWORD=$KIZUNA_OWNER_PASSWORD
KIZUNA_APP_PASSWORD=$KIZUNA_APP_PASSWORD
KIZUNA_TEST_DATABASE_URL=$(kz_app_url "$DB")
KIZUNA_TEST_OWNER_DATABASE_URL=$(kz_owner_url "$DB")
KIZUNA_TEST_ALT_OWNER_DATABASE_URL=postgres://$ALT_OWNER:$ALT_OPW@$KZ_PGHOST:$KZ_PGPORT/$ALT
KIZUNA_TEST_ALT_DATABASE_URL=postgres://$ALT_APP:$ALT_APW@$KZ_PGHOST:$KZ_PGPORT/$ALT
KIZUNA_REQUIRE_ALT_ROLES=1
ENV
echo "$DB, $ALT (roles $ALT_OWNER/$ALT_APP)"

step "pnpm install --frozen-lockfile"
(cd web && pnpm install --frozen-lockfile --silent)

step "scripts/check.sh"
t0=$(date +%s)
KIZUNA_ENV_FILE=$ENVF scripts/check.sh
echo "[ci-local] green in $(($(date +%s) - t0)) s on $(git rev-parse --short HEAD)"
