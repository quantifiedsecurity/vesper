//! Under `vesper-port`, compile vesper/quarry.ves into a static library and
//! check the committed binding (see docs/maintenance/porting-a-crate.md).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "vesper-port")]
    {
        let port = vesper_port_harness::Port::new("quarry", "vesper/quarry.ves")
            .binding("port/binding.rs");
        #[cfg(feature = "vesper-port-divergent")]
        let port = DIVERGENT
            .iter()
            .fold(port, |port, (from, to)| port.edit(from, to));
        port.build();
    }
}

/// One deliberately wrong edit per ported module (mirrored in BUILD.bazel);
/// the port test run must fail on each.
#[cfg(feature = "vesper-port-divergent")]
const DIVERGENT: &[(&str, &str)] = &[];
