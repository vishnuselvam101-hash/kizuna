export interface Artifact {
  id: string;
  sha256: string;
  size_bytes: number;
  media_type: string;
  kind: string;
  status: string;
  display_name: string;
  created_at: string;
  scan_result: { findings?: string[] };
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
  artifact_id: string | null;
  source_location: Record<string, string | number>;
  extraction_method: string;
  confidence_bps: number;
  review_status: string;
  recorded_at: string;
  superseded_at: string | null;
}

export interface PredicateEvidence {
  predicate: string;
  current: Claim[];
  superseded_count: number;
  disagreement: boolean;
}
