//! Self-hosting SH4 lane 15 (#1289): `@vesper/wasm-backend` against the
//! Rust wasm backend (src/wasm_backend).
#[path = "common/check_harness.rs"]
mod check_harness;
#[path = "common/check_words.rs"]
mod check_words;
#[path = "common/ir_words.rs"]
mod ir_words;

use check_harness::*;
use check_words::{Enc, pass_word};
use ir_words::{Payload, Tables};
use std::collections::BTreeMap;
use std::sync::OnceLock;
use vesper::ast::{Effect, Effects, Expr, ExprKind, Function, Literal, Pattern, PatternKind};
use vesper::check::World;
use vesper::ir::Program;
use vesper::wasm_backend::{self, EffectLowering, WasmError, WasmOptions, WasmWorld};

/// One program the backend is asked to compile.
pub struct WasmCase {
    pub name: String,
    pub world: World,
}

/// The program literals of tests/wasm_backend.rs (every string literal
/// that defines `main` and checks), then every corpus world with a `main`.
pub fn cases() -> &'static [WasmCase] {
    static C: OnceLock<Vec<WasmCase>> = OnceLock::new();
    C.get_or_init(|| {
        let mut out = vec![];
        let src = std::fs::read_to_string(repo().join("tests/wasm_backend.rs")).unwrap();
        for (i, lit) in string_literals(&src).into_iter().enumerate() {
            if !lit.contains("fn main") {
                continue;
            }
            let p = vesper::parser::parse(&lit);
            if !p.diagnostics.is_empty() {
                continue;
            }
            let c = vesper::check::check(&p.module);
            if c.diagnostics.is_empty() {
                out.push(WasmCase {
                    name: format!("tests/wasm_backend.rs#{i}"),
                    world: c.world,
                });
            }
        }
        for c in corpus() {
            if c.world.functions.contains_key("main") {
                out.push(WasmCase {
                    name: c.name.clone(),
                    world: c.world.clone(),
                });
            }
        }
        out
    })
}

/// The `"..."` and `r#"..."#` literals of a Rust source, unescaped.
fn string_literals(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b[i] == b'r' && b.get(i + 1) == Some(&b'#') && b.get(i + 2) == Some(&b'"') {
            let start = i + 3;
            let end = src[start..].find("\"#").map(|e| start + e).unwrap_or(b.len());
            out.push(src[start..end].to_string());
            i = end + 2;
            continue;
        }
        if b[i] == b'\'' && b.get(i + 2) == Some(&b'\'') {
            i += 3;
            continue;
        }
        if b[i] == b'"' {
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                    match b.get(i) {
                        Some(b'n') => s.push('\n'),
                        Some(b't') => s.push('\t'),
                        Some(b'\\') => s.push('\\'),
                        Some(b'"') => s.push('"'),
                        Some(b'\n') => {
                            while i + 1 < b.len() && (b[i + 1] == b' ' || b[i + 1] == b'\n') {
                                i += 1;
                            }
                        }
                        Some(c) => s.push(*c as char),
                        None => {}
                    }
                    i += 1;
                    continue;
                }
                let ch = src[i..].chars().next().unwrap();
                s.push(ch);
                i += ch.len_utf8();
            }
            out.push(s);
            i += 1;
            continue;
        }
        i += 1;
    }
    out
}

pub fn configs() -> [WasmOptions; 4] {
    let mk = |optimize, effects| WasmOptions {
        optimize,
        effects,
        ..WasmOptions::default()
    };
    [
        mk(true, EffectLowering::StackSwitching),
        mk(false, EffectLowering::StackSwitching),
        mk(true, EffectLowering::Portable),
        mk(false, EffectLowering::Portable),
    ]
}

fn error_class(e: &WasmError) -> String {
    match e {
        WasmError::Unsupported { construct, .. } => format!("Unsupported({construct:?})"),
        WasmError::Entry(_) => "Entry".into(),
        WasmError::Limit(_) => "Limit".into(),
        WasmError::Invalid(_) => "Invalid".into(),
        WasmError::Encode(_) => "Encode".into(),
    }
}

