//! Randomized differential check of every ported function against the frozen
//! Rust seed copy (`vesper-bench-rust`): the results, errors included, must
//! be identical, compared through their `Debug` text (the two builds share
//! one source, so equal values print equally). Under the default build both
//! sides are Rust and this is a self-check; under `vesper-port` it compares
//! the Vesper port with Rust, bit for bit. Inputs mix uniform words with the
//! edge values (zero, ties, `i64::MIN`, `u64::MAX`) where exact 128-bit
//! arithmetic matters.
use vesper_bench as port;
use vesper_bench_rust as rust;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn i64(&mut self) -> i64 {
        match self.below(8) {
            0 => [i64::MIN, i64::MAX, 0, -1, 1, i64::MIN + 1][self.below(6) as usize],
            1..=3 => self.below(21) as i64 - 10,
            4 | 5 => self.below(2001) as i64 - 1000,
            _ => self.next() as i64,
        }
    }
    fn u64(&mut self) -> u64 {
        match self.below(6) {
            0 => [0, 1, u64::MAX, u64::MAX - 1, 1 << 63][self.below(5) as usize],
            1 | 2 => self.below(20),
            3 => self.below(100_000),
            _ => self.next(),
        }
    }
    fn positive(&mut self) -> u64 {
        self.u64().max(1)
    }
    fn len(&mut self, max: u64) -> usize {
        match self.below(10) {
            0 => 0,
            1 => max as usize + 1 + self.below(3) as usize,
            _ => 1 + self.below(max) as usize,
        }
    }
}

fn same<A: std::fmt::Debug, B: std::fmt::Debug>(
    what: &str,
    input: &dyn std::fmt::Debug,
    a: A,
    b: B,
) {
    let (a, b) = (format!("{a:?}"), format!("{b:?}"));
    assert_eq!(a, b, "{what} diverged on {input:?}");
}

fn differences(rng: &mut Rng, max: u64) -> Vec<i64> {
    let n = rng.len(max);
    let narrow = rng.below(3) == 0;
    (0..n)
        .map(|_| {
            if narrow {
                rng.below(9) as i64 - 4
            } else {
                rng.i64()
            }
        })
        .collect()
}

/// The raw (baseline, candidate, operations) triples and both builds' samples.
type Samples = (
    Vec<(u64, u64, u64)>,
    Vec<port::PairedSample>,
    Vec<rust::PairedSample>,
);

fn samples(rng: &mut Rng, max: u64, common_ops: bool) -> Samples {
    let n = rng.len(max);
    let ops = rng.positive();
    let mut raw = Vec::new();
    for _ in 0..n {
        let operations = if common_ops || rng.below(4) != 0 {
            ops
        } else {
            rng.positive()
        };
        let baseline = rng.positive();
        let candidate = match rng.below(4) {
            0 => baseline,
            1 => baseline.saturating_add(rng.below(5).saturating_mul(operations)),
            _ => rng.positive(),
        };
        raw.push((baseline, candidate, operations));
    }
    let p = raw
        .iter()
        .map(|&(b, c, o)| port::PairedSample::new(b, c, o).unwrap())
        .collect();
    let r = raw
        .iter()
        .map(|&(b, c, o)| rust::PairedSample::new(b, c, o).unwrap())
        .collect();
    (raw, p, r)
}

#[test]
fn signed_rank_family_matches_rust() {
    let mut rng = Rng(0x5eed_0001);
    for trial in 0..400 {
        let max = if trial % 40 == 0 { 32 } else { 12 };
        let d = differences(&mut rng, max);
        same(
            "signed_rank_exact",
            &d,
            port::signed_rank_exact(&d),
            rust::signed_rank_exact(&d),
        );
        same(
            "signed_rank_confidence_set",
            &d,
            port::signed_rank_confidence_set(&d),
            rust::signed_rank_confidence_set(&d),
        );
        let threshold = rng.i64();
        same(
            "gate_signed_rank_confidence",
            &(&d, threshold),
            port::gate_signed_rank_confidence(&d, threshold),
            rust::gate_signed_rank_confidence(&d, threshold),
        );
        same(
            "walsh_pseudomedian",
            &d,
            port::walsh_pseudomedian(&d),
            rust::walsh_pseudomedian(&d),
        );
        same(
            "hodges_lehmann_paired",
            &d,
            port::hodges_lehmann_paired(&d),
            rust::hodges_lehmann_paired(&d),
        );
        same(
            "gate_paired_difference",
            &(&d, threshold),
            port::gate_paired_difference(&d, threshold),
            rust::gate_paired_difference(&d, threshold),
        );
    }
}

