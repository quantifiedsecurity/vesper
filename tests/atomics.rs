//! Source atomics (port wave P3a lane 2, #1037): `Atomic[T]` for `bool` and
//! every integer kind. Each program runs in the evaluator (optimized and
//! not), the JIT (optimized and not) and, where a C driver links
//! executables, an AOT executable, and matches the Rust model
//! (`vesper::atomics::AtomicOp::rmw`, itself checked against `std` atomics).
//! Contention runs compiled Vesper on four host threads through
//! `native::WordFunction`; optimized native code holds one Cranelift atomic
//! per operation and never a call; the checker refuses invalid and
//! non-literal orders, other cell kinds and the private cell; and the
//! verified backend refuses atomics with a typed error.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{self, AtomicU64, Ordering},
};
use vesper::{
    atomics::{AtomicOp, ORDERS},
    check, native,
    numeric::{INTEGER_KINDS, IntegerKind},
    parser,
    runtime::Runtime,
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "vesper-atomics-{}-{}",
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
    assert!(p.diagnostics.is_empty(), "{source}\n{:?}", p.diagnostics);
    check::check(&p.module)
}

fn accepted(source: &str) -> check::Checked {
    let c = checked(source);
    let shown: Vec<_> = c
        .diagnostics
        .iter()
        .map(|d| format!("{}: {} at `{}`", d.code, d.message, &source[d.span.clone()]))
        .collect();
    assert!(c.diagnostics.is_empty(), "{source}\n{shown:#?}");
    c
}

