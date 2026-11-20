//! Self-hosting SH2 lane 5 (#1177): `@vesper/ir-check` against
//! `src/ir_check.rs` over the corpus and the census's IR mutants (which make
//! the checker's diagnostics fire; the clean corpus has none).
//!
//! - Text: every kept diagnostic as one line (code, span, message, every
//!   related label and the help), Rust against Vesper.
//! - Words: the hybrid build's path. The world and the program are encoded
//!   by `tests/common/check_words.rs`, and the Vesper `ir_check_words`
//!   stream must decode to exactly `ir_check::validate`'s diagnostics.
#[path = "common/check_harness.rs"]
mod check_harness;
#[path = "common/check_words.rs"]
mod check_words;

use check_harness::*;
use std::fmt::Write;
use std::sync::OnceLock;
use vesper::ast::Diagnostic;

fn package() -> &'static Package {
    static P: OnceLock<Package> = OnceLock::new();
    P.get_or_init(|| Package::load("packages/vesper-ir-check"))
}

/// `check_ir_text` of packages/vesper-ir-check/src/lib.ves.
fn diagnostics_text(ds: &[Diagnostic]) -> String {
    let mut o = String::new();
    for d in ds {
        write!(o, "{} {}-{} {}", d.code, d.span.start, d.span.end, d.message).unwrap();
        for l in &d.related {
            write!(o, " [{}-{} {}]", l.span.start, l.span.end, l.message).unwrap();
        }
        if let Some(h) = d.help.as_deref().filter(|h| !h.is_empty()) {
            write!(o, " help: {h}").unwrap();
        }
        o.push('\n');
    }
    o
}

fn input(c: &IrCase) -> Vec<u8> {
    check_words::ir_input(c.world, &c.program).expect("encodable corpus program")
}

/// Texts of every case, with `mutants` mutants per world; returns how many
/// cases differ and how many diagnostics Rust gave.
fn text_differential(f: &Entry, mutants: usize, step: usize) -> (usize, usize, usize) {
    let (mut bad, mut diags, mut cases) = (0, 0, 0);
    for c in ir_cases(mutants).iter().step_by(step) {
        let rust = vesper::ir_check::validate(c.world, &c.program);
        diags += rust.len();
        cases += 1;
        let r = diagnostics_text(&rust);
        let v = f.text(&input(c));
        if let Some(d) = first_difference(&r, &v) {
            bad += 1;
            if bad <= 5 {
                eprintln!("ir check: {} differs at {d}", c.name);
            }
        }
    }
    eprintln!("ir check text: {bad} of {cases} cases differ ({diags} Rust diagnostics)");
    (bad, diags, cases)
}

#[test]
fn ir_check_text_matches_rust_with_mutants() {
    let f = package().entry("check_ir_text", true);
    let (bad, diags, _) = text_differential(&f, IR_MUTANTS, 1);
    assert!(diags > 1000, "the mutants produced only {diags} diagnostics");
    assert_eq!(bad, 0);
}

/// The optimizer off agrees with it on, on every seventh case.
#[test]
fn ir_check_text_unoptimized_matches_rust() {
    let f = package().entry("check_ir_text", false);
    let (bad, _, _) = text_differential(&f, IR_MUTANTS, 7);
    assert_eq!(bad, 0);
}

/// The hybrid build's word stream decodes to exactly `validate`'s result.
#[test]
fn ir_check_words_decode_to_rust_diagnostics() {
    let f = package().entry("check_ir_words", true);
    let (mut bad, mut diags) = (0, 0);
    let cases = ir_cases(IR_MUTANTS);
    for c in &cases {
        let rust = vesper::ir_check::validate(c.world, &c.program);
        let vesper = check_words::decode_diagnostics(&f.words(&input(c)));
        diags += rust.len();
        if rust != vesper {
            bad += 1;
            if bad <= 3 {
                eprintln!(
                    "{}:\n  rust:   {}\n  vesper: {}",
                    c.name,
                    diagnostics_text(&rust),
                    diagnostics_text(&vesper)
                );
            }
        }
    }
    eprintln!("ir check words: {bad} of {} cases differ ({diags} diagnostics)", cases.len());
    assert!(diags > 1000);
    assert_eq!(bad, 0);
}

/// ABBA: `validate` in Rust against the Vesper check with its input
/// encoding and result decoding (the hybrid build's cost per call).
#[test]
#[ignore = "timing; CHECK_HARNESS_SPEED=1"]
fn ir_check_speed() {
    if !speed() {
        return;
    }
    let f = package().entry("check_ir_words", true);
    let cases = ir_cases(IR_MUTANTS);
    let inputs: Vec<Vec<u8>> = cases.iter().map(input).collect();
    abba(
        "ir check per call (encode + Vesper + decode)",
        &mut || {
            time(|| {
                for c in &cases {
                    std::hint::black_box(vesper::ir_check::validate(c.world, &c.program));
                }
            })
        },
        &mut || {
            time(|| {
                for c in &cases {
                    std::hint::black_box(check_words::decode_diagnostics(&f.words(&input(c))));
                }
            })
        },
    );
    abba(
        "ir check, Vesper alone (input encoded beforehand)",
        &mut || {
            time(|| {
                for c in &cases {
                    std::hint::black_box(vesper::ir_check::validate(c.world, &c.program));
                }
            })
        },
        &mut || {
            time(|| {
                for i in &inputs {
                    std::hint::black_box(f.words(i));
                }
            })
        },
    );
}