/// What the Rust backend accepts over `cases()` under each option set (the
/// report's census; about 5 min in a debug build).
#[test]
#[ignore]
fn rust_acceptance_census() {
    let mut classes: BTreeMap<String, usize> = BTreeMap::new();
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    for c in cases() {
        for (k, o) in configs().iter().enumerate() {
            let r = std::panic::catch_unwind(|| wasm_backend::compile(&c.world, "main", o));
            let key = match r {
                Ok(Ok(_)) => "ok".to_string(),
                Ok(Err(e)) => error_class(&e),
                Err(_) => "panic".into(),
            };
            if k == 0 && key == "ok" {
                eprintln!("ok: {}", c.name);
            }
            *classes.entry(format!("{k} {key}")).or_default() += 1;
        }
    }
    std::panic::set_hook(quiet);
    eprintln!("{} cases", cases().len());
    for (k, n) in classes {
        eprintln!("{k}: {n}");
    }
}

// ------------------------------------------------------------------ input

/// Words of the wasm section, interning names and types through `enc`.
struct Sec<'a> {
    enc: &'a mut Enc,
    out: Vec<u64>,
}

impl Sec<'_> {
    fn raw(&mut self, x: u64) {
        self.out.push(x);
    }
    fn name(&mut self, s: &str) {
        let i = self.enc.name(s);
        self.out.push(i);
    }
    fn opt_name(&mut self, s: &Option<String>) {
        let i = self.enc.opt_name(s);
        self.out.push(i);
    }
    fn ty(&mut self, t: &vesper::ast::Type) {
        let i = self.enc.ty(t);
        self.out.push(i);
    }
    fn names<'s>(&mut self, ns: impl ExactSizeIterator<Item = &'s String>) {
        self.raw(ns.len() as u64);
        for n in ns {
            self.name(n);
        }
    }
    fn effect(&mut self, e: &Effect) {
        self.opt_name(&e.label);
        self.name(&e.name);
        self.raw(e.args.len() as u64);
        for t in &e.args {
            self.ty(t);
        }
    }
    fn row(&mut self, e: &Effects) {
        self.raw(e.instances.len() as u64);
        for i in &e.instances {
            self.effect(i);
        }
        self.opt_name(&e.tail);
    }
    /// A signature: generics (name, row), params (name, pass, type),
    /// result, effect row.
    fn signature(&mut self, f: &Function) {
        self.raw(f.generics.len() as u64);
        for g in &f.generics {
            self.name(&g.name);
            self.raw(g.row as u64);
        }
        self.raw(f.params.len() as u64);
        for p in &f.params {
            self.name(&p.name);
            self.raw(pass_word(p.pass));
            self.ty(&p.ty);
        }
        self.ty(&f.result);
        self.row(&f.effects);
    }
    /// Kind (0 int lo hi, 1 float, 2 bool, 3 string), as in ir_words.
    fn literal(&mut self, l: &Literal) {
        match l {
            Literal::Int(n) => {
                self.raw(0);
                self.raw(*n as u64);
                self.raw((*n >> 64) as u64);
            }
            Literal::Float(x) => {
                self.raw(1);
                self.raw(x.to_bits());
                self.raw(0);
            }
            Literal::Bool(b) => {
                self.raw(2);
                self.raw(*b as u64);
                self.raw(0);
            }
            Literal::String(t) => {
                self.raw(3);
                self.name(t);
                self.raw(0);
            }
        }
    }
    /// A pattern in prefix order, as in ir_words: kind, span, parts.
    fn pattern(&mut self, p: &Pattern) {
        let (a, b) = (p.span.start as u64, p.span.end as u64);
        match &p.kind {
            PatternKind::Wild => self.out.extend([0, a, b]),
            PatternKind::Bind(n) => {
                self.out.extend([1, a, b]);
                self.name(n);
            }
            PatternKind::Literal(l) => {
                self.out.extend([2, a, b]);
                self.literal(l);
            }
            PatternKind::Tuple(ps) => {
                self.out.extend([3, a, b, ps.len() as u64]);
                for c in ps {
                    self.pattern(c);
                }
            }
            PatternKind::Constructor {
                name,
                fields,
                named,
            } => {
                self.out.extend([4, a, b]);
                self.name(name);
                self.raw(*named as u64);
                self.raw(fields.len() as u64);
                for (f, c) in fields {
                    self.name(f);
                    self.pattern(c);
                }
            }
        }
    }
    fn patterns(&mut self, ps: &[Pattern]) {
        self.raw(ps.len() as u64);
        for p in ps {
            self.pattern(p);
        }
    }
    /// The facts the backend reads from a closure or handler expression
    /// (the pool keeps the `Expr` on the Rust side).
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Closure { params, body, .. } => {
                let mut free = vesper::check::free_names(body);
                for p in params {
                    free.remove(&p.name);
                }
                self.raw(1);
                self.name(&vesper::ir::nested_name("closure", &body.span));
                self.names(free.iter());
            }
            ExprKind::Handle {
                effect,
                state,
                body,
                ret,
                ops,
            } => {
                self.raw(2);
                self.names(vesper::check::free_names(e).iter());
                self.effect(effect);
                self.raw(state.is_some() as u64);
                self.name(&vesper::ir::nested_name("handler_body", &body.span));
                self.name(&vesper::ir::nested_name("handler_clause", &ret.body.span));
                self.patterns(&ret.pats);
                self.names(vesper::check::clause_free_names(ret).iter());
                self.raw(ops.len() as u64);
                for c in ops {
                    self.name(&c.name);
                    self.name(&vesper::ir::nested_name("handler_clause", &c.body.span));
                    self.patterns(&c.pats);
                    self.names(vesper::check::clause_free_names(c).iter());
                }
            }
            _ => self.raw(0),
        }
    }
}

