//! Disjoint field paths in `E0301` and the place primitives `swap` and
//! `replace` (§13, port gaps lane 3, #986).
//!
//! A builtin receiver and its source slice are disjoint when their field
//! paths are, with the rule call arguments use: `q.a.extend(in q.b[..])` is
//! accepted, `q.a.extend(in q.a[..])` is not. `swap(inout a, inout b)` and
//! `replace(inout a, v)` exchange values of any type through `inout`
//! places, so a non-copy field of an `inout` parameter is published without
//! a copy. The evaluator, both JIT configurations and an executable agree,
//! and the JIT frees every sequence that is swapped or replaced.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use vesper::{check, native, parser, runtime::Runtime, seq::native_runtime};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "vesper-borrow-paths-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn checked(source: &str) -> check::Checked {
    let p = parser::parse(source);
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    check::check(&p.module)
}

fn accepted(source: &str) -> check::Checked {
    let c = checked(source);
    assert!(c.diagnostics.is_empty(), "{:#?}", c.diagnostics);
    c
}

fn rejected(source: &str, code: &str, needle: &str) {
    let c = checked(source);
    assert!(
        c.diagnostics
            .iter()
            .any(|d| d.code == code && d.message.contains(needle)),
        "expected {code} ({needle}): {:#?}",
        c.diagnostics
    );
}

fn links() -> bool {
    cfg!(unix)
        && Command::new("cc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

/// Runs `main` in the evaluator, the JIT (optimizer on and off) and, where
/// a C compiler links, an executable; all must agree. The JIT must free
/// every sequence itself and never free one that is not live.
fn differential(name: &str, source: &str) -> u64 {
    differential_with(name, source, true)
}

/// [`differential`]; `teardown` also requires the JIT to free every
/// sequence itself, which a struct local that holds sequences does not yet
/// do (the drop gap of port gaps lane 2, #985).
fn differential_with(name: &str, source: &str, teardown: bool) -> u64 {
    let c = accepted(source);
    let evaluated = Runtime::new(&c.world)
        .run("main", vec![])
        .unwrap_or_else(|e| panic!("{name}: evaluator failed: {e}"))
        .to_string();
    for optimize in [true, false] {
        let stray = native_runtime::stray_frees();
        let jit = native::jit_with_options(&c.world, "main", optimize)
            .unwrap_or_else(|e| panic!("{name}: JIT (optimize {optimize}) failed: {e}"))
            .to_string();
        assert_eq!(
            jit, evaluated,
            "{name}: JIT (optimize {optimize}) vs evaluator"
        );
        assert!(
            !teardown || native_runtime::teardown_frees() == 0,
            "{name}: JIT (optimize {optimize}) left {} sequences for teardown",
            native_runtime::teardown_frees()
        );
        assert_eq!(
            native_runtime::stray_frees(),
            stray,
            "{name}: JIT (optimize {optimize}) freed a sequence that was not live"
        );
    }
    let expected: u64 = evaluated.parse().unwrap();
    if links() {
        let temp = Temp::new();
        let file = temp.0.join(format!("{name}.ves"));
        fs::write(&file, source).unwrap();
        let exe = temp.0.join(name);
        let built = Command::new(env!("CARGO_BIN_EXE_vesper"))
            .arg("build")
            .arg(&file)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{name}: vesper build failed\n{}{}",
            String::from_utf8_lossy(&built.stdout),
            String::from_utf8_lossy(&built.stderr)
        );
        let run = Command::new(&exe).output().unwrap();
        assert_eq!(
            run.status.code(),
            Some((expected & 0xff) as i32),
            "{name}: executable stderr={}",
            String::from_utf8_lossy(&run.stderr)
        );
    }
    expected
}

const DIGEST: &str = "fn digest(in s: Slice[u64]) -> u64 {
    let mut acc: u64 = 7;
    for i in 0..s.len() {
        let x = match s.try_get(i) {
            Option::Some(x) => x,
            Option::None => 0,
        };
        acc = wrapping_add(wrapping_mul(acc, 131), x);
    }
    acc
}
fn mix(acc: u64, v: u64) -> u64 {
    wrapping_add(wrapping_mul(acc, 1000003), v)
}
";

fn model_digest(s: &[u64]) -> u64 {
    s.iter()
        .fold(7u64, |acc, x| acc.wrapping_mul(131).wrapping_add(*x))
}

fn mix(acc: u64, v: u64) -> u64 {
    acc.wrapping_mul(1_000_003).wrapping_add(v)
}