#[test]
fn untied_hodges_lehmann_intervals_match_rust() {
    // Distinct magnitudes and Walsh sums reach the interval code.
    let mut rng = Rng(0x5eed_0002);
    for _ in 0..200 {
        let n = 1 + rng.below(32) as usize;
        let d: Vec<i64> = (0..n)
            .map(|i| {
                let v = (1i64 << (i + 2)) + rng.below(3) as i64;
                if rng.below(2) == 0 { v } else { -v }
            })
            .collect();
        let threshold = rng.i64() >> rng.below(64);
        same(
            "hodges_lehmann_paired",
            &d,
            port::hodges_lehmann_paired(&d),
            rust::hodges_lehmann_paired(&d),
        );
        same(
            "gate_paired_difference",
            &(&d, threshold),
            port::gate_paired_difference(&d, threshold),
            rust::gate_paired_difference(&d, threshold),
        );
    }
}

#[test]
fn paired_sample_functions_match_rust() {
    let mut rng = Rng(0x5eed_0003);
    for trial in 0..400 {
        let max = if trial % 50 == 0 { 64 } else { 12 };
        let common = rng.below(2) == 0;
        let (raw, p, r) = samples(&mut rng, max, common);
        same(
            "signed_rank_exact_paired_samples",
            &raw,
            port::signed_rank_exact_paired_samples(&p),
            rust::signed_rank_exact_paired_samples(&r),
        );
        same(
            "exact_paired_differences",
            &raw,
            port::exact_paired_differences(&p),
            rust::exact_paired_differences(&r),
        );
        same(
            "paired_samples_confidence_set",
            &raw,
            port::paired_samples_confidence_set(&p),
            rust::paired_samples_confidence_set(&r),
        );
        let threshold = rng.i64();
        same(
            "gate_paired_sample_confidence",
            &(&raw, threshold),
            port::gate_paired_sample_confidence(&p, threshold),
            rust::gate_paired_sample_confidence(&r, threshold),
        );
        let bp = if rng.below(2) == 0 {
            rng.below(30_000) as i64 - 15_000
        } else {
            rng.i64()
        };
        same(
            "analyze_pairs",
            &(&raw, bp),
            port::analyze_pairs(&p, bp),
            rust::analyze_pairs(&r, bp),
        );
        same(
            "layout_survival",
            &raw,
            port::layout::layout_survival(&p),
            rust::layout::layout_survival(&r),
        );
        for (s, t) in p.iter().zip(&r) {
            same(
                "regression_bp_bounds",
                s,
                s.regression_bp_bounds(),
                t.regression_bp_bounds(),
            );
        }
        let (b, c, o) = (rng.u64(), rng.u64(), rng.u64());
        same(
            "PairedSample::new",
            &(b, c, o),
            port::PairedSample::new(b, c, o),
            rust::PairedSample::new(b, c, o),
        );
    }
}

#[test]
fn mann_whitney_matches_rust() {
    let mut rng = Rng(0x5eed_0004);
    for trial in 0..300 {
        let max = if trial % 30 == 0 { 64 } else { 14 };
        let pick = |rng: &mut Rng| -> Vec<u64> {
            let n = rng.len(max);
            let narrow = rng.below(2) == 0;
            (0..n)
                .map(|_| if narrow { rng.below(12) } else { rng.u64() })
                .collect()
        };
        let (a, b) = (pick(&mut rng), pick(&mut rng));
        same(
            "mann_whitney_ranks",
            &(&a, &b),
            port::mann_whitney_ranks(&a, &b),
            rust::mann_whitney_ranks(&a, &b),
        );
        same(
            "mann_whitney_exact",
            &(&a, &b),
            port::mann_whitney_exact(&a, &b),
            rust::mann_whitney_exact(&a, &b),
        );
        same(
            "mann_whitney_exact_with_ties",
            &(&a, &b),
            port::mann_whitney_exact_with_ties(&a, &b),
            rust::mann_whitney_exact_with_ties(&a, &b),
        );
    }
}

