//! ABBA benchmark of parse + check + optimize over the planner golden case
//! set (`tests/fixtures/planner/cases.txt`), comparing the Rust build with
//! the `vesper-port` build (port wave P1 lane 7, #904).
//!
//! One binary holds one build, so the arms are two binaries:
//!
//! ```sh
//! cargo build --release -p vesper-quarry --example planner_abba
//! cp target/release/examples/planner_abba /tmp/rust_arm
//! cargo build --release -p vesper-quarry --example planner_abba --features vesper-port
//! cp target/release/examples/planner_abba /tmp/port_arm
//! /tmp/rust_arm driver /tmp/rust_arm /tmp/port_arm
//! ```
//!
//! `window N` prints the nanoseconds of N passes over the case set after one
//! warm-up pass. `driver A B` runs epochs of windows in the order A B B A,
//! pairs each epoch's A and B totals and reports the median regression of
//! B over A with `vesper_bench::analyze_pairs`, plus the 2x stop rule of the
//! port plan.
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;
use vesper_quarry::{HermeticCatalog, Limits, ParamDef, ScalarType};

const PASSES: usize = 200;
const EPOCHS: usize = 9;

fn cases() -> Vec<(String, Vec<ParamDef>)> {
    let text = include_str!("../tests/fixtures/planner/cases.txt");
    let mut out: Vec<(String, Vec<ParamDef>)> = Vec::new();
    let mut params = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(p) = line.strip_prefix("params: ") {
            for p in p.split(',').map(str::trim) {
                let (decl, _) = p.split_once('=').unwrap();
                let (name, ty) = decl.split_once(':').unwrap();
                let ty = match ty {
                    "U64" => ScalarType::U64,
                    "TEXT" => ScalarType::Text,
                    "BOOL" => ScalarType::Bool,
                    _ => ScalarType::I64,
                };
                params.push(ParamDef {
                    name: name.into(),
                    ty,
                    nullable: true,
                });
            }
        } else if line.starts_with("SELECT") {
            out.push((line.to_string(), std::mem::take(&mut params)));
        }
    }
    out
}

fn catalog() -> HermeticCatalog {
    let mut c = HermeticCatalog::default();
    c.apply_ddl(
        "CREATE TABLE p (id I64 NOT NULL, age I64 NOT NULL, name TEXT NOT NULL, city TEXT); \
         CREATE TABLE o (id I64 NOT NULL, owner I64 NOT NULL, amount I64 NOT NULL); \
         CREATE TABLE c (code TEXT NOT NULL, label TEXT NOT NULL)",
        Limits::default(),
    )
    .unwrap();
    c
}

fn pass(c: &HermeticCatalog, cases: &[(String, Vec<ParamDef>)]) -> usize {
    let mut nodes = 0;
    for (sql, params) in cases {
        let q = c.prepare(sql, params, Limits::default()).unwrap();
        nodes += q.optimized().columns().len();
    }
    nodes
}

fn window(passes: usize) -> u128 {
    let c = catalog();
    let cases = cases();
    std::hint::black_box(pass(&c, &cases));
    let start = Instant::now();
    for _ in 0..passes {
        std::hint::black_box(pass(&c, &cases));
    }
    start.elapsed().as_nanos()
}

fn run(bin: &str) -> u64 {
    let out = Command::new(bin)
        .args(["window", &PASSES.to_string()])
        .output()
        .expect("run arm");
    assert!(out.status.success(), "{bin} failed");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("window") => println!("{}", window(args[2].parse().unwrap())),
        Some("driver") => {
            let (a, b) = (&args[2], &args[3]);
            let mut pairs = Vec::new();
            let mut by_epoch = BTreeMap::new();
            for epoch in 0..EPOCHS {
                let a1 = run(a);
                let b1 = run(b);
                let b2 = run(b);
                let a2 = run(a);
                by_epoch.insert(epoch, (a1 + a2, b1 + b2));
                pairs.push(
                    vesper_bench::PairedSample::new(a1 + a2, b1 + b2, 2 * PASSES as u64).unwrap(),
                );
            }
            let cases = cases().len() as u64;
            for (e, (a, b)) in &by_epoch {
                println!(
                    "epoch {e}: A {:.1} us/query, B {:.1} us/query, B/A {:.3}",
                    *a as f64 / (2 * PASSES as u64 * cases) as f64 / 1e3,
                    *b as f64 / (2 * PASSES as u64 * cases) as f64 / 1e3,
                    *b as f64 / *a as f64
                );
            }
            let analysis = vesper_bench::analyze_pairs(&pairs, 10_000).unwrap();
            println!(
                "2x gate (B at most +10000 bp over A): {:?}; median regression bp {:?}",
                analysis.decision,
                analysis.median_upper.regression_bp_bounds()
            );
        }
        _ => {
            eprintln!("usage: planner_abba window N | driver RUST_BIN PORT_BIN");
            std::process::exit(2);
        }
    }
}
