//! Which build each package links (seed-copy rule, port wave P1 lane 6,
//! #903): the package `vesper-alloc` is the Vesper port of the region
//! bookkeeping, and the seed copy `vesper-alloc-seed`, which the compiler
//! links, is the Rust build. Every other test in this directory runs against
//! both.
#[test]
fn each_package_links_its_own_build() {
    let expected = match env!("CARGO_PKG_NAME") {
        "vesper-alloc-seed" => "rust",
        _ => "vesper",
    };
    assert_eq!(vesper_alloc::IMPLEMENTATION, expected);
}