#[test]
fn distinct_fields_are_disjoint_for_builtin_sources() {
    let source = format!(
        "{DIGEST}struct Q {{ a: ChunkSeq[u64], b: ChunkSeq[u64] }}
struct Outer {{ q: Q, c: ChunkSeq[u64] }}
fn grow(inout q: Q) -> u64 eff [Fail[RuntimeError]] {{
    q.a.extend(in q.b[0..2]);
    q.a.copy_from(0, in q.b[1..2]);
    let ok = q.b.try_copy_from(1, in q.a.as_slice());
    mix(digest(in q.a.as_slice()), mix(if ok {{ 1 }} else {{ 0 }}, digest(in q.b.as_slice())))
}}
fn nested(inout o: Outer) -> u64 {{
    o.q.a.extend(in o.c.as_slice());
    o.c.extend(in o.q.b[0..1]);
    mix(digest(in o.q.a.as_slice()), digest(in o.c.as_slice()))
}}
fn main() -> u64 {{
    let mut a: ChunkSeq[u64] = ChunkSeq::new[u64]();
    a.push(1);
    let mut b: ChunkSeq[u64] = ChunkSeq::new[u64]();
    b.push(20);
    b.push(30);
    b.push(40);
    let mut q = Q {{ a: a, b: b }};
    let r = handle Fail[RuntimeError] in {{
        grow(inout q)
    }} with {{
        return x => x,
        raise(e, k) => 7777,
    }};
    let mut c: ChunkSeq[u64] = ChunkSeq::new[u64]();
    c.push(5);
    let mut o = Outer {{ q: q, c: c }};
    let n = nested(inout o);
    let Outer {{ q: Q {{ a: a, b: b }}, c: c }} = o;
    mix(r, n)
}}
"
    );
    // a = [1, 20, 30] then a[0] = 30; b = [20, 30, 40]: b[1..4] = a fails.
    let mut a = vec![1u64, 20, 30];
    a[0] = 30;
    let b = vec![20u64, 30, 40];
    let r = mix(model_digest(&a), mix(0, model_digest(&b)));
    let mut oa = a.clone();
    oa.push(5);
    let oc = vec![5u64, 20];
    let n = mix(model_digest(&oa), model_digest(&oc));
    let c = accepted(&source);
    let got = Runtime::new(&c.world)
        .run("main", vec![])
        .unwrap()
        .to_string();
    assert_eq!(got, mix(r, n).to_string());
    for optimize in [true, false] {
        let jit = native::jit_with_options(&c.world, "main", optimize)
            .unwrap()
            .to_string();
        assert_eq!(jit, got, "JIT (optimize {optimize})");
    }
}

#[test]
fn overlapping_paths_stay_refused() {
    let prefix =
        "struct Q { a: ChunkSeq[u64], b: ChunkSeq[u64] }\nstruct T { s: String, t: String }\n";
    for (body, needle) in [
        (
            "fn f(inout q: Q) { q.a.extend(in q.a[0..1]); }",
            "extend cannot read from the sequence it writes",
        ),
        (
            "fn f(inout q: Q) { q.a.extend(in q.a.as_slice()); }",
            "extend cannot read from the sequence it writes",
        ),
        (
            "fn f(inout s: ChunkSeq[u64]) -> bool { s.try_copy_from(0, in s[1..2]) }",
            "try_copy_from cannot read from the sequence it writes",
        ),
        (
            "fn f(inout t: T) { t.s.push_str(in t.s.as_str()); }",
            "push_str cannot read from the string it grows",
        ),
    ] {
        rejected(
            &format!("{prefix}{body}\nfn main() -> u64 {{ 0 }}"),
            "E0301",
            needle,
        );
    }
    // Different string fields are disjoint.
    accepted(&format!(
        "{prefix}fn f(inout t: T) {{ t.s.push_str(in t.t.as_str()); }}\nfn main() -> u64 {{ 0 }}"
    ));
}

