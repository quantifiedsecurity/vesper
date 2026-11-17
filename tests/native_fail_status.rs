//! Register-status Fail ABI (Stage 1 perf lane 5, #935). In optimized
//! native builds, a function whose row holds only `Fail` instances returns
//! its failure status as a second `i64` value (0 = success, `k` = the `k`-th
//! `Fail` of its row) and writes the error payload through caller storage
//! only when it fails. Entry and export symbols keep the failure-record ABI.
//!
//! Every case runs on the evaluator, the unoptimized JIT (record ABI) and the
//! optimized JIT (status ABI), which must agree. The callees are recursive so
//! that the inliner keeps the calls. `status_abi_is_used` checks that the
//! optimized object really returns the status in a register.
use vesper::{check, native, parser, runtime::Runtime};

fn world(source: &str) -> check::World {
    let p = parser::parse(source);
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let c = check::check(&p.module);
    assert!(c.diagnostics.is_empty(), "{source}\n{:?}", c.diagnostics);
    c.world
}
fn big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}
/// The evaluator and both JIT builds must produce `expected`.
fn agree(source: impl Into<String>, expected: &'static str) {
    let source = source.into();
    big_stack(move || {
        let w = world(&source);
        let interpreted = Runtime::new(&w).run("main", vec![]).unwrap().to_string();
        assert_eq!(interpreted, expected, "evaluator: {source}");
        let plain = native::jit_with_options(&w, "main", false)
            .unwrap()
            .to_string();
        assert_eq!(plain, expected, "unoptimized JIT: {source}");
        let optimized = native::jit_with_options(&w, "main", true)
            .unwrap()
            .to_string();
        assert_eq!(optimized, expected, "optimized JIT: {source}");
    });
}
/// Return counts of every emitted function of `entry`'s optimized and
/// unoptimized objects, by source name.
fn return_counts(source: &str, optimize: bool) -> Vec<(String, usize)> {
    let w = world(source);
    let mut out = vec![];
    native::object_with_observer(
        &w,
        "main",
        None,
        optimize,
        Some(&mut |f: native::EmittedFunction| {
            out.push((f.source.clone(), f.function.signature.returns.len()));
        }),
    )
    .unwrap();
    out
}

/// `down(n, k)` recurses `n` times and raises `k` at the bottom when `k > 0`.
const DOWN: &str = "fn down(n: i64, k: i64) -> i64 eff [Fail[i64]] { if n == 0 { if k > 0 { Fail::raise(k) } 7 } else { wrapping_add(down(wrapping_sub(n, 1), k), 1) } }";

#[test]
fn status_abi_is_used_only_in_optimized_builds_and_never_at_the_entry() {
    let source = format!(
        "{DOWN} fn main() -> i64 {{ handle Fail[i64] in {{ down(3, 0) }} with {{ return v => v, raise(e, k) => e, }} }}"
    );
    let optimized = return_counts(&source, true);
    assert!(
        optimized.iter().any(|(n, r)| n == "down" && *r == 2),
        "{optimized:?}"
    );
    assert!(
        optimized.iter().any(|(n, r)| n == "main" && *r == 1),
        "{optimized:?}"
    );
    let plain = return_counts(&source, false);
    assert!(plain.iter().all(|(_, r)| *r == 1), "{plain:?}");
}

#[test]
fn one_fail_success_and_failure_through_several_frames() {
    let main = |n: i64, k: i64| {
        format!(
            "{DOWN} fn main() -> i64 {{ handle Fail[i64] in {{ down({n}, {k}) }} with {{ return v => v, raise(e, k) => wrapping_mul(e, 100), }} }}"
        )
    };
    agree(main(0, 0), "7");
    agree(main(5, 0), "12");
    agree(main(0, 3), "300");
    agree(main(6, 4), "400");
}

