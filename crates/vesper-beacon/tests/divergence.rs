//! The divergence check (port wave P1 lane 4, #901): each deliberately wrong
//! Vesper build, one edit per ported module, must fail that module's tests
//! and pass a test of another module. Bazel passes the divergent test
//! binaries (BUILD.bazel, :port_divergence_detected); under Cargo the check
//! is scripts/test-port-harness.sh. Without the variables this test has
//! nothing to run and passes.
use std::process::Command;

/// (variable, test that must fail, test that must pass)
const CHECKS: &[(&str, &str, &str)] = &[
    (
        "BEACON_DIVERGENT_HISTOGRAM",
        "all_ranges_partition_u64_and_bound_error",
        "footprint_is_fixed",
    ),
    (
        "BEACON_DIVERGENT_EXPONENTIAL",
        "starts_at_the_finest_scale_and_downscales_to_fit",
        "bucket_index_is_exact_at_small_scales",
    ),
    (
        "BEACON_DIVERGENT_TRACEPARENT",
        "lengths_separators_lowercase_version_and_nonzero_are_strict",
        "drain_wraparound_partial_zero_and_fifo_ownership",
    ),
    (
        "BEACON_DIVERGENT_SPANS",
        "ids_never_wrap_or_repeat_and_imported_self_parent_is_skipped",
        "actual_header_bytes_and_all_opaque_flags_roundtrip",
    ),
];

#[test]
fn each_divergent_module_fails_its_tests() {
    for (var, failing, passing) in CHECKS {
        let Some(binary) = std::env::var_os(var) else {
            continue;
        };
        let output = Command::new(&binary)
            .arg("--test-threads=1")
            .output()
            .expect("run the divergent port tests");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success(),
            "{var}: the divergent port passed its tests:\n{stdout}"
        );
        assert!(
            stdout.contains(&format!("test {failing} ... FAILED")),
            "{var}: the divergent port did not fail {failing}:\n{stdout}"
        );
        assert!(
            stdout.contains(&format!("test {passing} ... ok")),
            "{var}: an unchanged module must still agree ({passing}):\n{stdout}"
        );
    }
}