/// The Aura recorder shape: publish the staged state by swapping two
/// non-copy fields of an `inout` parameter, then reset the stage with
/// `replace`, which hands back the old value.
#[test]
fn swap_and_replace_publish_without_copies() {
    let source = format!(
        "{DIGEST}struct State {{ items: ChunkSeq[u64], tag: u64 }}
struct Rec {{ st: State, stage: State, commits: u64 }}
fn total(in s: State) -> u64 {{
    mix(s.tag, digest(in s.items.as_slice()))
}}
fn publish(inout r: Rec) {{
    swap(inout r.st, inout r.stage);
    r.commits = wrapping_add(r.commits, 1);
}}
fn drain(inout r: Rec) -> u64 {{
    let old = replace(inout r.stage.items, ChunkSeq::new[u64]());
    let n = old.len();
    let tag = replace(inout r.stage.tag, 0);
    mix(n, tag)
}}
fn main() -> u64 {{
    let mut a: ChunkSeq[u64] = ChunkSeq::new[u64]();
    a.push(1);
    a.push(2);
    let mut b: ChunkSeq[u64] = ChunkSeq::new[u64]();
    b.push(9);
    let mut r = Rec {{ st: State {{ items: a, tag: 3 }}, stage: State {{ items: b, tag: 4 }}, commits: 0 }};
    publish(inout r);
    let d = drain(inout r);
    r.stage.items.push(77);
    publish(inout r);
    let mut x: u64 = 5;
    let mut y: u64 = 6;
    swap(inout x, inout y);
    let k = replace(inout x, 40);
    let Rec {{ st: State {{ items: s1, tag: t1 }}, stage: State {{ items: s2, tag: t2 }}, commits: c }} = r;
    let v = mix(mix(digest(in s1.as_slice()), t1), mix(digest(in s2.as_slice()), t2));
    mix(mix(v, mix(d, c)), mix(mix(x, y), k))
}}
"
    );
    // After publish: st = [9]/4, stage = [1,2]/3; drain: old len 2, tag 3,
    // stage = []/0; push 77; publish: st = [77]/0, stage = [9]/4.
    let v = mix(mix(model_digest(&[77]), 0), mix(model_digest(&[9]), 4));
    let expected = mix(mix(v, mix(mix(2, 3), 2)), mix(mix(40, 5), 6));
    // `r` is a struct local holding sequences, which the JIT does not free
    // yet (#985); `swap_and_replace_add_no_leaks` checks the primitives.
    assert_eq!(differential_with("publish", &source, false), expected);
}

/// The JIT's teardown count of `source`.
fn teardown(source: &str) -> usize {
    let c = accepted(source);
    native::jit_with_options(&c.world, "main", true).unwrap();
    native_runtime::teardown_frees()
}

/// Swapping and replacing sequence fields of a struct local leaves exactly
/// what the same program without them leaves: the primitives neither leak
/// nor free a sequence twice.
#[test]
fn swap_and_replace_add_no_leaks() {
    let prefix = "struct State { items: ChunkSeq[u64], tag: u64 }
struct Rec { st: State, stage: State }
fn mk(n: u64) -> ChunkSeq[u64] {
    let mut s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    s.push(n);
    s
}
fn publish(inout r: Rec) { swap(inout r.st, inout r.stage); }
fn drain(inout r: Rec) -> u64 {
    let old = replace(inout r.stage.items, mk(5));
    old.len()
}
";
    let program = |body: &str| {
        format!(
            "{prefix}fn main() -> u64 {{
    let mut r = Rec {{ st: State {{ items: mk(1), tag: 3 }}, stage: State {{ items: mk(2), tag: 4 }} }};
    {body}
    let Rec {{ st: State {{ items: s1, tag: t1 }}, stage: State {{ items: s2, tag: t2 }} }} = r;
    wrapping_add(s1.len(), t1)
}}"
        )
    };
    let stray = native_runtime::stray_frees();
    let baseline = teardown(&program(""));
    assert_eq!(teardown(&program("publish(inout r);")), baseline);
    assert_eq!(teardown(&program("let n = drain(inout r);")), baseline);
    assert_eq!(
        teardown(&program(
            "publish(inout r);\n    let n = drain(inout r);\n    publish(inout r);"
        )),
        baseline
    );
    assert_eq!(
        native_runtime::stray_frees(),
        stray,
        "a sequence was freed twice"
    );
}

/// Sequences that are swapped or replaced are freed exactly once: the
/// replaced value is owned by the caller and freed with its binding.
#[test]
fn swapped_and_replaced_sequences_are_freed() {
    let source = format!(
        "{DIGEST}fn fresh(n: u64) -> ChunkSeq[u64] {{
    let mut s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    for i in 0..n {{
        s.push(wrapping_add(i, n));
    }}
    s
}}
fn rotate(inout a: ChunkSeq[u64], inout b: ChunkSeq[u64]) -> u64 {{
    swap(inout a, inout b);
    let old = replace(inout a, fresh(3));
    old.len()
}}
fn main() -> u64 {{
    let mut acc: u64 = 0;
    let rounds: u64 = 20;
    for round in 0..rounds {{
        let mut a = fresh(4);
        let mut b = fresh(round);
        swap(inout a, inout b);
        acc = mix(acc, mix(digest(in a.as_slice()), digest(in b.as_slice())));
        let old = replace(inout a, fresh(2));
        acc = mix(acc, mix(digest(in old.as_slice()), digest(in a.as_slice())));
        acc = mix(acc, rotate(inout a, inout b));
        acc = mix(acc, mix(digest(in a.as_slice()), digest(in b.as_slice())));
    }}
    acc
}}
"
    );
    let fresh = |n: u64| (0..n).map(|i| i + n).collect::<Vec<u64>>();
    let mut acc = 0u64;
    for round in 0..20u64 {
        let mut a = fresh(4);
        let mut b = fresh(round);
        std::mem::swap(&mut a, &mut b);
        acc = mix(acc, mix(model_digest(&a), model_digest(&b)));
        let old = std::mem::replace(&mut a, fresh(2));
        acc = mix(acc, mix(model_digest(&old), model_digest(&a)));
        std::mem::swap(&mut a, &mut b);
        let old = std::mem::replace(&mut a, fresh(3));
        acc = mix(acc, old.len() as u64);
        acc = mix(acc, mix(model_digest(&a), model_digest(&b)));
    }
    assert_eq!(differential("freed", &source), acc);
}

