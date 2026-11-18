//! Block and fixed-width sequence builtins (Stage 1 performance wave, lane
//! 3 part 2, #933): `copy_from`, `try_copy_from`, `extend_from_slice`,
//! `fill`, and `read_*` / `write_*` (with their total `try_` forms) on byte
//! sequences. The evaluator, both JIT configurations and an AOT executable
//! agree on every program, including the faults of short buffers.
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
            "vesper-native-bytes-builtins-{}-{}",
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
    assert!(c.diagnostics.is_empty(), "{:#?}", c.diagnostics);
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
/// driver links executables) an AOT executable; returns the result.
fn differential(name: &str, source: &str) -> String {
    let c = accepted(source);
    let evaluated = Runtime::new(&c.world)
        .run("main", vec![])
        .unwrap_or_else(|e| panic!("{name}: evaluator failed: {e}"))
        .to_string();
    for optimize in [true, false] {
        let jit = native::jit_with_options(&c.world, "main", optimize)
            .unwrap_or_else(|e| panic!("{name}: JIT (optimize {optimize}) failed: {e}"))
            .to_string();
        assert_eq!(
            jit, evaluated,
            "{name}: JIT (optimize {optimize}) vs evaluator"
        );
    }
    if links_executables() {
        let expected: u64 = evaluated.parse().unwrap();
        assert_eq!(
            executable_status(name, source),
            Some((expected & 0xff) as i32),
            "{name}: executable"
        );
    }
    evaluated
}

/// `bytes(n)`: `n` bytes `i * 37 + 11`; `digest` folds a byte slice.
const PRELUDE: &str = "fn bytes(n: u64) -> ChunkSeq[u8] {
    let mut s: ChunkSeq[u8] = ChunkSeq::new[u8]();
    for i in 0..n {
        s.push(wrapping_u8(wrapping_add(wrapping_mul(i, 37), 11)));
    }
    s
}
fn digest(in s: Slice[u8]) -> u64 {
    let mut acc: u64 = 7;
    for i in 0..s.len() {
        let b = match s.try_get(i) {
            Option::Some(b) => widen_u64(b),
            Option::None => 0,
        };
        acc = wrapping_add(wrapping_mul(acc, 131), b);
    }
    acc
}
";

fn program(body: &str) -> String {
    format!(
        "{PRELUDE}{body}\nfn main() -> u64 {{\n    handle Fail[RuntimeError] in {{\n        run()\n    }} with {{\n        return x => x,\n        raise(e, k) => 7777,\n    }}\n}}\n"
    )
}

fn model_bytes(n: u64) -> Vec<u8> {
    (0..n).map(|i| (i * 37 + 11) as u8).collect()
}

fn model_digest(s: &[u8]) -> u64 {
    s.iter().fold(7u64, |acc, b| {
        acc.wrapping_mul(131).wrapping_add(u64::from(*b))
    })
}

fn mix(acc: u64, v: u64) -> u64 {
    acc.wrapping_mul(1_000_003).wrapping_add(v)
}

#[test]
fn wide_reads_agree_at_every_offset() {
    // Every width and byte order, at every offset of a 19-byte sequence and
    // of a slice that starts at an odd byte (unaligned loads).
    let mut body = String::from(
        "fn mix(acc: u64, v: u64) -> u64 {\n    wrapping_add(wrapping_mul(acc, 1000003), v)\n}\n",
    );
    body.push_str(
        "fn reads(in s: Slice[u8]) -> u64 eff [Fail[RuntimeError]] {\n    let mut acc: u64 = 0;\n",
    );
    for (w, ty) in [(2, "u16"), (4, "u32"), (8, "u64")] {
        for e in ["le", "be"] {
            body.push_str(&format!(
                "    for at in 0..wrapping_sub(s.len(), {}) {{\n        acc = mix(acc, widen_u64(s.read_{ty}_{e}(at)));\n    }}\n",
                w - 1
            ));
        }
    }
    body.push_str("    acc\n}\n");
    body.push_str(
        "fn run() -> u64 eff [Fail[RuntimeError]] {
    let s = bytes(19);
    let mut acc = reads(in s.as_slice());
    acc = mix(acc, reads(in s[3..19]));
    acc = mix(acc, widen_u64(s.read_u32_be(15)));
    acc
}",
    );
    let reads = |s: &[u8]| {
        let mut acc = 0;
        for w in [2usize, 4, 8] {
            for big in [false, true] {
                for at in 0..=s.len() - w {
                    let mut bytes = [0u8; 8];
                    bytes[..w].copy_from_slice(&s[at..at + w]);
                    if big {
                        bytes[..w].reverse();
                    }
                    acc = mix(acc, u64::from_le_bytes(bytes));
                }
            }
        }
        acc
    };
    let s = model_bytes(19);
    let mut expected = reads(&s);
    expected = mix(expected, reads(&s[3..]));
    expected = mix(
        expected,
        u64::from(u32::from_be_bytes(s[15..19].try_into().unwrap())),
    );
    assert_eq!(
        differential("wide_reads", &program(&body)),
        expected.to_string()
    );
}

