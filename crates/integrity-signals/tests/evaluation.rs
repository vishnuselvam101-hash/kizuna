#![allow(clippy::unwrap_used)]
//! Precision / recall on generated data with known ground truth (see `synth`). The thresholds below are the
//! acceptance bar; the measured numbers are recorded in docs/testing/INTEGRITY_SIGNALS_EVAL.md
//! (`cargo run --release -p integrity-signals --example evaluate`). Synthetic data only: these numbers are an
//! upper bound for real documents, where extraction errors and real process behaviour add noise.

use integrity_signals::Settings;
use integrity_signals::synth::{certificates, drawings, score_certificates, score_drift, standard_inject};

#[test]
fn certificate_signals_precision_and_recall() {
    let s = Settings::default();
    let inject = standard_inject();
    let (mut inj, mut det, mut sig, mut tp, mut honest_fp, mut honest) = (0, 0, 0, 0, 0.0, 0);
    for seed in 1..=3 {
        let r = score_certificates(&certificates(seed, &inject, 40), &s);
        for (k, sc) in &r.per_kind {
            assert!(sc.recall >= 0.6, "seed {seed} {k}: recall {}", sc.recall);
            assert!(sc.precision >= 0.6, "seed {seed} {k}: precision {}", sc.precision);
        }
        inj += r.overall.injected;
        det += r.overall.detected;
        sig += r.overall.signals;
        tp += r.overall.true_positive_signals;
        honest_fp += r.false_signals_per_1000_honest * r.honest_certificates as f64 / 1000.0;
        honest += r.honest_certificates;
    }
    let (recall, precision) = (det as f64 / inj as f64, tp as f64 / sig as f64);
    assert!(recall >= 0.9, "overall recall {recall}");
    assert!(precision >= 0.9, "overall precision {precision}");
    assert!(
        honest_fp * 1000.0 / honest as f64 <= 2.0,
        "false signals per 1000 honest certificates"
    );
}

#[test]
fn revision_drift_precision_and_recall() {
    let s = Settings::default();
    for seed in 1..=3 {
        let r = score_drift(&drawings(seed, 60, 8), &s);
        for (k, sc) in &r.per_kind {
            assert!(sc.recall >= 0.9, "seed {seed} {k}: recall {}", sc.recall);
            assert!(sc.precision >= 0.9, "seed {seed} {k}: precision {}", sc.precision);
        }
    }
}
