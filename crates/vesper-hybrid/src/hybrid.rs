//! The swap-in glue: each swapped module's Vesper binding, wrapped as the
//! module's Rust function table and installed before `main` runs.
//!
//! Installing from a static constructor keeps the included CLI's `main`
//! untouched (the same std start-up, the same CLI thread). The table only
//! stores function pointers, so it needs nothing from std yet.
//!
//! Set `VESPER_HYBRID_STATS=1` to print, at exit, how many calls each
//! swapped module answered from Vesper (the differential harness uses it to
//! show the pilot really ran).
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[rustfmt::skip]
#[allow(clippy::all)]
#[path = "../port/layout.rs"]
mod layout_binding;

#[rustfmt::skip]
#[allow(clippy::all)]
#[path = "../port/parse.rs"]
mod parse_binding;

#[rustfmt::skip]
#[allow(clippy::all)]
#[path = "../port/check_layout.rs"]
mod check_layout_binding;

#[rustfmt::skip]
#[allow(clippy::all)]
#[path = "../port/ir_check.rs"]
mod ir_check_binding;

#[rustfmt::skip]
#[allow(clippy::all)]
#[path = "../port/codegen.rs"]
mod codegen_binding;

#[rustfmt::skip]
#[allow(clippy::all)]
#[path = "../port/codegen_export.rs"]
mod codegen_export_binding;

#[path = "parse_words.rs"]
mod parse_words;

// The export glue's interface encoder (#1286), shared with the root
// package's tests/self_host_codegen_export.rs.
#[allow(dead_code)]
#[path = "../../../tests/self_host_codegen_export/words.rs"]
mod export_words;

// The checker swaps' input encoder and result decoders (#1177), shared with
// the root package's tests/hybrid_layout.rs and tests/hybrid_ir_check.rs.
#[path = "../../../tests/common/check_words.rs"]
mod check_words;

/// Calls answered by the Vesper word-layout module.
static LAYOUT_CALLS: AtomicU64 = AtomicU64::new(0);
/// Sources parsed by the Vesper parse module.
static PARSE_CALLS: AtomicU64 = AtomicU64::new(0);
/// `Layout::of` calls answered by the Vesper check-layout module.
static CHECK_LAYOUT_CALLS: AtomicU64 = AtomicU64::new(0);
/// Programs checked by the Vesper IR-check module.
static IR_CHECK_CALLS: AtomicU64 = AtomicU64::new(0);
/// `aggregate` requests the Vesper codegen module emitted, and declined.
static CODEGEN_CALLS: AtomicU64 = AtomicU64::new(0);
static CODEGEN_DECLINED: AtomicU64 = AtomicU64::new(0);
/// Libraries whose v2 glue the Vesper export module emitted, and declined.
static EXPORT_CALLS: AtomicU64 = AtomicU64::new(0);
static EXPORT_DECLINED: AtomicU64 = AtomicU64::new(0);

fn count() {
    LAYOUT_CALLS.fetch_add(1, Relaxed);
}

/// `vesper::layout::WordLayout` from `modules/layout`.
const VESPER_WORD_LAYOUT: vesper::layout::WordLayout = vesper::layout::WordLayout {
    first_offset: |is_enum| {
        count();
        layout_binding::first_offset(is_enum)
    },
    next_offset: |offset, size| {
        count();
        let next = layout_binding::next_offset(offset, size);
        (next != layout_binding::no_fit()).then_some(next as i32)
    },
    product_size: |end| {
        count();
        layout_binding::product_size(end)
    },
    enum_size: |size, end| {
        count();
        layout_binding::enum_size(size, end)
    },
    word_offset: |index| {
        count();
        layout_binding::word_offset(index)
    },
};

/// `vesper_syntax::parser::ParseTable` from `modules/parse`: the front end
/// v2 and the AST of `@vesper/ast`, read back by `parse_words::decode`.
const VESPER_PARSE: vesper_syntax::parser::ParseTable = vesper_syntax::parser::ParseTable {
    parse: |source| {
        PARSE_CALLS.fetch_add(1, Relaxed);
        parse_words::decode(source, &parse_binding::parse_words(source))
    },
};

/// `vesper::layout::LayoutTable` from `modules/check_layout`: the layout of
/// `@vesper/check-layout`, over the ADTs the type can reach. A type whose
/// literals the decoder cannot read (over 64 bits) stays in Rust.
const VESPER_LAYOUT: vesper::layout::LayoutTable = vesper::layout::LayoutTable {
    of: |world, ty| {
        let Some(input) = check_words::layout_input(world, ty) else {
            return vesper::layout::rust_layout_of(world, ty);
        };
        CHECK_LAYOUT_CALLS.fetch_add(1, Relaxed);
        check_words::decode_layout(&check_layout_binding::layout_of_words(&input))
    },
};

/// `vesper::ir_check::IrCheckTable` from `modules/ir_check`: the IR check
/// of `@vesper/ir-check`. A program whose literals the decoder cannot read
/// stays in Rust.
const VESPER_IR_CHECK: vesper::ir_check::IrCheckTable = vesper::ir_check::IrCheckTable {
    validate: |world, program| {
        let Some(input) = check_words::ir_input(world, program) else {
            return vesper::ir_check::rust_validate(world, program);
        };
        IR_CHECK_CALLS.fetch_add(1, Relaxed);
        check_words::decode_diagnostics(&ir_check_binding::validate_words(&input))
    },
};

