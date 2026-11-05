#!/usr/bin/env bash
# Supply-chain pins (deep review KZ-SC1..SC5): CI actions by commit SHA, pinned security tools, mandatory secret scan.
set -euo pipefail
cd "$(dirname "$0")/../.."
fail() { echo "FAIL: $*" >&2; exit 1; }
unpinned=$(grep -hE '^\s*-?\s*uses: ' .github/workflows/*.yml | grep -vE 'uses: [^@ ]+@[0-9a-f]{40}( |$)' || true)
[[ -z $unpinned ]] || fail "actions not pinned by SHA: $unpinned"
grep -q 'pip-audit==[0-9]' scripts/check.sh || fail "pip-audit not pinned"
grep -q 'cargo deny' scripts/check.sh || fail "cargo deny not run"
grep -qE 'cargo-deny --locked --version [0-9]+\.[0-9]+\.[0-9]+' .github/workflows/ci.yml || fail "cargo-deny not pinned in CI"
grep -qE 'gitleaks.*sha256sum -c|sha256sum -c' .github/workflows/ci.yml || fail "gitleaks not checksum-verified"
grep -q 'gitleaks missing in CI' scripts/check.sh || fail "secret scan can be skipped in CI"
grep -qE 'version: "[0-9]+\.[0-9]+\.[0-9]+"' .github/workflows/ci.yml || fail "uv not pinned"
grep -q '^only-built-dependencies' web/.npmrc 2>/dev/null || fail "pnpm lifecycle scripts not restricted"
[[ -f deny.toml ]] || fail "deny.toml missing"
echo "supply-chain pins ok"