#[test]
fn several_fails_in_one_row_pick_the_right_instance() {
    // Row order: Fail[ArithOverflow] is status 1, Fail[RuntimeError] is
    // status 2. Primitive failures select the instance by type.
    let two = "fn two(n: i64, x: i64, d: i64) -> i64 eff [Fail[ArithOverflow], Fail[RuntimeError]] { if n == 0 { (x * 1000) / d } else { wrapping_add(two(wrapping_sub(n, 1), x, d), 1) } }";
    let main = |x: &str, d: i64| {
        format!(
            "{two} fn main() -> i64 {{ handle Fail[RuntimeError] in {{ handle Fail[ArithOverflow] in {{ two(3, {x}, {d}) }} with {{ return v => v, raise(e, k) => 41, }} }} with {{ return v => v, raise(e, k) => 900, }} }}"
        )
    };
    agree(main("5", 10), "503");
    agree(main("9223372036854775807", 10), "41");
    agree(main("5", 0), "900");
}

#[test]
fn payloads_of_every_size() {
    // A three-word struct, a two-word enum and a one-byte payload.
    let big = "struct Big { a: i64, b: i64, c: i64 } \
               fn big(n: i64) -> i64 eff [Fail[Big]] { if n == 0 { Fail::raise(Big { a: 1, b: 20, c: 300 }) } wrapping_add(big(wrapping_sub(n, 1)), 1) } \
               fn main() -> i64 { handle Fail[Big] in { big(4) } with { return v => v, raise(e, k) => wrapping_add(wrapping_add(e.a, e.b), e.c), } }";
    agree(big, "321");
    let tagged = "enum Why { Code(i64), Empty } \
                  fn why(n: i64, code: i64) -> i64 eff [Fail[Why]] { if n == 0 { if code == 0 { Fail::raise(Why::Empty) } Fail::raise(Why::Code(code)) } wrapping_add(why(wrapping_sub(n, 1), code), 1) } \
                  fn main() -> i64 { let a = handle Fail[Why] in { why(2, 0) } with { return v => v, raise(e, k) => match e { Why::Code(c) => c, Why::Empty => 77 }, }; \
                  let b = handle Fail[Why] in { why(3, 12) } with { return v => v, raise(e, k) => match e { Why::Code(c) => c, Why::Empty => 77 }, }; wrapping_add(wrapping_mul(a, 100), b) }";
    agree(tagged, "7712");
    let byte = "fn small(n: i64) -> i64 eff [Fail[u8]] { if n == 0 { let e: u8 = 200; Fail::raise(e) } wrapping_add(small(wrapping_sub(n, 1)), 1) } \
                fn main() -> i64 { handle Fail[u8] in { small(2) } with { return v => v, raise(e, k) => if e == 200 { 200 } else { 0 }, } }";
    agree(byte, "200");
}

#[test]
fn aggregate_and_float_results_with_a_status() {
    let pair = "struct Pair { x: i64, y: i64 } \
                fn pair(n: i64, fail: bool) -> Pair eff [Fail[i64]] { if n == 0 { if fail { Fail::raise(9) } Pair { x: 1, y: 2 } } else { let p = pair(wrapping_sub(n, 1), fail); Pair { x: wrapping_add(p.x, 1), y: p.y } } } \
                fn main() -> i64 { let ok = handle Fail[i64] in { let p = pair(3, false); wrapping_add(wrapping_mul(p.x, 10), p.y) } with { return v => v, raise(e, k) => e, }; \
                let bad = handle Fail[i64] in { let p = pair(3, true); p.x } with { return v => v, raise(e, k) => e, }; wrapping_add(wrapping_mul(ok, 100), bad) }";
    agree(pair, "4209");
    let float = "fn half(n: i64) -> f64 eff [Fail[i64]] { if n < 0 { Fail::raise(n) } if n == 0 { 1.0 } else { half(wrapping_sub(n, 1)) * 0.5 } } \
                 fn main() -> i64 { let a = handle Fail[i64] in { if half(3) == 0.125 { 1 } else { 0 } } with { return v => v, raise(e, k) => e, }; \
                 let b = handle Fail[i64] in { if half(-4) == 0.0 { 1 } else { 0 } } with { return v => v, raise(e, k) => e, }; wrapping_add(wrapping_mul(a, 10), b) }";
    agree(float, "6");
}