fn rejects(source: &str, code: &str, message: &str) {
    let c = checked(source);
    assert!(
        c.diagnostics
            .iter()
            .any(|d| d.code == code && d.message.contains(message)),
        "expected {code} `{message}` for {source}: {:#?}",
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

/// Runs `main` everywhere and checks each result against `expected`.
fn differential(name: &str, source: &str, expected: u64) {
    let c = accepted(source);
    for optimize in [true, false] {
        let mut runtime = Runtime::new(&c.world);
        runtime.optimize = optimize;
        let value = runtime
            .run("main", vec![])
            .unwrap_or_else(|e| panic!("{name}: evaluator (optimize {optimize}) failed: {e}"))
            .to_string();
        assert_eq!(
            value,
            expected.to_string(),
            "{name}: evaluator (optimize {optimize})"
        );
        let jit = native::jit_with_options(&c.world, "main", optimize)
            .unwrap_or_else(|e| panic!("{name}: JIT (optimize {optimize}) failed: {e}"))
            .to_string();
        assert_eq!(
            jit,
            expected.to_string(),
            "{name}: JIT (optimize {optimize})"
        );
    }
    for target in ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"] {
        native::object_for_target(&c.world, "main", Some(target))
            .unwrap_or_else(|e| panic!("{name}: {target} object failed: {e}"));
    }
    if links_executables() {
        assert_eq!(
            executable_status(name, source),
            Some((expected & 0xff) as i32),
            "{name}: executable"
        );
    }
}

const OFFSET: u64 = 14695981039346656037;
fn mix(acc: u64, v: u64) -> u64 {
    acc.wrapping_mul(1099511628211).wrapping_add(v)
}

/// Edge operands of `k`, as bit patterns.
fn edges(k: IntegerKind) -> Vec<u64> {
    let bits = k.bits();
    let mask = if bits == 64 {
        u64::MAX
    } else {
        (1 << bits) - 1
    };
    vec![
        0,
        1,
        3,
        1 << (bits - 1),
        mask,
        mask >> 1,
        0x5555_5555_5555_5555 & mask,
        0xaaaa_aaaa_aaaa_aaaa & mask,
    ]
}

const RMW: [(&str, AtomicOp); 6] = [
    ("swap", AtomicOp::Swap),
    ("fetch_add", AtomicOp::FetchAdd),
    ("fetch_sub", AtomicOp::FetchSub),
    ("fetch_and", AtomicOp::FetchAnd),
    ("fetch_or", AtomicOp::FetchOr),
    ("fetch_xor", AtomicOp::FetchXor),
];

/// A program that runs every operation of `Atomic[k]` over edge operands,
/// mixing every returned value and the final contents, and its model value.
fn program(k: IntegerKind) -> (String, u64) {
    let kn = k.name();
    let mut body = String::new();
    let mut acc = OFFSET;
    let mut cell = k.wrap(0x1234_5678_9abc_def1);
    let es = edges(k);
    for (step, p) in es.iter().enumerate() {
        let v = k.wrap(i128::from(*p));
        for (i, (name, op)) in RMW.iter().enumerate() {
            let order = ORDERS[(step + i) % ORDERS.len()];
            body += &format!(
                "    acc = mix(acc, wrapping_u64(c.{name}(wrapping_{kn}({p}), MemoryOrder::{order})));\n"
            );
            acc = mix(acc, cell as u64);
            cell = op.rmw(k, cell, v).unwrap();
        }
        // A failing and a succeeding compare-exchange.
        let other = es[(step + 1) % es.len()];
        for current in [k.wrap(cell + 1), cell] {
            let current = current as u64 & mask(k);
            body += &format!(
                "    acc = cas(acc, in c, wrapping_{kn}({current}), wrapping_{kn}({other}));\n"
            );
            let hit = k.wrap(i128::from(current)) == cell;
            acc = mix(mix(acc, u64::from(hit)), cell as u64);
            if hit {
                cell = k.wrap(i128::from(other));
            }
        }
        body += &format!("    c.store(wrapping_{kn}({p}), MemoryOrder::Release);\n");
        body += "    acc = mix(acc, wrapping_u64(c.load(MemoryOrder::Acquire)));\n";
        cell = v;
        acc = mix(acc, cell as u64);
    }
    let source = format!(
        "fn mix(acc: u64, v: u64) -> u64 {{ wrapping_add(wrapping_mul(acc, 1099511628211), v) }}
fn cas(acc: u64, in c: Atomic[{kn}], current: {kn}, new: {kn}) -> u64 {{
    match c.compare_exchange(current, new, MemoryOrder::AcqRel, MemoryOrder::Acquire) {{
        Result::Ok(p) => mix(mix(acc, 1), wrapping_u64(p)),
        Result::Err(p) => mix(mix(acc, 0), wrapping_u64(p)),
    }}
}}
fn main() -> u64 {{
    let c: Atomic[{kn}] = Atomic::new(wrapping_{kn}({}));
    let mut acc: u64 = {OFFSET};
{body}    mix(acc, wrapping_u64(c.into_inner()))
}}
",
        0x1234_5678_9abc_def1u64 & mask(k)
    );
    acc = mix(acc, cell as u64);
    (source, acc)
}

fn mask(k: IntegerKind) -> u64 {
    if k.bits() == 64 {
        u64::MAX
    } else {
        (1 << k.bits()) - 1
    }
}

#[test]
fn the_model_matches_std_atomics() {
    for a in [0u8, 1, 0x7f, 0x80, 0xff] {
        for b in [0u8, 1, 0x80, 0xff, 0x55] {
            let k = IntegerKind::U8;
            for (_, op) in RMW {
                let cell = atomic::AtomicU8::new(a);
                let r = match op {
                    AtomicOp::Swap => cell.swap(b, Ordering::SeqCst),
                    AtomicOp::FetchAdd => cell.fetch_add(b, Ordering::SeqCst),
                    AtomicOp::FetchSub => cell.fetch_sub(b, Ordering::SeqCst),
                    AtomicOp::FetchAnd => cell.fetch_and(b, Ordering::SeqCst),
                    AtomicOp::FetchOr => cell.fetch_or(b, Ordering::SeqCst),
                    AtomicOp::FetchXor => cell.fetch_xor(b, Ordering::SeqCst),
                    _ => unreachable!(),
                };
                assert_eq!(r, a);
                let model = op.rmw(k, a.into(), b.into()).unwrap();
                assert_eq!(model, i128::from(cell.into_inner()), "{op:?} {a} {b}");
            }
            let (sa, sb) = (a as i8, b as i8);
            for (_, op) in RMW {
                let cell = atomic::AtomicI8::new(sa);
                match op {
                    AtomicOp::Swap => cell.swap(sb, Ordering::SeqCst),
                    AtomicOp::FetchAdd => cell.fetch_add(sb, Ordering::SeqCst),
                    AtomicOp::FetchSub => cell.fetch_sub(sb, Ordering::SeqCst),
                    AtomicOp::FetchAnd => cell.fetch_and(sb, Ordering::SeqCst),
                    AtomicOp::FetchOr => cell.fetch_or(sb, Ordering::SeqCst),
                    AtomicOp::FetchXor => cell.fetch_xor(sb, Ordering::SeqCst),
                    _ => unreachable!(),
                };
                let model = op.rmw(IntegerKind::I8, sa.into(), sb.into()).unwrap();
                assert_eq!(model, i128::from(cell.into_inner()), "{op:?} {sa} {sb}");
            }
        }
    }
}

#[test]
fn every_integer_kind_agrees_with_the_model() {
    for k in INTEGER_KINDS {
        let (source, expected) = program(k);
        differential(&format!("atomic_{}", k.name()), &source, expected);
    }
}

#[test]
fn bool_cells_agree_with_the_model() {
    let source = "fn bit(b: bool) -> u64 { if b { 1 } else { 0 } }
fn main() -> u64 {
    let f: Atomic[bool] = Atomic::new(false);
    let a = bit(f.fetch_or(true, MemoryOrder::Relaxed));
    let b = bit(f.fetch_xor(true, MemoryOrder::AcqRel));
    let c = bit(f.swap(true, MemoryOrder::SeqCst));
    let d = bit(f.fetch_and(false, MemoryOrder::Release));
    let e: u64 = match f.compare_exchange(false, true, MemoryOrder::SeqCst, MemoryOrder::Relaxed) {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
    let g: u64 = match f.compare_exchange(false, true, MemoryOrder::SeqCst, MemoryOrder::SeqCst) {
        Result::Ok(_) => 1,
        Result::Err(_) => 0,
    };
    f.store(false, MemoryOrder::Relaxed);
    let h = bit(f.load(MemoryOrder::Acquire));
    wrapping_add(wrapping_add(wrapping_add(a, wrapping_mul(b, 2)), wrapping_add(wrapping_mul(c, 4), wrapping_mul(d, 8))),
        wrapping_add(wrapping_add(wrapping_mul(e, 16), wrapping_mul(g, 32)), wrapping_mul(h, 64)))
}
";
    // false |= true -> old 0; true ^= true -> old 1; swap true over false -> 0;
    // true &= false -> old 1; cas false->true succeeds; second fails; load false.
    differential("atomic_bool", source, 2 + 8 + 16);
}

/// `Atomic[T]` in the native layout, built by the host: the struct word is
/// the sequence handle, the handle points at `{len, cap, data}`, and the
/// one element is at offset 0 of `data`.
struct HostCell {
    _header: Box<[u64; 3]>,
    word: Box<u64>,
}
impl HostCell {
    fn new(element: *const u8) -> Self {
        let header = Box::new([1, 1, element as u64]);
        let word = Box::new(&*header as *const [u64; 3] as u64);
        Self {
            _header: header,
            word,
        }
    }
    fn address(&self) -> u64 {
        &*self.word as *const u64 as u64
    }
}

const CONTENDED: &str = "fn bump_relaxed(in c: Atomic[u64], n: u64) -> u64 {
    for i in 0..n { c.fetch_add(1, MemoryOrder::Relaxed); }
    0
}
fn bump_acqrel(in c: Atomic[u64], n: u64) -> u64 {
    for i in 0..n { c.fetch_add(1, MemoryOrder::AcqRel); }
    0
}
fn bump_seqcst(in c: Atomic[u64], n: u64) -> u64 {
    for i in 0..n { c.fetch_sub(wrapping_u64(-1), MemoryOrder::SeqCst); }
    0
}
fn bump_release(in c: Atomic[u64], n: u64) -> u64 {
    for i in 0..n { c.fetch_add(1, MemoryOrder::Release); }
    0
}
fn bump_acquire(in c: Atomic[u64], n: u64) -> u64 {
    for i in 0..n { c.fetch_add(1, MemoryOrder::Acquire); }
    0
}
// A compare-exchange loop: retries until its increment lands.
fn bump_cas(in c: Atomic[u64], n: u64) -> u64 {
    let mut retries: u64 = 0;
    for i in 0..n {
        let mut done = false;
        loop {
            if done { break; }
            let seen = c.load(MemoryOrder::Relaxed);
            match c.compare_exchange(seen, wrapping_add(seen, 1), MemoryOrder::AcqRel, MemoryOrder::Relaxed) {
                Result::Ok(_) => { done = true; },
                Result::Err(_) => { retries = wrapping_add(retries, 1); },
            }
        }
    }
    retries
}
fn bump_u8(in c: Atomic[u8], n: u64) -> u64 {
    for i in 0..n { c.fetch_add(1, MemoryOrder::Relaxed); }
    widen_u64(c.load(MemoryOrder::Relaxed))
}
fn toggle(in c: Atomic[bool], n: u64) -> u64 {
    for i in 0..n { c.fetch_xor(true, MemoryOrder::AcqRel); }
    0
}
// Every entry is reachable from `main`, so `vesper emit --emit clif` shows it.
fn main() -> u64 {
    let c: Atomic[u64] = Atomic::new(0);
    let b: Atomic[u8] = Atomic::new(0);
    let f: Atomic[bool] = Atomic::new(false);
    bump_relaxed(in c, 1);
    bump_acqrel(in c, 1);
    bump_seqcst(in c, 1);
    bump_release(in c, 1);
    bump_acquire(in c, 1);
    bump_cas(in c, 1);
    bump_u8(in b, 1);
    toggle(in f, 1);
    c.into_inner()
}
";

const THREADS: u64 = 4;
const ROUNDS: u64 = 20_000;

fn contend(world: &vesper::check::World, entry: &str, optimize: bool, address: u64) -> Vec<u64> {
    let f = native::WordFunction::compile(world, entry, optimize)
        .unwrap_or_else(|e| panic!("{entry}: {e}"));
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let f = &f;
                // SAFETY: `address` is a live host-built Atomic cell whose
                // element is an `AtomicU64`/`AtomicU8`/`AtomicBool` the
                // host only reads after the scope joins.
                s.spawn(move || unsafe { f.call(&[address, ROUNDS]) }.unwrap())
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

#[test]
fn counters_under_contention_lose_no_update() {
    let c = accepted(CONTENDED);
    for optimize in [true, false] {
        for entry in [
            "bump_relaxed",
            "bump_acqrel",
            "bump_seqcst",
            "bump_release",
            "bump_acquire",
            "bump_cas",
        ] {
            let element = AtomicU64::new(7);
            let cell = HostCell::new(&element as *const AtomicU64 as *const u8);
            contend(&c.world, entry, optimize, cell.address());
            assert_eq!(
                element.load(Ordering::SeqCst),
                7 + THREADS * ROUNDS,
                "{entry} (optimize {optimize})"
            );
        }
        let element = atomic::AtomicU8::new(250);
        let cell = HostCell::new(&element as *const atomic::AtomicU8 as *const u8);
        contend(&c.world, "bump_u8", optimize, cell.address());
        assert_eq!(
            u64::from(element.load(Ordering::SeqCst)),
            (250 + THREADS * ROUNDS) % 256,
            "u8 (optimize {optimize})"
        );
        let element = atomic::AtomicBool::new(true);
        let cell = HostCell::new(&element as *const atomic::AtomicBool as *const u8);
        contend(&c.world, "toggle", optimize, cell.address());
        // An even number of toggles leaves the flag as it was.
        assert!(element.load(Ordering::SeqCst), "bool (optimize {optimize})");
    }
}

#[test]
fn optimized_native_code_holds_one_atomic_per_operation() {
    let c = accepted(CONTENDED);
    assert_eq!(native::jit(&c.world, "main").unwrap().to_string(), "6");
    let temp = Temp::new();
    let file = temp.0.join("contended.ves");
    fs::write(&file, CONTENDED).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vesper"))
        .args(["emit", file.to_str().unwrap(), "--emit", "clif"])
        .output()
        .unwrap();
    let clif = String::from_utf8_lossy(&out.stdout);
    drop(c);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    for needle in [
        "atomic_rmw.i64 add",
        "atomic_rmw.i64 sub",
        "atomic_rmw.i8 add",
        "atomic_rmw.i8 xor",
        "atomic_cas.i64",
        "atomic_load.i64",
    ] {
        assert!(clif.contains(needle), "missing {needle} in\n{clif}");
    }
}

#[test]
fn every_allowed_order_is_accepted() {
    let mut body = String::new();
    for o in ORDERS {
        body +=
            &format!("    c.fetch_add(1, MemoryOrder::{o});\n    c.swap(1, MemoryOrder::{o});\n");
        if !matches!(o, "Release" | "AcqRel") {
            body += &format!("    c.load(MemoryOrder::{o});\n");
        }
        if !matches!(o, "Acquire" | "AcqRel") {
            body += &format!("    c.store(1, MemoryOrder::{o});\n");
        }
        for f in ORDERS {
            if !matches!(f, "Release" | "AcqRel") {
                body +=
                    &format!("    c.compare_exchange(1, 2, MemoryOrder::{o}, MemoryOrder::{f});\n");
            }
        }
    }
    accepted(&format!(
        "fn main() -> u64 {{\n    let c: Atomic[u64] = Atomic::new(0);\n{body}    c.into_inner()\n}}\n"
    ));
}

#[test]
fn invalid_orders_kinds_and_the_private_cell_are_refused() {
    let wrap = |body: &str| {
        format!(
            "fn main() -> u64 {{\n    let c: Atomic[u64] = Atomic::new(5);\n{body}\n    0\n}}\n"
        )
    };
    rejects(
        &wrap("    c.load(MemoryOrder::Release);"),
        "E0820",
        "'load' cannot use 'MemoryOrder::Release'",
    );
    rejects(
        &wrap("    c.load(MemoryOrder::AcqRel);"),
        "E0820",
        "'load' cannot use 'MemoryOrder::AcqRel'",
    );
    rejects(
        &wrap("    c.store(1, MemoryOrder::Acquire);"),
        "E0820",
        "'store' cannot use 'MemoryOrder::Acquire'",
    );
    rejects(
        &wrap("    c.compare_exchange(1, 2, MemoryOrder::SeqCst, MemoryOrder::Release);"),
        "E0820",
        "failure order of 'compare_exchange'",
    );
    rejects(
        &wrap("    let o = MemoryOrder::Relaxed;\n    c.fetch_add(1, o);"),
        "E0820",
        "must be a literal 'MemoryOrder'",
    );
    rejects(
        &wrap("    Atomic::load(in c, MemoryOrder::AcqRel);"),
        "E0820",
        "'load' cannot use",
    );
    rejects(
        "fn main() -> u64 {\n    let f: Atomic[bool] = Atomic::new(false);\n    f.fetch_add(true, MemoryOrder::Relaxed);\n    0\n}\n",
        "E0821",
        "'fetch_add' is not defined on 'Atomic[bool]'",
    );
    rejects(
        "struct P { x: u64 }\nfn main() -> u64 {\n    let p: Atomic[P] = Atomic::new(P { x: 1 });\n    0\n}\n",
        "E0821",
        "'Atomic[P]' has no atomic operations",
    );
    rejects(
        &wrap("    let n = c.cell.len();"),
        "E0822",
        "private to its operations",
    );
    rejects(
        "fn main() -> u64 {\n    let c = Atomic { cell: ChunkSeq::new[u64]() };\n    0\n}\n",
        "E0822",
        "built by 'Atomic::new'",
    );
}

#[test]
fn an_atomic_cell_is_never_copy() {
    // Two owners of one cell would each free it.
    let c = checked(
        "fn main() -> u64 {\n    let c: Atomic[u64] = Atomic::new(5);\n    let d = c;\n    c.load(MemoryOrder::Relaxed)\n}\n",
    );
    assert!(!c.diagnostics.is_empty(), "a moved Atomic was used again");
}

#[test]
fn a_user_atomic_type_takes_precedence() {
    accepted(
        "struct Atomic { v: u64 }\nfn main() -> u64 {\n    let a = Atomic { v: 3 };\n    a.v\n}\n",
    );
}

#[test]
fn the_verified_backend_refuses_atomics() {
    let c = accepted(
        "fn main() -> u64 {\n    let c: Atomic[u64] = Atomic::new(5);\n    c.fetch_add(1, MemoryOrder::Relaxed);\n    c.into_inner()\n}\n",
    );
    let verified = vesper::verified_codegen::compile(
        &c.world,
        "main",
        &vesper::verified_codegen::Options::new(vesper::verified_codegen::Target::X86_64_LINUX),
    );
    let Err(error) = verified else {
        panic!("the verified backend compiled an atomic");
    };
    assert!(format!("{error:?}").contains("Unsupported"), "{error:?}");
}

/// `cargo test --release --test atomics -- --ignored --nocapture`: a
/// contended `fetch_add` (four host threads) and an uncontended one, in
/// optimized compiled Vesper and in the same Rust loop.
#[test]
#[ignore]
fn measure_fetch_add_against_rust() {
    const N: u64 = 2_000_000;
    let c = accepted(CONTENDED);
    let f = native::WordFunction::compile(&c.world, "bump_relaxed", true).unwrap();
    #[inline(never)]
    fn rust_bump(c: &AtomicU64, n: u64) -> u64 {
        for _ in 0..n {
            c.fetch_add(1, Ordering::Relaxed);
        }
        0
    }
    for threads in [1u64, 4] {
        for round in 0..3 {
            let element = AtomicU64::new(0);
            let cell = HostCell::new(&element as *const AtomicU64 as *const u8);
            let address = cell.address();
            let start = std::time::Instant::now();
            std::thread::scope(|s| {
                for _ in 0..threads {
                    let f = &f;
                    // SAFETY: as in `contend`.
                    s.spawn(move || unsafe { f.call(&[address, N]) }.unwrap());
                }
            });
            let vesper = start.elapsed();
            assert_eq!(element.load(Ordering::SeqCst), threads * N);
            let element = AtomicU64::new(0);
            let start = std::time::Instant::now();
            std::thread::scope(|s| {
                for _ in 0..threads {
                    let e = &element;
                    s.spawn(move || rust_bump(std::hint::black_box(e), N));
                }
            });
            let rust = start.elapsed();
            assert_eq!(element.load(Ordering::SeqCst), threads * N);
            println!(
                "fetch_add x{N} on {threads} thread(s), round {round}: vesper {:.2} ns/op, rust {:.2} ns/op, ratio {:.2}",
                vesper.as_nanos() as f64 / (threads * N) as f64,
                rust.as_nanos() as f64 / (threads * N) as f64,
                vesper.as_secs_f64() / rust.as_secs_f64()
            );
        }
    }
}
