# Dependency register

Checked 2026-11-05: licence from crates.io/npm metadata, maintenance = release within the last year and active repo,
advisories via `cargo audit` (RustSec DB) and `pnpm audit` (GitHub advisories). cargo-deny is not installed; licence
review is manual (this table).

## Rust (direct)
| Crate | Version line | Licence | Why | Notes |
|---|---|---|---|---|
| axum / tower / tower-http | 0.8 / 0.5 / 0.6 | MIT | HTTP server, limits, headers | tokio-rs project |
| tokio | 1 | MIT | async runtime | |
| sqlx | 0.8 (0.9 exists) | MIT/Apache-2.0 | PostgreSQL, migrations | RUSTSEC-2024-0363 fixed in 0.8.1; lockfile lists `rsa` via unused sqlx-mysql (RUSTSEC-2023-0071, not compiled; ignored with reason in `.cargo/audit.toml`) |
| argon2 | 0.5 | MIT/Apache-2.0 | password hashing | RustCrypto |
| aes-gcm | 0.10 | MIT/Apache-2.0 | secret encryption at rest | RustCrypto; NCC audit 2020 |
| sha2, sha1, hmac | 0.10/0.10/0.12 | MIT/Apache-2.0 | SHA-256, TOTP HMAC-SHA1 | RustCrypto |
| rand | 0.9 | MIT/Apache-2.0 | token/nonce generation (ThreadRng, CSPRNG) | |
| subtle | 2 | BSD-3-Clause | constant-time compare | |
| base64, hex | 0.22, 0.4 | MIT/Apache-2.0 | encodings | |
| uuid, time, serde, serde_json, thiserror, tracing(-subscriber) | current | MIT/Apache-2.0 | basics | |
| ed25519-dalek | 2 | BSD-3-Clause | signed audit checkpoints | RUSTSEC-2022-0093 fixed in 2.0 |
| rust_decimal | 1 | MIT | exact decimal unit conversion / tolerance checks | no floats in rules |
| webauthn-rs | 0.6.2 | MPL-2.0 | WebAuthn ceremonies (ADR 0006) | kanidm project, SUSE-audited lineage; pure RustCrypto backend (`crypto-glue`), no OpenSSL; feature `danger-allow-state-serialisation` only so ceremony state can be sealed server-side (never sent to clients). MPL-2.0 is file-level copyleft: we link unmodified; modifying its files would require publishing those files. `cargo audit` 2026-11-05: 0 advisories (425 crates) |
| proptest (dev) | 1 | MIT/Apache-2.0 | property tests | |
| webauthn-authenticator-rs (dev, `softpasskey`) | 0.6.2 | MPL-2.0 | software authenticator for WebAuthn integration tests | test-only, not in the server binary |
| http-body-util (dev) | 0.1 | MIT | test body reads | |

Not adopted: `governor` (own 60-line limiter, ADR 0008), `jsonwebtoken` (opaque tokens, ADR 0005), `totp-rs`
(TOTP from RustCrypto HMAC, RFC vectors), MinIO (AGPL-3.0, ADR 0009).

## Bundled data
| File | Source | Licence | Use |
|---|---|---|---|
| `crates/kizuna-api/data/common-passwords.txt` | SecLists `Passwords/Common-Credentials/xato-net-10-million-passwords-1000000.txt` (commit c205c36, file sha256 424a3e03…caf51), filtered to ≥12 chars, lower-cased, de-duplicated: 46,146 entries | MIT (SecLists, `data/common-passwords.LICENSE`) | offline common/breached password screening; no network calls |

## Python (services/extraction, via uv)
| Package | Licence | Use |
|---|---|---|
| pypdf 6.21.4 (exact pin) | BSD-3-Clause | PDF text with positions + document info (inside sandbox) |
| pytest (dev) | MIT | tests |
| hypothesis (dev) | MPL-2.0 | property/fuzz tests of the worker on mutated documents (dev only, not shipped) |
| ruff (dev) | MIT | lint/format |