/// The `aggregate` hook of `vesper::codegen_swap` from `modules/codegen`:
/// the census lowering family, emitted in Vesper through the lent builder
/// and module (`vesper_cranelift_sys::lend`).
fn vesper_aggregate(lent: &mut vesper::codegen_swap::Lent<'_, '_>) -> Result<bool, String> {
    let (answer, crossing) = vesper_cranelift_sys::lend(
        lent.builder,
        &mut *lent.module,
        lent.op,
        lent.args,
        codegen_binding::aggregate,
    );
    if let Some(e) = crossing.error {
        return Err(format!("codegen (aggregate op {}): {e}", lent.op));
    }
    if crossing.unreturned {
        return Err(format!(
            "codegen (aggregate op {}) kept a lent handle",
            lent.op
        ));
    }
    if answer == 1 {
        CODEGEN_CALLS.fetch_add(1, Relaxed);
        lent.results.extend(crossing.results);
        Ok(true)
    } else {
        CODEGEN_DECLINED.fetch_add(1, Relaxed);
        Ok(false)
    }
}

/// The `export` hook of `vesper::codegen_swap` from `modules/codegen_export`
/// (#1286): the export ABI v2 glue after the runtime prelude, emitted in
/// Vesper over the lent module. The request's words are a flags word (what
/// the module already holds, `glue.ves`'s `F_*`) and the interface in the
/// words of tests/self_host_codegen_export/words.rs.
fn vesper_export(
    m: &mut dyn cranelift_module::Module,
    _world: &vesper::check::World,
    interface: &vesper::native::export::LibraryInterface,
) -> Option<Result<(), String>> {
    use cranelift_module::FuncOrDataId;
    // The Rust glue reports a library with no runtime symbols.
    interface.runtime.as_ref()?;
    let func = |name: &str| matches!(m.get_name(name), Some(FuncOrDataId::Func(_)));
    let data = |name: &str| matches!(m.get_name(name), Some(FuncOrDataId::Data(_)));
    let flags = u64::from(func("vesper_box_free"))
        | u64::from(data("vesper_seq_live")) << 1
        | u64::from(data("vesper_box_live")) << 2;
    let bytes = export_words::binding_input(interface, &Default::default());
    let mut words = vec![flags];
    words.extend(
        bytes
            .chunks(8)
            .map(|c| c.iter().rev().fold(0u64, |w, b| w << 8 | u64::from(*b))),
    );
    let (answer, crossing) =
        vesper_cranelift_sys::lend_module(m, 0, &words, codegen_export_binding::export);
    if let Some(e) = crossing.error {
        return Some(Err(format!("codegen (export glue): {e}")));
    }
    if crossing.unreturned {
        return Some(Err("codegen (export glue) kept a lent handle".into()));
    }
    if answer == 1 {
        EXPORT_CALLS.fetch_add(1, Relaxed);
        Some(Ok(()))
    } else {
        EXPORT_DECLINED.fetch_add(1, Relaxed);
        None
    }
}

extern "C" fn install() {
    // Nothing runs before this constructor, so the slots are empty.
    if vesper::layout::install_word_layout(VESPER_WORD_LAYOUT).is_err() {
        std::process::abort();
    }
    if vesper_syntax::parser::install_parse(VESPER_PARSE).is_err() {
        std::process::abort();
    }
    if vesper::layout::install_layout(VESPER_LAYOUT).is_err() {
        std::process::abort();
    }
    if vesper::ir_check::install_ir_check(VESPER_IR_CHECK).is_err() {
        std::process::abort();
    }
    let hooks = vesper::codegen_swap::CodegenHooks {
        aggregate: Some(vesper_aggregate),
        export: Some(vesper_export),
        ..Default::default()
    };
    if vesper::codegen_swap::install(hooks).is_err() {
        std::process::abort();
    }
    if std::env::var_os("VESPER_HYBRID_STATS").is_some_and(|v| v == "1") {
        unsafe extern "C" {
            fn atexit(f: extern "C" fn()) -> std::ffi::c_int;
        }
        // SAFETY: `report` is a plain `extern "C" fn` that only formats
        // and writes to stderr.
        unsafe { atexit(report) };
    }
}

extern "C" fn report() {
    eprintln!(
        "vesper-hybrid: layout answered {} calls from Vesper",
        LAYOUT_CALLS.load(Relaxed)
    );
    eprintln!(
        "vesper-hybrid: parse answered {} calls from Vesper",
        PARSE_CALLS.load(Relaxed)
    );
    eprintln!(
        "vesper-hybrid: check-layout answered {} calls from Vesper",
        CHECK_LAYOUT_CALLS.load(Relaxed)
    );
    eprintln!(
        "vesper-hybrid: ir-check answered {} calls from Vesper",
        IR_CHECK_CALLS.load(Relaxed)
    );
    eprintln!(
        "vesper-hybrid: codegen emitted {} aggregate requests from Vesper ({} declined)",
        CODEGEN_CALLS.load(Relaxed),
        CODEGEN_DECLINED.load(Relaxed)
    );
    eprintln!(
        "vesper-hybrid: codegen emitted the export glue of {} libraries from Vesper ({} declined)",
        EXPORT_CALLS.load(Relaxed),
        EXPORT_DECLINED.load(Relaxed)
    );
}

// Run `install` before `main` (the platform's static-constructor section).
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_vendor = "apple",
    windows
)))]
compile_error!(
    "vesper-hybrid installs its modules from a static constructor; add this target's section"
);

#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android", target_os = "freebsd"),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    target_vendor = "apple",
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(windows, unsafe(link_section = ".CRT$XCU"))]
static INSTALL: extern "C" fn() = install;
