//! ABBA comparison of the Rust build (the seed copy, `vesper_silicon_rust`,
//! baseline) and the Vesper port (this package, candidate), in one process:
//! each epoch times every workload in Rust, port, port, Rust order, and
//! vesper-bench analyzes the paired totals. Port wave P3a lane 7 (#1042).
//!
//! `abba compare <epochs> <ops>` prints one line per workload.
//! `abba one <rust|port> <workload> <ops>` runs one workload `ops` times
//! (for `valgrind --tool=callgrind`).
//!
//! Workloads, one run each:
//! - `dtb-parse`: `DeviceTree::parse` of the largest fixture (the StarFive
//!   VisionFive 2 tree, 36,175 bytes, 2,0xx properties).
//! - `dtb-platform`: `Platform::from_dtb` of the same tree and a query of
//!   every interrupt line's device (`mmio`, `irqs_of`, `dma_coherence`) and
//!   the usable frame ranges. Only the parse is ported so far; the queries
//!   run the same Rust code in both builds.
//! - `aia-intc`: the QEMU `aia=aplic-imsic` platform with a level-high
//!   UART through the S-level APLIC domain in MSI mode and the S-level IMSIC
//!   (tests/aia.rs `supervisor_uart_through_aplic_msi_and_imsic`), 64
//!   assertions of the line, each serviced by its handler.
//! - `aplic-direct`: a direct-mode APLIC domain with 63 sources and 2 IDCs:
//!   every source configured, then 256 rounds of wire changes and claims.
//! - `imsic`: a 2,047-identity IMSIC file: every identity enabled, then
//!   256 rounds of MSIs and claims.

use std::time::Instant;
use vesper_bench::{PairedSample, analyze_pairs};

const JH7110: &[u8] = include_bytes!("../fixtures/dtb/jh7110-starfive-visionfive-2-v1.3b.dtb");
const IMSIC: &[u8] = include_bytes!("../fixtures/dtb/qemu-riscv64-virt-aplic-imsic.dtb");
const UART: &str = "/soc/serial@10000000";

macro_rules! workloads {
    ($m:ident, $s:ident) => {
        mod $m {
            use super::{IMSIC, JH7110, UART};
            use $s::aia::{AplicDomain, ISEL_EIE0, ImsicFile};
            use $s::dtb::{DeviceTree, MemRange, Platform};
            use $s::intc::{Ctl, Privilege, RiscvIntc};
            use $s::ledger::Ledger;

            pub fn dtb_parse(_: u64) -> u64 {
                let t = DeviceTree::parse(JH7110).unwrap();
                t.nodes().iter().map(|n| n.props.len() as u64).sum()
            }

            pub fn dtb_platform(_: u64) -> u64 {
                let p = Platform::from_dtb(JH7110).unwrap();
                let mut acc = p.usable_frame_ranges().unwrap().len() as u64;
                for l in &p.irq_lines {
                    acc += p.mmio(&l.device).map_or(0, |m| m.len() as u64);
                    acc += p.irqs_of(&l.device).map_or(0, |v| v.len() as u64);
                    acc += u64::from(p.dma_coherence(&l.device).is_ok());
                }
                acc
            }

            pub fn aia_intc(_: u64) -> u64 {
                let p = Platform::from_dtb(IMSIC).unwrap();
                let ledger = Ledger::new();
                let mut intc = RiscvIntc::new(&p, Privilege::Supervisor, &ledger).unwrap();
                intc.install_isr(&p, UART, 1, 7, move |ev| {
                    ev.intc
                        .set_controller_line(Ctl::Aplic(0), 10, false)
                        .unwrap();
                })
                .unwrap();
                for _ in 0..64 {
                    intc.set_device_line(&p, UART, true).unwrap();
                }
                intc.deliveries().len() as u64
            }

            pub fn aplic_direct(seed: u64) -> u64 {
                let base = 0x1000_0000u64;
                let window = MemRange { base, size: 0x8000 };
                let mut d = AplicDomain::new("/d", window, 63, 2, false, true, false, 0).unwrap();
                d.write32(base, 0x100).unwrap();
                for s in 1..=63u32 {
                    let mode = [4, 5, 6, 7][(s % 4) as usize];
                    d.write32(base + 4 * u64::from(s), mode).unwrap();
                    d.write32(base + 0x3000 + 4 * u64::from(s), ((s % 2) << 18) | (s % 7))
                        .unwrap();
                    d.write32(base + 0x1edc, s).unwrap();
                }
                for i in 0..2u64 {
                    d.write32(base + 0x4000 + 32 * i, 1).unwrap();
                }
                let mut acc = 0u64;
                let mut x = seed | 1;
                for _ in 0..256 {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    let s = (x % 63) as u32 + 1;
                    d.set_wire(s, x & 64 != 0);
                    let idc = (x >> 8) & 1;
                    acc += u64::from(d.read32(base + 0x4000 + 32 * idc + 0x1c).unwrap());
                    acc += u64::from(d.idc_asserted(idc as usize));
                }
                acc
            }

            pub fn imsic(seed: u64) -> u64 {
                let mut f = ImsicFile::new(2047).unwrap();
                f.csr_write(0x70, 1).unwrap();
                for w in 0..32u32 {
                    f.csr_write(ISEL_EIE0 + 2 * w, u64::MAX).unwrap();
                }
                let mut acc = 0u64;
                let mut x = seed | 1;
                for _ in 0..256 {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    f.seteipnum((x % 2047) as u32 + 1);
                    f.mmio_write32(0, ((x >> 16) % 2047) as u32 + 1);
                    if x & 3 == 0 {
                        acc += u64::from(f.claim());
                    }
                    acc += u64::from(f.asserted());
                }
                acc
            }
        }
    };
}