#[test]
fn classifiers_match_rust() {
    let mut rng = Rng(0x5eed_0005);
    let endpoint = |rng: &mut Rng| -> Option<i128> {
        match rng.below(5) {
            0 => None,
            1 => Some([i128::MIN, i128::MAX, 0][rng.below(3) as usize]),
            2 => Some(rng.below(9) as i128 - 4),
            _ => Some(((rng.next() as u128) << 64 | rng.next() as u128) as i128 >> rng.below(100)),
        }
    };
    for _ in 0..2000 {
        let n = rng.below(4) as usize;
        let segments: Vec<(Option<i128>, bool, Option<i128>, bool)> = (0..n)
            .map(|_| {
                (
                    endpoint(&mut rng),
                    rng.below(2) == 0,
                    endpoint(&mut rng),
                    rng.below(2) == 0,
                )
            })
            .collect();
        let ps: Vec<port::SignedRankConfidenceSegment> = segments
            .iter()
            .map(|&(l, li, u, ui)| port::SignedRankConfidenceSegment {
                lower_times_two: l,
                lower_inclusive: li,
                upper_times_two: u,
                upper_inclusive: ui,
            })
            .collect();
        let rs: Vec<rust::SignedRankConfidenceSegment> = segments
            .iter()
            .map(|&(l, li, u, ui)| rust::SignedRankConfidenceSegment {
                lower_times_two: l,
                lower_inclusive: li,
                upper_times_two: u,
                upper_inclusive: ui,
            })
            .collect();
        let max = rng.i64();
        let ops = rng.positive();
        same(
            "classify_signed_rank_set",
            &(&segments, max),
            port::classify_signed_rank_set(&ps, max),
            rust::classify_signed_rank_set(&rs, max),
        );
        same(
            "classify_paired_sample_set",
            &(&segments, ops, max),
            port::classify_paired_sample_set(&ps, ops, max),
            rust::classify_paired_sample_set(&rs, ops, max),
        );
        let e = endpoint(&mut rng).unwrap_or(0);
        same(
            "compare_paired_endpoint",
            &(e, ops, max),
            port::compare_paired_endpoint(e, ops, max),
            rust::compare_paired_endpoint(e, ops, max),
        );
    }
}

#[test]
fn arithmetic_and_layout_match_rust() {
    let mut rng = Rng(0x5eed_0006);
    for _ in 0..3000 {
        let (a, b) = (rng.u64(), rng.u64());
        let width = rng.below(70) as u8;
        let (a, b) = if rng.below(2) == 0 && width > 0 && width < 64 {
            (a % (1 << width), b % (1 << width))
        } else {
            (a, b)
        };
        same(
            "modular_counter_delta",
            &(a, b, width),
            port::modular_counter_delta(a, b, width),
            rust::modular_counter_delta(a, b, width),
        );
        same(
            "layout_offset_bytes",
            &(a, b),
            port::layout_offset_bytes(a, b),
            rust::layout_offset_bytes(a, b),
        );
        let run = rng.next() as u32;
        same(
            "layout_plan",
            &(a, run),
            port::layout::layout_plan(a, run),
            rust::layout::layout_plan(a, run),
        );
        same(
            "backend_bound_bp",
            &(a, b),
            port::wire::backend_bound_bp(a, b),
            rust::wire::backend_bound_bp(a, b),
        );
        let bits = rng.next() as u16 >> rng.below(16);
        same(
            "AvailabilityMask::new",
            &bits,
            port::wire::AvailabilityMask::new(bits),
            rust::wire::AvailabilityMask::new(bits),
        );
    }
}

#[test]
fn evidence_requirements_match_rust() {
    use port::EvidenceRequirement as P;
    use rust::EvidenceRequirement as R;
    let pairs = [
        (P::HardwareCycles, R::HardwareCycles),
        (P::RetiredInstructions, R::RetiredInstructions),
        (P::L1DataMisses, R::L1DataMisses),
        (P::LlcMisses, R::LlcMisses),
        (P::BranchMispredicts, R::BranchMispredicts),
        (P::SimdVectorOperations, R::SimdVectorOperations),
        (P::TmaBackendSlots, R::TmaBackendSlots),
        (P::ZeroAllocation, R::ZeroAllocation),
        (P::Vectorization, R::Vectorization),
    ];
    for (p, r) in pairs {
        same(
            "unavailable_evidence",
            &p,
            port::unavailable_evidence(p),
            rust::unavailable_evidence(r),
        );
        same(
            "evaluate_evidence_requirement",
            &p,
            port::evaluate_evidence_requirement(p),
            rust::evaluate_evidence_requirement(r),
        );
    }
}

const TOKENS: [&str; 24] = [
    "@",
    "bench",
    "(",
    ")",
    ",",
    "=",
    " ",
    "\t",
    "\n",
    "true",
    "false",
    "zero_alloc",
    "expect_vectorized",
    "max_instructions_per_byte",
    "stabilizer_pages",
    "max_l1d_misses_per_op",
    "12",
    "0.25",
    "18446744073709551616",
    "1.0000000000000000001",
    "x",
    "Bench",
    ".",
    "7",
];

