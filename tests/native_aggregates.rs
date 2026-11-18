//! Aggregates built where they end up (port gaps lane 7, #990).
//!
//! On the native optimize path a variant, struct or tuple whose one use is
//! the function's result, a field of an enclosing constructor or an exit is
//! built in place, and a call whose result is consumed that way writes its
//! result there. Every program runs in the evaluator, the JIT with the
//! optimizer on and off and, where a C compiler links, an executable; all
//! must agree. Recursive helpers stay calls, so the result-buffer paths run
//! in the JIT as well as in the inlined forms.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use vesper::{check, native, parser, runtime::Runtime};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn accepted(source: &str) -> check::Checked {
    let p = parser::parse(source);
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let c = check::check(&p.module);
    assert!(c.diagnostics.is_empty(), "{:#?}", c.diagnostics);
    c
}

fn links_executables() -> bool {
    cfg!(unix)
        && Command::new("cc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

/// Runs `main` everywhere and returns the agreed result.
fn differential(name: &str, source: &str) -> u64 {
    let c = accepted(source);
    let evaluated = Runtime::new(&c.world)
        .run("main", vec![])
        .unwrap_or_else(|e| panic!("{name}: evaluator failed: {e}"))
        .to_string();
    for optimize in [true, false] {
        let jit = native::jit_with_options(&c.world, "main", optimize)
            .unwrap_or_else(|e| panic!("{name}: JIT (optimize {optimize}) failed: {e}"))
            .to_string();
        assert_eq!(jit, evaluated, "{name}: JIT (optimize {optimize})");
    }
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        let (obj, _) = native::object_with_options(&c.world, "main", Some(target), true)
            .unwrap_or_else(|e| panic!("{name}: {target}: {e}"));
        assert_eq!(&obj[..4], b"\x7fELF");
    }
    let expected: u64 = evaluated.parse().unwrap();
    if links_executables() {
        let dir = std::env::temp_dir().join(format!(
            "vesper-aggregates-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{name}.ves"));
        fs::write(&file, source).unwrap();
        let exe: PathBuf = dir.join(name);
        let built = Command::new(env!("CARGO_BIN_EXE_vesper"))
            .arg("build")
            .arg(&file)
            .arg("-o")
            .arg(&exe)
            .output()
            .unwrap();
        assert!(
            built.status.success(),
            "{name}: build failed: {}",
            String::from_utf8_lossy(&built.stderr)
        );
        let run = Command::new(&exe).output().unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(run.status.code(), Some((expected & 0xff) as i32), "{name}");
    }
    expected
}

const CHECK: &str = r#"
copy struct Rec {
    a: u64,
    b: u64,
    c: u32,
    d: bool
}

copy enum Check {
    Ok(Rec),
    BadA(u64),
    BadB(u64),
    BadC(u32)
}

fn check(a: u64, b: u64, c: u32, depth: u64) -> Check {
    if depth > 0 {
        return check(a, b, c, wrapping_sub(depth, 1));
    }
    if a == 0 {
        return Check::BadA(a);
    }
    if b == 0 {
        return Check::BadB(b);
    }
    if c == 0 {
        return Check::BadC(c);
    }
    Check::Ok(Rec {
        a: a,
        b: b,
        c: c,
        d: true
    })
}

fn score(x: Check) -> u64 {
    match x {
        Check::Ok(r) => {
            let d: u64 = if r.d { 1000 } else { 0 };
            wrapping_add(wrapping_add(wrapping_mul(r.a, 3), wrapping_mul(r.b, 5)), wrapping_add(widen_u64(r.c), d))
        }
        Check::BadA(v) => wrapping_add(v, 11),
        Check::BadB(v) => wrapping_add(v, 13),
        Check::BadC(v) => wrapping_add(widen_u64(v), 17),
    }
}
"#;

#[test]
fn variants_built_in_the_result_agree() {
    let source = format!(
        "{CHECK}
fn main() -> u64 {{
    let mut total: u64 = 0;
    let depth_end: u64 = 3;
    for depth in 0..depth_end {{
        total = wrapping_add(total, score(check(0, 2, 3, depth)));
        total = wrapping_add(total, score(check(1, 0, 3, depth)));
        total = wrapping_add(total, score(check(1, 2, 0, depth)));
        total = wrapping_add(total, score(check(7, 9, 4, depth)));
    }}
    total
}}
"
    );
    assert_eq!(
        differential("variants", &source),
        3 * (11 + 13 + 17 + (21 + 45 + 4 + 1000))
    );
}

#[test]
fn nested_constant_aggregates_agree() {
    let source = r#"
copy struct Inner {
    a: u64,
    flag: bool,
    when: Option[u64],
    b: u64
}

copy struct Outer {
    inner: Inner,
    seen: bool,
    x: u64,
    y: u64,
    z: u64
}

fn outer_new() -> Outer {
    Outer {
        inner: Inner {
            a: 0,
            flag: false,
            when: Option::None,
            b: 0
        },
        seen: false,
        x: 0,
        y: 0,
        z: 0
    }
}

fn outer_some(n: u64, depth: u64) -> Outer {
    if depth > 0 {
        return outer_some(wrapping_add(n, 1), wrapping_sub(depth, 1));
    }
    Outer {
        inner: Inner {
            a: n,
            flag: true,
            when: Option::Some(wrapping_mul(n, 2)),
            b: wrapping_add(n, 5)
        },
        seen: true,
        x: 1,
        y: 2,
        z: 3
    }
}

fn sum(o: Outer) -> u64 {
    let w: u64 = match o.inner.when {
        Option::Some(v) => v,
        Option::None => 100,
    };
    let f: u64 = if o.inner.flag { 10 } else { 0 };
    let s: u64 = if o.seen { 20 } else { 0 };
    wrapping_add(wrapping_add(wrapping_add(o.inner.a, o.inner.b), wrapping_add(w, f)), wrapping_add(s, wrapping_add(o.x, wrapping_add(o.y, o.z))))
}

fn main() -> u64 {
    let a = sum(outer_new());
    let b = sum(outer_some(4, 3));
    wrapping_add(wrapping_mul(a, 1000), b)
}
"#;
    // a = 100; b with n = 7: 7 + 12 + 14 + 10 + 20 + 6.
    assert_eq!(differential("constants", source), 100 * 1000 + 69);
}

#[test]
fn call_results_built_in_fields_and_tuples_agree() {
    let source = r#"
copy struct Pair {
    lo: u64,
    hi: u64
}

copy struct Wide {
    first: Pair,
    second: Pair,
    tag: u64
}

copy enum Shape {
    Empty,
    Two(Pair, Pair),
    One(Wide)
}

fn pair(a: u64, b: u64, depth: u64) -> Pair {
    if depth > 0 {
        return pair(b, a, wrapping_sub(depth, 1));
    }
    Pair { lo: a, hi: b }
}

fn wide(a: u64, depth: u64) -> Wide {
    Wide {
        first: pair(a, wrapping_add(a, 1), depth),
        second: pair(wrapping_mul(a, 10), wrapping_mul(a, 20), depth),
        tag: 9
    }
}

fn shape(k: u64) -> Shape {
    if k == 0 {
        return Shape::Empty;
    }
    if k == 1 {
        return Shape::Two(pair(1, 2, 1), pair(3, 4, 0));
    }
    Shape::One(wide(k, k))
}

fn swap(in p: Pair) -> Pair {
    Pair { lo: p.hi, hi: p.lo }
}

fn tuple(a: u64) -> (Pair, u64, Pair) {
    let w = pair(a, 2, 0);
    (pair(a, 1, 0), wrapping_add(a, 1), swap(in w))
}

fn value(s: Shape) -> u64 {
    match s {
        Shape::Empty => 1,
        Shape::Two(a, b) => wrapping_add(wrapping_add(wrapping_mul(a.lo, 1000), wrapping_mul(a.hi, 100)), wrapping_add(wrapping_mul(b.lo, 10), b.hi)),
        Shape::One(w) => wrapping_add(wrapping_add(wrapping_add(w.first.lo, w.first.hi), wrapping_add(w.second.lo, w.second.hi)), w.tag),
    }
}

fn main() -> u64 {
    let p = pair(5, 6, 0);
    let q = swap(in p);
    let p = swap(in p);
    let (x, y, z) = tuple(7);
    let t = wrapping_add(wrapping_add(wrapping_add(x.lo, x.hi), y), wrapping_add(z.lo, wrapping_mul(z.hi, 3)));
    let mut total = wrapping_add(t, wrapping_add(wrapping_mul(q.lo, 2), wrapping_mul(p.hi, 4)));
    let k_end: u64 = 4;
    for k in 0..k_end {
        total = wrapping_add(total, value(shape(k)));
    }
    total
}
"#;
    // pair(1, 2, 1) swaps to (2, 1): 2000 + 100 + 30 + 4.
    // k = 2: 2 + 3 + 40 + 20 + 9 = 74 (depth 2 swaps twice);
    // k = 3: odd depth swaps: 4 + 3 + 60 + 30 + 9 = 106.
    // t = 7 + 1 + 8 + 2 + 21 = 39; q.lo = 6, p.hi = 5.
    assert_eq!(
        differential("calls", source),
        39 + 12 + 20 + 1 + 2134 + 74 + 106
    );
}

#[test]
fn early_exits_and_loops_build_their_own_values() {
    let source = r#"
copy struct Hit {
    at: u64,
    key: u64
}

copy enum Found {
    Miss(u64),
    At(Hit)
}

fn find(in s: ChunkSeq[u64], key: u64) -> Found {
    let mut i: u64 = 0;
    loop {
        match s.try_get(i) {
            Option::Some(v) => {
                if v == key {
                    return Found::At(Hit { at: i, key: v });
                }
            }
            Option::None => {
                return Found::Miss(i);
            }
        }
        i = wrapping_add(i, 1);
    }
}

fn rank(f: Found) -> u64 {
    match f {
        Found::Miss(n) => wrapping_add(1000, n),
        Found::At(h) => wrapping_add(wrapping_mul(h.at, 10), h.key),
    }
}

fn main() -> u64 {
    let mut s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    let i_end: u64 = 6;
    for i in 0..i_end {
        s.push(wrapping_mul(i, 7));
    }
    let a = rank(find(in s, 21));
    let b = rank(find(in s, 22));
    let c = rank(find(in s, 0));
    wrapping_add(wrapping_add(a, b), c)
}
"#;
    // 21 is at 3: 51; 22 misses after 6: 1006; 0 is at 0: 0.
    assert_eq!(differential("exits", source), 51 + 1006);
}