#[test]
fn wide_writes_round_trip() {
    let source = program(
        "fn run() -> u64 eff [Fail[RuntimeError]] {
    let mut s = bytes(24);
    s.write_u16_le(1, 0xbeef);
    s.write_u16_be(3, 0xbeef);
    s.write_u32_le(5, 0x01234567);
    s.write_u32_be(9, 0x89abcdef);
    s.write_u64_le(13, 0x0123456789abcdef);
    let ok = s.try_write_u16_be(22, 0x7f80);
    let mut acc = digest(in s.as_slice());
    if ok {
        acc = wrapping_add(acc, 1);
    }
    acc = wrapping_add(acc, widen_u64(s.read_u16_le(3)));
    acc = wrapping_add(acc, s.read_u64_be(13));
    acc
}",
    );
    let mut s = model_bytes(24);
    s[1..3].copy_from_slice(&0xbeefu16.to_le_bytes());
    s[3..5].copy_from_slice(&0xbeefu16.to_be_bytes());
    s[5..9].copy_from_slice(&0x0123_4567u32.to_le_bytes());
    s[9..13].copy_from_slice(&0x89ab_cdefu32.to_be_bytes());
    s[13..21].copy_from_slice(&0x0123_4567_89ab_cdefu64.to_le_bytes());
    s[22..24].copy_from_slice(&0x7f80u16.to_be_bytes());
    let expected = model_digest(&s)
        .wrapping_add(1)
        .wrapping_add(u64::from(u16::from_le_bytes([s[3], s[4]])))
        .wrapping_add(u64::from_be_bytes(s[13..21].try_into().unwrap()));
    assert_eq!(differential("wide_writes", &source), expected.to_string());
}

#[test]
fn total_forms_report_short_buffers() {
    // `try_` reads are None and `try_` writes are false (writing nothing)
    // when the bytes do not all fit, including offsets near u64::MAX.
    let source = program(
        "// A slice that starts past 0, at offsets near u64::MAX.
fn far(in s: Slice[u8]) -> u64 {
    let mut n: u64 = 0;
    match s.try_read_u16_le(wrapping_sub(0, 1)) {
        Option::Some(_) => n = 1,
        Option::None => (),
    }
    match s.try_read_u64_be(wrapping_sub(0, 3)) {
        Option::Some(_) => n = wrapping_add(n, 1),
        Option::None => (),
    }
    n
}
fn run() -> u64 eff [Fail[RuntimeError]] {
    let mut s = bytes(7);
    let mut acc: u64 = 0;
    let mut at: u64 = 0;
    loop {
        if at > 9 {
            break;
        }
        acc = wrapping_mul(acc, 3);
        acc = wrapping_add(acc, match s.try_read_u16_be(at) {
            Option::Some(v) => widen_u64(v),
            Option::None => 100000,
        });
        acc = wrapping_add(acc, match s.try_read_u32_le(at) {
            Option::Some(v) => widen_u64(v),
            Option::None => 200000,
        });
        acc = wrapping_add(acc, match s.try_read_u64_le(at) {
            Option::Some(v) => v,
            Option::None => 300000,
        });
        at = wrapping_add(at, 1);
    }
    if s.try_write_u32_be(4, 0xffffffff) {
        acc = wrapping_add(acc, 1);
    }
    if s.try_write_u16_le(wrapping_sub(0, 1), 0xffff) {
        acc = wrapping_add(acc, 2);
    }
    acc = wrapping_add(acc, match s.try_read_u16_le(wrapping_sub(0, 1)) {
        Option::Some(_) => 4,
        Option::None => 0,
    });
    acc = wrapping_add(acc, far(in s[1..7]));
    if s.try_write_u16_le(5, 0x0102) {
        acc = wrapping_add(acc, 8);
    }
    wrapping_add(wrapping_mul(acc, 7), digest(in s.as_slice()))
}",
    );
    let mut s = model_bytes(7);
    let mut acc: u64 = 0;
    for at in 0..=9usize {
        acc = acc.wrapping_mul(3);
        let read = |w: usize| (at + w <= s.len()).then(|| &s[at..at + w]);
        acc = acc.wrapping_add(match read(2) {
            Some(b) => u64::from(u16::from_be_bytes(b.try_into().unwrap())),
            None => 100_000,
        });
        acc = acc.wrapping_add(match read(4) {
            Some(b) => u64::from(u32::from_le_bytes(b.try_into().unwrap())),
            None => 200_000,
        });
        acc = acc.wrapping_add(match read(8) {
            Some(b) => u64::from_le_bytes(b.try_into().unwrap()),
            None => 300_000,
        });
    }
    s[5..7].copy_from_slice(&0x0102u16.to_le_bytes());
    acc = acc.wrapping_add(8);
    let expected = acc.wrapping_mul(7).wrapping_add(model_digest(&s));
    assert_eq!(differential("wide_total", &source), expected.to_string());
}

