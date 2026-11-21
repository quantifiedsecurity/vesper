//! Self-hosting SH4 lane 11 (#1285), part 2: the vector operations
//! (`Op::Vector`, `Codegen::vector_op` of src/native_vector.rs) emitted by
//! `@vesper/codegen-numeric` through the `numeric` hook of
//! `vesper::codegen_swap` (lane 1, #1275).
//!
//! A generated corpus covers every lanes intrinsic on every element type and
//! every shape. Each program is compiled by the native backend with no hook
//! and with the Vesper hook, with the optimizer on and off; every function's
//! CLIF text, the object bytes and the JIT result must be equal
//! (tests/common/clif_diff.rs).
use std::sync::OnceLock;
use std::sync::atomic::Ordering;
use vesper::codegen_swap::{self, CodegenHooks, INSTRUCTION, Lent};
use vesper::ffi_lowering::{self, ForeignChecked, Handler, Imports};
use vesper::ir::Op;
use vesper::native::WordFunction;
use vesper::vector::{
    BinOp, CmpOp, Conv, Elem, Intrinsic, MaskOp, ReduceOp, Shape, ShiftOp, UnaryOp, VectorOp,
};
use vesper_codegen_swap_test::*;

/// Package files in assembly order after `@vesper/cranelift` (shim gap 4:
/// a package cannot import a `.vsys` library through the JIT harness).
const FILES: [&str; 3] = ["rt.ves", "shapes.ves", "vector.ves"];

/// `vector.ves`'s requests.
const NUM_VECTOR: u32 = 1;
const NUM_ISA_SUPPORTED: u32 = 2;

fn repo() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for base in [manifest.join("../.."), std::path::PathBuf::from(".")] {
        if base
            .join("packages/vesper-codegen-numeric/src/vector.ves")
            .exists()
        {
            return base;
        }
    }
    panic!("cannot find packages/vesper-codegen-numeric");
}

/// The `sys library` of `@vesper/cranelift`'s `// @export vsys` directive.
fn vsys(source: &str, target: (&str, &str)) -> String {
    let mut out = String::new();
    for line in source.lines() {
        if let Some(rest) = line.strip_prefix("// | ") {
            out.push_str(rest);
            out.push('\n');
        } else if line == "// |" {
            out.push('\n');
        }
    }
    assert!(out.contains("sys library Cranelift"), "no .vsys directive");
    out.replace("@TARGET@", target.0).replace("@ABI@", target.1)
}

fn host_abi() -> (&'static str, &'static str) {
    let target = host_target().expect("a host the shim supports");
    let abi = if target.ends_with("windows-msvc") {
        "win64"
    } else if target.ends_with("apple-darwin") {
        "darwin64"
    } else if target.starts_with("aarch64") {
        "aapcs64"
    } else {
        "sysv64"
    };
    (target, abi)
}

/// The hook, JIT-compiled once with no hook in effect (a hook compiled
/// under `with_hooks` would call itself while its `OnceLock` initializes).
fn hook() -> &'static WordFunction {
    static HOOK: OnceLock<WordFunction> = OnceLock::new();
    HOOK.get_or_init(|| {
        codegen_swap::with_hooks(CodegenHooks::default(), || {
            let repo = repo();
            let shim = std::fs::read_to_string(repo.join("packages/vesper-cranelift/src/lib.ves"))
                .unwrap();
            let mut source = shim.clone();
            for f in FILES {
                source.push('\n');
                source.push_str(
                    &std::fs::read_to_string(
                        repo.join("packages/vesper-codegen-numeric/src").join(f),
                    )
                    .unwrap(),
                );
            }
            let mut imports = Imports::new();
            imports
                .add(&vsys(&shim, host_abi()), Handler::Direct)
                .unwrap();
            let checked: ForeignChecked =
                ffi_lowering::check_source(&source, &imports).unwrap_or_else(|e| panic!("{e}"));
            hook_function(&checked, "numeric_hook")
        })
    })
}

// ------------------------------------------------------------- the adapter

/// `shapes.ves`'s element codes.
fn elem_code(e: Elem) -> u64 {
    match e {
        Elem::U8 => 0,
        Elem::U32 => 1,
        Elem::U64 => 2,
        Elem::I64 => 3,
        Elem::F32 => 4,
        Elem::F64 => 5,
    }
}

