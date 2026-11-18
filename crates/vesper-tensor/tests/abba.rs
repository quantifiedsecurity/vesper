//! ABBA benchmarks of the Vesper port against the Rust build (port wave P1
//! lane 8, #905), with `vesper-bench`: dequantization (Q4_K and Q8_0 rows),
//! a Q8_0 matrix-vector product, Q4_K quantization and top-p sampling. Both
//! builds are in one process: `vesper_tensor` is the port-backed package and
//! `seed` the Rust seed copy (a dev-dependency). The benchmarks are ignored
//! by default; run them with
//!
//! ```text
//! cargo test --release -p vesper-tensor --test abba -- --ignored --nocapture --test-threads=1
//! ```
use std::hint::black_box;

use vesper_bench::{analyze_pairs, run_abba, BenchCase, Invocation, RunConfig};

/// Deterministic pseudo-random values in about `[-1, 1]`.
fn values(seed: u64, n: usize) -> Vec<f32> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((x >> 40) as f32 / (1u64 << 23) as f32) - 1.0
        })
        .collect()
}

struct Case<F>(F);
impl<F: FnMut() -> u64> BenchCase for Case<F> {
    type Input = ();
    type Output = u64;
    type Error = ();
    fn setup(&mut self, _: Invocation) -> Result<(), ()> {
        Ok(())
    }
    fn run(&mut self, (): ()) -> Result<u64, ()> {
        Ok(black_box((self.0)()))
    }
}

fn checksum(v: &[f32]) -> u64 {
    v.iter()
        .fold(0u64, |h, x| h.rotate_left(5) ^ u64::from(x.to_bits()))
}

/// Runs one ABBA comparison and prints the Vesper/Rust ratio with the 2x
/// stop rule of the wave plan.
fn abba(name: &str, rust: impl FnMut() -> u64, vesper: impl FnMut() -> u64) {
    let run = RunConfig {
        epochs: 20,
        warmup_epochs: 2,
        operations_per_window: 4,
        max_total_calls: 100_000,
        max_elapsed_nanos: 600_000_000_000,
        layout_seed: 1,
    };
    let report = run_abba(run, &mut Case(rust), &mut Case(vesper)).expect("ABBA run");
    let pairs = report.pairs();
    let mut ratios: Vec<f64> = pairs
        .iter()
        .map(|p| p.candidate_total() as f64 / p.baseline_total() as f64)
        .collect();
    ratios.sort_by(f64::total_cmp);
    let per_op = |total: u64, p: &vesper_bench::PairedSample| {
        total as f64 / p.operations_per_arm() as f64 / 1e3
    };
    let mut rust_us: Vec<f64> = pairs
        .iter()
        .map(|p| per_op(p.baseline_total(), p))
        .collect();
    let mut vesper_us: Vec<f64> = pairs
        .iter()
        .map(|p| per_op(p.candidate_total(), p))
        .collect();
    rust_us.sort_by(f64::total_cmp);
    vesper_us.sort_by(f64::total_cmp);
    let mid = pairs.len() / 2;
    println!(
        "{name}: {} epochs; median per op rust {:.1} us, vesper {:.1} us; Vesper/Rust median {:.3} (min {:.3}, max {:.3})",
        pairs.len(),
        rust_us[mid],
        vesper_us[mid],
        ratios[mid],
        ratios[0],
        ratios[ratios.len() - 1]
    );
    let analysis = analyze_pairs(pairs, 10_000).expect("analysis");
    if let Some(i) = analysis.interval {
        println!(
            "{name}: 95% interval of Vesper/Rust [{:.3}, {:.3}]; 2x gate {:?}",
            i.lower.candidate_total() as f64 / i.lower.baseline_total() as f64,
            i.upper.candidate_total() as f64 / i.upper.baseline_total() as f64,
            analysis.decision
        );
    }
}

const ROWS: usize = 64;
const COLS: usize = 4096;

macro_rules! matrices {
    ($krate:ident, $format:ident) => {{
        let data = values(7, ROWS * COLS);
        $krate::quant::Matrix::f32(ROWS, COLS, data)
            .unwrap()
            .quantize($krate::quant::WeightFormat::$format)
            .unwrap()
    }};
}

#[test]
fn both_builds_agree_on_the_workloads() {
    for (r, v) in [
        (matrices!(seed, Q4K), matrices!(vesper_tensor, Q4K)),
        (matrices!(seed, Q8_0), matrices!(vesper_tensor, Q8_0)),
    ] {
        assert_eq!(r.raw_bytes(), v.raw_bytes());
        assert_eq!(
            checksum(&r.to_f32().unwrap()),
            checksum(&v.to_f32().unwrap())
        );
    }
    assert_eq!(seed::IMPLEMENTATION, "rust");
    assert_eq!(vesper_tensor::IMPLEMENTATION, "vesper");
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn abba_dequantize_q4_k() {
    let r = matrices!(seed, Q4K);
    let v = matrices!(vesper_tensor, Q4K);
    abba(
        "dequantize Q4_K 64x4096",
        || checksum(&r.to_f32().unwrap()),
        || checksum(&v.to_f32().unwrap()),
    );
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn abba_dequantize_q8_0() {
    let r = matrices!(seed, Q8_0);
    let v = matrices!(vesper_tensor, Q8_0);
    abba(
        "dequantize Q8_0 64x4096",
        || checksum(&r.to_f32().unwrap()),
        || checksum(&v.to_f32().unwrap()),
    );
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn abba_matvec_q8_0() {
    let r = matrices!(seed, Q8_0);
    let v = matrices!(vesper_tensor, Q8_0);
    let x = values(3, COLS);
    let (mut ro, mut vo) = (vec![0.0; ROWS], vec![0.0; ROWS]);
    abba(
        "matvec Q8_0 64x4096 (scalar kernel)",
        || {
            r.matvec(&x, &mut ro).unwrap();
            checksum(&ro)
        },
        || {
            v.matvec(&x, &mut vo).unwrap();
            checksum(&vo)
        },
    );
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn abba_quantize_q4_k() {
    let data = values(11, 16 * COLS);
    abba(
        "quantize Q4_K 16x4096",
        || seed::formats::quantize_q4_k(&data, COLS).unwrap().len() as u64,
        || {
            vesper_tensor::formats::quantize_q4_k(&data, COLS)
                .unwrap()
                .len() as u64
        },
    );
}

#[test]
#[ignore = "benchmark; run in release with --ignored --nocapture"]
fn abba_sample_top_p() {
    const VOCAB: usize = 32_000;
    let logits = values(5, VOCAB)
        .into_iter()
        .map(|v| v * 8.0)
        .collect::<Vec<_>>();
    let mut r = seed::sampling::Sampler::new(
        VOCAB,
        seed::sampling::SamplerConfig {
            temperature: 0.8,
            top_p: 0.9,
            seed: 42,
        },
    )
    .unwrap();
    let mut v = vesper_tensor::sampling::Sampler::new(
        VOCAB,
        vesper_tensor::sampling::SamplerConfig {
            temperature: 0.8,
            top_p: 0.9,
            seed: 42,
        },
    )
    .unwrap();
    abba(
        "sample temperature 0.8 top-p 0.9, vocab 32000",
        || u64::from(r.sample(&logits).unwrap()),
        || u64::from(v.sample(&logits).unwrap()),
    );
}