#[test]
fn short_buffers_fault_before_writing() {
    // Checked forms raise `IndexOutOfBounds` on a short buffer and write
    // nothing; the writes of earlier calls are kept (keep-writes).
    let source = format!(
        "{PRELUDE}fn step(inout s: ChunkSeq[u8], n: u64) -> u64 eff [Fail[RuntimeError]] {{
    for k in 0..n {{
        s.write_u16_be(wrapping_mul(k, 2), wrapping_u16(wrapping_add(k, 256)));
    }}
    0
}}
fn copy_in(inout s: ChunkSeq[u8], in src: Slice[u8]) -> u64 eff [Fail[RuntimeError]] {{
    s.copy_from(5, in src);
    1
}}
fn read(in s: Slice[u8], at: u64) -> u64 eff [Fail[RuntimeError]] {{
    s.read_u64_le(at)
}}
fn main() -> u64 {{
    let mut s: ChunkSeq[u8] = bytes(7);
    let r = handle Fail[RuntimeError] in {{
        step(inout s, 10)
    }} with {{
        return x => x,
        raise(e, k) => 7777,
    }};
    let src = bytes(3);
    let c = handle Fail[RuntimeError] in {{
        copy_in(inout s, in src.as_slice())
    }} with {{
        return x => x,
        raise(e, k) => 5000,
    }};
    let d = handle Fail[RuntimeError] in {{
        read(in s.as_slice(), 0)
    }} with {{
        return x => x,
        raise(e, k) => 9000,
    }};
    wrapping_add(wrapping_add(wrapping_add(r, c), d), digest(in s.as_slice()))
}}
"
    );
    let mut s = model_bytes(7);
    for k in 0..3u16 {
        let at = usize::from(k) * 2;
        s[at..at + 2].copy_from_slice(&(k + 256).to_be_bytes());
    }
    let expected = 7777u64 + 5000 + 9000 + model_digest(&s);
    assert_eq!(differential("wide_faults", &source), expected.to_string());
}

