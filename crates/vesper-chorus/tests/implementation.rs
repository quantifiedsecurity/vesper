//! The `vesper-port` feature selects the Vesper build of the ported entry
//! points (port wave P3b lane 6, #1068), and that build frees what it
//! allocates.
use vesper_chorus::check::{ExploreLimits, explore};
use vesper_chorus::{IMPLEMENTATION, parse_choreography, port_live_allocations, project_all};

#[test]
fn feature_selects_the_build() {
    let want = if cfg!(feature = "vesper-port") {
        "vesper"
    } else {
        "rust"
    };
    assert_eq!(IMPLEMENTATION, want);
}

#[test]
fn ported_calls_free_their_allocations() {
    let src = "roles A, B, C; rec X { A -> B { Go(u64) { B -> C: Fwd(u64); continue X; } Stop() { B -> C: Done(); } } }";
    let before = port_live_allocations();
    for _ in 0..3 {
        let c = parse_choreography(src).unwrap();
        project_all(&c).unwrap();
        explore(&c, ExploreLimits::default()).unwrap();
        assert!(parse_choreography("roles A; A -> A: X();").is_err());
    }
    assert_eq!(port_live_allocations(), before);
}
