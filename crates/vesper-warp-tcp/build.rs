//! Under `vesper-port`, compile vesper/tcp.ves into a static library and
//! check the committed binding (see docs/maintenance/porting-a-crate.md).
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "vesper-port")]
    {
        let port = vesper_port_harness::Port::new("tcp", "vesper/tcp.ves")
            .binding("port/binding.rs")
            .rust_types("crate::ported::types");
        #[cfg(feature = "vesper-port-divergent")]
        let port = DIVERGENT_EDITS
            .iter()
            .fold(port, |port, (from, to)| port.edit(from, to));
        port.build();
    }
}

/// The divergence check: one wrong line per ported module (mirrored in
/// BUILD.bazel). Each must make the tests that cover its module fail.
#[cfg(feature = "vesper-port-divergent")]
const DIVERGENT_EDITS: &[(&str, &str)] = &[
    // seq.rs: `<=` in sequence space becomes `<`.
    ("    sn_diff(a, b) <= 0\n", "    sn_diff(a, b) < 0\n"),
    // rtt.rs: RTTVAR starts at R instead of R/2.
    ("        e.rttvar_ns = r_ns / 2;\n", "        e.rttvar_ns = r_ns;\n"),
    // cc.rs: NewReno halves to a single segment floor.
    (
        "    cc.ssthresh = max32(cc.cwnd / 2, wrapping_mul(cc.mss, 2));\n    cc.cwnd = cc.ssthresh;",
        "    cc.ssthresh = max32(cc.cwnd / 2, cc.mss);\n    cc.cwnd = cc.ssthresh;",
    ),
    // wire.rs: the odd trailing checksum byte is dropped.
    (
        "        a = wrapping_add(a, widen_u64(bytes[i]) << 8);\n",
        "        a = wrapping_add(a, 0);\n",
    ),
    // conn.rs: the duplicate-ACK threshold drops from 3 to 2.
    ("fn dup_thresh() -> u32 {\n    3\n}", "fn dup_thresh() -> u32 {\n    2\n}"),
];