#[test]
fn contract_parser_and_gate_match_rust() {
    let mut rng = Rng(0x5eed_0007);
    let fields = [
        "zero_alloc = true",
        "zero_alloc = false",
        "expect_vectorized = true",
        "max_instructions_per_byte = 2.5",
        "max_instructions_per_byte = 0.000000000000000001",
        "stabilizer_pages = 4",
        "max_l1d_misses_per_op = 3",
    ];
    for trial in 0..4000 {
        let source = if trial % 2 == 0 {
            let n = rng.below(5) as usize;
            let mut s = String::from("@bench(");
            for i in 0..n {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(fields[rng.below(fields.len() as u64) as usize]);
            }
            s.push(')');
            s
        } else {
            let n = rng.below(12) as usize;
            (0..n)
                .map(|_| TOKENS[rng.below(TOKENS.len() as u64) as usize])
                .collect()
        };
        let p = port::contract::parse_attribute(&source);
        same(
            "parse_attribute",
            &source,
            &p,
            rust::contract::parse_attribute(&source),
        );
        let Ok(contract) = p else { continue };
        let rc = rust::contract::parse_attribute(&source).unwrap();
        let (mut ps, mut rs) = (port::wire::PmuSample::new(), rust::wire::PmuSample::new());
        for (pf, rf) in [
            (
                port::wire::PmuField::InstructionsRetired,
                rust::wire::PmuField::InstructionsRetired,
            ),
            (
                port::wire::PmuField::L1dReadMisses,
                rust::wire::PmuField::L1dReadMisses,
            ),
        ] {
            if rng.below(3) != 0 {
                let v = rng.u64();
                ps.set(pf, v).unwrap();
                rs.set(rf, v).unwrap();
            }
        }
        let opt = |rng: &mut Rng| {
            if rng.below(3) == 0 {
                None
            } else {
                Some(rng.u64())
            }
        };
        let (bytes, ops) = (opt(&mut rng), opt(&mut rng));
        same(
            "contract_gate::evaluate",
            &(&source, bytes, ops),
            port::contract_gate::evaluate(
                &contract,
                &ps,
                port::contract_gate::Counts {
                    input_bytes: bytes,
                    operations: ops,
                },
            ),
            rust::contract_gate::evaluate(
                &rc,
                &rs,
                rust::contract_gate::Counts {
                    input_bytes: bytes,
                    operations: ops,
                },
            ),
        );
    }
    // Programmatic contracts may carry an out-of-range scale.
    for scale in [18, 19, u32::MAX] {
        let pc = port::contract::BenchContract {
            max_instructions_per_byte: Some(port::contract::Decimal {
                coefficient: 1,
                scale,
            }),
            ..Default::default()
        };
        let rc = rust::contract::BenchContract {
            max_instructions_per_byte: Some(rust::contract::Decimal {
                coefficient: 1,
                scale,
            }),
            ..Default::default()
        };
        same(
            "contract_gate::evaluate",
            &scale,
            port::contract_gate::evaluate(&pc, &port::wire::PmuSample::new(), Default::default()),
            rust::contract_gate::evaluate(&rc, &rust::wire::PmuSample::new(), Default::default()),
        );
    }
}

#[test]
fn wire_records_match_rust() {
    let mut rng = Rng(0x5eed_0008);
    for _ in 0..3000 {
        let mut bytes: Vec<u8> = (0..64).map(|_| rng.next() as u8).collect();
        if rng.below(2) == 0 {
            // A valid basis-point count and layout offset.
            bytes[56..60].copy_from_slice(&(rng.below(10_001) as u32).to_le_bytes());
            bytes[60..64].copy_from_slice(&((rng.below(64) * 64) as u32).to_le_bytes());
        }
        let len = if rng.below(8) == 0 {
            rng.below(70) as usize
        } else {
            64
        };
        bytes.resize(len, 0);
        let p = port::wire::PmuSampleWire::decode(&bytes);
        same(
            "PmuSampleWire::decode",
            &bytes,
            &p,
            rust::wire::PmuSampleWire::decode(&bytes),
        );
        if let Ok(wire) = p {
            let r = rust::wire::PmuSampleWire::decode(&bytes).unwrap();
            same("PmuSampleWire::encode", &bytes, wire.encode(), r.encode());
        }
        let (mut ps, mut rs) = (port::wire::PmuSample::new(), rust::wire::PmuSample::new());
        for (i, (&pf, &rf)) in port::wire::PmuField::ALL
            .iter()
            .zip(&rust::wire::PmuField::ALL)
            .enumerate()
        {
            if rng.below(4) == 0 {
                continue;
            }
            let v = match (i, rng.below(3)) {
                (7, 0) => rng.below(10_002),
                (8, 0) => rng.below(65) * 64,
                _ => rng.u64(),
            };
            same("PmuSample::set", &(i, v), ps.set(pf, v), rs.set(rf, v));
        }
        same(
            "PmuSample::to_wire_masked",
            &ps,
            ps.to_wire_masked(),
            rs.to_wire_masked(),
        );
        same("PmuSample::to_wire", &ps, ps.to_wire(), rs.to_wire());
    }
}