/// `shapes.ves`'s intrinsic code `family << 8 | op`, or `None` for the
/// slice operations and `Isa::supported` (not lanes requests).
fn intrinsic_code(i: Intrinsic) -> Option<u64> {
    use Intrinsic as I;
    let (family, op): (u64, u64) = match i {
        I::Splat => (2, 0),
        I::From => (3, 0),
        I::Binary(op) => (
            4,
            match op {
                BinOp::Add => 0,
                BinOp::Sub => 1,
                BinOp::Mul => 2,
                BinOp::Div => 3,
                BinOp::Min => 4,
                BinOp::Max => 5,
                BinOp::And => 6,
                BinOp::Or => 7,
                BinOp::Xor => 8,
                BinOp::SaturatingAdd => 9,
                BinOp::SaturatingSub => 10,
            },
        ),
        I::Not => (5, 0),
        I::Unary(op) => (
            6,
            match op {
                UnaryOp::Sqrt => 0,
                UnaryOp::Abs => 1,
                UnaryOp::Neg => 2,
            },
        ),
        I::MulAdd => (7, 0),
        I::Convert(c) => (
            8,
            match c {
                Conv::ToBits => 0,
                Conv::FromBits => 1,
                Conv::Widen => 2,
                Conv::Narrow => 3,
            },
        ),
        I::Shift(op) => (
            9,
            match op {
                ShiftOp::Shl => 0,
                ShiftOp::Shr => 1,
            },
        ),
        I::Compare(op) => (
            10,
            match op {
                CmpOp::Eq => 0,
                CmpOp::Ne => 1,
                CmpOp::Lt => 2,
                CmpOp::Le => 3,
                CmpOp::Gt => 4,
                CmpOp::Ge => 5,
            },
        ),
        I::Select => (11, 0),
        I::MaskBinary(op) => (
            12,
            match op {
                MaskOp::And => 0,
                MaskOp::Or => 1,
                MaskOp::Xor => 2,
            },
        ),
        I::MaskNot => (13, 0),
        I::MaskCount => (14, 0),
        I::MaskBitmask => (15, 0),
        I::MaskAny => (16, 0),
        I::MaskAll => (17, 0),
        I::Reduce(op) => (
            18,
            match op {
                ReduceOp::Add => 0,
                ReduceOp::Xor => 1,
                ReduceOp::Or => 2,
                ReduceOp::And => 3,
                ReduceOp::Min => 4,
                ReduceOp::Max => 5,
            },
        ),
        I::Extract => (19, 0),
        I::Replace => (20, 0),
        I::Shuffle => (21, 0),
        I::LoadMasked => (22, 0),
        I::InBounds => (26, 0),
        I::Active => (27, 0),
        I::Load | I::Store | I::StoreMasked | I::IsaSupported => return None,
    };
    Some(family << 8 | op)
}

/// The request of one `Op::Vector` instruction: `(op, argument words)`.
fn encode(lent: &Lent<'_, '_>, op: &VectorOp) -> Option<(u32, Vec<u64>)> {
    match op {
        VectorOp::IsaSupported(p) => {
            let enabled = vesper::vector::isa_enabled(lent.module.isa(), *p);
            Some((NUM_ISA_SUPPORTED, vec![u64::from(enabled)]))
        }
        VectorOp::Lanes {
            intrinsic,
            shape,
            constants,
        } => {
            let mut words = vec![
                intrinsic_code(*intrinsic)?,
                elem_code(shape.elem),
                u64::from(shape.lanes),
                constants.len() as u64,
            ];
            words.extend(constants.iter().map(|c| u64::from(*c)));
            words.extend_from_slice(lent.args);
            Some((NUM_VECTOR, words))
        }
    }
}

/// The `numeric` hook: an `Op::Vector` instruction goes to the Vesper
/// emission; everything else is declined.
fn numeric(lent: &mut Lent<'_, '_>) -> Result<bool, String> {
    if lent.op != INSTRUCTION {
        return Ok(false);
    }
    let inst = lent
        .program
        .functions
        .get(lent.site.function)
        .and_then(|f| f.blocks.get(lent.site.block as usize))
        .and_then(|b| b.instructions.get(lent.site.inst as usize))
        .ok_or("numeric hook: no instruction at the site")?;
    let Op::Vector { op, .. } = &inst.op else {
        return Ok(false);
    };
    let Some((request, words)) = encode(lent, op) else {
        return Ok(false);
    };
    let f = hook();
    let (answer, crossing) = vesper_cranelift_sys::lend(
        lent.builder,
        &mut *lent.module,
        request,
        &words,
        // SAFETY: the hook takes one word, the lent call's address.
        |call| unsafe { f.call(&[call]) },
    );
    let answer = answer?;
    if let Some(e) = crossing.error {
        return Err(format!("numeric hook: {e}"));
    }
    if crossing.unreturned {
        return Err("numeric hook kept a lent handle".into());
    }
    if answer == 1 {
        HANDLED.fetch_add(1, Ordering::Relaxed);
        lent.results.extend(crossing.results);
        Ok(true)
    } else {
        DECLINED.fetch_add(1, Ordering::Relaxed);
        Ok(false)
    }
}

fn hooks() -> CodegenHooks {
    CodegenHooks {
        numeric: Some(numeric),
        ..CodegenHooks::default()
    }
}

