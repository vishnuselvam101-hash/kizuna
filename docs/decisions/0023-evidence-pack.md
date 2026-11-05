# ADR 0023: Offline-verifiable evidence pack

Status: accepted (2026-11-05). Context: directive §9 (hash ≠ truth), customer/auditor hand-over of a decision.

## Decision
- `POST /decisions/{id}/evidence-pack` (`decision_read` + `artifact_read`, heavy rate limit; body
  `{"hash_only": [artifact ids]}`) returns one ZIP, format `kizuna-evidence-pack` v1:
  `manifest.json` (canonical JSON, `deny_unknown_fields`, re-encoded byte-for-byte on read),
  `manifest.sig.json` (Ed25519 over `"KZEP1" || SHA-256(manifest)`), `decision/derivation.json` (canonical; its hash
  is in the `decision.recorded` audit event, so the trace claims → source locations is proven),
  `decision/{record,trace,report}.json` (state at export time, export-signed only), `artifacts/<id>/<name>` and
  `README.txt`. Hash-only documents keep hash + proofs and leave the file out; never-released (quarantined)
  documents are always hash-only. Audited as `decision.evidence_pack_exported` (+ `export.report` security event).
- Inclusion proofs: RFC 6962 Merkle tree over the chain hashes of events `1..=seq` (leaf `i` = hash of event
  `i+1`), via the maintained `ct-merkle` crate (no own tree code). New checkpoint message **KZCP3** = KZCP2 ‖ Merkle
  root (migration 0022 adds `audit_checkpoints.merkle_root`); every new checkpoint is KZCP3; KZCP1/2 rows still verify.
  The export reuses the newest KZCP3 checkpoint under the current key that covers the needed events; otherwise it
  signs one if the caller has `audit_verify`, else `409 checkpoint_required`. Before shipping, the tree is rebuilt
  from the stored chain and must match the signed root (else `409 audit_chain_not_intact` + security event).
- Proven events: the decision's own history (except earlier exports) and the `artifact.uploaded|recaptured` event
  whose `details.sha256` equals the document hash. Other tenants' and other decisions' events are not disclosed.
- `crates/kizuna-verify-pack`: pure library (format, builder, verifier) + `kizuna-verify-pack` CLI; no DB/network.
  Every check runs and has a stable code; the report (EN/JA) lists what is proven (bytes unchanged since capture,
  events included up to checkpoint #N, signed by key X at time T) and always what is not (content truth, authorship,
  key ownership unless `--trusted-key`, hash-only content, export-time JSON, anything after the checkpoint).
  Untrusted-input limits: ≤ 10 000 entries, JSON ≤ 64 MiB, file ≤ 4 GiB (read with a hard cap, not the declared
  size), strict relative paths, no duplicates, no unlisted entries.

## Consequences / limits
- The pack is only as trustworthy as the key pin: a full forgery under another key is self-consistent and is caught
  only with `--trusted-key` (tenants publish the key id, e.g. with the off-host checkpoint copies of KZ-C3).
- Documents > 256 MiB in total per pack must be exported hash-only (`409 pack_too_large_use_hash_only`).
- Building a KZCP3 checkpoint/export holds all chain hashes of the tenant in memory (32 B/event; ~2× for the tree).
