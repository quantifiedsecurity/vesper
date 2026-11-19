//! The port build's side of the boundary (`cfg(vesper_port)`): the types the
//! generated binding (`port/binding.rs`) names as `crate::port::*`, and the
//! flat encodings of edits and diagnostics that the Vesper functions read
//! (documented in `vesper/src/lib.ves`).

pub use crate::port_binding::*;
pub use crate::{ByteSpan, Error, Limits, Position, Range, Resource};
use crate::{DiagnosticData, SourceEdit};

/// The output length and the metadata bytes `validate_edits` charges.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditTotals {
    pub output: usize,
    pub metadata: usize,
}

/// `size_of` of the Rust records that the budgets charge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sizes {
    pub line: usize,
    pub edit: usize,
    pub label: usize,
    pub fix: usize,
    pub diagnostic: usize,
}

const PRIMARY: u64 = 1 << 63;

fn push_edits(edits: &[SourceEdit], words: &mut Vec<u64>, pool: &mut String) {
    for e in edits {
        words.extend_from_slice(&[
            e.span.start as u64,
            e.span.end as u64,
            pool.len() as u64,
            e.expected.len() as u64,
            e.replacement.len() as u64,
        ]);
        pool.push_str(&e.expected);
    }
}

/// Five words per edit, and the expected texts.
pub fn encode_edits(edits: &[SourceEdit]) -> (Vec<u64>, String) {
    let mut words = Vec::with_capacity(edits.len() * 5);
    let mut pool = String::new();
    push_edits(edits, &mut words, &mut pool);
    (words, pool)
}

/// The sorted edits as `SourceEdits::apply` reads them.
pub fn encode_replacements(edits: &[SourceEdit]) -> (Vec<u64>, String) {
    let mut words = Vec::with_capacity(edits.len() * 3);
    let mut pool = String::with_capacity(edits.iter().map(|e| e.replacement.len()).sum());
    for e in edits {
        words.extend_from_slice(&[
            e.span.start as u64,
            e.span.end as u64,
            e.replacement.len() as u64,
        ]);
        pool.push_str(&e.replacement);
    }
    (words, pool)
}

/// The diagnostics encoding of `diagnostics_check`.
pub fn encode_diagnostics(data: &[DiagnosticData]) -> (Vec<u64>, String) {
    let mut words = Vec::new();
    let mut pool = String::new();
    for d in data {
        words.extend_from_slice(&[
            d.code.len() as u64,
            d.message.len() as u64,
            d.help.is_some() as u64,
            d.help.as_ref().map_or(0, String::len) as u64,
            d.labels.len() as u64,
            d.fixes.len() as u64,
        ]);
        for l in &d.labels {
            words.extend_from_slice(&[
                l.span.start as u64,
                l.span.end as u64,
                l.message.len() as u64 | if l.primary { PRIMARY } else { 0 },
            ]);
        }
        for f in &d.fixes {
            words.extend_from_slice(&[
                f.id.len() as u64,
                f.title.len() as u64,
                f.edits.len() as u64,
            ]);
            push_edits(&f.edits, &mut words, &mut pool);
        }
    }
    (words, pool)
}
