//! Untrusted bytes never panic (port wave P1 lane 9, #906): 10^5 random
//! inputs through every decoder of the crate, against whichever build this
//! package links. Each outcome (value or error) is folded into a digest,
//! printed with `--nocapture`, so the Rust and Vesper builds can be compared
//! on the same inputs.
use vesper_grammar::grammar::codec::{CodecLimits, decode_exact};
use vesper_grammar::grammar::{Branch, Grammar, Limits as GrammarLimits, Rule};
use vesper_grammar::protobuf::{FieldSchema, Kind, Limits as PbLimits, MessageSchema, Schema};
use vesper_grammar::{capnp, capnp_packed, der, framing, ipc};

const CASES: u64 = 100_000;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// Mostly short inputs, some biased toward small or 0xff bytes so the
    /// length and tag paths are reached.
    fn bytes(&mut self, out: &mut Vec<u8>) {
        out.clear();
        let len = match self.next() % 8 {
            0..=4 => self.next() % 16,
            5 | 6 => self.next() % 96,
            _ => self.next() % 512,
        } as usize;
        let style = self.next() % 4;
        for _ in 0..len {
            let r = self.next();
            out.push(match style {
                0 => (r % 4) as u8,
                1 => [0, 0x30, 0x31, 0x02, 0x80, 0x81, 0xff][(r % 7) as usize],
                _ => r as u8,
            });
        }
    }
}

struct Digest(u64);
impl Digest {
    fn add(&mut self, value: impl core::fmt::Debug) {
        for b in format!("{value:?}").bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

#[test]
fn random_bytes_never_panic_in_any_decoder() {
    let grammar_rules = vec![
        Rule::Choice(vec![
            Branch { id: 1, rule: 1 },
            Branch { id: 2, rule: 4 },
            Branch { id: 3, rule: 7 },
        ]),
        Rule::Sequence(vec![2, 3]),
        Rule::Byte(0),
        Rule::Counted(8),
        Rule::Sequence(vec![5, 6]),
        Rule::Byte(1),
        Rule::Frame(9),
        Rule::Sequence(vec![10, 11]),
        Rule::Uleb128,
        Rule::Blob,
        Rule::Byte(2),
        Rule::U24Frame(12),
        Rule::Repeat { rule: 8, count: 2 },
    ];
    let grammar = Grammar::validate(&grammar_rules, 0, GrammarLimits::default()).unwrap();
    let field = |number, kind, repeated| FieldSchema {
        number,
        kind,
        repeated,
    };
    let messages = [
        MessageSchema {
            fields: vec![
                field(1, Kind::U64, false),
                field(2, Kind::String, false),
                field(3, Kind::Message(1), true),
                field(4, Kind::Int32, true),
                field(5, Kind::Bytes, false),
                field(6, Kind::Bool, false),
            ],
        },
        MessageSchema {
            fields: vec![field(1, Kind::Fixed32, false), field(2, Kind::Sint64, true)],
        },
    ];
    let schema = Schema::validate(&messages, 0, PbLimits::default()).unwrap();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut input = Vec::new();
    let mut out = vec![0u8; 8192];
    let mut digest = Digest(0xcbf2_9ce4_8422_2325);
    for _ in 0..CASES {
        rng.bytes(&mut input);
        let fl = framing::Limits::default();
        digest.add(framing::decode_canonical_u64_leb128(&input));
        digest.add(framing::parse_frame(&input, fl));
        digest.add(framing::parse_u24_frame(&input, fl));
        digest.add(framing::parse_u64(&input, fl));
        digest.add(
            capnp::parse_prefix(&input, capnp::Limits::default())
                .map(|(m, t)| (m.segments().count(), t.len())),
        );
        let pl = capnp_packed::Limits::default();
        digest.add(capnp_packed::unpacked_len(&input, pl));
        digest.add(capnp_packed::unpack_into(&input, &mut out, pl));
        digest.add(capnp_packed::packed_len(&input, pl));
        digest.add(
            grammar
                .parse_prefix(&input)
                .map(|p| (p.captures, p.tail.offset())),
        );
        digest.add(decode_exact(&grammar, &input, CodecLimits::default()));
        digest.add(schema.parse_exact(&input));
        digest.add(
            der::parse_prefix(&input, &der::Schema::Any, der::Limits::default()).map(|(d, n)| {
                let kinds: Vec<_> = d.nodes().map(|n| (n.kind(), n.raw().len())).collect();
                (kinds, n)
            }),
        );
        digest.add(ipc::parse_prefix(&input, ipc::Limits::default()).map(|p| p.consumed()));
    }
    println!("no_panic digest {:016x} over {CASES} inputs", digest.0);
}
