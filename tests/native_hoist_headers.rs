//! Sequence headers read once before a loop (port gaps lane 7, #990).
//!
//! A loop that cannot change a sequence's handle, length or buffer reads its
//! `{len, data}` header once, before the loop, on the native optimize path
//! ([`vesper::bounds_hoist::header_hoists`]); accesses still check bounds
//! against the hoisted length. The plan tests check which receivers are
//! hoisted; the differential tests check that every backend agrees,
//! including loops that resize, rebind or lend the sequence (never
//! hoisted) and loops that write a sibling field (hoisted).
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
use vesper::{check, ir, ir_opt, native, parser, runtime::Runtime};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn accepted(source: &str) -> check::Checked {
    let p = parser::parse(source);
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    let c = check::check(&p.module);
    assert!(c.diagnostics.is_empty(), "{:#?}", c.diagnostics);
    c
}

/// The receiver places [`vesper::bounds_hoist::header_hoists`] hoists in
/// `function` of `source`, after the native optimize pipeline's passes.
fn hoisted_places(source: &str, function: &str) -> Vec<String> {
    let c = accepted(source);
    let mut program = ir::lower_world(&c.world);
    ir_opt::optimize(&c.world, &mut program).unwrap();
    vesper::fip::elaborate_non_fip_drops(&c.world, &mut program);
    vesper::bounds_hoist::run(&mut program);
    let mut places: Vec<String> = vesper::bounds_hoist::header_hoists(&program.functions[function])
        .into_iter()
        .map(|h| h.place.display())
        .collect();
    places.sort();
    places
}

fn links_executables() -> bool {
    cfg!(unix)
        && Command::new("cc")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
}

/// Runs `main` in the evaluator, both JIT configurations, x86-64 and
/// AArch64 objects and (where a C driver links) an executable.
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
            "vesper-hoist-headers-test-{}-{}",
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

const Q: &str = r#"
struct Q {
    pool: ChunkSeq[u8],
    seen: u64
}

fn make() -> Q {
    let mut q = Q { pool: ChunkSeq::new[u8](), seen: 0 };
    let i_end: u64 = 40;
    for i in 0..i_end {
        q.pool.push(wrapping_u8(wrapping_add(60, wrapping_mul(i, 7))));
    }
    q
}
"#;

/// Quarry's `lower_ok` (gap 6): reads through `inout q`, writes a sibling.
const LOWER_OK: &str = r#"
fn lower_ok(inout q: Q, off: u64, n: u64) -> u64 {
    let mut upper: u64 = 0;
    for i in 0..n {
        let c = match q.pool.try_get(wrapping_add(off, i)) {
            Option::Some(c) => c,
            Option::None => 0,
        };
        if c >= 65 && c <= 90 {
            upper = wrapping_add(upper, 1);
            q.seen = wrapping_add(q.seen, widen_u64(c));
        }
    }
    upper
}
"#;

#[test]
fn reads_through_inout_hoist_past_sibling_writes() {
    let source = format!(
        "{Q}{LOWER_OK}
fn main() -> u64 {{
    let mut q = make();
    let a = lower_ok(inout q, 0, 40);
    let b = lower_ok(inout q, 30, 20);
    wrapping_add(wrapping_mul(wrapping_add(a, b), 100000), q.seen)
}}
"
    );
    assert_eq!(hoisted_places(&source, "lower_ok"), ["q.pool"]);
    differential("lower_ok", &source);
}

