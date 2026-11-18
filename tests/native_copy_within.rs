//! `copy_within` and `try_copy_within` (§13, port gaps lane 3, #986): an
//! in-place `memmove` within one sequence or slice, with one bounds check.
//! The evaluator, both JIT configurations and an executable agree on every
//! shift, overlapping in either direction, and on the faults of intervals
//! that do not fit.
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
            "vesper-copy-within-{}-{}",
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
        assert_eq!(
            native_runtime::teardown_frees(),
            0,
            "{name}: JIT (optimize {optimize}) left sequences for teardown"
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

/// `seq(n)`: `i * 10 + 1` for `i < n`; `digest` folds a slice.
const PRELUDE: &str = "fn seq(n: u64) -> ChunkSeq[u64] {
    let mut s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    for i in 0..n {
        s.push(wrapping_add(wrapping_mul(i, 10), 1));
    }
    s
}
fn bytes(n: u64) -> ChunkSeq[u8] {
    let mut s: ChunkSeq[u8] = ChunkSeq::new[u8]();
    for i in 0..n {
        s.push(wrapping_u8(wrapping_add(wrapping_mul(i, 37), 11)));
    }
    s
}
fn digest(in s: Slice[u64]) -> u64 {
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
fn digest8(in s: Slice[u8]) -> u64 {
    let mut acc: u64 = 7;
    for i in 0..s.len() {
        let x = match s.try_get(i) {
            Option::Some(x) => widen_u64(x),
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

fn program(body: &str) -> String {
    format!(
        "{PRELUDE}{body}\nfn main() -> u64 {{\n    handle Fail[RuntimeError] in {{\n        run()\n    }} with {{\n        return x => x,\n        raise(e, k) => 7777,\n    }}\n}}\n"
    )
}

fn model_digest<T: Copy + Into<u64>>(s: &[T]) -> u64 {
    s.iter().fold(7u64, |acc, x| {
        acc.wrapping_mul(131).wrapping_add((*x).into())
    })
}

fn mix(acc: u64, v: u64) -> u64 {
    acc.wrapping_mul(1_000_003).wrapping_add(v)
}

/// Every `(lo, hi, at)` up to one past a 7-element sequence, both overlap
/// directions included, against `slice::copy_within`; shifts that do not
/// fit are `false` and leave the sequence as it was.
#[test]
fn every_shift_agrees_with_memmove() {
    let n = 7u64;
    let body = "fn run() -> u64 eff [Fail[RuntimeError]] {
    let n: u64 = 7;
    let mut acc: u64 = 0;
    for lo in 0..wrapping_add(n, 2) {
        for hi in 0..wrapping_add(n, 2) {
            for at in 0..wrapping_add(n, 2) {
                let mut s = seq(n);
                let ok = s.try_copy_within(lo, hi, at);
                acc = mix(mix(acc, if ok { 1 } else { 0 }), digest(in s.as_slice()));
            }
        }
    }
    acc
}";
    let mut expected = 0u64;
    for lo in 0..n + 2 {
        for hi in 0..n + 2 {
            for at in 0..n + 2 {
                let mut model: Vec<u64> = (0..n).map(|i| i * 10 + 1).collect();
                let ok = lo <= hi && hi <= n && at <= n && hi - lo <= n - at;
                if ok {
                    model.copy_within(lo as usize..hi as usize, at as usize);
                }
                expected = mix(mix(expected, u64::from(ok)), model_digest(&model));
            }
        }
    }
    assert_eq!(differential("every_shift", &program(body)), expected);
}

/// Byte sequences (packed one per byte), the checked form, a slice receiver
/// (offsets relative to the view) and the insert/remove shapes the Aura port
/// uses.
#[test]
fn bytes_slices_and_insert_remove() {
    let body = "fn shift(inout s: Slice[u8]) -> bool {
    s.try_copy_within(0, 3, 2)
}
fn insert_at(inout s: ChunkSeq[u64], at: u64, v: u64) eff [Fail[RuntimeError]] {
    let n = s.len();
    s.push(0);
    s.copy_within(at, n, wrapping_add(at, 1));
    s.set(at, v);
}
fn remove_at(inout s: ChunkSeq[u64], at: u64) -> u64 eff [Fail[RuntimeError]] {
    let v = s.get(at);
    let n = s.len();
    s.copy_within(wrapping_add(at, 1), n, at);
    s.truncate(wrapping_sub(n, 1));
    v
}
fn run() -> u64 eff [Fail[RuntimeError]] {
    let mut b = bytes(40);
    b.copy_within(3, 37, 0);
    let mut acc = digest8(in b.as_slice());
    b.copy_within(0, 33, 5);
    acc = mix(acc, digest8(in b.as_slice()));
    let shifted = shift(inout b[10..20]);
    acc = mix(acc, if shifted { 1 } else { 0 });
    acc = mix(acc, digest8(in b.as_slice()));
    let mut s = seq(6);
    insert_at(inout s, 2, 99);
    insert_at(inout s, 7, 98);
    acc = mix(acc, digest(in s.as_slice()));
    acc = mix(acc, remove_at(inout s, 0));
    acc = mix(acc, digest(in s.as_slice()));
    acc
}";
    let mut b: Vec<u8> = (0..40u64).map(|i| (i * 37 + 11) as u8).collect();
    b.copy_within(3..37, 0);
    let mut acc = model_digest(&b);
    b.copy_within(0..33, 5);
    acc = mix(acc, model_digest(&b));
    b[10..20].copy_within(0..3, 2);
    acc = mix(acc, 1);
    acc = mix(acc, model_digest(&b));
    let mut s: Vec<u64> = (0..6u64).map(|i| i * 10 + 1).collect();
    s.insert(2, 99);
    s.insert(7, 98);
    acc = mix(acc, model_digest(&s));
    acc = mix(acc, s.remove(0));
    acc = mix(acc, model_digest(&s));
    assert_eq!(differential("bytes_slices", &program(body)), acc);
}

/// The checked form raises `IndexOutOfBounds` before it writes anything.
#[test]
fn checked_form_faults_without_writing() {
    let body = "fn run() -> u64 eff [Fail[RuntimeError]] {
    let mut s = seq(4);
    s.copy_within(2, 5, 0);
    digest(in s.as_slice())
}";
    assert_eq!(differential("checked_fault", &program(body)), 7777);
    let body = "fn attempt(inout s: ChunkSeq[u64]) -> u64 eff [Fail[RuntimeError]] {
    s.copy_within(1, 3, 3);
    0
}
fn run() -> u64 eff [Fail[RuntimeError]] {
    let mut s = seq(4);
    let r = handle Fail[RuntimeError] in {
        attempt(inout s)
    } with {
        return x => x,
        raise(e, k) => 1,
    };
    mix(r, digest(in s.as_slice()))
}";
    let model: Vec<u64> = (0..4u64).map(|i| i * 10 + 1).collect();
    assert_eq!(
        differential("checked_unwritten", &program(body)),
        mix(1, model_digest(&model))
    );
}

#[test]
fn typing_rules() {
    rejected(
        &program(
            "fn run() -> u64 eff [Fail[RuntimeError]] {\n    let mut s = seq(3);\n    s.copy_within(0, 1);\n    0\n}",
        ),
        "E0207",
        "expects 3 arguments",
    );
    rejected(
        &program(
            "fn run() -> u64 { 0 }\nfn f(in s: ChunkSeq[u64]) -> bool {\n    s.try_copy_within(0, 1, 2)\n}",
        ),
        "E0307",
        "cannot mutate shared parameter",
    );
    rejected(
        &program(
            "fn run() -> u64 { 0 }\nfn f(inout s: ChunkSeq[ChunkSeq[u64]]) -> bool {\n    s.try_copy_within(0, 1, 2)\n}",
        ),
        "E1522",
        "not copy",
    );
    // The checked form raises, so it needs `Fail` in the row.
    rejected(
        &program(
            "fn run() -> u64 { 0 }\nfn f(inout s: ChunkSeq[u64]) {\n    s.copy_within(0, 1, 2);\n}",
        ),
        "E0401",
        "Fail",
    );
}