## Web (pnpm)
| Package | Licence | Use |
|---|---|---|
| react, react-dom 18.3.1 | MIT | UI |
| vite 8, @vitejs/plugin-react | MIT | build/dev server |
| vitest 5, jsdom, @testing-library/* | MIT | tests |
| typescript 5.6 | Apache-2.0 | typecheck |

Initial pins (vite 5.4.11, vitest 2.1.8) had 25 `pnpm audit` findings incl. critical; upgraded to latest, now 0.

## Upgrade backlog
sqlx 0.9, rand 0.10, sha2 0.11, argon2 0.6, aes-gcm 0.11 (new majors; API changes; schedule after M6).

## System tools (invoked as separate programs, not linked)
| Tool | Licence | Use |
|---|---|---|
| bubblewrap (`/usr/bin/bwrap`) | LGPL-2.0-or-later | extraction sandbox (ADR 0012) |
| prlimit (util-linux) | GPL-2.0 | worker resource limits |
| tesseract 5.5 | Apache-2.0 | OCR of image uploads, when installed |
| ImageMagick (tests only) | ImageMagick licence | renders an OCR test image |

Python pins: `services/extraction/uv.lock` (hashes); `scripts/check.sh` runs `uv lock --check` and `uv run --locked`.

Python licence/advisory check (2026-11-05): `uv export --frozen --all-groups` → `uvx pip-audit --strict
--require-hashes` (PyPI/OSV advisories): **No known vulnerabilities found** for pypdf 6.21.4, hypothesis 6.170.14,
pytest 9.2.0, ruff 0.17.3, sortedcontainers 2.4.0, packaging 26.3, pluggy 1.7.0, iniconfig 2.3.1, pygments 2.21.0.
Licences (PyPI metadata): pypdf BSD-3-Clause; hypothesis MPL-2.0 (file-level copyleft, dev-only, unmodified);
pytest/pluggy/iniconfig MIT; ruff MIT; sortedcontainers Apache-2.0; packaging Apache-2.0/BSD-2-Clause; pygments
BSD-2-Clause. Only pypdf enters the sandbox. pip-audit now runs in `scripts/check.sh` (audit section).

Not adopted (Python): ezdxf (MIT, maintained) for DXF: it pulls numpy, fonttools and pyparsing into the sandbox for
what is a HEADER/ATTRIB read, so a ~90-line stdlib group-code reader is used (`drawing.py`, fuzzed);
pdfplumber/camelot (table extraction; add pdfminer.six/OpenCV/Ghostscript) - tables are rebuilt from pypdf text
positions instead; pytesseract (thin wrapper; the CLI is called directly with a fixed argv); any hosted AI SDK
(ADR 0012: no external providers).

Not adopted: defusedxml (stdlib ElementTree + DTD refusal + size caps instead, see `tabular.py`), ClamAV (ADR 0011).

## M6 additions (2026-11-05)
No new Rust, Python or npm packages. `crates/kizuna-bench` uses only crates already in `Cargo.lock`
(tokio, axum, sqlx, base64, serde_json, uuid, time, hex, rust_decimal) plus workspace crates; `cargo audit` covers it.
`scripts/e2e/*.py` are Python stdlib only.

| Tool (separate program) | Licence | Use | Advisory check |
|---|---|---|---|
| GnuPG 2.4 (`gpg`) | GPL-3.0-or-later | symmetric AES-256 encryption of backups (`scripts/backup.sh`) | distro package; follow Ubuntu USNs for gnupg2 |
| pg_dump / pg_restore 18 | PostgreSQL licence | logical backup and restore | same package as the server |
| Caddy 2 (2.6.2 on the dev host; deployment only) | Apache-2.0 | TLS termination / reverse proxy (`docs/deploy/DEPLOYMENT.md`) | track caddyserver/caddy GitHub advisories |

Not adopted: `age` (not installed on the target host; GnuPG is already present and its symmetric mode is
integrity-protected), `openssl enc` (no authenticated mode for files).

## Ops pilot additions (2026-11-05, branch ops-pilot)
Licence from crates.io metadata; `cargo audit` 2026-11-05 after adding them: 0 advisories (exit 0).

| Crate | Version | Licence | Why | Notes |
|---|---|---|---|---|
| seccompiler | 0.5.0 | Apache-2.0 OR BSD-3-Clause | compile the extraction worker's seccomp deny list to BPF (ADR 0016) | rust-vmm project, used by Firecracker; we only compile (never `apply_filter` in-process); bwrap installs it |
| command-fds | 0.3.3 | Apache-2.0 | pass the BPF pipe to bwrap as fd 3 without `unsafe` in our crate | Google; uses `nix` 0.31 + `pre_exec` internally |
| libc | 0.2 | MIT OR Apache-2.0 | `SYS_*` syscall numbers for the policy | already in the tree |
| lettre | 0.11.23 | MIT | SMTP alerts (blocking transport, rustls + ring + webpki-roots; no native-tls/OpenSSL) | features: smtp-transport, builder, hostname, rustls-tls |
| ureq | 3.5.1 | MIT OR Apache-2.0 | Telegram Bot API alerts over HTTPS | default features off, `rustls` only (ring, webpki-roots) |

| Tool (separate program) | Licence | Use | Advisory check |
|---|---|---|---|
| ClamAV `clamd` + `freshclam` (Ubuntu `clamav-daemon`) | GPL-2.0 | malware scan over the local socket (ADR 0017); no linking | Ubuntu USNs for clamav; ClamAV blog advisories |
| rclone (Ubuntu package, 1.60 on the host) | MIT | off-host copy of encrypted backups (`scripts/offsite-backup.sh`) | rclone GitHub security advisories |
| Tailscale (`tailscale serve`) | BSD-3-Clause | tailnet-only HTTPS for the pilot URL | Tailscale security bulletins |
