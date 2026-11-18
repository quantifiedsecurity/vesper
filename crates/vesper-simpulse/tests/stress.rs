//! A long seeded workload over every transition: the Rust build and the
//! Vesper port must produce the same outcomes, errors included, and the same
//! journal bytes (the digests below come from the Rust build), and the
//! journal must replay.
#[path = "../workload/mod.rs"]
mod workload;
use vesper_simpulse::{Journal, Simulator};
use workload::{Digest, MIXED, Mix, drive};

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}

/// (outcome digest, journal digest, journal events) of one recorded run,
/// after checking that its journal replays.
fn run(seed: u64, tasks: u64, dispatches: u64, mix: Mix) -> (u64, u64, usize) {
    let mut sim = Simulator::new(seed);
    let mut d = Digest::new();
    drive(&mut sim, seed, tasks, dispatches, mix, &mut d);
    let json = sim.journal().to_json().unwrap();
    let mut replay = Simulator::replay(Journal::from_json(&json).unwrap()).unwrap();
    let mut again = Digest::new();
    drive(&mut replay, seed, tasks, dispatches, mix, &mut again);
    assert_eq!(again.0, d.0, "the replay took the same path");
    replay.finish_replay().unwrap();
    (d.0, fnv(json.as_bytes()), sim.journal().events.len())
}

#[test]
fn mixed_workload_outcomes_and_journal_bytes_match_the_rust_build() {
    let got = [
        run(1, 8, 3000, MIXED),
        run(0xdead_beef, 64, 5000, MIXED),
        run(u64::MAX, 3, 500, MIXED),
    ];
    assert_eq!(got, EXPECTED);
}

// Recorded from the Rust build (vesper-simpulse-seed).
const EXPECTED: [(u64, u64, usize); 3] = [
    (4086547412011094588, 5805743941590931853, 9660),
    (17362088469668394175, 3297338991213011535, 16309),
    (3791375692797341520, 1999925069516765833, 1590),
];
