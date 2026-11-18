//! vesper-beacon-arrow is outside the compiler's dependency closure, so it
//! depends on the `vesper-beacon` package, not the seed copy the compiler
//! links (seed-copy rule, docs/maintenance/porting-a-crate.md). That
//! package builds Beacon's Rust code by default and its Vesper port under
//! this crate's `vesper-port` feature (port wave P1 lane 4, #901).
use vesper_beacon::FastLogHistogram;

#[test]
fn arrow_links_the_selected_beacon_build() {
    let expected = if cfg!(feature = "vesper-port") {
        "vesper"
    } else {
        "rust"
    };
    assert_eq!(vesper_beacon::IMPLEMENTATION, expected);
    assert_eq!(FastLogHistogram::bucket_index(15), 15);
    assert_eq!(FastLogHistogram::bucket_index(16), 16);
    assert_eq!(FastLogHistogram::bucket_index(u64::MAX), 495);
}
