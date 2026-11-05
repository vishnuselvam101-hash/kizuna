//! Precision / recall of the certificate signals and the revision drift radar on generated data with known ground
//! truth. Usage: cargo run --release -p integrity-signals --example evaluate [seeds]
use integrity_signals::Settings;
use integrity_signals::synth::{certificates, drawings, score_certificates, score_drift, standard_inject};

fn main() {
    let seeds: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(8);
    let s = Settings::default();
    let inject = standard_inject();
    let mut certs = Vec::new();
    let mut drift = Vec::new();
    for seed in 1..=seeds {
        let t = std::time::Instant::now();
        let c = score_certificates(&certificates(seed, &inject, 40), &s);
        let d = score_drift(&drawings(seed, 60, 8), &s);
        eprintln!("seed {seed}: {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        certs.push(c);
        drift.push(d);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({ "seeds": seeds, "certificates": certs, "drift": drift }))
            .unwrap_or_default()
    );
}
