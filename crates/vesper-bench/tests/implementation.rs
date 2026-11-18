//! Which build ran the other tests, and that the port leaks nothing: the
//! port-backed build under `vesper-port`, the Rust build otherwise (the
//! default, and the seed package `vesper-bench-seed`).
use vesper_bench::{
    IMPLEMENTATION, PairedSample, analyze_pairs, hodges_lehmann_paired, port_live_allocations,
    signed_rank_confidence_set,
};

// The seed package has no `vesper-port` feature; there the check is off.
#[allow(unexpected_cfgs)]
#[test]
fn the_feature_selects_the_build() {
    let port = cfg!(feature = "vesper-port");
    assert_eq!(IMPLEMENTATION, if port { "vesper" } else { "rust" });
}

#[test]
fn statistics_free_every_sequence() {
    let before = port_live_allocations();
    let differences: Vec<i64> = (1..=24).map(|i| i * 7 - 60).collect();
    assert!(!signed_rank_confidence_set(&differences).unwrap().is_empty());
    // Distinct powers of two: no tied magnitudes or Walsh sums.
    let untied: Vec<i64> = (1..=20).map(|i| 1 << i).collect();
    assert!(hodges_lehmann_paired(&untied).unwrap().interval.is_some());
    let samples: Vec<PairedSample> = (1..=12)
        .map(|i| PairedSample::new(1000 + i, 1010 + 2 * i, 4).unwrap())
        .collect();
    analyze_pairs(&samples, 50).unwrap();
    assert_eq!(port_live_allocations(), before);
}
