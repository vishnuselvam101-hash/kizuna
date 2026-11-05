# ADR 0019: Certificate integrity signals and revision drift radar

Status: accepted (2026-11-05). Context: docs/research/MARKET_GAPS_2026-11.md innovations I2 and I3.

## Decision
- New pure crate `crates/integrity-signals` (`integrity-signals@1`): deterministic signals over current claims, no I/O.
  - Certificates (inspection reports, one per artifact × part × lot/heat): identical values across different
    lots/heats and whole-table copies (coincidence chance from the supplier's own value history, add-one smoothed,
    Bonferroni over all pairs); narrow band (spread ≤ x % of tolerance; needs limits, ≥ n samples, ≥ 20 resolution
    steps per tolerance); values at a spec limit; precision finer than the configured gauge resolution; chemistry
    above 100 % or outside 0–100 %; certificate after shipment / inspection after shipment / certificate before
    inspection; lot numbers against date order; terminal-digit preference via `quality-stats` (χ², n ≥ 50, values
    re-scaled to the resolution so normalised trailing zeros cannot fake a preference).
  - Drawings/specs: versions grouped by 図番 (else part), compared in capture order on extracted fields:
    `silent_change` (same revision, content differs), `revision_without_change` (checked against every copy of the
    previous revision since its last content change), `revision_regression`. Up to N fields present on one side
    only are treated as an extraction gap; a file-hash-only change is reported only if the tenant enables it.
- Output = explanation (EN/JA, neutral, names ordinary causes), statistic + thresholds + sample sizes, evidence
  locations, *strength* (weak/moderate/strong; explicitly not a probability of wrongdoing). Every finding is a
  review item; nothing rejects a claim. Reviewer decisions: signals `explained | follow_up | request_evidence`;
  drift `accept_change | confirm_drift | request_evidence` (reason required, four-eyes, MFA). `confirm_drift` feeds
  the later version's changed claims to selective invalidation (`on_claim_change`, materiality-checked).
- Viewing a signal is audited (`integrity_signal.viewed`). Thresholds per tenant, versioned, bounded
  (`Settings::bounds`), weakening raises `admin.settings_changed` with `weakening=true`.
- Schema: migration 0016 (`integrity_signal_settings`, `integrity_signals`, `revision_drift_findings`,
  `integrity_reviews`), FORCE RLS, append-only settings/reviews, findings never deleted (identity immutable).
- Mapping `mfg-map@1` extended additively: heat number (scope), certificate/ship/inspection dates and chemistry
  columns with a % unit on inspection reports only.

## Consequences / limits
- Measured only on synthetic data with known ground truth (docs/testing/INTEGRITY_SIGNALS_EVAL.md); the generator
  was written alongside the detectors, so these numbers are an upper bound. Real precision needs a blind pilot.
- Spec limits come from the most recently captured drawing/spec per part; units must match exactly (no conversion).
- Content not extracted (views, unread notes) is invisible to drift detection.
