//! The port-backed package links the Vesper kernel, and a dropped simulator,
//! recording or replaying, gives back every allocation the kernel made.
//! (Port package only: the seed copy does not list this test.)
#[path = "../workload/mod.rs"]
mod workload;
use vesper_simpulse::{IMPLEMENTATION, Journal, Simulator, port_live_allocations};
use workload::{Digest, MIXED, drive};

#[test]
fn the_port_backed_package_links_the_port() {
    assert_eq!(IMPLEMENTATION, "vesper");
}

#[test]
fn dropped_simulators_release_every_kernel_allocation() {
    let warm = |seed| {
        let mut sim = Simulator::new(seed);
        drive(&mut sim, seed, 16, 2000, MIXED, &mut Digest::new());
        let json = sim.journal().to_json().unwrap();
        drop(sim);
        let mut replay = Simulator::replay(Journal::from_json(&json).unwrap()).unwrap();
        drive(&mut replay, seed, 16, 2000, MIXED, &mut Digest::new());
        replay.finish_replay().unwrap();
    };
    warm(7);
    let before = port_live_allocations();
    for seed in 0..4 {
        warm(seed);
    }
    assert_eq!(port_live_allocations(), before);
}
