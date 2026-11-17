//! The divergence check (port wave P1 lane 12, #909): the Vesper build with
//! one deliberate edit per ported module (EDITS in build.rs) must fail the
//! test that guards each module. Bazel passes the divergent test binaries;
//! under Cargo the check is port/check.sh. Without the variables this test
//! has nothing to run and passes.
use std::process::Command;

/// Per test binary: its variable, and the tests the divergent build must
/// fail, each named with the module whose edit it catches.
pub const EXPECTED: &[(&str, &[(&str, &str)])] = &[
    (
        "VESPER_IGNIS_DIVERGENT_PLANNER",
        &[
            ("d3d12", "sync_aggregates_expand_as_specified"),
            ("graph", "declaration_errors_are_structured"),
            (
                "plan",
                "buffer_writers_are_serialized_even_on_disjoint_ranges",
            ),
            (
                "assemble",
                "cross_queue_read_after_write_uses_fence_not_barrier",
            ),
        ],
    ),
    (
        "VESPER_IGNIS_DIVERGENT_MULTI_BACKEND",
        &[("multi_backend", "metal4_emits_native_mtlstages_bitmasks")],
    ),
    (
        "VESPER_IGNIS_DIVERGENT_LIFECYCLE",
        &[
            (
                "lifecycle",
                "stale_foreign_and_future_completions_return_the_report_and_keep_work",
            ),
            (
                "recording",
                "partial_multi_queue_publication_cannot_retire_when_unsubmitted_terminal_never_occurs",
            ),
        ],
    ),
];

#[test]
fn divergent_port_fails_one_test_per_module() {
    for (variable, failing) in EXPECTED {
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
            "the divergent port passed {variable}:\n{stdout}"
        );
        for (module, test) in *failing {
            assert!(
                stdout.contains(&format!("test {test} ... FAILED")),
                "the divergent {module} edit was not caught by {test}:\n{stdout}"
            );
        }
    }
}