#[test]
fn copies_agree_on_sequences_and_slices() {
    let source = program(
        "fn into(inout dst: Slice[u64], at: u64, in src: Slice[u64]) -> bool {
    dst.try_copy_from(at, in src)
}
fn sum(in s: Slice[u64]) -> u64 {
    let mut acc: u64 = 0;
    for i in 0..s.len() {
        let v = match s.try_get(i) {
            Option::Some(v) => v,
            Option::None => 0,
        };
        acc = wrapping_add(wrapping_mul(acc, 31), v);
    }
    acc
}
fn shift(inout lo: Slice[u64], in hi: Slice[u64]) eff [Fail[RuntimeError]] {
    lo.copy_from(0, in hi);
}
fn run() -> u64 eff [Fail[RuntimeError]] {
    let mut d = bytes(12);
    let src = bytes(4);
    d.copy_from(8, in src.as_slice());
    d.copy_from(0, in src[1..3]);
    d.copy_from(12, in src[2..2]);
    let mut acc = digest(in d.as_slice());
    if d.try_copy_from(9, in src.as_slice()) {
        acc = wrapping_add(acc, 1);
    }
    let mut w: ChunkSeq[u64] = ChunkSeq::new[u64]();
    let mut v: ChunkSeq[u64] = ChunkSeq::new[u64]();
    let ten: u64 = 10;
    for i in 0..ten {
        w.push(i);
        v.push(wrapping_mul(i, 1000));
    }
    if into(inout w[2..9], 3, in v[0..4]) {
        acc = wrapping_add(acc, 2);
    }
    if into(inout w[2..9], 4, in v[0..4]) {
        acc = wrapping_add(acc, 4);
    }
    // Disjoint views of one sequence: the upper half moves down.
    shift(inout w[0..5], in w[5..10]);
    w.extend_from_slice(in v[7..10]);
    wrapping_add(wrapping_mul(acc, 1000003), sum(in w.as_slice()))
}",
    );
    let mut d = model_bytes(12);
    let src = model_bytes(4);
    d[8..12].copy_from_slice(&src);
    d[0..2].copy_from_slice(&src[1..3]);
    let mut acc = model_digest(&d);
    let mut w: Vec<u64> = (0..10).collect();
    let v: Vec<u64> = (0..10).map(|i| i * 1000).collect();
    w[5..9].copy_from_slice(&v[0..4]);
    acc = acc.wrapping_add(2);
    w.copy_within(5..10, 0);
    w.extend_from_slice(&v[7..10]);
    let sum = w
        .iter()
        .fold(0u64, |a, x| a.wrapping_mul(31).wrapping_add(*x));
    let expected = acc.wrapping_mul(1_000_003).wrapping_add(sum);
    assert_eq!(differential("copies", &source), expected.to_string());
}

#[test]
fn fill_agrees_for_bytes_and_words() {
    let source = program(
        "fn zero(inout s: Slice[u8]) {
    s.fill(0);
}
fn run() -> u64 eff [Fail[RuntimeError]] {
    let mut s = bytes(20);
    s.fill(0xa5);
    zero(inout s[4..9]);
    zero(inout s[20..20]);
    let mut w: ChunkSeq[u64] = ChunkSeq::new[u64]();
    let six: u64 = 6;
    for i in 0..six {
        w.push(i);
    }
    w.fill(wrapping_sub(0, 3));
    let mut acc = digest(in s.as_slice());
    for i in 0..w.len() {
        acc = wrapping_add(acc, w[i]);
    }
    acc
}",
    );
    let mut s = vec![0xa5u8; 20];
    s[4..9].fill(0);
    let expected = model_digest(&s).wrapping_add(6u64.wrapping_mul(0u64.wrapping_sub(3)));
    assert_eq!(differential("fill", &source), expected.to_string());
}

#[test]
fn builtins_are_typed() {
    let wrap = |body: &str| format!("{PRELUDE}fn main() -> u64 {{\n{body}\n}}\n");
    rejected(
        &wrap(
            "    let s: ChunkSeq[u64] = ChunkSeq::new[u64]();\n    match s.try_read_u16_le(0) {\n        Option::Some(v) => widen_u64(v),\n        Option::None => 0,\n    }",
        ),
        "E1525",
        "ChunkSeq[u8] or Slice[u8]",
    );
    rejected(
        &wrap("    let mut s = bytes(8);\n    s.try_copy_from(0, in s[4..8]);\n    0"),
        "E0301",
        "cannot read from the sequence it writes",
    );
    rejected(
        &wrap("    let mut s = bytes(8);\n    s.try_write_u16_le(0, 1, 2);\n    0"),
        "E0207",
        "expects 2 arguments",
    );
    // The fallible forms need a handler for Fail[RuntimeError].
    let p = parser::parse(&wrap("    let s = bytes(8);\n    s.read_u64_be(0)"));
    let c = check::check(&p.module);
    assert!(!c.diagnostics.is_empty(), "read_u64_be is fallible");
}