/// Every name a world's effect rows mention that is a capability effect.
fn capability_names(w: &World) -> Vec<String> {
    let mut out = std::collections::BTreeSet::new();
    let mut row = |e: &Effects| {
        for i in &e.instances {
            if vesper::capability_effects::is_capability_effect(&i.name) {
                out.insert(i.name.clone());
            }
        }
    };
    for f in w.functions.values() {
        row(&f.effects);
    }
    for d in w.effects.values() {
        for o in &d.ops {
            row(&o.effects);
        }
    }
    out.into_iter().collect()
}

/// The input of `wasm_backend_compile`: check-data's names, types and
/// world, the Place IR program section (tests/common/ir_words.rs), then
/// the wasm section:
///
/// ```text
/// options: world, stack switching, max pages, max functions, max instructions
/// entry name, trace active, n capability effect names
/// functions: n, n x (name, signature)
/// effects: n, n x (name, linear, ngen, ngen names, nops, nops x (name, signature))
/// calls: n, n x (start, end, k, k x (name, type))
/// exprs: one per pool row: 0 | 1 closure fn, free | 2 handler (see `Sec::expr`)
/// ```
pub fn wasm_input(w: &World, program: &Program, entry: &str, o: &WasmOptions) -> Option<Vec<u8>> {
    if !ir_words::encodable(w, program) {
        return None;
    }
    let mut t = Tables::of(program);
    let labels: Vec<String> = t
        .pool
        .items
        .iter()
        .map(|p| match p {
            Payload::Seq(x) => format!("{x:?}"),
            Payload::Vector(x) => format!("{x:?}"),
            Payload::Tiled(x) => format!("{x:?}"),
            Payload::Fip(x) => format!("{x:?}"),
            Payload::Expr(_) => String::new(),
        })
        .collect();
    for l in &labels {
        t.enc.name(l);
    }
    let mut section = t.section();
    let world = t.enc.world(w);
    let items = t.pool.items.clone();
    let mut s = Sec {
        enc: &mut t.enc,
        out: vec![],
    };
    s.raw(match o.world {
        WasmWorld::Command => 0,
        WasmWorld::NexusPlugin => 1,
    });
    s.raw((o.effects == EffectLowering::StackSwitching) as u64);
    s.raw(o.max_memory_pages as u64);
    s.raw(o.max_functions as u64);
    s.raw(o.max_instructions as u64);
    s.name(entry);
    s.raw(vesper::trace_effects::active(w) as u64);
    let caps = capability_names(w);
    s.names(caps.iter());
    s.raw(w.functions.len() as u64);
    for (name, f) in &w.functions {
        s.name(name);
        s.signature(f);
    }
    s.raw(w.effects.len() as u64);
    for (name, d) in &w.effects {
        s.name(name);
        s.raw(d.linear as u64);
        s.raw(d.generics.len() as u64);
        for g in &d.generics {
            s.name(&g.name);
        }
        s.raw(d.ops.len() as u64);
        for op in &d.ops {
            s.name(&op.name);
            s.signature(op);
        }
    }
    s.raw(w.calls.len() as u64);
    for ((a, b), vars) in &w.calls {
        s.raw(*a as u64);
        s.raw(*b as u64);
        s.raw(vars.len() as u64);
        for (k, ty) in vars {
            s.name(k);
            s.ty(ty);
        }
    }
    for p in &items {
        match p {
            Payload::Expr(e) => s.expr(e),
            _ => s.raw(0),
        }
    }
    let extra = s.out;
    section.extend(extra);
    Some(t.enc.finish(&world, &section))
}

