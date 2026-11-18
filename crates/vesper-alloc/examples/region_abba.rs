//! ABBA benchmark of the region hot paths, comparing the Rust seed build
//! (`vesper-alloc-seed`) with the Vesper port (`vesper-alloc`), port wave P1
//! lane 6 (#903).
//!
//! One binary holds one build, so the arms are two binaries:
//!
//! ```sh
//! cargo build --release -p vesper-alloc-seed --example region_abba
//! cp target/release/examples/region_abba /tmp/rust_arm
//! cargo build --release -p vesper-alloc --example region_abba
//! cp target/release/examples/region_abba /tmp/port_arm
//! /tmp/rust_arm driver /tmp/rust_arm /tmp/port_arm
//! ```
//!
//! `window W N` prints the nanoseconds of N passes of workload W after one
//! warm-up pass. `driver A B` runs, per workload, epochs of windows in the
//! order A B B A, pairs each epoch's totals and reports the median
//! regression of B over A with `vesper_bench::analyze_pairs`, against the
//! port plan's 2x stop rule.
//!
//! Workloads:
//! - `vec_push`: checkpoint, push 4096 `u64` into a fresh region vector
//!   (amortized growth in place at the tip), read them back, rewind.
//! - `values`: checkpoint, 1024 `alloc_value` + `get`, rewind.
//! - `bytes`: checkpoint, 1024 `alloc_bytes_copy` of 24 bytes + `bytes`, rewind.
//! - `btree`: `CacheBTree<u64, u64>` insert 1024 keys, look each up, sum a
//!   range. The B-tree is not forwarded to the port (see the lane report),
//!   so both arms run the same Rust code: this is the noise control.
use std::process::Command;
use std::time::Instant;
use vesper_alloc::{CacheBTree, Region, RegionConfig};

const EPOCHS: usize = 9;
const WORKLOADS: &[(&str, usize, u64)] = &[
    // (name, passes per window, operations per pass)
    ("vec_push", 400, 4096),
    ("values", 400, 1024),
    ("bytes", 400, 1024),
    ("btree", 200, 1024),
];

fn vec_push(r: &mut Region) -> u64 {
    let mark = r.checkpoint().unwrap();
    let mut v = r.vec_new::<u64>().unwrap();
    for i in 0..4096u64 {
        r.vec_push(&mut v, i).unwrap();
    }
    let sum = r.vec_as_slice(&v).unwrap().iter().sum();
    r.rewind(mark).unwrap();
    sum
}

fn values(r: &mut Region) -> u64 {
    let mark = r.checkpoint().unwrap();
    let mut sum = 0u64;
    for i in 0..1024u64 {
        let b = r.alloc_value(i).unwrap();
        sum = sum.wrapping_add(*r.get(&b).unwrap());
    }
    r.rewind(mark).unwrap();
    sum
}

fn bytes(r: &mut Region) -> u64 {
    let mark = r.checkpoint().unwrap();
    let data = [7u8; 24];
    let mut sum = 0u64;
    for _ in 0..1024 {
        let h = r.alloc_bytes_copy(&data, 8).unwrap();
        sum = sum.wrapping_add(u64::from(r.bytes(&h).unwrap()[3]));
    }
    r.rewind(mark).unwrap();
    sum
}

fn btree() -> u64 {
    let mut t = CacheBTree::<u64, u64>::new();
    for i in 0..1024u64 {
        t.insert(i.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40, i)
            .unwrap();
    }
    let mut sum = 0u64;
    for i in 0..1024u64 {
        sum = sum.wrapping_add(
            *t.get(&(i.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 40))
                .unwrap(),
        );
    }
    for (_, v) in t.range(1000..9_000_000) {
        sum = sum.wrapping_add(*v);
    }
    sum
}

fn window(workload: &str, passes: usize) -> u128 {
    let mut r = Region::new(RegionConfig::bounded(64 << 20)).unwrap();
    let mut pass = || match workload {
        "vec_push" => vec_push(&mut r),
        "values" => values(&mut r),
        "bytes" => bytes(&mut r),
        "btree" => btree(),
        _ => panic!("unknown workload {workload}"),
    };
    std::hint::black_box(pass());
    let start = Instant::now();
    for _ in 0..passes {
        std::hint::black_box(pass());
    }
    start.elapsed().as_nanos()
}

fn run(bin: &str, workload: &str, passes: usize) -> u64 {
    let out = Command::new(bin)
        .args(["window", workload, &passes.to_string()])
        .output()
        .expect("run arm");
    assert!(out.status.success(), "{bin} failed");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("window") => println!("{}", window(&args[2], args[3].parse().unwrap())),
        Some("driver") => {
            let (a, b) = (&args[2], &args[3]);
            for &(workload, passes, ops) in WORKLOADS {
                let mut pairs = Vec::new();
                let (mut ta, mut tb) = (0u64, 0u64);
                for _ in 0..EPOCHS {
                    let a1 = run(a, workload, passes);
                    let b1 = run(b, workload, passes);
                    let b2 = run(b, workload, passes);
                    let a2 = run(a, workload, passes);
                    ta += a1 + a2;
                    tb += b1 + b2;
                    pairs.push(
                        vesper_bench::PairedSample::new(a1 + a2, b1 + b2, 2 * passes as u64)
                            .unwrap(),
                    );
                }
                let per_op = |t: u64| t as f64 / (2 * EPOCHS as u64 * passes as u64 * ops) as f64;
                let analysis = vesper_bench::analyze_pairs(&pairs, 10_000).unwrap();
                println!(
                    "{workload}: A {:.2} ns/op, B {:.2} ns/op, B/A {:.2}; 2x gate {:?}, median regression bp {:?}",
                    per_op(ta),
                    per_op(tb),
                    tb as f64 / ta as f64,
                    analysis.decision,
                    analysis.median_upper.regression_bp_bounds()
                );
            }
        }
        _ => {
            eprintln!("usage: region_abba window WORKLOAD N | driver RUST_BIN PORT_BIN");
            std::process::exit(2);
        }
    }
}
