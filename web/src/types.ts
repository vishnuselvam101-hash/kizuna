export interface Artifact {
  id: string;
  sha256: string;
  size_bytes: number;
  media_type: string;
  kind: string;
  status: string;
  display_name: string;
  created_at: string;
  created_by?: string;
  scan_result: { findings?: string[]; released_by?: string; released_at?: string; quarantined_by_incident?: unknown };
}

export interface ArtifactDetail {
  artifact: Artifact;
  captures: { uploaded_by: string; uploaded_at: string; original_filename: string; declared_type: string | null }[];
  claim_count: number;
}

export interface MfaStatus {
  totp_enabled: boolean;
  webauthn_enabled: boolean;
  credentials: { id: string; label: string; created_at: string; last_used_at: string | null }[];
  recovery_codes_remaining: number;
}

export interface Claim {
  id: string;
  subject_type: string;
  subject_key: string;
  predicate: string;
  value_text: string | null;
  value_num: string | null;
  value_unit: string | null;
  revision: string | null;
  scope?: Record<string, unknown>;
  artifact_id: string | null;
  artifact_display_name?: string | null;
  source_location: Record<string, string | number>;
  extraction_method: string;
  confidence_bps: number;
  review_status: string;
  recorded_at: string;
  superseded_at: string | null;
  recorded_by?: string | null;
  provenance?: Record<string, unknown>;
}

export interface PredicateEvidence {
  predicate: string;
  current: Claim[];
  superseded_count: number;
  disagreement: boolean;
}

export interface Me {
  user_id: string;
  tenant_id: string;
  email: string;
  display_name: string;
  role: string;
  mfa_enabled: boolean;
  session_mfa_verified: boolean;
}

export interface User {
  id: string;
  email: string;
  display_name: string;
  role: string;
  status: string;
  totp_enabled: boolean;
  locked_until: string | null;
}

export interface AuditEvent {
  seq: number;
  occurred_at: string;
  actor_user_id: string | null;
  actor_kind: string;
  action: string;
  target_type: string;
  target_id: string | null;
  details: unknown;
  hash: string;
}

export interface ChainStatus {
  intact: boolean;
  events_checked: number;
  problem: string | null;
  head_seq: number;
  head_hash: string;
}

export interface Conflict {
  id: string;
  kind: string;
  subject_key: string;
  status: string;
  explanation: { why?: string; [k: string]: unknown };
  detected_at: string;
}

export interface ConflictSource {
  claim_id: string;
  artifact_id?: string;
  display_name: string;
  sha256?: string;
  location: Record<string, string | number>;
}

export interface ConflictDetail {
  conflict: Conflict;
  claim_a: Claim;
  claim_b: Claim | null;
  sources: ConflictSource[];
  reviews: { decision: string; comment: string | null; decided_at: string }[];
  requires_human_review: boolean;
  limitations: string;
  potentially_affected_decisions?: { id: string; title: string; status: string }[];
}

export interface Decision {
  id: string;
  kind: string;
  title: string;
  subject_type?: string;
  subject_key: string;
  outcome: string;
  status: string;
  decided_at: string;
  derivation_sha256: string;
  evidence_claim_ids?: string[];
}

export interface Snap {
  num?: string | null;
  text?: string | null;
  unit?: string | null;
}

export interface TraceEvidence {
  claim_id?: string;
  upstream_decision?: string;
  predicate?: string;
  snapshot_at_decision: Snap;
  current_value?: Snap;
  current_status?: string;
  current_review_status?: string;
  changed: boolean;
}

export interface Invalidation {
  reason: string;
  material: boolean;
  recorded_at: string;
  cause_kind: string;
  previous_status?: string;
  new_status?: string;
}

export interface Trace {
  decision: Decision & { rationale: string };
  derivation_reproducible: boolean;
  evidence: TraceEvidence[];
  invalidations: Invalidation[];
  changed_since_decision: { claim_id: string; predicate: string; value: Snap; recorded_at: string; event: string }[];
}

export interface Difference {
  predicate: string;
  from: string | null;
  to: string | null;
  change: string;
}

export interface EntityCandidate {
  left: string;
  right: string;
  outcome: string;
  reasons: string[];
}

/** `GET /dashboard/summary`: counts keyed by status (or kind / subject type). */
export interface DashboardSummary {
  artifacts: Record<string, number>;
  claims: Record<string, number>;
  conflicts: Record<string, number>;
  conflict_kinds_open: Record<string, number>;
  decisions: Record<string, number>;
  subjects: Record<string, number>;
}

/** `GET /subjects?type=` row. */
export interface Subject {
  subject_type: string;
  subject_key: string;
  claims: number;
  predicates: number;
}

/** `POST /reports` organisation-level report. */
export interface OrgReport {
  report_version: string;
  generated_at: string;
  scope: string;
  findings: { id: string; kind: string; subject: string; status: string; why: string | null; detected_at: string }[];
  decisions: { id: string; title: string; subject: string; outcome: string; status: string; derivation_sha256: string }[];
  audit_chain: { intact: boolean; events: number; head_sha256: string | null };
  statement: string;
  report_sha256: string;
  signature?: { algorithm: string; public_key: string; signature: string };
}
