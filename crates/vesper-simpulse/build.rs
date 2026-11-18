//! Link the Vesper port in vesper/ and set cfg(vesper_port); see "Crates
//! the compiler links" in docs/maintenance/porting-a-crate.md. The seed
//! package (seed/) has no build script, so it builds the Rust code.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let port = vesper_port_harness::Port::new("simpulse", "vesper").binding("port/binding.rs");
    #[cfg(feature = "vesper-port-divergent")]
    let port = EDITS
        .iter()
        .fold(port, |port, (from, to)| port.edit(from, to));
    port.build();
}

/// The divergence check for the one ported module (mirrored in BUILD.bazel
/// and asserted by scripts/test-port-harness.sh): a round no longer resets,
/// so the second round dispatches a task that already ran in the first.
#[cfg(feature = "vesper-port-divergent")]
const EDITS: &[(&str, &str)] = &[(
    "        k.t_seen.try_set(i, true);\n",
    "        k.t_seen.try_set(i, false);\n",
)];
