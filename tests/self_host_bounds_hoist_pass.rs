//! Self-hosting SH3 lane 12 (#1251): the `@vesper/bounds-hoist` pass
//! against `bounds_hoist::run` over the corpus.
//!
//! Every corpus world is lowered to the bounds-hoisting boundary of the
//! native optimize pipeline (tests/self_host_bounds_hoist/corpus.rs). The
//! program goes through lane 1's pass harness (`ir_words::compare_pass`):
//! Rust runs `bounds_hoist::run`, the Vesper package reads the program into
//! @vesper/place-ir tables, versions loops, marks accesses unchecked and
//! writes the program back. The two outputs must have the same canonical
//! dump, be `Debug`-equal and get the same IR check diagnostics. The
//! guards and unchecked accesses the port makes are the pool kinds 5 and 6
//! of tests/common/ir_words.rs (`made_payload`).
#[path = "common/check_harness.rs"]
mod check_harness;
#[path = "common/check_words.rs"]
mod check_words;
#[path = "self_host_bounds_hoist/corpus.rs"]
mod corpus;
#[path = "common/ir_words.rs"]
mod ir_words;

use check_harness::*;
use std::sync::OnceLock;
use vesper::bounds_hoist;
use vesper::ir::Program;

fn package() -> &'static Package {
    static P: OnceLock<Package> = OnceLock::new();
    P.get_or_init(|| Package::load("packages/vesper-bounds-hoist"))
}

fn entry() -> &'static Entry {
    static E: OnceLock<Entry> = OnceLock::new();
    E.get_or_init(|| package().entry("hoist_pass", true))
}

/// Every `BOUNDS_HOIST_PASS_STRIDE`th corpus world (default 6).
fn stride() -> usize {
    std::env::var("BOUNDS_HOIST_PASS_STRIDE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(6)
}

/// The first function whose output differs, for the failure message.
fn first_function_difference(input: &Program) -> String {
    let mut ours = input.clone();
    bounds_hoist::run(&mut ours);
    let Some(theirs) = ir_words::through_vesper(None, input, |i| entry().words(i)) else {
        return "not encodable".into();
    };
    for (name, f) in &ours.functions {
        match theirs.functions.get(name) {
            None => return format!("{name}: missing"),
            Some(g) if format!("{f:?}") != format!("{g:?}") => {
                let (a, b) = (format!("{f:#?}"), format!("{g:#?}"));
                let line = a
                    .lines()
                    .zip(b.lines())
                    .enumerate()
                    .find(|(_, (x, y))| x != y)
                    .map(|(i, (x, y))| format!("line {i}:\n    rust:   {x}\n    vesper: {y}"))
                    .unwrap_or_else(|| "lengths differ".into());
                return format!("{name}: {line}");
            }
            Some(_) => {}
        }
    }
    "no function differs".into()
}

#[test]
fn pass_matches_bounds_hoist_run() {
    let (mut programs, mut skipped, mut bad, mut versioned, mut changed) = (0, 0, 0, 0, 0);
    let start = std::time::Instant::now();
    for (case, program) in corpus::programs(stride()) {
        let mut probe = program.clone();
        let n = bounds_hoist::run(&mut probe);
        versioned += n;
        changed += (format!("{probe:?}") != format!("{program:?}")) as usize;
        let out = ir_words::compare_pass(
            &case.world,
            program,
            |p| {
                bounds_hoist::run(p);
            },
            |i| entry().words(i),
        );
        if out.skipped {
            skipped += 1;
            continue;
        }
        programs += 1;
        if !out.agrees() {
            bad += 1;
            if bad <= 6 {
                eprintln!(
                    "bounds hoist pass: {} differs ({out:?})\n  {}",
                    case.name,
                    first_function_difference(program)
                );
            }
        }
    }
    eprintln!(
        "bounds hoist pass: {bad} of {programs} programs differ ({skipped} not encodable); \
         Rust versions {versioned} loops and changes {changed} programs; {:.1?}",
        start.elapsed()
    );
    assert!(changed > 0, "the sample has nothing to hoist");
    assert_eq!(bad, 0);
}