// --------------------------------------------------------------- the corpus

const ELEMS: [Elem; 6] = [
    Elem::U8,
    Elem::U32,
    Elem::U64,
    Elem::I64,
    Elem::F32,
    Elem::F64,
];

fn literal(e: Elem, i: usize) -> String {
    if e.is_float() {
        format!("{}.5", i + 1)
    } else if e == Elem::U8 {
        format!("{}", (i * 37 + 3) % 251)
    } else {
        format!("{}", i * 1_000_003 + 7)
    }
}

/// `Vec::from[T, N](...)` with distinct lanes.
fn vector(e: Elem, n: u16, salt: usize) -> String {
    let lanes: Vec<String> = (0..n as usize).map(|i| literal(e, i * 3 + salt)).collect();
    format!("Vec::from[{}, {n}]({})", e.name(), lanes.join(", "))
}

/// One program per operation and shape: `k` applies the operation to its
/// parameters (so nothing is folded), `main` calls it.
fn programs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for e in ELEMS {
        let t = e.name();
        for bits in [128u16, 256, 512] {
            let n = bits / e.bits() as u16;
            let Some(shape) = Shape::new(e, n) else {
                continue;
            };
            let profile = shape.required_profile().name();
            let v = format!("Vec[{t}, {n}]");
            let last = n - 1;
            let x = literal(e, 5);
            // (name, result type, body over a, b, c and the scalar x)
            let mut ops: Vec<(String, String, String)> = Vec::new();
            let lane = |expr: &str| format!("Vec::extract({expr}, {last})");
            let bits_of = |expr: &str| format!("Mask::bitmask({expr})");
            ops.push((
                "splat".into(),
                t.into(),
                lane(&format!("Vec::splat[{t}, {n}](x)")),
            ));
            ops.push(("from".into(), t.into(), lane(&vector(e, n, 1))));
            for op in [
                "add",
                "sub",
                "mul",
                "div",
                "min",
                "max",
                "and",
                "or",
                "xor",
                "saturating_add",
                "saturating_sub",
            ] {
                ops.push((op.into(), t.into(), lane(&format!("Vec::{op}(a, b)"))));
            }
            for op in ["not", "sqrt", "abs", "neg"] {
                ops.push((op.into(), t.into(), lane(&format!("Vec::{op}(a)"))));
            }
            ops.push(("mul_add".into(), t.into(), lane("Vec::mul_add(a, b, c)")));
            for op in ["to_bits", "from_bits", "widen_f64", "nearest_f32"] {
                ops.push((
                    op.into(),
                    "u64".into(),
                    format!("Mask::count(Vec::eq(Vec::{op}(a), Vec::{op}(b)))"),
                ));
            }
            for op in ["shl", "shr"] {
                ops.push((op.into(), t.into(), lane(&format!("Vec::{op}(a, 3)"))));
            }
            for op in ["eq", "ne", "lt", "le", "gt", "ge"] {
                ops.push((
                    op.into(),
                    "u64".into(),
                    bits_of(&format!("Vec::{op}(a, b)")),
                ));
            }
            ops.push((
                "select".into(),
                t.into(),
                lane("Vec::select(Vec::lt(a, b), a, c)"),
            ));
            for op in ["and", "or", "xor"] {
                ops.push((
                    format!("mask_{op}"),
                    "u64".into(),
                    bits_of(&format!("Mask::{op}(Vec::lt(a, b), Vec::lt(c, a))")),
                ));
            }
            ops.push((
                "mask_not".into(),
                "u64".into(),
                bits_of("Mask::not(Vec::lt(a, b))"),
            ));
            for op in ["count", "any", "all"] {
                let r = if op == "count" { "u64" } else { "bool" };
                ops.push((
                    format!("mask_{op}"),
                    r.into(),
                    format!("Mask::{op}(Vec::lt(a, b))"),
                ));
            }
            for op in ["add", "xor", "or", "and", "min", "max"] {
                ops.push((
                    format!("reduce_{op}"),
                    t.into(),
                    format!("Vec::reduce_{op}(a)"),
                ));
            }
            ops.push((
                "extract".into(),
                t.into(),
                format!("Vec::extract(a, {})", n / 2),
            ));
            ops.push((
                "replace".into(),
                t.into(),
                lane(&format!("Vec::replace(a, {last}, x)")),
            ));
            let reversed: Vec<String> = (0..n).rev().map(|i| i.to_string()).collect();
            ops.push((
                "shuffle".into(),
                t.into(),
                lane(&format!("Vec::shuffle(a, {})", reversed.join(", "))),
            ));
            ops.push((
                "load_masked".into(),
                t.into(),
                lane(&format!(
                    "Vec::load_masked[{t}, {n}](\"needle in a haystack, twice over and more\", 3)"
                )),
            ));
            ops.push((
                "in_bounds".into(),
                "u64".into(),
                bits_of(&format!(
                    "Mask::in_bounds[{t}, {n}](\"needle in a haystack\", 5)"
                )),
            ));
            ops.push((
                "active".into(),
                "u64".into(),
                bits_of(&format!("Mask::active[{t}, {n}](2, {})", n / 2 + 1)),
            ));
            for (name, result, body) in ops {
                // `main` handles the profile by checked dispatch, so a
                // host without it runs the fallback.
                let fallback = match result.as_str() {
                    "bool" => "false",
                    "f32" | "f64" => "0.0",
                    _ => "0",
                };
                let source = format!(
                    "fn k(a: {v}, b: {v}, c: {v}, x: {t}) -> {result} eff [Isa[{profile}]] {{ {body} }}\n\
                     fn main() -> {result} {{ if Isa::supported[{profile}]() {{ k({}, {}, {}, {x}) }} else {{ {fallback} }} }}\n",
                    vector(e, n, 0),
                    vector(e, n, 2),
                    vector(e, n, 4),
                );
                out.push((format!("{t}x{n} {name}"), source));
            }
        }
    }
    // `Isa::supported` of every profile.
    for p in ["Baseline128", "Avx2", "Avx512", "Sve2", "Rvv1"] {
        out.push((
            format!("isa {p}"),
            format!("fn main() -> u64 {{ if Isa::supported[{p}]() {{ 1 }} else {{ 2 }} }}\n"),
        ));
    }
    out
}