/// The program `wasm_backend::compile` lowers: `ir::lower_world`, then
/// `ir_opt::optimize` when the options ask for it.
pub fn backend_program(w: &World, o: &WasmOptions) -> Option<Program> {
    let mut p = vesper::ir::lower_world(w);
    if o.optimize && vesper::ir_opt::optimize(w, &mut p).is_err() {
        return None;
    }
    Some(p)
}

// ------------------------------------------------------------ differential

fn package() -> &'static Package {
    static P: OnceLock<Package> = OnceLock::new();
    P.get_or_init(|| Package::load("packages/vesper-wasm-backend"))
}

/// `Unsupported` constructs in declaration order (src/err.ves `U_*`).
const CONSTRUCTS: [&str; 16] = [
    "FunctionValue",
    "String",
    "FloatRemainder",
    "Index",
    "Projection",
    "GenericOperation",
    "GenericEffectInstance",
    "LabelledEffectRow",
    "InoutEffectArgument",
    "FirstClassContinuation",
    "NonTailResumptiveHandler",
    "ExternalFunction",
    "ExportType",
    "Operation",
    "IntegerWidth",
    "Float32",
];

/// What the Vesper backend returned.
#[derive(Debug)]
pub enum Outcome {
    Module { core: Vec<u8>, wit: String },
    /// An error class as `error_class` spells it, and its site.
    Error(String, u64),
    /// A construct this port does not lower yet (handlers, host frames).
    NotPorted(u64),
}