#[test]
fn resizing_rebinding_or_lending_is_never_hoisted() {
    let source = format!(
        "{Q}
fn bump(inout q: Q) {{
    q.seen = wrapping_add(q.seen, 1);
}}

fn grow(inout q: Q, n: u64) -> u64 {{
    let mut total: u64 = 0;
    for i in 0..n {{
        match q.pool.try_get(wrapping_mul(i, 2)) {{
            Option::Some(c) => {{ total = wrapping_add(total, widen_u64(c)); }}
            Option::None => {{ q.pool.push(1); }}
        }}
    }}
    total
}}

fn lend(inout q: Q, n: u64) -> u64 {{
    let mut total: u64 = 0;
    for i in 0..n {{
        let c = match q.pool.try_get(wrapping_add(i, 1)) {{
            Option::Some(c) => c,
            Option::None => 0,
        }};
        bump(inout q);
        total = wrapping_add(total, widen_u64(c));
    }}
    total
}}

fn rebind(n: u64) -> u64 {{
    let mut s: ChunkSeq[u64] = ChunkSeq::new[u64]();
    s.push(3);
    let mut total: u64 = 0;
    for i in 0..n {{
        let v = match s.try_get(0) {{
            Option::Some(v) => v,
            Option::None => 0,
        }};
        total = wrapping_add(total, v);
        s = ChunkSeq::new[u64]();
        s.push(wrapping_add(i, 5));
    }}
    total
}}

fn main() -> u64 {{
    let mut q = make();
    let a = grow(inout q, 50);
    let b = lend(inout q, 45);
    let c = rebind(6);
    wrapping_add(wrapping_add(wrapping_mul(a, 3), b), wrapping_add(c, q.seen))
}}
"
    );
    for f in ["grow", "lend", "rebind"] {
        assert!(hoisted_places(&source, f).is_empty(), "{f}");
    }
    differential("not_hoisted", &source);
}

#[test]
fn slices_nested_loops_and_element_writes_agree() {
    let source = r#"
fn window(in s: Slice[u64], at: u64, n: u64) -> u64 {
    let mut total: u64 = 0;
    for i in 0..n {
        let v = match s.try_get(wrapping_mul(wrapping_add(at, i), 3)) {
            Option::Some(v) => v,
            Option::None => 1,
        };
        total = wrapping_add(total, v);
    }
    total
}

fn grid(in rows: ChunkSeq[u64], in cols: ChunkSeq[u64]) -> u64 {
    let mut total: u64 = 0;
    let mut i: u64 = 0;
    loop {
        if i >= 9 {
            break;
        }
        let r = match rows.try_get(i) {
            Option::None => 0,
            Option::Some(r) => r,
        };
        let mut j: u64 = 0;
        loop {
            if j >= 7 {
                break;
            }
            let c: u64 = match cols.try_get(wrapping_mul(j, 2)) {
                _ => 1,
            };
            let d = match cols.try_get(j) {
                Option::Some(d) => d,
                _ => 2,
            };
            total = wrapping_add(total, wrapping_mul(wrapping_add(r, c), d));
            j = wrapping_add(j, 1);
        }
        i = wrapping_add(i, 1);
    }
    total
}

fn scale(inout s: ChunkSeq[u64], k: u64) -> u64 {
    let mut i: u64 = 0;
    let mut wrote: u64 = 0;
    loop {
        match s.try_get(i) {
            Option::Some(v) => {
                if s.try_set(i, wrapping_mul(v, k)) {
                    wrote = wrapping_add(wrote, 1);
                }
            }
            Option::None => {
                return wrote;
            }
        }
        i = wrapping_add(i, 1);
    }
}

fn main() -> u64 {
    let mut rows: ChunkSeq[u64] = ChunkSeq::new[u64]();
    let mut cols: ChunkSeq[u64] = ChunkSeq::new[u64]();
    let i_end: u64 = 6;
    for i in 0..i_end {
        rows.push(wrapping_add(i, 1));
        cols.push(wrapping_mul(i, 4));
    }
    let wrote = scale(inout cols, 3);
    let g = grid(in rows, in cols);
    let w = window(in cols.as_slice(), 0, 4);
    wrapping_add(wrapping_add(wrapping_mul(wrote, 1000000), g), wrapping_mul(w, 1000))
}
"#;
    assert_eq!(hoisted_places(source, "window"), ["s"]);
    assert_eq!(hoisted_places(source, "grid"), ["cols", "rows"]);
    assert_eq!(hoisted_places(source, "scale"), ["s"]);
    differential("slices_nested", source);
}