fn accepted(source: &str) -> Option<vesper::check::World> {
    let parsed = vesper::parser::parse(source);
    if !parsed.diagnostics.is_empty() {
        return None;
    }
    let c = vesper::check::check(&parsed.module);
    c.diagnostics.is_empty().then_some(c.world)
}

/// The operations the corpus must compile for at least one shape.
const EXPECTED: [&str; 54] = [
    "Avx2",
    "Avx512",
    "Baseline128",
    "Rvv1",
    "Sve2",
    "abs",
    "active",
    "add",
    "and",
    "div",
    "eq",
    "extract",
    "from",
    "from_bits",
    "ge",
    "gt",
    "in_bounds",
    "le",
    "load_masked",
    "lt",
    "mask_all",
    "mask_and",
    "mask_any",
    "mask_count",
    "mask_not",
    "mask_or",
    "mask_xor",
    "max",
    "min",
    "mul",
    "mul_add",
    "ne",
    "nearest_f32",
    "neg",
    "not",
    "or",
    "reduce_add",
    "reduce_and",
    "reduce_max",
    "reduce_min",
    "reduce_or",
    "reduce_xor",
    "replace",
    "saturating_add",
    "saturating_sub",
    "select",
    "shl",
    "shr",
    "shuffle",
    "splat",
    "sqrt",
    "sub",
    "to_bits",
    "xor",
];

/// The operation part of each program name (`u8x16 add` is `add`).
fn programs_names(names: &[String]) -> std::collections::BTreeSet<&str> {
    names
        .iter()
        .filter_map(|n| n.split_once(' ').map(|(_, op)| op))
        .collect()
}

#[test]
fn vector_operations_match_the_rust_lowering() {
    let _ = hook();
    let mut diff = Diff::default();
    let mut compiled = 0;
    let mut skipped = Vec::new();
    let mut compiled_names: Vec<String> = Vec::new();
    let handled_before = HANDLED.load(Ordering::Relaxed);
    for (name, source) in programs() {
        let Some(world) = accepted(&source) else {
            skipped.push(name);
            continue;
        };
        compiled += 1;
        compiled_names.push(name.clone());
        for optimize in [false, true] {
            diff.add(compare(&name, &world, "main", hooks(), optimize, true));
        }
    }
    let handled = HANDLED.load(Ordering::Relaxed) - handled_before;
    eprintln!(
        "vector differential: {compiled} programs ({} rejected by the checker), {}; {handled} requests from Vesper, {} declined",
        skipped.len(),
        diff.summary(),
        DECLINED.load(Ordering::Relaxed)
    );
    // Every operation must be compiled for at least one shape.
    let ops: std::collections::BTreeSet<&str> = programs_names(&compiled_names);
    eprintln!("operations compiled: {ops:?}");
    // `widen_f64` doubles the vector's bits, so no template shape checks;
    // the Vesper hook declines it anyway (no `fvpromote_low` in the shim).
    for op in EXPECTED {
        assert!(ops.contains(op), "no program compiled {op}");
    }
    assert!(diff.is_clean(), "{}", diff.summary());
    assert!(
        compiled > 300,
        "only {compiled} programs checked: {skipped:?}"
    );
    assert!(handled > 0);
}
