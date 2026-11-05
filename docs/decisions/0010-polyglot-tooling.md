# ADR 0010: Python via uv, TypeScript via pnpm, one check script

Status: accepted (2026-11-05)

## Decision
- Python 3.12+ project in `services/extraction`, managed with `uv` (lockfile `uv.lock`), lint `ruff`, tests `pytest`.
- Web app in `web/`, pnpm with `pnpm-lock.yaml`, TypeScript strict, tests with `vitest`.
- `scripts/check.sh` runs every gate locally; `.github/workflows/ci.yml` runs the same steps.
