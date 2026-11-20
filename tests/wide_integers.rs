//! 128-bit integers (`i128`, `u128`; self-hosting wave SH2 lane 2, #1174).
//!
//! The census sample (`docs/parallel-results/self-host-census.md`, gap C2)
//! stores `Type::IntLiteral(i128)` as a sign and a `u64` magnitude because
//! Vesper had no 128-bit kind. These tests pin the kinds down: literal
//! typing, the checked contract, the total operators, conversions and the
//! builtins, agreeing in the evaluator (optimized and not) and the JIT
//! (optimized and not).
use vesper::{check, native, parser, runtime::Runtime};

fn checked(source: &str) -> check::Checked {
    let p = parser::parse(source);
    assert!(p.diagnostics.is_empty(), "{source}\n{:?}", p.diagnostics);
    check::check(&p.module)
}
fn accepted(source: &str) -> check::Checked {
    let c = checked(source);
    assert!(c.diagnostics.is_empty(), "{source}\n{:?}", c.diagnostics);
    c
}
/// The evaluator and the JIT, optimized and not, agree on `main` and equal
/// `expected`.
fn agree(source: &str, expected: &str) {
    let c = accepted(source);
    for optimize in [true, false] {
        let mut runtime = Runtime::new(&c.world);
        runtime.optimize = optimize;
        let value = runtime.run("main", vec![]).unwrap().to_string();
        assert_eq!(value, expected, "evaluator (optimize {optimize}): {source}");
        let jit = native::jit_with_options(&c.world, "main", optimize)
            .unwrap()
            .to_string();
        assert_eq!(jit, expected, "JIT (optimize {optimize}): {source}");
    }
}

/// The census repro: a literal above `i64::MAX` typed as an `i128`.
#[test]
fn census_literal_is_an_i128() {
    agree(
        "fn big() -> i128 { 18408377700990114895 }\nfn main() -> i128 { big() }",
        "18408377700990114895",
    );
}
