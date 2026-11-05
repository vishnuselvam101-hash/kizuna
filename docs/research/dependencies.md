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
| pypdf 6.x | BSD-3-Clause | PDF text extraction (inside sandbox) |
| pytest (dev) | MIT | tests |
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

Not adopted: defusedxml (stdlib ElementTree + DTD refusal + size caps instead, see `tabular.py`), ClamAV (ADR 0011).