#[test]
fn place_primitive_rules() {
    let prefix = "struct P { a: ChunkSeq[u64], b: ChunkSeq[u64], n: u64 }\n";
    for (body, code, needle) in [
        (
            "fn f(inout p: P) { swap(inout p.a, inout p.a); }",
            "E0301",
            "overlapping arguments",
        ),
        (
            "fn f(inout p: P, inout q: ChunkSeq[u64]) { swap(inout p, inout p.a); }",
            "E0301",
            "overlapping",
        ),
        (
            "fn f(inout p: P) { swap(p.a, inout p.b); }",
            "E0308",
            "swap borrows its places by 'inout'",
        ),
        (
            "fn f(inout p: P) { swap(inout p.a, inout p.n); }",
            "E0200",
            "expected ChunkSeq[u64], found u64",
        ),
        (
            "fn f(in p: P, inout q: ChunkSeq[u64]) { swap(inout p.a, inout q); }",
            "E0",
            "",
        ),
        (
            "fn f(inout p: P) { let x = replace(inout p.a, p.a); }",
            "E0301",
            "conflicts with active argument loan",
        ),
        (
            "fn f(inout p: P) -> u64 { replace(inout p.n, 1, 2) }",
            "E0207",
            "expects 2 arguments",
        ),
        (
            "fn g() -> ChunkSeq[u64] { ChunkSeq::new[u64]() }\nfn f(inout p: P) { let x = replace(inout g(), g()); }",
            "E0300",
            "requires a place",
        ),
    ] {
        let c = checked(&format!("{prefix}{body}\nfn main() -> u64 {{ 0 }}"));
        assert!(
            c.diagnostics
                .iter()
                .any(|d| d.code.starts_with(code) && d.message.contains(needle)),
            "{body}: expected {code} ({needle}): {:#?}",
            c.diagnostics
        );
    }
    // A moved-out field through `inout` is still refused, and now points at
    // the primitives.
    let c = checked(&format!(
        "{prefix}fn f(inout p: P) -> ChunkSeq[u64] {{ p.a }}\nfn main() -> u64 {{ 0 }}"
    ));
    let d = c
        .diagnostics
        .iter()
        .find(|d| d.code == "E0305")
        .unwrap_or_else(|| panic!("{:#?}", c.diagnostics));
    assert!(format!("{d:?}").contains("replace(inout a, v)"), "{d:?}");
    // A declared function named `swap` shadows the primitive (the
    // `@vesper-std/core` shape).
    let source =
        "fn swap(inout a: u64, inout b: u64) { let t = a; a = b; b = wrapping_add(t, 100); }
fn main() -> u64 {
    let mut x: u64 = 1;
    let mut y: u64 = 2;
    swap(inout x, inout y);
    wrapping_add(wrapping_mul(x, 1000), y)
}
";
    assert_eq!(differential("shadowed", source), 2101);
}

/// The places a primitive writes lose their value facts, as `inout`
/// arguments of calls do: `x + 1` after `replace(inout x, max)` may
/// overflow, and swapping function values needs interchangeable types.
#[test]
fn primitives_invalidate_facts_and_need_interchangeable_types() {
    let source = "fn grow() -> u64 eff [Fail[ArithOverflow]] {
    let mut x: u64 = 0;
    let mut old = replace(inout x, 18446744073709551615);
    let mut y: u64 = 7;
    swap(inout y, inout old);
    wrapping_add(x + 1, y)
}
fn main() -> u64 {
    handle Fail[ArithOverflow] in {
        grow()
    } with {
        return v => v,
        raise(e, k) => 4242,
    }
}
";
    assert_eq!(differential("facts", source), 4242);
    // Without the row, the overflow is unhandled: the fact `x == 0` is gone.
    rejected(
        "fn main() -> u64 {
    let mut x: u64 = 0;
    let old = replace(inout x, 18446744073709551615);
    x + 1
}
",
        "E0401",
        "unhandled effect",
    );
    rejected(
        "fn boom() -> u64 eff [Fail[RuntimeError]] {
    let s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    s.get(3)
}
fn calm() -> u64 { 1 }
fn main() -> u64 {
    let mut a = boom;
    let mut b = calm;
    swap(inout a, inout b);
    b()
}
",
        "E0403",
        "must be interchangeable",
    );
}
