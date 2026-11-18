//! Byte scans (Stage 1 performance wave, lane 6, #936): `sum_be16`, `eq`
//! and `find_byte` on `ChunkSeq[u8]` and `Slice[u8]`. Optimized native code
//! runs them on 128-bit vectors with a scalar tail; unoptimized code runs
//! the scalar loop. Every program here runs in the evaluator, the JIT with
//! and without `optimize`, and an AOT executable, and is checked against a
//! Rust model, over every length 0..=256 at unaligned starts and random
//! bytes.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use vesper::{check, native, parser, runtime::Runtime};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "vesper-native-simd-bytes-{}-{}",
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

fn accepted(source: &str) -> check::Checked {
    let p = parser::parse(source);
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let c = check::check(&p.module);
    let shown: Vec<_> = c
        .diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}: {} at `{}`",
                d.code,
                d.message,
                &source[d.span.start..d.span.end]
            )
        })
        .collect();
    assert!(c.diagnostics.is_empty(), "{shown:#?}");
    c
}

fn rejected(source: &str, code: &str, needle: &str) {
    let p = parser::parse(source);
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let c = check::check(&p.module);
    assert!(
        c.diagnostics
            .iter()
            .any(|d| d.code == code && d.message.contains(needle)),
        "expected {code} ({needle}): {:#?}",
        c.diagnostics
    );
}

fn links_executables() -> bool {
    cfg!(unix)
        && Command::new("cc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

fn executable_status(name: &str, source: &str) -> Option<i32> {
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
    Command::new(&exe).output().unwrap().status.code()
}

/// Runs `main` in the evaluator, both JIT configurations and (where a C
/// driver links executables) an AOT executable, and checks every result
/// against `expected`.
fn differential(name: &str, source: &str, expected: u64) {
    let c = accepted(source);
    let evaluated = Runtime::new(&c.world)
        .run("main", vec![])
        .unwrap_or_else(|e| panic!("{name}: evaluator failed: {e}"))
        .to_string();
    assert_eq!(
        evaluated,
        expected.to_string(),
        "{name}: evaluator vs model"
    );
    for optimize in [true, false] {
        let jit = native::jit_with_options(&c.world, "main", optimize)
            .unwrap_or_else(|e| panic!("{name}: JIT (optimize {optimize}) failed: {e}"))
            .to_string();
        assert_eq!(jit, evaluated, "{name}: JIT (optimize {optimize})");
    }
    if links_executables() {
        assert_eq!(
            executable_status(name, source),
            Some((expected & 0xff) as i32),
            "{name}: executable"
        );
    }
}

/// `noise(n, seed)`: `n` pseudo-random bytes from a 64-bit LCG, the same
/// sequence as [`model_noise`].
const PRELUDE: &str = "fn noise(n: u64, seed: u64) -> ChunkSeq[u8] {
    let mut s: ChunkSeq[u8] = ChunkSeq::new[u8]();
    let mut x = seed;
    for i in 0..n {
        x = wrapping_add(wrapping_mul(x, 6364136223846793005), 1442695040888963407);
        s.push(wrapping_u8(x >> 56));
    }
    s
}
fn mix(acc: u64, v: u64) -> u64 {
    wrapping_add(wrapping_mul(acc, 1099511628211), v)
}
/// Mismatch position `r` (0, 1 or 2) of the interval `off..off + n`:
/// its first byte, one at seven eighths, and its last byte.
fn pos(off: u64, n: u64, r: u64) -> u64 {
    if r == 0 {
        off
    } else if r == 1 {
        wrapping_add(off, wrapping_mul(n, 7) >> 3)
    } else {
        wrapping_sub(wrapping_add(off, n), 1)
    }
}
fn bit(b: bool) -> u64 {
    let one: u64 = 1;
    let zero: u64 = 0;
    if b { one } else { zero }
}
fn main() -> u64 {
    handle Fail[RuntimeError] in {
        run()
    } with {
        return x => x,
        raise(e, k) => 7777,
    }
}
";

fn model_noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 56) as u8
        })
        .collect()
}

fn mix(acc: u64, v: u64) -> u64 {
    acc.wrapping_mul(1099511628211).wrapping_add(v)
}

fn model_sum(s: &[u8]) -> u64 {
    s.chunks(2).fold(0u64, |acc, w| {
        acc.wrapping_add(u64::from(w[0]) << 8 | u64::from(*w.get(1).unwrap_or(&0)))
    })
}

/// Starts `0..OFFSETS` cover every alignment of a 16-byte vector step.
const OFFSETS: usize = 17;
const MAX_LEN: usize = 256;
const BUF: usize = OFFSETS + MAX_LEN;

