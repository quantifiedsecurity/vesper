//! The divergence checks of the Vesper port (port wave P1 lane 9, #906): a
//! build of the port with one deliberately wrong line in a ported module
//! (DIVERGENT in build.rs and BUILD.bazel) must fail a test of that module,
//! while an unrelated test in the same file still passes. Bazel passes the
//! divergent test binaries; under Cargo the same check is in
//! scripts/test-port-harness.sh. Without the variables this test has nothing
//! to run and passes.
use std::process::Command;

/// The variable naming the divergent binary, the test it must fail, and a
/// test it must still pass.
const CHECKS: [(&str, &str, &str); 9] = [
    (
        "VESPER_GRAMMAR_DIVERGENT_FRAMING",
        "known_uleb_vectors_exact_prefix_and_transactional_output",
        "u24_big_endian_frames_roundtrip_and_preserve_remainder",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_ARROW",
        "checked_nullable_aggregation_matches_i128_prefix_oracle",
        "alignment_checks_distinguish_relative_offsets_from_actual_host_addresses",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_CAPNP",
        "capnp_stream_segments_are_borrowed_and_framing_is_exact",
        "u24_boundaries_fail_transactionally",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_CAPNP_PACKED",
        "official_encoding_vectors_and_raw_runs_with_zeroes",
        "every_short_output_preserves_the_whole_buffer",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_GRAMMAR",
        "aliased_dag_expansion_cannot_bypass_capture_or_work_budgets",
        "ambiguity_duplicate_branch_ids_bad_references_and_recursion_fail",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_CODEC",
        "independently_constructed_numbers_have_known_canonical_encodings",
        "codec_limit_configuration_is_checked_before_all_operations",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_PROTOBUF",
        "signed_twos_complement_varints_are_canonical_in_singular_and_packed_fields",
        "singular_presence_and_defaults_are_actual_distinct_logical_values",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_DER",
        "typed_set_of_sorts_complete_encodings_retains_duplicates_and_rejects_untyped_set",
        "utf8_string_is_strict_borrowed_and_transactional",
    ),
    (
        "VESPER_GRAMMAR_DIVERGENT_IPC",
        "independent_arrow_values_and_versions",
        "hostile_schema_and_union_discriminants",
    ),
];

#[test]
fn each_divergent_module_fails_its_tests() {
    for (variable, failing, passing) in CHECKS {
        let Some(binary) = std::env::var_os(variable) else {
            continue;
        };
        let output = Command::new(&binary)
            .arg("--test-threads=1")
            .output()
            .expect("run the divergent port tests");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success(),
            "{variable}: the divergent port passed:\n{stdout}"
        );
        assert!(
            stdout.contains(&format!("test {failing} ... FAILED")),
            "{variable}: the divergent port did not fail {failing}:\n{stdout}"
        );
        assert!(
            stdout.contains(&format!("test {passing} ... ok")),
            "{variable}: the unchanged code must still agree ({passing}):\n{stdout}"
        );
    }
}
