//! The scalar stage 1: the per-byte reference for the SIMD kernels
//! (`kernels.rs`) and `Isa::Scalar`'s implementation. It is kept apart from
//! the kernels and from the shared index machinery in `mod.rs` so that it
//! is a self-contained codec a port can replace: it reads input bytes and
//! writes `BlockMasks` through the shared `block_finish` and `Sink`, and
//! needs no `unsafe` and no target features.
use super::*;

fn classify(block: &[u8; 64]) -> RawMasks {
    let mut m = RawMasks::default();
    for (i, &b) in block.iter().enumerate() {
        let bit = 1u64 << i;
        match b {
            b'\\' => m.backslash |= bit,
            b'"' => m.quote |= bit,
            b'{' | b'}' | b'[' | b']' | b':' | b',' => m.op |= bit,
            _ => {}
        }
        if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
            m.whitespace |= bit;
        }
        if b < 0x20 {
            m.control |= bit;
        }
    }
    m
}

/// Independent per-bit reference for quotes and string interiors.
fn reference_block(raw: RawMasks, carry: &mut Carry) -> BlockMasks {
    let (escaped, carry_out) =
        crate::compute_escaped_mask_64(raw.backslash, carry.prev_escaped != 0);
    carry.prev_escaped = u64::from(carry_out);
    let quotes = raw.quote & !escaped;
    let mut parity = 0u64;
    let mut inside = false;
    for i in 0..64 {
        if quotes & (1 << i) != 0 {
            inside = !inside;
        }
        if inside {
            parity |= 1 << i;
        }
    }
    block_finish(raw, escaped, quotes, parity, carry)
}

pub(super) fn run<R: Recorder>(input: &[u8], sink: &mut Sink<'_>, rec: &mut R) -> RunOutcome {
    let mut carry = Carry::default();
    let (blocks, rest) = input.as_chunks::<64>();
    let mut base = 0u32;
    for block in blocks {
        let m = reference_block(classify(block), &mut carry);
        rec.record(&m);
        sink.push(&m, base);
        base += 64;
    }
    if !rest.is_empty() {
        let m = reference_block(classify(&padded_tail(rest)), &mut carry);
        rec.record(&m);
        sink.push(&m, base);
    }
    RunOutcome {
        utf8_ok: core::str::from_utf8(input).is_ok(),
        carry,
    }
}