#[test]
fn sum_be16_every_length_and_offset() {
    let source = format!(
        "{PRELUDE}fn sum(in s: Slice[u8]) -> u64 {{
    s.sum_be16()
}}
fn run() -> u64 eff [Fail[RuntimeError]] {{
    let offsets: u64 = {OFFSETS};
    let max_len: u64 = {MAX_LEN};
    let s = noise({BUF}, 7);
    let mut acc: u64 = 1;
    for off in 0..offsets {{
        for n in 0..=max_len {{
            let hi = wrapping_add(off, n);
            acc = mix(acc, sum(in s[off..hi]));
        }}
    }}
    mix(acc, s.sum_be16())
}}
"
    );
    let s = model_noise(BUF, 7);
    let mut acc = 1u64;
    for off in 0..OFFSETS {
        for n in 0..=MAX_LEN {
            acc = mix(acc, model_sum(&s[off..off + n]));
        }
    }
    differential("sum_be16", &source, mix(acc, model_sum(&s)));
}

/// Long all-ones buffers cross the 4,096-byte block where the vector sum
/// widens its 16-bit lanes; odd lengths add the high-byte tail.
#[test]
fn sum_be16_long_buffers() {
    let source = format!(
        "{PRELUDE}fn ones(n: u64) -> ChunkSeq[u8] {{
    let mut s: ChunkSeq[u8] = ChunkSeq::with_capacity[u8](n);
    for i in 0..n {{
        s.push(255);
    }}
    s
}}
fn run() -> u64 eff [Fail[RuntimeError]] {{
    let offsets: u64 = {OFFSETS};
    let max_len: u64 = {MAX_LEN};
    let a = ones(70001);
    let b = noise(9000, 3);
    let c = ones(4096);
    let d = ones(4112);
    mix(mix(mix(a.sum_be16(), b.sum_be16()), c.sum_be16()), d.sum_be16())
}}
"
    );
    let ones = |n: usize| vec![255u8; n];
    let expected = mix(
        mix(
            mix(model_sum(&ones(70001)), model_sum(&model_noise(9000, 3))),
            model_sum(&ones(4096)),
        ),
        model_sum(&ones(4112)),
    );
    differential("sum_be16_long", &source, expected);
}

#[test]
fn eq_every_length_offset_and_mismatch() {
    // `t` is `s` with byte `j` of every interval flipped in turn: the
    // mismatch moves through every vector lane and the scalar tail.
    let source = format!(
        "{PRELUDE}fn same(in a: Slice[u8], in b: Slice[u8]) -> u64 {{
    bit(a.eq(in b))
}}
fn run() -> u64 eff [Fail[RuntimeError]] {{
    let offsets: u64 = {OFFSETS};
    let max_len: u64 = {MAX_LEN};
    let s = noise({BUF}, 11);
    let mut t = noise({BUF}, 11);
    let mut acc: u64 = 1;
    for off in 0..offsets {{
        for n in 0..=max_len {{
            let hi = wrapping_add(off, n);
            let shift = wrapping_sub(offsets, off);
            let hi2 = wrapping_add(shift, n);
            acc = mix(acc, same(in s[off..hi], in t[off..hi]));
            acc = mix(acc, same(in s[off..hi], in t[shift..hi2]));
            if n > 0 {{
                let short = wrapping_sub(hi, 1);
                acc = mix(acc, same(in s[off..hi], in t[off..short]));
                let three: u64 = 3;
                for r in 0..three {{
                    let j = pos(off, n, r);
                    let old = t[j];
                    t.set(j, old ^ 128);
                    acc = mix(acc, same(in s[off..hi], in t[off..hi]));
                    t.set(j, old);
                }}
            }}
        }}
    }}
    let u = noise({BUF}, 11);
    acc = mix(acc, bit(s.eq(in u.as_slice())));
    let all: u64 = {BUF};
    mix(acc, bit(s.eq(in s[0..all])))
}}
"
    );
    let s = model_noise(BUF, 11);
    let mut t = s.clone();
    let mut acc = 1u64;
    let same = |a: &[u8], b: &[u8]| u64::from(a == b);
    for off in 0..OFFSETS {
        for n in 0..=MAX_LEN {
            let shift = OFFSETS - off;
            acc = mix(acc, same(&s[off..off + n], &t[off..off + n]));
            acc = mix(acc, same(&s[off..off + n], &t[shift..shift + n]));
            if n > 0 {
                acc = mix(acc, same(&s[off..off + n], &t[off..off + n - 1]));
                for j in [off, off + ((n * 7) >> 3), off + n - 1] {
                    t[j] ^= 128;
                    acc = mix(acc, same(&s[off..off + n], &t[off..off + n]));
                    t[j] ^= 128;
                }
            }
        }
    }
    acc = mix(acc, 1);
    differential("bytes_eq", &source, mix(acc, 1));
}

