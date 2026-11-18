//! Differential tests of the port-backed package against the frozen Rust
//! build (the seed copy, `vesper_silicon_rust`): the same inputs give the
//! same results and the same errors. Port wave P3a lane 7 (#1042).
//!
//! The values of the two builds are different Rust types, so results are
//! compared through their `Debug` text (both builds derive the same
//! formatting for every type compared here).

use vesper_silicon as port;
use vesper_silicon_rust as rust;

fn rng(seed: &mut u64) -> u64 {
    // splitmix64
    *seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[test]
fn builds_are_the_port_and_the_seed() {
    assert_eq!(port::IMPLEMENTATION, "vesper");
    assert_eq!(rust::IMPLEMENTATION, "rust");
}

// ---------------------------------------------------------------- devicetree

const FIXTURES: &[&[u8]] = &[
    include_bytes!("../fixtures/dtb/hifive-unleashed-a00.dtb"),
    include_bytes!("../fixtures/dtb/hifive-unmatched-a00-v16.dtb"),
    include_bytes!("../fixtures/dtb/hifive-unmatched-a00.dtb"),
    include_bytes!("../fixtures/dtb/jh7100-starfive-visionfive-v1.dtb"),
    include_bytes!("../fixtures/dtb/jh7110-starfive-visionfive-2-v1.3b.dtb"),
    include_bytes!("../fixtures/dtb/qemu-aarch64-virt-gicv3.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt-2socket.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt-aclint.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt-aia-2socket.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt-aplic-imsic-guests.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt-aplic-imsic.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt-aplic.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt.dtb"),
    include_bytes!("../fixtures/dtb/sun20i-d1-nezha.dtb"),
    include_bytes!("../fixtures/dtb/vesper-aia-edge.dtb"),
    include_bytes!("../fixtures/dtb/vesper-aia-hyp.dtb"),
    include_bytes!("../fixtures/dtb/vesper-edge-cases.dtb"),
    include_bytes!("../fixtures/dtb/vesper-nexus-loop.dtb"),
    include_bytes!("../fixtures/dtb/vesper-nexus-ranges.dtb"),
    include_bytes!("../fixtures/dtb/vesper-rv-board-v16.dtb"),
    include_bytes!("../fixtures/dtb/vesper-rv-board.dtb"),
];

/// Both builds' parse of `blob`, as text; the error variant name, if any.
fn parse_both(blob: &[u8]) -> (String, Option<String>) {
    let p = format!("{:?}", port::dtb::DeviceTree::parse(blob));
    let r = rust::dtb::DeviceTree::parse(blob);
    let kind = r.as_ref().err().map(|e| {
        let d = format!("{e:?}");
        d.split([' ', '(', '{']).next().unwrap().to_string()
    });
    let r = format!("{r:?}");
    assert_eq!(p, r, "the builds disagree on a {}-byte blob", blob.len());
    (p, kind)
}

/// A flattened devicetree with the given nodes, written the way `dtc`
/// lays one out. Each node is (depth, name, properties); depth 0 is the
/// root.
/// A node of [`write_dtb`]: depth, name and properties.
type DtNode<'a> = (usize, &'a str, Vec<(&'a str, Vec<u8>)>);

fn write_dtb(version: u32, nodes: &[DtNode<'_>]) -> Vec<u8> {
    let mut strings: Vec<u8> = Vec::new();
    let mut structs: Vec<u8> = Vec::new();
    let name_off = |strings: &mut Vec<u8>, n: &str| -> u32 {
        let at = strings.len() as u32;
        strings.extend_from_slice(n.as_bytes());
        strings.push(0);
        at
    };
    let push = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_be_bytes());
    let pad = |v: &mut Vec<u8>| {
        while !v.len().is_multiple_of(4) {
            v.push(0);
        }
    };
    let mut depth = 0usize;
    for (i, (d, name, props)) in nodes.iter().enumerate() {
        while i > 0 && depth >= *d {
            push(&mut structs, 2);
            depth -= 1;
        }
        push(&mut structs, 1);
        structs.extend_from_slice(name.as_bytes());
        structs.push(0);
        pad(&mut structs);
        for (pn, value) in props {
            push(&mut structs, 3);
            push(&mut structs, value.len() as u32);
            let off = name_off(&mut strings, pn);
            push(&mut structs, off);
            structs.extend_from_slice(value);
            pad(&mut structs);
        }
        depth = *d;
    }
    loop {
        push(&mut structs, 2);
        if depth == 0 {
            break;
        }
        depth -= 1;
    }
    push(&mut structs, 9);
    let header = if version >= 17 { 40 } else { 36 };
    let rsv = 40usize.next_multiple_of(8).max(header);
    let off_struct = rsv + 32;
    let off_strings = off_struct + structs.len();
    let total = off_strings + strings.len();
    let mut b = Vec::new();
    for x in [
        0xd00d_feed,
        total as u32,
        off_struct as u32,
        off_strings as u32,
        rsv as u32,
        version,
        16,
        0,
        strings.len() as u32,
        structs.len() as u32,
    ] {
        push(&mut b, x);
    }
    b.resize(rsv, 0);
    // One reservation, then the terminator.
    b.extend_from_slice(&0x8000_0000u64.to_be_bytes());
    b.extend_from_slice(&0x1000u64.to_be_bytes());
    b.extend_from_slice(&[0; 16]);
    b.extend_from_slice(&structs);
    b.extend_from_slice(&strings);
    b
}

fn cell(v: u32) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}

#[test]
fn every_fixture_parses_identically() {
    for blob in FIXTURES {
        let (_, err) = parse_both(blob);
        assert_eq!(err, None);
        // Bytes after `totalsize` are ignored.
        let mut longer = blob.to_vec();
        longer.extend_from_slice(&[0xff; 7]);
        parse_both(&longer);
    }
}

#[test]
fn phandle_tables_and_their_errors_match() {
    let dup = write_dtb(
        17,
        &[
            (0, "", vec![("#address-cells", cell(2))]),
            (1, "a", vec![("phandle", cell(1))]),
            (1, "b", vec![("linux,phandle", cell(1))]),
        ],
    );
    assert_eq!(parse_both(&dup).1.as_deref(), Some("DuplicatePhandle"));
    for (name, value) in [
        ("phandle", vec![0, 0, 1]),
        ("phandle", vec![0; 8]),
        ("linux,phandle", vec![]),
    ] {
        let bad = write_dtb(
            17,
            &[
                (0, "", vec![]),
                (1, "x", vec![("phandle", cell(5))]),
                (1, "y", vec![("other", cell(1)), (name, value)]),
                (2, "z", vec![("phandle", cell(5))]),
            ],
        );
        assert_eq!(parse_both(&bad).1.as_deref(), Some("BadProperty"));
    }
    // The same phandle twice on one node, both spellings, a later
    // property of the same name (ignored), and v16.
    for version in [16, 17] {
        let ok = write_dtb(
            version,
            &[
                (0, "", vec![("phandle", cell(9))]),
                (
                    1,
                    "cpus",
                    vec![
                        ("phandle", cell(3)),
                        ("linux,phandle", cell(3)),
                        ("phandle", vec![1, 2]),
                    ],
                ),
                (2, "cpu@0", vec![("linux,phandle", cell(4))]),
                (1, "soc", vec![("phandle", cell(0xffff_ffff))]),
            ],
        );
        assert_eq!(parse_both(&ok).1, None);
    }
    // A duplicate after a malformed phandle reports the malformed one.
    let order = write_dtb(
        17,
        &[
            (0, "", vec![]),
            (1, "p", vec![("phandle", vec![1])]),
            (1, "q", vec![("phandle", cell(7))]),
            (1, "r", vec![("phandle", cell(7))]),
        ],
    );
    assert_eq!(parse_both(&order).1.as_deref(), Some("BadProperty"));
}

#[test]
fn structural_errors_match() {
    let mut kinds = std::collections::BTreeSet::new();
    let base = write_dtb(
        17,
        &[
            (0, "", vec![("model", b"m\0".to_vec())]),
            (1, "n@1", vec![]),
        ],
    );
    kinds.extend(parse_both(&base).1);
    let structs = 72usize;
    // Hand-made token streams after the root's BEGIN_NODE.
    let patched = |at: usize, words: &[u32]| {
        let mut b = base.clone();
        for (k, w) in words.iter().enumerate() {
            let i = at + 4 * k;
            if i + 4 <= b.len() {
                b[i..i + 4].copy_from_slice(&w.to_be_bytes());
            }
        }
        b
    };
    for words in [
        &[2u32, 2][..],    // unbalanced end node
        &[9],              // FDT_END inside a node
        &[7],              // bad token
        &[3, 4, 0xffff],   // property name out of bounds
        &[3, 0xffff_0000], // property value truncated
        &[2, 1, 0, 9],     // second root node
        &[2, 3, 0, 0, 9],  // property outside a node
        &[4, 4, 4, 2, 9],  // NOPs
        &[1, 0x8000_0000], // a node name that is not UTF-8
        &[1, 0x6e6f_6e75], // an unterminated node name
    ] {
        kinds.extend(parse_both(&patched(structs + 8, words)).1);
    }
    // The root with a name.
    kinds.extend(parse_both(&patched(structs + 4, &[0x7200_0000])).1);
    // Header fields.
    for (field, values) in [
        (0, &[0u32, 0xd00d_feee][..]),
        (1, &[0, 39, 41, 100_000]),
        (2, &[0, 3, 41, 100_000]),
        (3, &[0, 100_000]),
        (4, &[0, 4, 41, 100_000]),
        (5, &[1, 15, 16, 18, 99]),
        (6, &[0, 15, 17, 18]),
        (8, &[0, 5, 100_000]),
        (9, &[0, 1, 100_000]),
    ] {
        for &v in values {
            kinds.extend(parse_both(&patched(4 * field, &[v])).1);
        }
    }
    // Deep and wide trees.
    let deep: Vec<_> = (0..70)
        .map(|d| (d, if d == 0 { "" } else { "d" }, vec![]))
        .collect();
    kinds.extend(parse_both(&write_dtb(17, &deep)).1);
    // Reservations without a terminator.
    let mut many = base.clone();
    many.truncate(40);
    for _ in 0..1100 {
        many.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1]);
    }
    let total = many.len() as u32;
    many[4..8].copy_from_slice(&total.to_be_bytes());
    kinds.extend(parse_both(&many).1);
    for k in [
        "BadMagic",
        "BadStructure",
        "BadToken",
        "BlockOutOfBounds",
        "Misaligned",
        "NotUtf8",
        "TooDeep",
        "TooManyReservations",
        "Truncated",
        "UnsupportedVersion",
        "UnterminatedString",
    ] {
        assert!(kinds.contains(k), "no case reached {k}: {kinds:?}");
    }
}