fn unpack_bytes(words: &[u64]) -> (Vec<u8>, usize) {
    let len = words[0] as usize;
    let n = len.div_ceil(8);
    let mut bytes = Vec::with_capacity(n * 8);
    for w in &words[1..1 + n] {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    bytes.truncate(len);
    (bytes, 1 + n)
}

pub fn outcome(words: &[u64]) -> Outcome {
    if words[0] == 0 {
        let (core, used) = unpack_bytes(&words[1..]);
        let (wit, _) = unpack_bytes(&words[1 + used..]);
        return Outcome::Module {
            core,
            wit: String::from_utf8(wit).unwrap(),
        };
    }
    let (kind, construct, site) = (words[1], words[2] as usize, words[3]);
    let class = match kind {
        0 => format!("Unsupported({})", CONSTRUCTS[construct]),
        1 => "Entry".into(),
        2 => "Limit".into(),
        3 => "Invalid".into(),
        4 => "Encode".into(),
        _ => return Outcome::NotPorted(site),
    };
    Outcome::Error(class, site)
}

/// The WASI packages a command world without host imports names
/// (src/wasm_backend/host.rs `IO_WIT`, `CLI_WIT`).
const IO_WIT: &str = "package wasi:io@0.2.0;\n\ninterface error {\n  resource error;\n}\n\ninterface streams {\n  use error.{error};\n  variant stream-error {\n    last-operation-failed(error),\n    closed,\n  }\n  resource output-stream {\n    blocking-write-and-flush: func(contents: list<u8>) -> result<_, stream-error>;\n  }\n}\n";
const CLI_WIT: &str = "package wasi:cli@0.2.0;\n\ninterface run {\n  run: func() -> result;\n}\n\ninterface stdout {\n  use wasi:io/streams@0.2.0.{output-stream};\n  get-stdout: func() -> output-stream;\n}\n";

/// `component::encode` for a port module: the WIT world embedded, then
/// wit-component (the part of the backend that stays in Rust).
pub fn wrap(mut core: Vec<u8>, wit: &str, world: WasmWorld) -> Vec<u8> {
    let mut resolve = wit_parser::Resolve::default();
    resolve.push_source("wasi-io.wit", IO_WIT).unwrap();
    resolve.push_source("wasi-cli.wit", CLI_WIT).unwrap();
    let pkg = resolve.push_source("vesper-program.wit", wit).unwrap();
    let name = match world {
        WasmWorld::Command => "program",
        WasmWorld::NexusPlugin => "plugin",
    };
    let id = resolve.select_world(&[pkg], Some(name)).unwrap();
    wit_component::embed_component_metadata(
        &mut core,
        &resolve,
        id,
        wit_component::StringEncoding::UTF8,
        false,
    )
    .unwrap();
    wit_component::ComponentEncoder::default()
        .validate(true)
        .module(&core)
        .unwrap()
        .encode()
        .unwrap()
}

fn engine(stack_switching: bool) -> wasmtime::Engine {
    let mut c = wasmtime::Config::new();
    c.wasm_component_model(true);
    c.consume_fuel(true);
    if stack_switching {
        c.wasm_function_references(true);
        c.wasm_exceptions(true);
        c.wasm_stack_switching(true);
    }
    wasmtime::Engine::new(&c).expect("engine")
}

/// Run a nullary command `main` under Wasmtime; its result, or the trap.
pub fn run_main(component: &[u8], stack_switching: bool) -> String {
    use wasmtime::component::{Component, Linker, Val};
    let engine = engine(stack_switching);
    let component = match Component::new(&engine, component) {
        Ok(c) => c,
        Err(e) => return format!("load: {e}"),
    };
    let linker = Linker::<()>::new(&engine);
    let mut store = wasmtime::Store::new(&engine, ());
    store.set_fuel(100_000_000).unwrap();
    let instance = match linker.instantiate(&mut store, &component) {
        Ok(i) => i,
        Err(e) => return format!("instantiate: {e}"),
    };
    let iface = instance
        .get_export_index(&mut store, None, wasm_backend::ENTRY_INTERFACE)
        .expect("entry interface");
    let main = instance
        .get_export_index(&mut store, Some(&iface), "main")
        .expect("main export");
    let func = instance.get_func(&mut store, main).expect("main func");
    let mut results = vec![Val::Bool(false); func.ty(&store).results().len()];
    match func.call(&mut store, &[], &mut results) {
        Ok(()) => format!("{results:?}"),
        Err(_) => "trap".into(),
    }
}

#[derive(Default, Debug)]
pub struct Tally {
    pub compared: usize,
    pub identical: usize,
    pub errors_agree: usize,
    pub not_ported: usize,
    pub not_encodable: usize,
    pub ran: usize,
    pub diffs: Vec<String>,
}

/// Every case under every configuration: Rust `compile` against the port.
pub fn differential_run(step: usize, execute: bool) -> Tally {
    let f = package().entry("wasm_backend_compile", true);
    let mut t = Tally::default();
    for c in cases().iter().step_by(step) {
        for (k, o) in configs().iter().enumerate() {
            let Some(program) = backend_program(&c.world, o) else {
                continue;
            };
            let Some(input) = wasm_input(&c.world, &program, "main", o) else {
                t.not_encodable += 1;
                continue;
            };
            let rust = wasm_backend::compile(&c.world, "main", o);
            let port = outcome(&f.words(&input));
            t.compared += 1;
            let tag = format!("{} [{k}]", c.name);
            match (&rust, &port) {
                (_, Outcome::NotPorted(_)) => t.not_ported += 1,
                (Ok(a), Outcome::Module { core, wit }) => {
                    if &a.core != core {
                        t.diffs.push(format!("{tag}: core differs ({} vs {} bytes)", a.core.len(), core.len()));
                    } else if &a.wit != wit {
                        t.diffs.push(format!("{tag}: WIT differs\n{}\n---\n{wit}", a.wit));
                    } else {
                        let component = wrap(core.clone(), wit, o.world);
                        if component != a.component {
                            t.diffs.push(format!("{tag}: component differs"));
                            continue;
                        }
                        t.identical += 1;
                        if execute
                            && k == 0
                            && c.world.functions["main"].params.is_empty()
                            && !a.stack_switching
                        {
                            let ours = run_main(&component, false);
                            let theirs = run_main(&a.component, false);
                            if ours != theirs {
                                t.diffs.push(format!("{tag}: run {ours} vs {theirs}"));
                            }
                            t.ran += 1;
                        }
                    }
                }
                (Err(e), Outcome::Error(class, _)) if &error_class(e) == class => t.errors_agree += 1,
                _ => t.diffs.push(format!(
                    "{tag}: rust {:?} port {:?}",
                    rust.as_ref().map(|_| "module").map_err(error_class),
                    match &port {
                        Outcome::Module { core, .. } => format!("module of {} bytes", core.len()),
                        o => format!("{o:?}"),
                    }
                )),
            }
        }
    }
    t
}

/// Every `n`-th case (`WASM_PORT_STEP`, default 1: every case takes about
/// 10 min in a debug build, most of it compiling the package).
fn step() -> usize {
    std::env::var("WASM_PORT_STEP")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1)
}