#[test]
fn nested_handlers_and_fail_inside_loops() {
    // The inner handler catches one failure per iteration; the outer one
    // catches the failure that escapes the loop.
    let source = format!(
        "{DOWN} fn main() -> i64 {{ handle Fail[i64] in {{ let mut total = 0; for i in 0..20 {{ \
         let r = handle Fail[i64] in {{ down(i, if i % 3 == 0 {{ i }} else {{ 0 }}) }} with {{ return v => v, raise(e, k) => wrapping_mul(e, 1000), }}; \
         total = wrapping_add(total, r); if i == 15 {{ let _ = down(2, 55); }} }} total }} with {{ return v => v, raise(e, k) => e, }} }}"
    );
    agree(source, "55");
    // The same loop without the escaping failure sums every iteration.
    let source = format!(
        "{DOWN} fn main() -> i64 {{ let mut total = 0; for i in 0..20 {{ \
         let r = handle Fail[i64] in {{ down(i, if i % 3 == 0 {{ i }} else {{ 0 }}) }} with {{ return v => v, raise(e, k) => wrapping_mul(e, 1000), }}; \
         total = wrapping_add(total, r); }} total }}"
    );
    agree(source, "63225");
}

#[test]
fn a_status_call_inside_a_status_callee_reraises_the_inner_failure() {
    // `outer` (status ABI) calls `down` (status ABI) and adds its own Fail.
    let source = format!(
        "{DOWN} fn outer(n: i64, k: i64) -> i64 eff [Fail[i64]] {{ if n == 0 {{ down(4, k) }} else {{ wrapping_add(outer(wrapping_sub(n, 1), k), 100) }} }} \
         fn main() -> i64 {{ let a = handle Fail[i64] in {{ outer(2, 0) }} with {{ return v => v, raise(e, k) => e, }}; \
         let b = handle Fail[i64] in {{ outer(2, 6) }} with {{ return v => v, raise(e, k) => e, }}; wrapping_add(wrapping_mul(a, 10), b) }}"
    );
    agree(source, "2116");
}

#[test]
fn keep_writes_across_status_frames() {
    // The callee's last write before the failure reaches the caller's place.
    let source = "fn tick(inout x: i64, n: i64) eff [Fail[i64]] { x = wrapping_add(x, 1); if n == 0 { Fail::raise(x) } tick(inout x, wrapping_sub(n, 1)); x = wrapping_add(x, 1000); } \
                  fn main() -> i64 { let mut x = 0; let r = handle Fail[i64] in { tick(inout x, 3); 0 } with { return v => v, raise(e, k) => e, }; wrapping_add(wrapping_mul(x, 100), r) }";
    agree(source, "404");
}

#[test]
fn primitive_failures_use_the_status_abi_too() {
    // Checked arithmetic and indexing raise Fail[ArithOverflow] and
    // Fail[RuntimeError] from a recursive callee.
    let source = "fn grow(n: i64, x: i64) -> i64 eff [Fail[ArithOverflow]] { if n == 0 { x } else { grow(n - 1, x * 10) } } \
                  fn main() -> i64 { let a = handle Fail[ArithOverflow] in { grow(3, 7) } with { return v => v, raise(e, k) => -1, }; \
                  let b = handle Fail[ArithOverflow] in { grow(30, 7) } with { return v => v, raise(e, k) => -1, }; wrapping_add(a, b) }";
    agree(source, "6999");
    let source = "fn pick(n: i64, d: i64) -> i64 eff [Fail[RuntimeError]] { if n == 0 { 100 / d } else { pick(wrapping_sub(n, 1), d) } } \
                  fn main() -> i64 { let a = handle Fail[RuntimeError] in { pick(3, 4) } with { return v => v, raise(e, k) => 99, }; \
                  let b = handle Fail[RuntimeError] in { pick(3, 0) } with { return v => v, raise(e, k) => match e { RuntimeError::DivisionByZero => 42, _ => 0 }, }; wrapping_add(wrapping_mul(a, 100), b) }";
    agree(source, "2542");
}

