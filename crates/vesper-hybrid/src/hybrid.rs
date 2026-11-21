//! The swap-in glue: each swapped module's Vesper binding, wrapped as the
//! module's Rust function table and installed before `main` runs.
//!
//! Installing from a static constructor keeps the included CLI's `main`
//! untouched (the same std start-up, the same CLI thread). The table only
//! stores function pointers, so it needs nothing from std yet.
//!
//! Set `VESPER_HYBRID_STATS=1` to print, at exit, how many calls each
//! swapped module answered from Vesper (the differential harness uses it to
//! show the pilot really ran) and what each checker swap's input and output
//! cost (`stats`; `VESPER_HYBRID_STATS=time` adds the Rust pass).
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

#[path = "parse_words.rs"]
mod parse_words;

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
        // The source is lent in place: the parse swap has no input to
        // encode, only its AST words to decode.
        let timer = stats::Timer::start();
        let words = parse_binding::parse_words(source);
        let timer = timer.lap(&stats::PARSE.vesper);
        let parsed = parse_words::decode(source, &words);
        timer.lap(&stats::PARSE.decode);
        stats::PARSE.call(
            false,
            || vesper_syntax::parser::rust_parse(source),
            |rust| *rust == parsed,
        );
        parsed
    },
};

/// One compilation's swap inputs (SH4 lane 2, #1276), kept in its
/// `vesper::swap_input` scope: the world's encoding, built once, and each
/// Vesper module's decoded copy of it with what that copy already holds.
struct Inputs {
    log: check_words::WorldLog,
    layout: Option<(check_words::Cursor, check_layout_binding::LayoutInput)>,
    ir: Option<(check_words::Cursor, ir_check_binding::IrInput)>,
}

impl Inputs {
    fn new(world: &vesper::check::World) -> Self {
        Self {
            log: check_words::WorldLog::new(world),
            layout: None,
            ir: None,
        }
    }
}

/// `vesper::layout::LayoutTable` from `modules/check_layout`: the layout of
/// `@vesper/check-layout`. Inside a compilation's scope the world crosses
/// once (each call sends only the ADTs and types the Vesper copy lacks);
/// outside one, each call encodes the ADTs the type can reach. A type
/// whose literals the decoder cannot read (over 64 bits) stays in Rust.
const VESPER_LAYOUT: vesper::layout::LayoutTable = vesper::layout::LayoutTable {
    of: |world, ty| {
        let timer = stats::Timer::start();
        let cached = vesper::swap_input::with(
            world,
            || Inputs::new(world),
            |inputs| {
                let (cur, input) = inputs.layout.get_or_insert_with(|| {
                    (
                        check_words::Cursor::default(),
                        check_layout_binding::LayoutInput::new(),
                    )
                });
                let part = inputs.log.layout_part(world, cur, ty)?;
                let timer = timer.lap(&stats::LAYOUT.encode);
                let words = input.layout_of(&part);
                Some((timer.lap(&stats::LAYOUT.vesper), words))
            },
        );
        let (timer, words, scoped) = match cached {
            Some(Some((timer, words))) => (timer, words, true),
            Some(None) => return vesper::layout::rust_layout_of(world, ty),
            None => {
                let Some(input) = check_words::layout_input(world, ty) else {
                    return vesper::layout::rust_layout_of(world, ty);
                };
                let timer = timer.lap(&stats::LAYOUT.encode);
                let words = check_layout_binding::layout_of_words(&input);
                (timer.lap(&stats::LAYOUT.vesper), words, false)
            }
        };
        CHECK_LAYOUT_CALLS.fetch_add(1, Relaxed);
        let layout = check_words::decode_layout(&words);
        timer.lap(&stats::LAYOUT.decode);
        stats::LAYOUT.call(
            scoped,
            || vesper::layout::rust_layout_of(world, ty),
            |rust| format!("{rust:?}") == format!("{layout:?}"),
        );
        layout
    },
};

/// `vesper::ir_check::IrCheckTable` from `modules/ir_check`: the IR check
/// of `@vesper/ir-check`, over the world without its expression types and
/// span tables (which it does not read). Inside a compilation's scope the
/// world crosses once. A program whose literals the decoder cannot read
/// stays in Rust.
const VESPER_IR_CHECK: vesper::ir_check::IrCheckTable = vesper::ir_check::IrCheckTable {
    validate: |world, program| {
        let timer = stats::Timer::start();
        let cached = vesper::swap_input::with(
            world,
            || Inputs::new(world),
            |inputs| {
                let (cur, input) = inputs.ir.get_or_insert_with(|| {
                    (
                        check_words::Cursor::default(),
                        ir_check_binding::IrInput::new(),
                    )
                });
                let part = inputs.log.ir_part(world, cur, program)?;
                let timer = timer.lap(&stats::IR_CHECK.encode);
                let words = input.validate(&part);
                Some((timer.lap(&stats::IR_CHECK.vesper), words))
            },
        );
        let (timer, words, scoped) = match cached {
            Some(Some((timer, words))) => (timer, words, true),
            Some(None) => return vesper::ir_check::rust_validate(world, program),
            None => {
                let Some(input) = check_words::ir_input(world, program) else {
                    return vesper::ir_check::rust_validate(world, program);
                };
                let timer = timer.lap(&stats::IR_CHECK.encode);
                let words = ir_check_binding::validate_words(&input);
                (timer.lap(&stats::IR_CHECK.vesper), words, false)
            }
        };
        IR_CHECK_CALLS.fetch_add(1, Relaxed);
        let diagnostics = check_words::decode_diagnostics(&words);
        timer.lap(&stats::IR_CHECK.decode);
        stats::IR_CHECK.call(
            scoped,
            || vesper::ir_check::rust_validate(world, program),
            |rust| *rust == diagnostics,
        );
        diagnostics
    },
};

