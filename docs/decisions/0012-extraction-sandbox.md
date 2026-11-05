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
- Extractors: CSV (stdlib; UTF-8 or CP932), XLSX (zipfile + ElementTree with member-count, per-member, total-inflated
  and compression-ratio caps, cell cap, DTD/ENTITY refusal; cached values only, formulas never evaluated), PDF text
  with positions (pypdf, BSD-3, decompression ceilings via `apply_configuration`), tables rebuilt from text positions
  (column-aligned and pipe-delimited; page/table/row/col), drawing title blocks (label cell + value right/below) and
  PDF document info, ASCII DXF HEADER variables/custom properties and ATTRIB title-block tags (stdlib reader), images
  via the tesseract CLI (Apache-2.0, TSV word boxes + confidence) behind an `OcrEngine` interface. Scanned PDFs are
  not OCRed yet (page rasterisation would need another dependency).
- Worker output (v0.3): `records` (label/value/location/group, plus `method` and `confidence`; consumed by the Rust
  mapping) and `claims` from a `ClaimExtractor` interface whose only allowed implementation is the deterministic
  local `rules@1` (vocabulary mirrors `mfg-map@1`). Requests carry bytes only; unknown keys (paths, tenant ids, URLs)
  are refused; limits can only be tightened. Instruction-like text is flagged in `warnings` and stays data.
- Claims are produced in Rust by the versioned manufacturing mapping (`mfg-map@1`): a fixed label vocabulary maps to
  predicates; unknown labels are reported, never turned into claims. Document text cannot add predicates, change
  rules or trigger actions (prompt-injection resistant by construction: there is no model to instruct).
- Model-based AI-assisted extraction is **not implemented** (only the interface + rule-based implementation). Any future model must run locally or under an explicit customer
  authorisation + data-processing policy, produce `unreviewed` claims with provenance, and pass the same sandbox.

## Consequences
Extraction quality is limited to well-labelled documents; accuracy has not been measured on real supplier documents.
bubblewrap is LGPL-2.0+, used as an external program (no linking).