#[test]
fn port_matches_rust() {
    let t = differential_run(step(), true);
    eprintln!(
        "compared {} identical {} errors agree {} not ported {} not encodable {} ran {}",
        t.compared, t.identical, t.errors_agree, t.not_ported, t.not_encodable, t.ran
    );
    for d in t.diffs.iter().take(40) {
        eprintln!("DIFF {d}");
    }
    assert!(t.diffs.is_empty(), "{} differences", t.diffs.len());
}

/// ABBA timing over the cases the port compiles to a module, under the
/// default options. The Rust side is `compile` less the IR it is handed
/// (`lower_world` + `optimize`) and the wit-component wrap, which the port
/// leaves to Rust; the port side is the JIT entry on the encoded input.
/// Run with `CHECK_HARNESS_SPEED=1 --ignored` in a release build.
#[test]
#[ignore]
fn timing() {
    if !speed() {
        return;
    }
    let f = package().entry("wasm_backend_compile", true);
    let o = WasmOptions::default();
    let mut work = vec![];
    for c in cases() {
        let Some(program) = backend_program(&c.world, &o) else {
            continue;
        };
        let Some(input) = wasm_input(&c.world, &program, "main", &o) else {
            continue;
        };
        if let (Ok(a), Outcome::Module { .. }) =
            (wasm_backend::compile(&c.world, "main", &o), outcome(&f.words(&input)))
        {
            work.push((c, input, a));
        }
    }
    let reps = 5;
    let mut rust = || {
        let mut d = std::time::Duration::ZERO;
        for (c, _, a) in &work {
            for _ in 0..reps {
                let total = time(|| {
                    std::hint::black_box(wasm_backend::compile(&c.world, "main", &o).unwrap());
                });
                let front = time(|| {
                    std::hint::black_box(backend_program(&c.world, &o));
                });
                let wrap_t = time(|| {
                    std::hint::black_box(wrap(a.core.clone(), &a.wit, o.world));
                });
                d += total.saturating_sub(front + wrap_t);
            }
        }
        d
    };
    let mut port = || {
        time(|| {
            for (_, input, _) in &work {
                for _ in 0..reps {
                    std::hint::black_box(f.words(input));
                }
            }
        })
    };
    let encode = time(|| {
        for (c, _, _) in &work {
            for _ in 0..reps {
                let p = backend_program(&c.world, &o).unwrap();
                std::hint::black_box(wasm_input(&c.world, &p, "main", &o));
            }
        }
    });
    let front = time(|| {
        for (c, _, _) in &work {
            for _ in 0..reps {
                std::hint::black_box(backend_program(&c.world, &o));
            }
        }
    });
    eprintln!(
        "{} modules; encoding the input {:?}",
        work.len(),
        encode.saturating_sub(front)
    );
    abba("wasm backend", &mut rust, &mut port);
}