workloads!(rust_build, vesper_silicon_rust);
workloads!(port_build, vesper_silicon);

type Work = fn(u64) -> u64;

const WORKLOADS: [(&str, Work, Work); 5] = [
    ("dtb-parse", rust_build::dtb_parse, port_build::dtb_parse),
    (
        "dtb-platform",
        rust_build::dtb_platform,
        port_build::dtb_platform,
    ),
    ("aia-intc", rust_build::aia_intc, port_build::aia_intc),
    (
        "aplic-direct",
        rust_build::aplic_direct,
        port_build::aplic_direct,
    ),
    ("imsic", rust_build::imsic, port_build::imsic),
];

/// Total nanoseconds for `ops` timed runs after `ops / 4` warm-up ones.
fn measure(work: Work, ops: u64) -> u64 {
    let mut sink = 0;
    for seed in 0..ops / 4 {
        sink ^= work(seed);
    }
    let start = Instant::now();
    for seed in 0..ops {
        sink ^= work(seed);
    }
    let elapsed = start.elapsed().as_nanos() as u64;
    std::hint::black_box(sink);
    elapsed
}

fn compare(epochs: usize, ops: u64) {
    for (name, rust, port) in WORKLOADS {
        // Both builds compute the same answers.
        for seed in 0..4 {
            assert_eq!(rust(seed), port(seed), "{name} differs");
        }
        let mut pairs = Vec::new();
        for _ in 0..epochs {
            let a1 = measure(rust, ops);
            let b1 = measure(port, ops);
            let b2 = measure(port, ops);
            let a2 = measure(rust, ops);
            pairs.push(PairedSample::new(a1 + a2, b1 + b2, 2 * ops).unwrap());
        }
        let per_op = |t: u64| t as f64 / (2 * ops) as f64;
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let rust_ns = median(pairs.iter().map(|p| per_op(p.baseline_total())).collect());
        let port_ns = median(pairs.iter().map(|p| per_op(p.candidate_total())).collect());
        let ratios: Vec<f64> = pairs
            .iter()
            .map(|p| p.candidate_total() as f64 / p.baseline_total() as f64)
            .collect();
        let lo = ratios.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = ratios.iter().copied().fold(0.0, f64::max);
        let ratio = median(ratios);
        let analysis = analyze_pairs(&pairs, 0).unwrap();
        let bp = |s: PairedSample| s.regression_bp_bounds().floor as f64 / 100.0;
        println!(
            "{name:<13} rust {rust_ns:>10.0} ns/run  port {port_ns:>10.0} ns/run  port/rust {ratio:.2}x \
             (range {lo:.2}x..{hi:.2}x, sample median regression {:+.0}%..{:+.0}%, {epochs} epochs)",
            bp(analysis.median_lower),
            bp(analysis.median_upper),
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["compare", epochs, ops] => compare(epochs.parse().unwrap(), ops.parse().unwrap()),
        ["one", build, workload, ops] => {
            let (_, rust, port) = WORKLOADS
                .iter()
                .find(|w| w.0 == *workload)
                .expect("a workload name");
            let work = if *build == "rust" { rust } else { port };
            let mut sink = 0;
            for seed in 0..ops.parse::<u64>().unwrap() {
                sink ^= work(seed);
            }
            println!("{sink}");
        }
        _ => {
            eprintln!("usage: abba compare <epochs> <ops> | abba one <rust|port> <workload> <ops>");
            std::process::exit(2);
        }
    }
}
