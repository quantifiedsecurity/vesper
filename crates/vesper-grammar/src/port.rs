//! The port-backed build's glue (cfg(vesper_port)): converts limits to the
//! word slices and failure records of the Vesper library `grammar`
//! (`vesper/src/lib.ves`), and back. The public functions of each module
//! forward here; the Rust bodies they replace are the seed copy's.
pub(crate) use crate::port_binding as vesper;
use crate::port_binding::GErr;
use crate::{Error, ErrorKind};

/// `crate::ErrorKind` in declaration order: the port raises the index.
const KINDS: [ErrorKind; 25] = [
    ErrorKind::UnexpectedEof,
    ErrorKind::NonCanonical,
    ErrorKind::Overflow,
    ErrorKind::TrailingBytes,
    ErrorKind::OutputTooSmall,
    ErrorKind::InputLimit,
    ErrorKind::OutputLimit,
    ErrorKind::PayloadLimit,
    ErrorKind::WorkLimit,
    ErrorKind::DepthLimit,
    ErrorKind::CountLimit,
    ErrorKind::AllocationLimit,
    ErrorKind::AllocationFailed,
    ErrorKind::Descriptor,
    ErrorKind::Alignment,
    ErrorKind::Range,
    ErrorKind::Bitmap,
    ErrorKind::NullCount,
    ErrorKind::Aliasing,
    ErrorKind::Rule,
    ErrorKind::RecursiveRule,
    ErrorKind::DuplicateBranch,
    ErrorKind::AmbiguousChoice,
    ErrorKind::NullableChoice,
    ErrorKind::Mismatch,
];

pub(crate) fn kind(index: u64) -> ErrorKind {
    KINDS[index as usize]
}

/// A crate-level failure of the port.
pub(crate) fn error(e: GErr) -> Error {
    Error::new(kind(e.kind), e.offset as usize)
}

pub(crate) fn framing_limits(l: crate::framing::Limits) -> [u64; 4] {
    [
        l.max_input_bytes as u64,
        l.max_payload_bytes as u64,
        l.max_output_bytes as u64,
        l.max_work as u64,
    ]
}

// grammar.rs and grammar/codec.rs

use crate::grammar::codec::{CodecLimits, Value as CodecValue};
use crate::grammar::{Capture, Rule};

/// The grammar's limits, then the Rust sizes its budgets count.
pub(crate) fn grammar_limits(l: crate::grammar::Limits, meta_size: usize) -> [u64; 14] {
    [
        l.max_rules as u64,
        l.max_edges as u64,
        l.max_literal_bytes as u64,
        l.max_total_literal_bytes as u64,
        l.max_depth as u64,
        l.max_input_bytes as u64,
        l.max_frame_bytes as u64,
        l.max_events as u64,
        l.max_metadata_bytes as u64,
        l.max_capture_bytes as u64,
        l.max_work as u64,
        l.max_repetitions as u64,
        meta_size as u64,
        size_of::<Capture>() as u64,
    ]
}

pub(crate) fn codec_limits(l: CodecLimits) -> [u64; 7] {
    [
        l.max_values as u64,
        l.max_metadata_bytes as u64,
        l.max_output_bytes as u64,
        l.max_depth as u64,
        l.max_work as u64,
        size_of::<CodecValue<'_>>() as u64,
        crate::grammar::codec::WRITE_SIZE as u64,
    ]
}

/// Rules as the port reads them (vesper/src/grammar.ves): the count, one
/// record offset per rule, then the records; literal bytes go to `lits`.
pub(crate) fn flatten_rules(rules: &[Rule]) -> (Vec<u64>, Vec<u8>) {
    let mut words = vec![0u64; rules.len() + 1];
    words[0] = rules.len() as u64;
    let mut lits = Vec::new();
    for (i, rule) in rules.iter().enumerate() {
        words[i + 1] = words.len() as u64;
        match rule {
            Rule::Empty => words.push(0),
            Rule::Byte(b) => words.extend([1, u64::from(*b)]),
            Rule::Literal(bytes) => {
                words.extend([2, lits.len() as u64, bytes.len() as u64]);
                lits.extend_from_slice(bytes);
            }
            Rule::Uleb128 => words.push(3),
            Rule::Blob => words.push(4),
            Rule::Frame(c) => words.extend([5, *c as u64]),
            Rule::U24Frame(c) => words.extend([6, *c as u64]),
            Rule::Sequence(children) => {
                words.extend([7, children.len() as u64]);
                words.extend(children.iter().map(|c| *c as u64));
            }
            Rule::Choice(branches) => {
                words.extend([8, branches.len() as u64]);
                for b in branches {
                    words.extend([u64::from(b.id), b.rule as u64]);
                }
            }
            Rule::Repeat { rule, count } => words.extend([9, *rule as u64, *count as u64]),
            Rule::Counted(c) => words.extend([10, *c as u64]),
        }
    }
    (words, lits)
}