#[test]
fn truncations_and_byte_flips_match() {
    let mut kinds = std::collections::BTreeSet::new();
    for (f, blob) in FIXTURES.iter().enumerate() {
        let step = (blob.len() / 600).max(1);
        for n in (0..blob.len()).step_by(step) {
            kinds.extend(parse_both(&blob[..n]).1);
        }
        let mut seed = 0x5111_c0e5 ^ f as u64;
        for _ in 0..400 {
            let mut b = blob.to_vec();
            for _ in 0..(1 + rng(&mut seed) % 4) {
                let i = (rng(&mut seed) as usize) % b.len();
                b[i] = rng(&mut seed) as u8;
            }
            kinds.extend(parse_both(&b).1);
        }
    }
    assert!(kinds.len() >= 8, "{kinds:?}");
}

// ---------------------------------------------------------------- AIA

fn same<T: std::fmt::Debug, U: std::fmt::Debug>(what: &str, step: usize, p: T, r: U) {
    let (p, r) = (format!("{p:?}"), format!("{r:?}"));
    assert_eq!(p, r, "{what} differs at step {step}");
}

#[test]
fn imsic_files_match() {
    use port::aia::ImsicFile as P;
    use rust::aia::ImsicFile as R;
    for ids in [0, 62, 63, 64, 127, 128, 255, 2047, 2048, 4095] {
        same("new", 0, P::new(ids).err(), R::new(ids).err());
        same(
            "with_external_delivery",
            0,
            P::with_external_delivery(ids).err(),
            R::with_external_delivery(ids).err(),
        );
    }
    let mut seed = 0x1351c;
    for script in 0..60 {
        let ids = [63, 127, 255, 511, 2047][script % 5];
        let (mut p, mut r) = if script % 2 == 0 {
            (P::new(ids).unwrap(), R::new(ids).unwrap())
        } else {
            (
                P::with_external_delivery(ids).unwrap(),
                R::with_external_delivery(ids).unwrap(),
            )
        };
        for step in 0..400 {
            let id = (rng(&mut seed) % u64::from(ids + 3)) as u32;
            let isel = 0x60 + (rng(&mut seed) % 0xb0) as u32;
            let v = match rng(&mut seed) % 4 {
                0 => 0x4000_0000,
                1 => rng(&mut seed) & 1,
                _ => rng(&mut seed),
            };
            match rng(&mut seed) % 9 {
                0 | 1 => {
                    p.seteipnum(id);
                    r.seteipnum(id);
                }
                2 => {
                    let off = [0, 4, 8][(v % 3) as usize];
                    p.mmio_write32(off, id.swap_bytes());
                    r.mmio_write32(off, id.swap_bytes());
                }
                3 => same("csr_read", step, p.csr_read(isel), r.csr_read(isel)),
                4 => same(
                    "csr_write",
                    step,
                    p.csr_write(isel, v),
                    r.csr_write(isel, v),
                ),
                5 => {
                    // Enable the identity.
                    let isel = 0xc0 + 2 * (id / 64);
                    let w = p.csr_read(isel).unwrap_or(0) | 1 << (id % 64);
                    same("enable", step, p.csr_write(isel, w), r.csr_write(isel, w));
                }
                6 => same("claim", step, p.claim(), r.claim()),
                7 => {
                    let d = 0x70;
                    same(
                        "delivery",
                        step,
                        p.csr_write(d, v & 1),
                        r.csr_write(d, v & 1),
                    );
                }
                _ => same(
                    "threshold",
                    step,
                    p.csr_write(0x72, v % 300),
                    r.csr_write(0x72, v % 300),
                ),
            }
            same("top", step, p.top(), r.top());
            same("topei", step, p.topei(), r.topei());
            same("asserted", step, p.asserted(), r.asserted());
            same("eidelivery", step, p.eidelivery(), r.eidelivery());
            same("pending", step, p.is_pending(id), r.is_pending(id));
            same("enabled", step, p.is_enabled(id), r.is_enabled(id));
            same(
                "external",
                step,
                p.delivers_external(),
                r.delivers_external(),
            );
        }
        same("state", 400, &p, &r);
        assert!(p.clone() == p);
        assert_eq!(p.ids(), r.ids());
        assert_eq!(
            p.supports_external_delivery(),
            r.supports_external_delivery()
        );
    }
}