#[test]
fn find_byte_every_length_offset_and_position() {
    // A needle planted at every position `j` of every interval, plus the
    // first natural hit of a random byte and a byte that never occurs.
    let source = format!(
        "{PRELUDE}fn at(in s: Slice[u8], b: u8) -> u64 {{
    match s.find_byte(b) {{
        Option::Some(i) => wrapping_add(i, 1),
        Option::None => 0,
    }}
}}
fn run() -> u64 eff [Fail[RuntimeError]] {{
    let offsets: u64 = {OFFSETS};
    let max_len: u64 = {MAX_LEN};
    let mut s = noise({BUF}, 5);
    let all: u64 = {BUF};
    for i in 0..all {{
        if s[i] == 0 {{
            s.set(i, 1);
        }}
    }}
    let mut acc: u64 = 1;
    for off in 0..offsets {{
        for n in 0..=max_len {{
            let hi = wrapping_add(off, n);
            acc = mix(acc, at(in s[off..hi], 0));
            acc = mix(acc, at(in s[off..hi], s[wrapping_add(off, n & 3)]));
            if n > 0 {{
                let three: u64 = 3;
                for r in 0..three {{
                    let j = pos(off, n, r);
                    let old = s[j];
                    s.set(j, 0);
                    acc = mix(acc, at(in s[off..hi], 0));
                    s.set(j, old);
                }}
            }}
        }}
    }}
    acc
}}
"
    );
    let mut s = model_noise(BUF, 5);
    for b in &mut s {
        if *b == 0 {
            *b = 1;
        }
    }
    let at = |s: &[u8], b: u8| s.iter().position(|&x| x == b).map_or(0, |i| i as u64 + 1);
    let mut acc = 1u64;
    for off in 0..OFFSETS {
        for n in 0..=MAX_LEN {
            let hi = off + n;
            acc = mix(acc, at(&s[off..hi], 0));
            acc = mix(acc, at(&s[off..hi], s[off + (n & 3)]));
            if n > 0 {
                for j in [off, off + ((n * 7) >> 3), hi - 1] {
                    let old = s[j];
                    s[j] = 0;
                    acc = mix(acc, at(&s[off..hi], 0));
                    s[j] = old;
                }
            }
        }
    }
    differential("find_byte", &source, acc);
}

/// The optimized build of each scan holds 128-bit vector code for x86-64
/// (SSE2 baseline flags) and AArch64, and the unoptimized one does not.
#[test]
fn optimized_objects_use_vectors_on_both_targets() {
    let source = "fn main() -> u64 {
    let n: u64 = 100;
    let mut s: ChunkSeq[u8] = ChunkSeq::new[u8]();
    for i in 0..n {
        s.push(wrapping_u8(i));
    }
    let found: u64 = match s.find_byte(42) {
        Option::Some(i) => i,
        Option::None => 0,
    };
    let same: u64 = if s.eq(in s[0..n]) { 1 } else { 0 };
    wrapping_add(wrapping_add(s.sum_be16(), found), same)
}
";
    let c = accepted(source);
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        let (_, fast) = native::object_with_options(&c.world, "main", Some(target), true)
            .unwrap_or_else(|e| panic!("{target}: optimized object failed: {e}"));
        for needle in ["i8x16", "i16x8", "vhigh_bits"] {
            assert!(fast.contains(needle), "{target}: no {needle} in\n{fast}");
        }
        let (_, slow) = native::object_with_options(&c.world, "main", Some(target), false)
            .unwrap_or_else(|e| panic!("{target}: object failed: {e}"));
        assert!(
            !slow.contains("i8x16"),
            "{target}: unoptimized object uses vectors"
        );
    }
}

#[test]
fn scans_need_byte_elements() {
    rejected(
        "fn main() -> u64 {
    let mut s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    s.push(1);
    s.sum_be16()
}
",
        "E1525",
        "'sum_be16' reads and writes bytes",
    );
    rejected(
        "fn main() -> bool {
    let s: ChunkSeq[u8] = ChunkSeq::new[u8]();
    s.eq(in s.as_slice(), 3)
}
",
        "E0207",
        "'eq' expects 1 arguments",
    );
    rejected(
        "fn main() -> u64 {
    let s: ChunkSeq[u8] = ChunkSeq::new[u8]();
    match s.find_byte(true) {
        Option::Some(i) => i,
        Option::None => 0,
    }
}
",
        "E0200",
        "expected u8, found bool",
    );
}