/// The swaps' input and output costs (SH4 lane 2, #1276), reported at exit
/// with `VESPER_HYBRID_STATS=1`. `VESPER_HYBRID_STATS=time` also runs the
/// Rust pass on every swapped call, times it and counts the calls whose
/// answers differ, so one hybrid run gives each swap's encode, Vesper and
/// decode time beside the Rust pass it replaces.
mod stats {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    use std::time::Instant;

    /// 0: off, 1: costs, 2: costs and the Rust pass.
    pub fn level() -> u8 {
        static LEVEL: OnceLock<u8> = OnceLock::new();
        *LEVEL.get_or_init(|| match std::env::var("VESPER_HYBRID_STATS").as_deref() {
            Ok("1") => 1,
            Ok("time") => 2,
            _ => 0,
        })
    }

    pub struct Cost {
        pub name: &'static str,
        pub calls: AtomicU64,
        pub scoped: AtomicU64,
        pub encode: AtomicU64,
        pub vesper: AtomicU64,
        pub decode: AtomicU64,
        pub rust: AtomicU64,
        pub differ: AtomicU64,
    }

    impl Cost {
        const fn new(name: &'static str) -> Self {
            Self {
                name,
                calls: AtomicU64::new(0),
                scoped: AtomicU64::new(0),
                encode: AtomicU64::new(0),
                vesper: AtomicU64::new(0),
                decode: AtomicU64::new(0),
                rust: AtomicU64::new(0),
                differ: AtomicU64::new(0),
            }
        }

        /// Counts one swapped call; at level 2 also times the Rust pass
        /// (`rust`) and counts the call if `same` says its answer differs
        /// from the swap's.
        pub fn call<T>(
            &self,
            scoped: bool,
            rust: impl FnOnce() -> T,
            same: impl FnOnce(&T) -> bool,
        ) {
            if level() == 0 {
                return;
            }
            self.calls.fetch_add(1, Relaxed);
            self.scoped.fetch_add(scoped as u64, Relaxed);
            if level() == 2 {
                let t = Instant::now();
                let answer = std::hint::black_box(rust());
                self.rust.fetch_add(t.elapsed().as_nanos() as u64, Relaxed);
                self.differ.fetch_add(!same(&answer) as u64, Relaxed);
            }
        }

        pub fn report(&self) {
            let ms = |a: &AtomicU64| a.load(Relaxed) as f64 / 1e6;
            eprintln!(
                "vesper-hybrid: cost {}: {} calls ({} in a compilation scope), encode {:.3} ms, \
                 vesper {:.3} ms, decode {:.3} ms, rust {:.3} ms, {} differ",
                self.name,
                self.calls.load(Relaxed),
                self.scoped.load(Relaxed),
                ms(&self.encode),
                ms(&self.vesper),
                ms(&self.decode),
                ms(&self.rust),
                self.differ.load(Relaxed),
            );
        }
    }

    pub static PARSE: Cost = Cost::new("parse");
    pub static LAYOUT: Cost = Cost::new("check-layout");
    pub static IR_CHECK: Cost = Cost::new("ir-check");

    /// A stopwatch that only reads the clock when stats are on.
    #[derive(Clone, Copy)]
    pub struct Timer(Option<Instant>);

    impl Timer {
        pub fn start() -> Self {
            Self((level() > 0).then(Instant::now))
        }

        /// Adds the time since the last lap to `into`.
        pub fn lap(self, into: &AtomicU64) -> Self {
            match self.0 {
                Some(t) => {
                    let now = Instant::now();
                    into.fetch_add((now - t).as_nanos() as u64, Relaxed);
                    Self(Some(now))
                }
                None => self,
            }
        }
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
    // Swaps keep one input per compilation (SH4 lane 2, #1276).
    // `VESPER_HYBRID_SCOPES=0` encodes per call instead (the before side of
    // the report's measurement).
    if !std::env::var_os("VESPER_HYBRID_SCOPES").is_some_and(|v| v == "0") {
        vesper::swap_input::enable();
    }
    if std::env::var_os("VESPER_HYBRID_STATS").is_some_and(|v| v == "1" || v == "time") {
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
    stats::PARSE.report();
    stats::LAYOUT.report();
    stats::IR_CHECK.report();
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