/// Captures from ten words each.
pub(crate) fn captures(words: &[u64]) -> Vec<Capture> {
    words
        .as_chunks::<10>()
        .0
        .iter()
        .map(|w| Capture {
            rule: w[0] as usize,
            branch: (w[1] != 0).then_some(w[2] as u16),
            range: w[3] as usize..w[4] as usize,
            number: (w[5] != 0).then_some(w[6]),
            payload: (w[7] != 0).then_some(w[8] as usize..w[9] as usize),
        })
        .collect()
}

/// Rebuilds a decoded value from its preorder stream (codec.ves), borrowing
/// blob payloads from `input`.
pub(crate) fn codec_value<'a>(words: &[u64], at: &mut usize, input: &'a [u8]) -> CodecValue<'a> {
    let tag = words[*at];
    *at += 1;
    let mut next = || {
        let w = words[*at];
        *at += 1;
        w
    };
    match tag {
        0 => CodecValue::Unit,
        1 => CodecValue::Number(next()),
        2 => {
            let start = next() as usize;
            let len = next() as usize;
            CodecValue::Bytes(&input[start..start + len])
        }
        3 | 6 => {
            let count = next() as usize;
            let mut values = Vec::with_capacity(count);
            for _ in 0..count {
                values.push(codec_value(words, at, input));
            }
            if tag == 3 {
                CodecValue::Sequence(values)
            } else {
                CodecValue::Repeat(values)
            }
        }
        4 => {
            let branch = next() as u16;
            CodecValue::Choice {
                branch,
                value: Box::new(codec_value(words, at, input)),
            }
        }
        _ => CodecValue::Frame(Box::new(codec_value(words, at, input))),
    }
}

/// A value as the encoder reads it (codec.ves), with every blob copied to
/// one arena. Iterative, so an arbitrarily deep value cannot overflow the
/// Rust stack before the port's depth budget rejects it.
pub(crate) fn flatten_codec_value(value: &CodecValue<'_>) -> (Vec<u64>, Vec<u8>) {
    let mut words = Vec::new();
    let mut arena = Vec::new();
    let mut stack = vec![value];
    while let Some(v) = stack.pop() {
        match v {
            CodecValue::Unit => words.push(0),
            CodecValue::Number(n) => words.extend([1, *n]),
            CodecValue::Bytes(b) => {
                words.extend([2, arena.len() as u64, b.len() as u64]);
                arena.extend_from_slice(b);
            }
            CodecValue::Sequence(values) | CodecValue::Repeat(values) => {
                let tag = if matches!(v, CodecValue::Sequence(_)) {
                    3
                } else {
                    6
                };
                words.extend([tag, values.len() as u64]);
                stack.extend(values.iter().rev());
            }
            CodecValue::Choice { branch, value } => {
                words.extend([4, u64::from(*branch)]);
                stack.push(value);
            }
            CodecValue::Frame(value) => {
                words.push(5);
                stack.push(value);
            }
        }
    }
    (words, arena)
}

// protobuf.rs

use crate::protobuf::{
    FieldValue, Kind as PbKind, Message as PbMessage, MessageSchema, Value as PbValue,
};

pub(crate) fn protobuf_limits(l: crate::protobuf::Limits, meta_size: usize) -> [u64; 12] {
    [
        l.max_schema_messages as u64,
        l.max_schema_fields as u64,
        l.max_fields_per_message as u64,
        l.max_depth as u64,
        l.max_input_bytes as u64,
        l.max_output_bytes as u64,
        l.max_payload_bytes as u64,
        l.max_nodes as u64,
        l.max_metadata_bytes as u64,
        l.max_work as u64,
        meta_size as u64,
        size_of::<FieldValue<'_>>() as u64,
    ]
}

fn pb_kind(kind: PbKind) -> (u64, u64) {
    match kind {
        PbKind::U32 => (0, 0),
        PbKind::U64 => (1, 0),
        PbKind::Int32 => (2, 0),
        PbKind::Int64 => (3, 0),
        PbKind::Sint32 => (4, 0),
        PbKind::Sint64 => (5, 0),
        PbKind::Bool => (6, 0),
        PbKind::Fixed32 => (7, 0),
        PbKind::Fixed64 => (8, 0),
        PbKind::Sfixed32 => (9, 0),
        PbKind::Sfixed64 => (10, 0),
        PbKind::String => (11, 0),
        PbKind::Bytes => (12, 0),
        PbKind::Message(child) => (13, child as u64),
    }
}

