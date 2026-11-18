//! Which implementation each package links (port wave P1 lane 8, #905): the
//! port-backed package `vesper-tensor` forwards to the Vesper port, and the
//! seed copy `vesper-tensor-seed` (which the compiler links) is the Rust
//! build.
#[test]
fn each_package_links_its_build() {
    let expected = match env!("CARGO_PKG_NAME") {
        "vesper-tensor" => "vesper",
        _ => "rust",
    };
    assert_eq!(vesper_tensor::IMPLEMENTATION, expected);
}