#[test]
fn aplic_domains_match() {
    use port::aia::AplicDomain as P;
    use port::dtb::MemRange as PM;
    use rust::aia::AplicDomain as R;
    use rust::dtb::MemRange as RM;
    // Construction errors.
    for (sources, idcs, msi, size) in [
        (0, 1, false, 0x8000),
        (1024, 1, false, 0x8000),
        (5, 0, false, 0x8000),
        (5, 4, false, 0x4000),
        (5, 4, true, 0x4080),
        (1023, 0, true, 0x4000),
    ] {
        same(
            "new",
            0,
            P::new(
                "/soc/aplic",
                PM {
                    base: 0xc00_0000,
                    size,
                },
                sources,
                idcs,
                msi,
                true,
                false,
                0,
            )
            .err(),
            R::new(
                "/soc/aplic",
                RM {
                    base: 0xc00_0000,
                    size,
                },
                sources,
                idcs,
                msi,
                true,
                false,
                0,
            )
            .err(),
        );
    }
    let mut seed = 0xa911c;
    for script in 0..80 {
        let sources = 1 + (rng(&mut seed) % 96) as u32;
        let idcs = (rng(&mut seed) % 3) as usize;
        let msi = idcs == 0 || rng(&mut seed).is_multiple_of(2);
        let root = !rng(&mut seed).is_multiple_of(3);
        let supervisor = rng(&mut seed).is_multiple_of(2);
        let children = (rng(&mut seed) % 3) as u32;
        let base = 0xd00_0000;
        let window = 0x4000 + 32 * idcs as u64 + (rng(&mut seed) % 2) * 0x1000;
        let mut p = P::new(
            "/soc/aplic",
            PM { base, size: window },
            sources,
            idcs,
            msi,
            root,
            supervisor,
            children,
        )
        .unwrap();
        let mut r = R::new(
            "/soc/aplic",
            RM { base, size: window },
            sources,
            idcs,
            msi,
            root,
            supervisor,
            children,
        )
        .unwrap();
        let regs = [
            0x0000u64, 0x0004, 0x1bc0, 0x1bc4, 0x1bc8, 0x1bcc, 0x1c00, 0x1cdc, 0x1d00, 0x1ddc,
            0x1e00, 0x1edc, 0x1f00, 0x1fdc, 0x2000, 0x2004, 0x3000, 0x3004, 0x4000,
        ];
        for step in 0..500 {
            let src = (rng(&mut seed) % u64::from(sources + 2)) as u32;
            let reg = regs[(rng(&mut seed) as usize) % regs.len()];
            let off = match reg {
                0x0004 | 0x3004 => reg + 4 * (rng(&mut seed) % u64::from(sources + 1)),
                0x1c00 | 0x1d00 | 0x1e00 | 0x1f00 => reg + 4 * (rng(&mut seed) % 5),
                0x4000 => reg + rng(&mut seed) % (32 * (idcs as u64 + 1)),
                _ => reg,
            };
            let addr = base
                + off
                + if rng(&mut seed).is_multiple_of(40) {
                    2
                } else {
                    0
                };
            let v: u32 = match rng(&mut seed) % 6 {
                0 => src,
                1 => src.swap_bytes(),
                2 => {
                    [0, 1, 4, 5, 6, 7, 2, 3][(rng(&mut seed) % 8) as usize]
                        | if rng(&mut seed).is_multiple_of(4) {
                            1 << 10 | 1
                        } else {
                            0
                        }
                }
                3 => 0x104,
                _ => rng(&mut seed) as u32,
            };
            match rng(&mut seed) % 12 {
                0..=3 => same("write32", step, p.write32(addr, v), r.write32(addr, v)),
                4 | 5 => same("read32", step, p.read32(addr), r.read32(addr)),
                6 | 7 => {
                    let high = rng(&mut seed).is_multiple_of(2);
                    p.set_wire(src, high);
                    r.set_wire(src, high);
                }
                8 => {
                    let on = !rng(&mut seed).is_multiple_of(3);
                    p.set_active_here(src, on);
                    r.set_active_here(src, on);
                }
                9 => {
                    let i = (rng(&mut seed) % 3) as usize;
                    same("claimi", step, p.claimi(i), r.claimi(i));
                }
                10 => same("take_msis", step, p.take_msis(), r.take_msis()),
                _ => {
                    let c = p.msi_cfg();
                    same("msi_cfg", step, c, r.msi_cfg());
                }
            }
            same("mode", step, p.mode(src), r.mode(src));
            same("delegated", step, p.delegated_to(src), r.delegated_to(src));
            same(
                "here",
                step,
                p.is_delegated_here(src),
                r.is_delegated_here(src),
            );
            same("pending", step, p.is_pending(src), r.is_pending(src));
            same("rectified", step, p.rectified(src), r.rectified(src));
            same("dm", step, p.msi_mode(), r.msi_mode());
            same("contains", step, p.contains(addr), r.contains(addr));
            for i in 0..3 {
                same("topi", step, p.topi(i), r.topi(i));
                same("asserted", step, p.idc_asserted(i), r.idc_asserted(i));
            }
        }
        // The fabric's copy of the root's configuration.
        let mut cfg = r.msi_cfg();
        cfg.lhxw = (script % 4) as u32;
        cfg.hhxw = (script % 3) as u32;
        cfg.lhxs_s = 1;
        p.set_msi_cfg(port::aia::MsiAddrCfg {
            m_ppn: cfg.m_ppn,
            s_ppn: cfg.s_ppn,
            hhxs: cfg.hhxs,
            lhxs_m: cfg.lhxs_m,
            lhxs_s: cfg.lhxs_s,
            hhxw: cfg.hhxw,
            lhxw: cfg.lhxw,
            locked: cfg.locked,
        });
        r.set_msi_cfg(cfg);
        for hart in 0..20 {
            same(
                "address",
                hart as usize,
                p.msi_cfg().address(supervisor, hart, hart % 5),
                r.msi_cfg().address(supervisor, hart, hart % 5),
            );
        }
        let q = p.clone();
        same("clone", 0, q.take_msis_count(), r.clone().take_msis().len());
        assert_eq!(p.sources(), r.sources());
        assert_eq!(p.idc_count(), r.idc_count());
        assert_eq!(p.base(), r.base());
        assert_eq!(p.is_root(), r.is_root());
        assert_eq!(p.is_supervisor(), r.is_supervisor());
    }
}

trait TakeCount {
    fn take_msis_count(self) -> usize;
}

impl TakeCount for port::aia::AplicDomain {
    fn take_msis_count(mut self) -> usize {
        self.take_msis().len()
    }
}