/// Exported symbols keep the failure-record ABI: a C caller passes a zeroed
/// record and reads its status word. Inside the library, `outer` calls the
/// status-ABI `down` and the record-ABI export `inner`.
#[cfg(unix)]
#[test]
fn exports_keep_the_failure_record_at_the_boundary() {
    use vesper::native::export::{LibraryOptions, compile_source};
    let source = format!(
        "{DOWN}\npub fn inner(n: i64, k: i64) -> i64 eff [Fail[i64]] {{ down(n, k) }}\n\
         pub fn outer(n: i64, k: i64) -> i64 eff [Fail[i64]] {{ if n > 100 {{ Fail::raise(-1) }} wrapping_add(down(n, k), inner(n, 0)) }}\n"
    );
    let library = compile_source(
        "fail.ves",
        &source,
        &LibraryOptions {
            library: "failabi",
            target: None,
            optimize: true,
        },
    )
    .unwrap();
    let symbol = |name: &str| {
        library
            .interface
            .functions
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.symbol.clone())
            .unwrap()
    };
    let dir = std::env::temp_dir().join(format!("vesper-fail-abi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("lib.o"), &library.object).unwrap();
    let (inner, outer) = (symbol("inner"), symbol("outer"));
    std::fs::write(
        dir.join("main.c"),
        format!(
            "#include <stdio.h>\n#include <stdint.h>\n\
             extern int64_t {inner}(int64_t, int64_t, int64_t *);\n\
             extern int64_t {outer}(int64_t, int64_t, int64_t *);\n\
             static void run(const char *name, int64_t (*f)(int64_t, int64_t, int64_t *), int64_t n, int64_t k) {{\n\
               int64_t record[2] = {{0, 0}};\n\
               int64_t r = f(n, k, record);\n\
               if (record[0] == 0) printf(\"%s ok %lld\\n\", name, (long long)r);\n\
               else printf(\"%s fail %lld %lld\\n\", name, (long long)record[0], (long long)record[1]);\n\
             }}\n\
             int main(void) {{ run(\"inner\", {inner}, 4, 0); run(\"inner\", {inner}, 4, 9); \
             run(\"outer\", {outer}, 3, 0); run(\"outer\", {outer}, 3, 5); run(\"outer\", {outer}, 200, 0); return 0; }}\n"
        ),
    )
    .unwrap();
    let built = std::process::Command::new("cc")
        .arg(dir.join("main.c"))
        .arg(dir.join("lib.o"))
        .arg("-o")
        .arg(dir.join("main"))
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let run = std::process::Command::new(dir.join("main")).output().unwrap();
    assert!(run.status.success());
    assert_eq!(
        String::from_utf8_lossy(&run.stdout),
        "inner ok 11\ninner fail 1 9\nouter ok 20\nouter fail 1 5\nouter fail 1 -1\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Register results: a function with an empty row that returns a copy
/// aggregate of at most two words returns its words in registers in
/// optimized builds.
#[test]
fn small_aggregate_results_return_in_registers() {
    let source = "struct Pair { x: i64, y: i64 } \
                  fn find(n: i64, k: i64) -> Option[i64] { if n == 0 { if k > 0 { Option::Some(k) } else { Option::None } } else { find(wrapping_sub(n, 1), k) } } \
                  fn swap(n: i64, p: Pair) -> Pair { if n == 0 { Pair { x: p.y, y: p.x } } else { swap(wrapping_sub(n, 1), Pair { x: p.y, y: p.x }) } } \
                  fn main() -> i64 { let a = match find(3, 9) { Option::Some(v) => v, Option::None => 0 }; \
                  let b = match find(3, 0) { Option::Some(v) => v, Option::None => 5 }; \
                  let p = swap(3, Pair { x: 1, y: 2 }); wrapping_add(wrapping_add(wrapping_mul(a, 100), wrapping_mul(b, 10)), wrapping_add(wrapping_mul(p.x, 1000), p.y)) }";
    agree(source, "1952");
    let optimized = return_counts(source, true);
    assert!(
        optimized.iter().any(|(n, r)| n == "find" && *r == 2)
            && optimized.iter().any(|(n, r)| n == "swap" && *r == 2),
        "{optimized:?}"
    );
}
