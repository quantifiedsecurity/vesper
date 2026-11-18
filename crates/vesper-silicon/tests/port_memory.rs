//! The port-backed package frees every sequence the Vesper port allocates
//! (one test in its own binary, so no other test allocates meanwhile).
//! Port wave P3a lane 7 (#1042).

use vesper_silicon as port;

const FIXTURES: &[&[u8]] = &[
    include_bytes!("../fixtures/dtb/jh7110-starfive-visionfive-2-v1.3b.dtb"),
    include_bytes!("../fixtures/dtb/qemu-riscv64-virt-aplic-imsic-guests.dtb"),
    include_bytes!("../fixtures/dtb/vesper-edge-cases.dtb"),
];

#[test]
fn the_port_frees_what_it_allocates() {
    let before = port::port_live_allocations();
    for blob in FIXTURES {
        let _ = port::dtb::DeviceTree::parse(blob);
        let _ = port::dtb::DeviceTree::parse(&blob[..blob.len() / 2]);
    }
    {
        let mut f = port::aia::ImsicFile::new(255).unwrap();
        f.seteipnum(3);
        let _g = f.clone();
        let mut a = port::aia::AplicDomain::new(
            "/a",
            port::dtb::MemRange {
                base: 0,
                size: 0x8000,
            },
            40,
            0,
            true,
            true,
            false,
            0,
        )
        .unwrap();
        a.write32(0x3000, 5).unwrap();
        let _ = a.take_msis();
        let _b = a.clone();
    }
    assert_eq!(port::port_live_allocations(), before);
}
