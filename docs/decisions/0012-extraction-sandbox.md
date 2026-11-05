# ADR 0012: Sandboxed, deterministic document extraction (no external AI)

Status: accepted (2026-11-05)

## Decision
- Extraction runs in a Python worker launched per document by the API: bubblewrap (`--unshare-all`: no network,
  separate PID/IPC/UTS/user namespaces; `--clearenv`; `--cap-drop ALL`; read-only binds of `/usr`, the interpreter and
  the worker source only; tmpfs `/tmp`), then `prlimit` (3 GiB address space, CPU seconds = timeout, 64 files, no
  core), then in-process hardening (RLIMIT_FSIZE=0, sockets disabled). Bytes in on stdin, JSON out on stdout
  (≤16 MiB). Wall-clock timeout (default 45 s), max 2 concurrent workers. The worker never sees DB credentials,
  the repository, `.env*`, the object store or other tenants' documents.
- `KIZUNA_EXTRACTION_SANDBOX=none` (rlimits only) exists for development hosts without bubblewrap; not for production.
- Extractors: CSV (stdlib), XLSX (zipfile + ElementTree with member size caps and DTD/ENTITY refusal; cached values
  only, formulas never evaluated), PDF text (pypdf, BSD-3), images via the tesseract CLI (Apache-2.0) when installed.
  PDF tables: only `label: value` lines and pipe-delimited rows. Scanned PDFs are not OCRed yet.
- Claims are produced in Rust by the versioned manufacturing mapping (`mfg-map@1`): a fixed label vocabulary maps to
  predicates; unknown labels are reported, never turned into claims. Document text cannot add predicates, change
  rules or trigger actions (prompt-injection resistant by construction: there is no model to instruct).
- AI-assisted extraction is **not implemented**. Any future model must run locally or under an explicit customer
  authorisation + data-processing policy, produce `unreviewed` claims with provenance, and pass the same sandbox.

## Consequences
Extraction quality is limited to well-labelled documents; accuracy has not been measured on real supplier documents.
bubblewrap is LGPL-2.0+, used as an external program (no linking).
