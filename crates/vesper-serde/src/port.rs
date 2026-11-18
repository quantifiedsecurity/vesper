//! The port-backed build's adapter (cfg(vesper_port)): the crate's public
//! types and borrowed views stay in Rust, and each scalar codec forwards to
//! the Vesper port in `vesper/` through the generated binding
//! (`port/binding.rs`). Only this module and the binding see the port.
use crate::port_binding as vp;
use std::sync::OnceLock;

/// The types the binding converts (`Port::rust_types("crate::port_types")`):
/// the crate's own boundary types, plus the port's result records.
pub(crate) mod types {
    pub(crate) use crate::index::Token;
    pub(crate) use crate::{Error, ErrorKind, Kind, Limits};

    /// A decoded binary64 (vesper/src/lib.ves `Decoded`).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Decoded {
        pub path: u64,
        pub bits: u64,
    }
}

/// The 128-bit power-of-five table, built once by the port.
fn pow5() -> &'static [u64] {
    static TABLE: OnceLock<Vec<u64>> = OnceLock::new();
    TABLE.get_or_init(vp::number_pow5)
}

/// `number::decode_f64` through the port; the undecided case falls back to
/// `str::parse` exactly as the Rust build does.
pub(crate) fn decode_f64(text: &str) -> Option<(f64, crate::lemire::DecodePath)> {
    use crate::lemire::DecodePath;
    let d = vp::number_decode(text.as_bytes(), pow5());
    match d.path {
        1 => Some((f64::from_bits(d.bits), DecodePath::Clinger)),
        2 => Some((f64::from_bits(d.bits), DecodePath::EiselLemire)),
        3 => Some((text.parse::<f64>().ok()?, DecodePath::Fallback)),
        _ => None,
    }
}

use crate::index::Token;
use crate::{Error, ErrorKind, IndexedJson, Limits};

/// The token tape of a document: the port's `Doc` handle, read in place.
pub(crate) struct Tape {
    pub(crate) doc: vp::Doc,
}

impl Tape {
    pub(crate) fn new() -> Self {
        Self { doc: vp::doc_new() }
    }
    pub(crate) fn len(&self) -> usize {
        self.doc.as_ref().tokens().len()
    }
    /// Tokens reserved (the tape's `try_reserve_exact` capacity).
    pub(crate) fn capacity(&self) -> usize {
        self.doc.as_ref().capacity() as usize
    }
    pub(crate) fn get(&self, i: usize) -> Token {
        self.doc
            .as_ref()
            .tokens()
            .get(i)
            .expect("tape index in bounds")
    }
}

impl core::fmt::Debug for Tape {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.doc.as_ref().tokens().iter()).finish()
    }
}

fn limits(l: &Limits) -> vp::repr::Limits {
    vp::repr::Limits::from_value(l)
}

/// The `&str` view of input the port validated (std's safe check again,
/// so the adapter needs no `from_utf8_unchecked`).
fn whole_str(input: &[u8]) -> Result<&str, Error> {
    core::str::from_utf8(input).map_err(|e| Error::new(ErrorKind::Utf8, e.valid_up_to()))
}

/// `IndexedJson::parse`: the scalar recursive-descent parser of the port.
pub(crate) fn parse(input: &[u8], l: Limits) -> Result<IndexedJson<'_>, Error> {
    let mut tape = Tape::new();
    vp::doc_parse(&mut tape.doc, input, &limits(&l))?;
    let text = whole_str(input)?;
    let r = tape.doc.as_ref();
    let (work_used, index_entries) = (r.work() as usize, r.entries() as usize);
    Ok(IndexedJson {
        input: text,
        tokens: tape,
        work_used,
        index_entries,
    })
}

/// `Node::field` over the document's tape.
pub(crate) fn field(
    doc: &IndexedJson<'_>,
    index: usize,
    key: &str,
    max_work: usize,
) -> Result<Option<usize>, Error> {
    vp::doc_field(
        &doc.tokens.doc,
        doc.input.as_bytes(),
        index as u64,
        key.as_bytes(),
        max_work as u64,
    )
    .map(|found| found.map(|i| i as usize))
}

/// `JsonString::equals`.
pub(crate) fn string_equals(raw: &str, text: &str) -> bool {
    vp::string_equals(raw.as_bytes(), text.as_bytes())
}

/// `JsonString::decode` into scratch that holds the decoded length.
pub(crate) fn string_decode(raw: &str, scratch: &mut [u8]) -> usize {
    vp::string_decode(raw.as_bytes(), scratch) as usize
}

use crate::simd::{IndexLimits, Isa, StructuralIndex};

/// Admission of src/stage2.rs (`admit`): the stage 1 limits.
pub(crate) fn admit(input: &[u8], l: &Limits) -> Result<IndexLimits, Error> {
    let entries = vp::doc_admit(input.len() as u64, &limits(l))?;
    Ok(IndexLimits {
        max_input_bytes: l.max_input_bytes,
        max_entries: entries as usize,
    })
}

/// Stage 2 of the port over a stage 1 index built by the Rust kernels.
fn build_into(
    tape: &mut Tape,
    input: &[u8],
    index: &StructuralIndex,
    l: &Limits,
) -> Result<(), Error> {
    vp::doc_build(
        &mut tape.doc,
        input,
        index.structurals(),
        index.escapes(),
        index.first_string_control().map_or(u64::MAX, u64::from),
        index.truncated_at().map_or(u64::MAX, u64::from),
        index.unclosed_string(),
        &limits(l),
    )
}

fn document<'a>(input: &'a [u8], text: &'a str, tape: Tape) -> IndexedJson<'a> {
    debug_assert_eq!(input.len(), text.len());
    let r = tape.doc.as_ref();
    let (work_used, index_entries) = (r.work() as usize, r.entries() as usize);
    IndexedJson {
        input: text,
        tokens: tape,
        work_used,
        index_entries,
    }
}

/// `IndexedJson::parse_with_index` through the port's stage 2.
pub(crate) fn parse_with_index<'a>(
    input: &'a [u8],
    l: Limits,
    isa: Isa,
    index: &mut StructuralIndex,
) -> Result<IndexedJson<'a>, Error> {
    let index_limits = admit(input, &l)?;
    index.build(input, isa, index_limits)?;
    let mut tape = Tape::new();
    build_into(&mut tape, input, index, &l)?;
    let text = whole_str(input)?;
    Ok(document(input, text, tape))
}

/// `SimdParser::parse` after stage 1: stage 2 into the parser's reusable
/// tape, which every outcome returns to `slot`.
pub(crate) fn parse_reusing<'a, R>(
    input: &'a [u8],
    l: Limits,
    index: &StructuralIndex,
    slot: &mut Option<Tape>,
    f: impl FnOnce(&IndexedJson<'a>) -> R,
) -> Result<R, Error> {
    let mut tape = slot.take().unwrap_or_else(Tape::new);
    let checked = build_into(&mut tape, input, index, &l).and_then(|()| whole_str(input));
    let text = match checked {
        Ok(text) => text,
        Err(e) => {
            *slot = Some(tape);
            return Err(e);
        }
    };
    let doc = document(input, text, tape);
    let result = f(&doc);
    *slot = Some(doc.tokens);
    Ok(result)
}
