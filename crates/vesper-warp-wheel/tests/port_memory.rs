//! The Vesper build frees what it allocates: building, using, draining and
//! dropping every type many times leaves the Vesper heap where it was. The
//! Rust build has nothing to check here.
#![cfg(feature = "vesper-port")]

use vesper_warp_wheel::{
    EdtLaunchQueue, PacketNumberSpace, RetransmitLeases, Wheel, WheelConfig, port_live_allocations,
};

fn config() -> WheelConfig {
    WheelConfig {
        tick_ns: 1_000,
        slot_bits: 4,
        levels: 3,
        capacity: 64,
        start_ns: 0,
    }
}

fn cycle(round: u64) {
    let mut wheel: Wheel<u64> = Wheel::new(config()).unwrap();
    let mut leases: RetransmitLeases<u64> = RetransmitLeases::new(config()).unwrap();
    let mut edt: EdtLaunchQueue<u64> = EdtLaunchQueue::new(config(), 1_000_000).unwrap();
    for i in 0..32 {
        let _ = wheel.insert(i * 997 + round, i).unwrap();
        leases
            .park(PacketNumberSpace::Handshake, i, i * 1_500, i)
            .unwrap();
        edt.submit(0, i * 100, i).unwrap();
    }
    while wheel.poll(10_000).unwrap().is_some() {}
    while leases.poll_pto(10_000).unwrap().is_some() {}
    let mut n = 0;
    leases.discard_space(PacketNumberSpace::Handshake, |_, _| n += 1);
    wheel.drain(|_, _| n += 1);
    leases.drain(|_, _| n += 1);
    edt.drain(|_, _| n += 1);
    assert!(n > 0);
}

#[test]
fn vesper_heap_returns_to_baseline() {
    cycle(0);
    let before = port_live_allocations();
    for round in 0..2_000 {
        cycle(round);
    }
    assert_eq!(
        port_live_allocations(),
        before,
        "the Vesper build leaked over 2000 cycles"
    );
}
