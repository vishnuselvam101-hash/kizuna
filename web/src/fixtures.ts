import type { Artifact, Claim, Conflict, ConflictDetail, Decision, Trace, User } from "./types";

export const claim = (id: string, v: string, over: Partial<Claim> = {}): Claim => ({
  id,
  subject_type: "part",
  subject_key: "BRK-100",
  predicate: "mfg.material",
  value_text: v,
  value_num: null,
  value_unit: null,
  revision: "C",
  artifact_id: "art-1",
  source_location: { page: 2 },
  extraction_method: "manual",
  confidence_bps: 9000,
  review_status: "unreviewed",
  recorded_at: "2026-11-01T00:00:00Z",
  superseded_at: null,
  ...over,
});

export const artifact: Artifact = {
  id: "art-1",
  sha256: "ab".repeat(32),
  size_bytes: 2048,
  media_type: "application/pdf",
  kind: "spec_document",
  status: "accepted",
  display_name: "BRK-100 spec rev C.pdf",
  created_at: "2026-11-01T00:00:00Z",
  scan_result: {},
};

export const conflict: Conflict = {
  id: "cf-1",
  kind: "value_mismatch",
  subject_key: "BRK-100",
  status: "open",
  explanation: { why: "Specification says S355 but the purchase order says S275.", rule: "value_mismatch@1" },
  detected_at: "2026-11-02T00:00:00Z",
};

export const conflictDetail: ConflictDetail = {
  conflict,
  claim_a: claim("c-a", "S355"),
  claim_b: claim("c-b", "S275", { artifact_id: "art-2", revision: "C", confidence_bps: 7000 }),
  sources: [
    { claim_id: "c-a", display_name: "BRK-100 spec rev C.pdf", sha256: "ab".repeat(32), location: { page: 2 } },
    { claim_id: "c-b", display_name: "PO-5521.csv", sha256: "cd".repeat(32), location: { row: 14, col: 3 } },
  ],
  reviews: [],
  requires_human_review: true,
  limitations: "Deterministic rule v1; extracted values may contain extraction errors.",
  potentially_affected_decisions: [{ id: "d-1", title: "Release BRK-100 from Supplier A", status: "valid" }],
};

export const decision: Decision = {
  id: "d-1",
  kind: "supplier_part_release",
  title: "Release BRK-100 from Supplier A",
  subject_key: "BRK-100",
  outcome: "approved",
  status: "potentially_affected",
  decided_at: "2026-11-01T10:00:00Z",
  derivation_sha256: "ef".repeat(32),
};

export const trace: Trace = {
  decision: { ...decision, rationale: "Material and finish match the drawing." },
  derivation_reproducible: true,
  evidence: [
    { claim_id: "c-a", predicate: "mfg.material", snapshot_at_decision: { text: "S355" }, current_value: { text: "S275" }, changed: true },
    { claim_id: "c-f", predicate: "mfg.finish", snapshot_at_decision: { text: "zinc" }, current_value: { text: "zinc" }, changed: false },
  ],
  invalidations: [{ reason: "material claim superseded", material: true, recorded_at: "2026-11-03T00:00:00Z", cause_kind: "claim_superseded" }],
  changed_since_decision: [],
};

export const users: User[] = [
  { id: "u-admin", email: "admin@example.test", display_name: "Aiko Admin", role: "tenant_admin", status: "active", totp_enabled: true, locked_until: null },
  { id: "u-2", email: "rev@example.test", display_name: "Ren Reviewer", role: "reviewer", status: "active", totp_enabled: true, locked_until: null },
];
