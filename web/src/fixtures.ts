import type { Artifact, Claim, Conflict, ConflictDetail, DashboardSummary, Decision, OrgReport, Subject, Trace, User } from "./types";
import type { AiExtraction } from "./AiExtractionPanel";
import type { DetectionSettings } from "./DetectionSettingsPanel";

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
  artifact_display_name: "BRK-100 spec rev C.pdf",
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
  evidence_claim_ids: ["c-a"],
};

export const trace: Trace = {
  decision: { ...decision, rationale: "Material and finish match the drawing." },
  derivation_reproducible: true,
  derivation: {
    evidence: [
      { claim_id: "c-a", predicate: "mfg.material", artifact_id: "a-1111111111", source_location: { page: 1 } },
      { claim_id: "c-f", predicate: "mfg.finish", artifact_id: null },
    ],
  },
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

export const dashboardSummary: DashboardSummary = {
  artifacts: { accepted: 1, quarantined: 1 },
  claims: { unreviewed: 3, accepted: 1 },
  conflicts: { open: 1, needs_evidence: 2, resolved: 4 },
  conflict_kinds_open: { value_mismatch: 1 },
  decisions: { valid: 2, potentially_affected: 1 },
  subjects: { part: 1, supplier: 1 },
};

export const subjects: Subject[] = [{ subject_type: "part", subject_key: "BRK-100", claims: 4, predicates: 2 }];

export const orgReport: OrgReport = {
  report_version: "kizuna-org-report@1",
  generated_at: "2026-11-05T00:00:00Z",
  scope: "open_findings",
  findings: [{ id: "cf-1", kind: "value_mismatch", subject: "BRK-100", status: "open", why: "S355 vs S275", detected_at: "2026-11-02T00:00:00Z" }],
  decisions: [{ id: "d-1", title: "Release BRK-100 from Supplier A", subject: "BRK-100", outcome: "approved", status: "potentially_affected", derivation_sha256: "ef".repeat(32) }],
  audit_chain: { intact: true, events: 7, head_sha256: "aa".repeat(32) },
  statement: "Hashes show integrity since capture, not that document contents are true or authentic.",
  report_sha256: "9f".repeat(32),
  signature: { algorithm: "ed25519", public_key: "12".repeat(32), signature: "34".repeat(64) },
};

export const AI_NOTICE_TEXT =
  "Document text is sent to Example AI Inc. (US) for extraction only.\nIt is not retained or used for training. 文書テキストは抽出のみに使用されます。";

export const aiExtraction = (over: Partial<AiExtraction> = {}): AiExtraction => ({
  version: 3,
  settings: { enabled: false, notice_version: null },
  active: false,
  notice: { version: "2026-11-01", text: AI_NOTICE_TEXT },
  provider: { configured: true, model: "extract-small-1", host: "api.example-ai.test", daily_calls: 500, daily_chars: 2000000, calls_per_min: 20 },
  usage_today: { calls: 12, prompt_chars: 48000 },
  ...over,
});

const detThresholds = {
  brute_force: 5,
  brute_force_lock: 10,
  spray_failures: 20,
  spray_users: 5,
  authz_denials: 30,
  privileged_ops: 50,
  upload_abuse: 100,
  extraction_failures: 25,
  exports: 20,
};

export const detectionSettings = (over: Partial<DetectionSettings> = {}): DetectionSettings => ({
  version: 2,
  settings: {},
  effective: { mode: "static", thresholds: { ...detThresholds }, adaptive_max_factor: 3, baseline_days: 14 },
  defaults: { mode: "static", thresholds: { ...detThresholds }, adaptive_max_factor: 3, baseline_days: 14 },
  bounds: {
    brute_force: [3, 50],
    brute_force_lock: [3, 100],
    spray_failures: [5, 500],
    spray_users: [2, 100],
    authz_denials: [3, 150],
    privileged_ops: [5, 1000],
    upload_abuse: [10, 5000],
    extraction_failures: [5, 1000],
    exports: [5, 1000],
    adaptive_max_factor: [1, 5],
    baseline_days: [3, 60],
  },
  active_thresholds: { ...detThresholds, upload_abuse: 180 },
  ...over,
});
