//! ABBA benchmark of the bench statistics, comparing the frozen Rust seed
//! copy (`vesper-bench-rust`) with this package's build (port wave P3a lane
//! 5, #1040). Run it with the port:
//!
//! ```sh
//! cargo run --release -p vesper-bench --features vesper-port --example abba
//! ```
//!
//! Both builds live in one binary, so the arms run in one process and the
//! seed's own `run_abba` times them: A is the Rust build and B this build,
//! in windows A B B A. Both estimators accept at most 32 pairs, so the
//! default workload (`BENCH_ABBA_WORKLOAD=estimators`) is
//! `BENCH_ABBA_BATCHES` batches (default 10,000) of `BENCH_ABBA_PAIRS`
//! distinct paired differences (default 32, at most 32); each batch gets
//! `signed_rank_confidence_set` and `hodges_lehmann_paired`. The
//! `statistics` workload is a series of `BENCH_ABBA_SAMPLES` paired samples
//! (default 100,000) in windows of 64 epochs, the most `analyze_pairs`
//! takes; each window gets `analyze_pairs` and each sample
//! `regression_bp_bounds`. `BENCH_ABBA_ONLY=rust|port` runs one arm once and
//! nothing else (for callgrind). The report is the median B/A ratio and its
//! exact 95% interval from `analyze_pairs`.
use std::convert::Infallible;
use vesper_bench as port;
use vesper_bench_rust as rust;

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Distinct nonzero differences (no tied magnitudes, so Hodges-Lehmann
/// reaches its interval): a fixed xorshift stream around a small shift.
fn stream(samples: usize) -> Vec<i64> {
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    (0..samples)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let noise = (x % 2_000_001) as i64 - 1_000_000;
            let v = noise * 4096 + 5_000 + i as i64;
            if v == 0 { 1 } else { v }
        })
        .collect()
}

/// A long series of paired samples: baselines from the stream, candidates
/// a few percent off in either direction, a common operation count.
fn series(samples: usize) -> Vec<(u64, u64)> {
    stream(samples)
        .iter()
        .map(|&v| {
            let base = 1_000_000 + v.unsigned_abs() % 1_000_000;
            let shift = (v.unsigned_abs() >> 20) % 101;
            (base, base + base * shift / 2_000)
        })
        .collect()
}

macro_rules! passes {
    ($name:ident, $series:ident, $lib:ident) => {
        fn $name(data: &[i64], pairs: usize) -> usize {
            let mut work = 0;
            for w in data.chunks(pairs) {
                work += $lib::signed_rank_confidence_set(w).unwrap().len();
                if let Ok(e) = $lib::hodges_lehmann_paired(w) {
                    work += e.interval.map_or(0, |i| i.critical_rank);
                }
            }
            work
        }
        fn $series(data: &[(u64, u64)]) -> usize {
            let mut work = 0;
            for w in data.chunks($lib::MAX_EPOCHS) {
                let samples: Vec<$lib::PairedSample> = w
                    .iter()
                    .map(|&(b, c)| $lib::PairedSample::new(b, c, 1_000).unwrap())
                    .collect();
                for s in &samples {
                    work += s.regression_bp_bounds().ceil.unsigned_abs() as usize;
                }
                work += $lib::analyze_pairs(&samples, 100).unwrap().decision as usize;
            }
            work
        }
    };
}
passes!(rust_pass, rust_series, rust);
passes!(port_pass, port_series, port);

enum Workload<'a> {
    Estimators(&'a [i64], usize),
    Statistics(&'a [(u64, u64)]),
}

impl Workload<'_> {
    fn run(&self, port: bool) -> usize {
        match (self, port) {
            (Workload::Estimators(d, p), false) => rust_pass(d, *p),
            (Workload::Estimators(d, p), true) => port_pass(d, *p),
            (Workload::Statistics(d), false) => rust_series(d),
            (Workload::Statistics(d), true) => port_series(d),
        }
    }
}

struct Arm<'a> {
    work: &'a Workload<'a>,
    port: bool,
}
impl rust::BenchCase for Arm<'_> {
    type Input = ();
    type Output = usize;
    type Error = Infallible;
    fn setup(&mut self, _: rust::Invocation) -> Result<(), Infallible> {
        Ok(())
    }
    fn run(&mut self, (): ()) -> Result<usize, Infallible> {
        Ok(std::hint::black_box(self.work.run(self.port)))
    }
}

fn main() {
    let pairs = env("BENCH_ABBA_PAIRS", 32).clamp(1, 32);
    let batches = env("BENCH_ABBA_BATCHES", 10_000);
    let samples = env("BENCH_ABBA_SAMPLES", 100_000);
    let differences;
    let paired;
    let (work, label) = match std::env::var("BENCH_ABBA_WORKLOAD").as_deref() {
        Ok("statistics") => {
            paired = series(samples);
            (
                Workload::Statistics(&paired),
                format!("statistics over {samples} paired samples"),
            )
        }
        _ => {
            differences = stream(batches * pairs);
            (
                Workload::Estimators(&differences, pairs),
                format!("estimators on {batches} batches of {pairs} pairs"),
            )
        }
    };
    match std::env::var("BENCH_ABBA_ONLY").as_deref() {
        Ok("rust") => {
            println!("{}", work.run(false));
            return;
        }
        Ok("port") => {
            println!("{}", work.run(true));
            return;
        }
        _ => {}
    }
    assert_eq!(work.run(false), work.run(true), "the builds disagree");
    let config = rust::RunConfig {
        epochs: env("BENCH_ABBA_EPOCHS", 12),
        warmup_epochs: 1,
        operations_per_window: 1,
        max_total_calls: 1_000_000,
        max_elapsed_nanos: 600_000_000_000,
        layout_seed: 0,
    };
    let mut a = Arm {
        work: &work,
        port: false,
    };
    let mut b = Arm {
        work: &work,
        port: true,
    };
    let report = rust::run_abba(config, &mut a, &mut b).expect("ABBA run");
    let pairs_out = report.pairs();
    let ratios: Vec<f64> = pairs_out
        .iter()
        .map(|p| p.candidate_total() as f64 / p.baseline_total() as f64)
        .collect();
    let mut sorted = ratios.clone();
    sorted.sort_by(f64::total_cmp);
    let analysis = rust::analyze_pairs(pairs_out, 0).expect("analysis");
    let ratio = |s: rust::PairedSample| s.candidate_total() as f64 / s.baseline_total() as f64;
    let a_ms = pairs_out.iter().map(|p| p.baseline_total()).sum::<u64>() as f64
        / (2 * pairs_out.len()) as f64
        / 1e6;
    let b_ms = pairs_out.iter().map(|p| p.candidate_total()).sum::<u64>() as f64
        / (2 * pairs_out.len()) as f64
        / 1e6;
    println!(
        "{label}: {} epochs, Rust {a_ms:.2} ms, {} {b_ms:.2} ms per pass",
        pairs_out.len(),
        port::IMPLEMENTATION
    );
    println!(
        "median B/A {:.3}, exact interval [{:.3}, {:.3}], decision {:?}",
        sorted[sorted.len() / 2],
        analysis.interval.map_or(f64::NAN, |i| ratio(i.lower)),
        analysis.interval.map_or(f64::NAN, |i| ratio(i.upper)),
        analysis.decision
    );
}
