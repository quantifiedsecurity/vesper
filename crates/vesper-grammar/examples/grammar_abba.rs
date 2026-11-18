//! ABBA benchmark of Protobuf and DER decoding, comparing the Rust build
//! (the seed package) with the Vesper port (port wave P1 lane 9, #906).
//!
//! One binary holds one build, so the arms are two binaries:
//!
//! ```sh
//! cargo build --release -p vesper-grammar-seed --example grammar_abba_seed
//! cp target/release/examples/grammar_abba_seed /tmp/rust_arm
//! cargo build --release -p vesper-grammar --example grammar_abba
//! cp target/release/examples/grammar_abba /tmp/port_arm
//! /tmp/rust_arm driver /tmp/rust_arm /tmp/port_arm
//! ```
//!
//! `window pb|der N` prints the nanoseconds of N decodes after one warm-up
//! decode. `driver A B` runs epochs of windows in the order A B B A for each
//! workload, pairs each epoch's A and B totals and reports the median
//! regression of B over A with `vesper_bench::analyze_pairs`, plus the 2x
//! stop rule of the port plan.
use std::process::Command;
use std::time::Instant;
use vesper_grammar::{der, protobuf as pb};

const EPOCHS: usize = 9;

/// Protobuf: a record with a scalar, a string, 16 nested messages and 128
/// packed u32 values (about 1 KiB on the wire).
fn protobuf_case() -> (Vec<pb::MessageSchema>, Vec<u8>) {
    let field = |number, kind, repeated| pb::FieldSchema {
        number,
        kind,
        repeated,
    };
    let messages = vec![
        pb::MessageSchema {
            fields: vec![
                field(1, pb::Kind::U64, false),
                field(2, pb::Kind::String, false),
                field(3, pb::Kind::Message(1), true),
                field(4, pb::Kind::U32, true),
            ],
        },
        pb::MessageSchema {
            fields: vec![
                field(1, pb::Kind::Sint64, false),
                field(2, pb::Kind::Bytes, false),
            ],
        },
    ];
    let payload = [0x5a; 24];
    let mut fields = vec![
        pb::FieldValue {
            number: 1,
            value: pb::Value::U64(1 << 40),
        },
        pb::FieldValue {
            number: 2,
            value: pb::Value::String("grammar port benchmark record"),
        },
    ];
    for i in 0..16 {
        fields.push(pb::FieldValue {
            number: 3,
            value: pb::Value::Message(pb::Message {
                fields: vec![
                    pb::FieldValue {
                        number: 1,
                        value: pb::Value::Sint64(-(i * 1000)),
                    },
                    pb::FieldValue {
                        number: 2,
                        value: pb::Value::Bytes(&payload),
                    },
                ],
            }),
        });
    }
    for i in 0..128u32 {
        fields.push(pb::FieldValue {
            number: 4,
            value: pb::Value::U32(i * 997),
        });
    }
    let message = pb::Message { fields };
    let schema = pb::Schema::validate(&messages, 0, pb::Limits::default()).unwrap();
    let mut out = vec![0; schema.encoded_size(&message).unwrap()];
    schema.encode_into(&message, &mut out).unwrap();
    (messages, out)
}

/// DER: a certificate-shaped tree of 16 entries, each a SEQUENCE of an
/// INTEGER, an OID, a UTF8String, a BIT STRING and an OCTET STRING (about
/// 1.5 KiB).
fn der_case() -> Vec<u8> {
    let payload = [0xa5; 32];
    let oid = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 1, 1, 11];
    let ints: Vec<[u8; 2]> = (0..16u8).map(|i| [0x01, i]).collect();
    let entries: Vec<[der::Value<'_>; 5]> = ints
        .iter()
        .map(|int| {
            [
                der::Value::Integer(int),
                der::Value::ObjectIdentifier(&oid),
                der::Value::Utf8String("grammar port benchmark"),
                der::Value::BitString {
                    unused_bits: 0,
                    bytes: &payload,
                },
                der::Value::OctetString(&payload),
            ]
        })
        .collect();
    let children: Vec<der::Value<'_>> = entries.iter().map(|e| der::Value::Sequence(e)).collect();
    let root = der::Value::Sequence(&children);
    let mut out = vec![0; 4096];
    let n = der::encode_into(&root, &der::Schema::Any, &mut out, der::Limits::default()).unwrap();
    out.truncate(n);
    out
}

fn window(workload: &str, passes: usize) -> u128 {
    match workload {
        "pb" => {
            let (messages, bytes) = protobuf_case();
            let schema = pb::Schema::validate(&messages, 0, pb::Limits::default()).unwrap();
            std::hint::black_box(schema.parse_exact(&bytes).unwrap());
            let start = Instant::now();
            for _ in 0..passes {
                std::hint::black_box(schema.parse_exact(std::hint::black_box(&bytes)).unwrap());
            }
            start.elapsed().as_nanos()
        }
        "der" => {
            let bytes = der_case();
            let decode = |b: &[u8]| {
                der::parse_exact(b, &der::Schema::Any, der::Limits::default())
                    .unwrap()
                    .node_count()
            };
            std::hint::black_box(decode(&bytes));
            let start = Instant::now();
            for _ in 0..passes {
                std::hint::black_box(decode(std::hint::black_box(&bytes)));
            }
            start.elapsed().as_nanos()
        }
        _ => panic!("unknown workload {workload}"),
    }
}

fn run(bin: &str, workload: &str, passes: usize) -> u64 {
    let out = Command::new(bin)
        .args(["window", workload, &passes.to_string()])
        .output()
        .expect("run arm");
    assert!(out.status.success(), "{bin} failed");
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("window") => println!("{}", window(&args[2], args[3].parse().unwrap())),
        Some("driver") => {
            let (a, b) = (&args[2], &args[3]);
            for (workload, passes, bytes) in [
                ("pb", 20_000, protobuf_case().1.len()),
                ("der", 20_000, der_case().len()),
            ] {
                let mut pairs = Vec::new();
                for epoch in 0..EPOCHS {
                    let a1 = run(a, workload, passes);
                    let b1 = run(b, workload, passes);
                    let b2 = run(b, workload, passes);
                    let a2 = run(a, workload, passes);
                    let n = 2 * passes as u64;
                    println!(
                        "{workload} epoch {epoch}: A {:.2} us/decode, B {:.2} us/decode, B/A {:.3}",
                        (a1 + a2) as f64 / n as f64 / 1e3,
                        (b1 + b2) as f64 / n as f64 / 1e3,
                        (b1 + b2) as f64 / (a1 + a2) as f64
                    );
                    pairs.push(vesper_bench::PairedSample::new(a1 + a2, b1 + b2, n).unwrap());
                }
                let analysis = vesper_bench::analyze_pairs(&pairs, 10_000).unwrap();
                println!(
                    "{workload} ({bytes} bytes): 2x gate (B at most +10000 bp over A): {:?}; median regression bp {:?}",
                    analysis.decision,
                    analysis.median_upper.regression_bp_bounds()
                );
            }
        }
        _ => {
            eprintln!("usage: grammar_abba window pb|der N | driver RUST_BIN PORT_BIN");
            std::process::exit(2);
        }
    }
}
