//! Cardinality correlations must remove impossible paths, not real windows.
use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;

fn hazards(source: &str) -> Vec<String> {
    Analyzer::new(Config::default())
        .analyze_source("cardinality.ts", source)
        .unwrap()
        .diagnostics
        .into_iter()
        .filter(|d| d.function == "resize" && d.code.as_str() != "MW9001")
        .map(|d| d.code.as_str().to_owned())
        .collect()
}

const SOURCE: &str = include_str!("../../../fixtures/safe/complementary_partition.ts");

#[test]
fn complementary_filter_partitions_cannot_grow_and_shrink_together() {
    assert!(hazards(SOURCE).is_empty());
}

#[test]
fn non_complementary_filters_keep_a_real_window() {
    let source = SOURCE.replace("!row.enabled", "row.enabled");
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn a_different_slice_index_keeps_a_real_window() {
    let source = SOURCE.replace("ordered.slice(requested)", "ordered.slice(0)");
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn changing_the_array_length_invalidates_the_relation() {
    for mutation in [
        "rows.push({ enabled: true });",
        "rows.length = 0;",
        "mutate(rows);",
    ] {
        let source = SOURCE.replace(
            "if (requested > rows.length)",
            &format!("{mutation}\n  if (requested > rows.length)"),
        );
        assert_eq!(hazards(&source), ["MW1004"], "{mutation}");
    }
}

#[test]
fn exposing_an_alias_invalidates_the_relation() {
    let source = SOURCE.replace(
        "const removed =",
        "const exposed = ordered; mutate(exposed);\n  const removed =",
    );
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn user_defined_object_methods_are_not_native_array_proofs() {
    let source = SOURCE
        .replace(
            "const ordered = [",
            "const rows = customRows();\n  const ordered = [",
        )
        .replace(
            "rows: { enabled: boolean }[], requested",
            "input: unknown, requested",
        )
        .replace(
            "customRows()",
            "{ length: 1, filter: (cb: unknown) => [1, 2, 3] }",
        );
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn an_object_with_user_defined_numeric_coercion_is_not_a_scalar_count() {
    let source = SOURCE.replace("requested: number", "initial: number")
        .replace("const ordered =", "const requested = { valueOf() { return Math.random() > 0.5 ? 0 : 100; } };\n  const ordered =");
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn named_pure_predicates_and_immutable_aliases_keep_the_exclusion() {
    let source = SOURCE
        .replace("rows.filter((row) => row.enabled)", "rows.filter(enabled)")
        .replace(
            "const removed = ordered.slice(requested);",
            "const limit = requested; const copy = ordered; const removed = copy.slice(limit);",
        )
        + "\nfunction enabled(row: { enabled: boolean }) { return !!row.enabled; }\n";
    assert!(hazards(&source).is_empty());
}

#[test]
fn filters_with_effectful_or_array_mutating_callbacks_are_not_partitions() {
    for replacement in [
        "rows.filter((row) => check(row))",
        "rows.filter((row, index, array) => { array.push(row); return row.enabled; })",
    ] {
        let source = SOURCE.replace("rows.filter((row) => row.enabled)", replacement);
        assert_eq!(hazards(&source), ["MW1004"], "{replacement}");
    }
}

#[test]
fn getters_do_not_supply_complementary_predicates() {
    let source = SOURCE
        .replace(
            "rows: { enabled: boolean }[], requested",
            "input: unknown, requested",
        )
        .replace(
            "const ordered =",
            "const rows = [{ get enabled() { return Math.random() > 0.5; } }];\n  const ordered =",
        );
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn a_sparse_partition_is_only_an_upper_bound_not_length_equality() {
    let source = SOURCE
        .replace(
            "rows: { enabled: boolean }[], requested",
            "input: unknown, requested",
        )
        .replace(
            "const ordered =",
            "const rows = [, , { enabled: true }];\n  const ordered =",
        )
        .replace("ordered.slice(requested)", "rows.slice(requested)")
        .replace("requested > rows.length", "requested > ordered.length");
    // requested=2: the partition has length 1 while the original array's
    // tail still has length 1. Both writes really can run.
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn an_array_passed_through_arguments_is_not_known_readonly() {
    let source = SOURCE.replace(
        "const removed =",
        "mutate(arguments[0]);\n  const removed =",
    );
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn a_reassigned_count_is_not_a_stable_snapshot() {
    let source = SOURCE.replace(
        "if (requested > rows.length)",
        "requested = rows.length + 1;\n  if (requested > rows.length)",
    );
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn arrays_with_user_defined_string_coercion_are_not_scalar_counts() {
    let source = SOURCE.replace("requested: number", "initial: number")
        .replace("const ordered =", "const requested = [{ toString() { return Math.random() > 0.5 ? '0' : '100'; } }];\n  const ordered =");
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn equivalent_growth_and_nonempty_guards_share_the_relation() {
    for guard in [
        "removed.length",
        "removed.length !== 0",
        "!(removed.length === 0)",
        "0 < removed.length",
    ] {
        let source = SOURCE
            .replace("requested > rows.length", "rows.length < requested")
            .replace("removed.length > 0", guard);
        assert!(hazards(&source).is_empty(), "{guard}");
    }
}

#[test]
fn a_nonnegative_length_does_not_imply_a_nonempty_tail() {
    let source = SOURCE.replace("removed.length > 0", "removed.length >= 0");
    assert_eq!(hazards(&source), ["MW1004"]);
}

#[test]
fn different_parameter_defaults_do_not_make_filters_complementary() {
    let source = SOURCE
        .replace(
            "rows: { enabled: boolean }[], requested",
            "input: unknown, requested",
        )
        .replace(
            "const ordered =",
            "const rows: any[] = [undefined, undefined];\n  const ordered =",
        )
        .replace(
            "(row) => row.enabled",
            "(row = { enabled: true }) => row.enabled",
        )
        .replace(
            "(row) => !row.enabled",
            "(row = { enabled: false }) => !row.enabled",
        );
    // Both filters keep both undefined entries with different defaults:
    // requested=3 exceeds the source length 2, but leaves a nonempty tail.
    assert_eq!(hazards(&source), ["MW1004"]);
}
