# Environment inspection (M0, 2026-11-05)

| Item | Found |
|---|---|
| Workspace | `/root/kizuna`: only DIRECTIVE.md, BRIEF.md (stub), PLAN.md, NOTES.md. No code. Notes moved to `docs/research/background/`. |
| OS / HW | Linux, 8 CPU, 31 GB RAM (≈8 GB available, shared with other apps), no GPU, ≈57 GB disk free |
| Rust | rustc/cargo 1.95.0 (rustup at /opt/rust), clippy + rustfmt; cargo-audit 0.22.2; cargo-deny **not installed**; sqlx-cli present |
| PostgreSQL | 18.6, local; scram over 127.0.0.1; other apps' roles/DBs exist (untouched). KIZUNA uses roles `kizuna`, `kizuna_app` and DBs `kizuna_dev`, `kizuna_test` only |
| Node / pnpm | Node 24.22.2, pnpm 12.15.0 |
| Python | 3.14.4 system; `uv` available; ruff/pytest not installed system-wide (installed into the project venv by uv) |
| Other | gitleaks available (secret scan in check.sh) |
| Git | repository initialised on `main`; no remote (by instruction) |