/// A schema as protobuf.ves reads it: the message count, one record offset
/// per message, then each message's field count and four words per field.
pub(crate) fn flatten_schema(messages: &[MessageSchema]) -> Vec<u64> {
    let mut words = vec![0u64; messages.len() + 1];
    words[0] = messages.len() as u64;
    for (i, message) in messages.iter().enumerate() {
        words[i + 1] = words.len() as u64;
        words.push(message.fields.len() as u64);
        for field in &message.fields {
            let (kind, child) = pb_kind(field.kind);
            words.extend([
                u64::from(field.number),
                kind,
                child,
                u64::from(field.repeated),
            ]);
        }
    }
    words
}

/// Rebuilds a parsed message from its stream (protobuf.ves), borrowing
/// strings and bytes from `input`.
pub(crate) fn protobuf_message<'a>(
    words: &[u64],
    at: &mut usize,
    input: &'a [u8],
) -> PbMessage<'a> {
    let count = words[*at] as usize;
    *at += 1;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let w = &words[*at..*at + 4];
        *at += 4;
        let (number, x) = (w[0] as u32, w[2]);
        let bytes = || &input[w[2] as usize..(w[2] + w[3]) as usize];
        let value = match w[1] {
            0 => PbValue::U32(x as u32),
            1 => PbValue::U64(x),
            2 => PbValue::Int32(x as i32),
            3 => PbValue::Int64(x as i64),
            4 => PbValue::Sint32(x as i32),
            5 => PbValue::Sint64(x as i64),
            6 => PbValue::Bool(x != 0),
            7 => PbValue::Fixed32(x as u32),
            8 => PbValue::Fixed64(x),
            9 => PbValue::Sfixed32(x as i32),
            10 => PbValue::Sfixed64(x as i64),
            // The port checked the UTF-8 (protobuf.ves `valid_up_to`).
            11 => PbValue::String(core::str::from_utf8(bytes()).expect("validated UTF-8")),
            12 => PbValue::Bytes(bytes()),
            _ => PbValue::Message(protobuf_message(words, at, input)),
        };
        fields.push(FieldValue { number, value });
    }
    PbMessage { fields }
}

/// A message as the encoder reads it (protobuf.ves), with every string and
/// byte payload copied to one arena. Iterative, so an arbitrarily deep
/// message cannot overflow the Rust stack before the port's budgets do.
pub(crate) fn flatten_protobuf(message: &PbMessage<'_>) -> (Vec<u64>, Vec<u8>) {
    let mut words = vec![message.fields.len() as u64];
    let mut arena = Vec::new();
    let mut stack = vec![message.fields.iter()];
    while let Some(fields) = stack.last_mut() {
        let Some(field) = fields.next() else {
            stack.pop();
            continue;
        };
        let number = u64::from(field.number);
        let mut bytes = |b: &[u8]| {
            let start = arena.len() as u64;
            arena.extend_from_slice(b);
            [start, b.len() as u64]
        };
        let (tag, x, y) = match &field.value {
            PbValue::U32(n) => (0, u64::from(*n), 0),
            PbValue::U64(n) => (1, *n, 0),
            PbValue::Int32(n) => (2, *n as i64 as u64, 0),
            PbValue::Int64(n) => (3, *n as u64, 0),
            PbValue::Sint32(n) => (4, *n as i64 as u64, 0),
            PbValue::Sint64(n) => (5, *n as u64, 0),
            PbValue::Bool(b) => (6, u64::from(*b), 0),
            PbValue::Fixed32(n) => (7, u64::from(*n), 0),
            PbValue::Fixed64(n) => (8, *n, 0),
            PbValue::Sfixed32(n) => (9, *n as i64 as u64, 0),
            PbValue::Sfixed64(n) => (10, *n as u64, 0),
            PbValue::String(text) => {
                let [x, y] = bytes(text.as_bytes());
                (11, x, y)
            }
            PbValue::Bytes(b) => {
                let [x, y] = bytes(b);
                (12, x, y)
            }
            PbValue::Message(child) => {
                words.extend([number, 13, 0, 0, child.fields.len() as u64]);
                stack.push(child.fields.iter());
                continue;
            }
        };
        words.extend([number, tag, x, y]);
    }
    (words, arena)
}
