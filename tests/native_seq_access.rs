//! Option-free element reads (§13, Stage 1 perf lane 2, #932): a `try_get`
//! that only a `match` consumes lowers to one bounds check that branches to
//! the arms, and the `Some` arm loads the element directly. Every form runs
//! in the evaluator, the Cranelift JIT (optimizer on and off) and, where a C
//! compiler links, an executable, and all must agree.
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
    let expected: u64 = evaluated.parse().unwrap();
    let links = cfg!(unix)
        && Command::new("cc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
    if links {
        let dir = std::env::temp_dir().join(format!(
            "vesper-seq-access-test-{}-{}",
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

#[test]
fn match_on_try_get_agrees_across_backends() {
    let source = r#"
fn u(x: u64) -> u64 { x }
fn sum(in s: ChunkSeq[u64]) -> u64 {
    let mut total: u64 = 0;
    let mut i: u64 = 0;
    loop {
        match s.try_get(i) {
            Option::Some(v) => { total = wrapping_add(total, v); }
            Option::None => { break; }
        }
        i = wrapping_add(i, 1);
    }
    total
}
fn none_first(in s: ChunkSeq[u64], i: u64) -> u64 {
    match s.try_get(i) {
        Option::None => 100,
        Option::Some(v) => v,
    }
}
fn present(in s: ChunkSeq[u64], i: u64) -> u64 {
    match s.try_get(i) {
        Option::Some(_) => 1,
        _ => 0,
    }
}
fn wild_only(in s: ChunkSeq[u64], i: u64) -> u64 {
    match s.try_get(i) {
        _ => 3,
    }
}
fn from_slice(in v: Slice[u64], i: u64) -> u64 {
    match v.try_get(i) {
        Option::Some(x) => wrapping_add(x, 1000),
        Option::None => 1,
    }
}
fn bytes(in s: ChunkSeq[u8]) -> u64 {
    let mut total: u64 = 0;
    for i in u(0)..u(10) {
        match s.try_get(i) {
            Option::Some(b) => { if b > 100 { total = wrapping_add(total, 200); } else { total = wrapping_add(total, 7); } }
            Option::None => { total = wrapping_add(total, 1); }
        }
    }
    total
}
fn kept(in s: ChunkSeq[u64], i: u64) -> u64 {
    // The Option is used as a value, so it is built in memory.
    let o = s.try_get(i);
    match o {
        Option::Some(v) => v,
        Option::None => 9,
    }
}
fn main() -> u64 {
    let mut s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    for i in u(1)..u(6) { s.push(wrapping_mul(i, 3)); }
    let mut b: ChunkSeq[u8] = ChunkSeq::new[u8]();
    b.push(200);
    b.push(7);
    let a = sum(in s);
    let c = wrapping_add(none_first(in s, 2), none_first(in s, 9));
    let d = wrapping_add(present(in s, 0), present(in s, 5));
    let e = wild_only(in s, 0);
    let f = wrapping_add(from_slice(in s[1..3], 1), from_slice(in s[1..3], 2));
    let g = bytes(in b);
    let h = wrapping_add(kept(in s, 4), kept(in s, 5));
    wrapping_add(wrapping_add(wrapping_add(a, c), wrapping_add(d, e)), wrapping_add(wrapping_add(f, g), h))
}
"#;
    let sum = 3 + 6 + 9 + 12 + 15;
    assert_eq!(
        differential("try_get", source),
        sum + (9 + 100) + 1 + 3 + (1009 + 1) + (207 + 8) + (15 + 9)
    );
}

#[test]
fn aggregate_elements_load_from_the_matched_slot() {
    let source = r#"
fn u(x: u64) -> u64 { x }
copy struct P { x: u64, y: u64 }
fn dot(in s: ChunkSeq[P]) -> u64 {
    let mut total: u64 = 0;
    for i in u(0)..u(4) {
        match s.try_get(i) {
            Option::Some(p) => { total = wrapping_add(total, wrapping_mul(p.x, p.y)); }
            Option::None => {}
        }
    }
    total
}
fn pairs(in s: ChunkSeq[(u64, u64)]) -> u64 {
    match s.try_get(1) {
        Option::Some(t) => { let (a, b) = t; wrapping_sub(b, a) }
        Option::None => 0,
    }
}
fn main() -> u64 {
    let mut s: ChunkSeq[P] = ChunkSeq::new[P]();
    s.push(P { x: 2, y: 3 });
    s.push(P { x: 4, y: 5 });
    let mut t: ChunkSeq[(u64, u64)] = ChunkSeq::new();
    t.push((1, 2));
    t.push((10, 50));
    wrapping_add(dot(in s), pairs(in t))
}
"#;
    assert_eq!(differential("aggregates", source), 6 + 20 + 40);
}
