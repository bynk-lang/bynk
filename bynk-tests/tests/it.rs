//! Every `bynk-tests` integration test, in ONE test binary, grouped by tier.
//!
//! Each file in these directories used to be its own test crate, so `cargo`
//! built and linked ~130 binaries, each carrying the whole compiler. Linking
//! them dominated the Windows CI leg's build phase. This root includes each as
//! a module, and `autotests = false` in `Cargo.toml` stops cargo from also
//! building them one by one.
//!
//! # Tiers (C4b, #2940)
//!
//! A file lives in exactly one tier directory, and is included under the
//! matching module here, so the tier is the first segment of a test's name and
//! `.config/nextest.toml` selects it by prefix:
//!
//! - **`integration/`**: in-process pipeline tests and the fixture corpus. No
//!   external process.
//! - **`system/`**: anything that starts the `bynk` binary, `tsc`, `node`,
//!   `workerd` or `wrangler`.
//!
//! (The *unit* tier is the other crates' own tests and `xtask`'s file checks:
//! see the `ci-unit` filterset.) Put a new file in the tier its test needs; the
//! tier probe (`cargo xtask tier-probe`) fails if a test falls in none or in
//! two. A file that mixes both kinds goes in `system/` whole: do not split it
//! in a rename.
//!
//! A test's name is `<tier>::<file stem>::<test>`: `tsc_verify.rs`'s
//! `emitted_typescript_passes_tsc_strict` is
//! `system::tsc_verify::emitted_typescript_passes_tsc_strict`. Run one file's
//! tests with `cargo nextest run -p bynk-tests --test it -E 'test(/^system::tsc_verify::/)'`,
//! or `cargo test -p bynk-tests --test it tsc_verify::` (cargo matches a
//! substring, so the tier prefix is optional there).
//!
//! Adding a test file: add it to its tier's directory and its `mod` line below,
//! in order. `it_lists_every_file` fails if a file is missing from the list.

// Each file keeps declaring the helper modules it uses (`mod require;`,
// `mod wrangler;`, ...) with a `#[path]` to the shared directory, so a helper is
// compiled once per file that uses it, exactly as it was when each file was its
// own crate. Sharing one copy would mean editing every file's `mod`/`use`
// lines; the duplication is harmless.
#![allow(clippy::duplicate_mod)]

mod integration {
    #[path = "agent_state_corpus.rs"]
    mod agent_state_corpus;
    #[path = "by_on_agent.rs"]
    mod by_on_agent;
    #[path = "call_hierarchy_index.rs"]
    mod call_hierarchy_index;
    #[path = "capability_paths.rs"]
    mod capability_paths;
    #[path = "check_format_short.rs"]
    mod check_format_short;
    #[path = "comment_syntax_fixes.rs"]
    mod comment_syntax_fixes;
    #[path = "contract_hash.rs"]
    mod contract_hash;
    #[path = "debug_metadata.rs"]
    mod debug_metadata;
    #[path = "diagnose_project.rs"]
    mod diagnose_project;
    #[path = "diagnostic_hints.rs"]
    mod diagnostic_hints;
    #[path = "doc_diagnostics.rs"]
    mod doc_diagnostics;
    #[path = "doc_examples.rs"]
    mod doc_examples;
    #[path = "duration.rs"]
    mod duration;
    #[path = "e2e.rs"]
    mod e2e;
    #[path = "eq_bound_migration.rs"]
    mod eq_bound_migration;
    #[path = "events_schema_registry_behaviour.rs"]
    mod events_schema_registry_behaviour;
    #[path = "explain_examples.rs"]
    mod explain_examples;
    #[path = "expr_types_capture.rs"]
    mod expr_types_capture;
    #[path = "expression_depth_limit.rs"]
    mod expression_depth_limit;
    #[path = "field_op_index.rs"]
    mod field_op_index;
    #[path = "firstparty_sources.rs"]
    mod firstparty_sources;
    #[path = "fuzz_smoke.rs"]
    mod fuzz_smoke;
    #[path = "host_globals_qualified.rs"]
    mod host_globals_qualified;
    #[path = "implementation_index.rs"]
    mod implementation_index;
    #[path = "import_closure.rs"]
    mod import_closure;
    #[path = "inlay_hints.rs"]
    mod inlay_hints;
    #[path = "interpolation.rs"]
    mod interpolation;
    #[path = "kernel_registry.rs"]
    mod kernel_registry;
    #[path = "key_bound_fixes.rs"]
    mod key_bound_fixes;
    #[path = "locals_capture.rs"]
    mod locals_capture;
    #[path = "method_index.rs"]
    mod method_index;
    #[path = "multi_file_commons_barrel.rs"]
    mod multi_file_commons_barrel;
    #[path = "ord_bound_fixes.rs"]
    mod ord_bound_fixes;
    #[path = "provider_agent_cross_context_target.rs"]
    mod provider_agent_cross_context_target;
    #[path = "query_kernel_registry.rs"]
    mod query_kernel_registry;
    #[path = "recursive_codec_depth_agreement.rs"]
    mod recursive_codec_depth_agreement;
    #[path = "reference_index.rs"]
    mod reference_index;
    #[path = "refinement_index.rs"]
    mod refinement_index;
    #[path = "requirements_ledger.rs"]
    mod requirements_ledger;
    #[path = "reserved_builtin_type_name.rs"]
    mod reserved_builtin_type_name;
    #[path = "semantic_tokens_index.rs"]
    mod semantic_tokens_index;
    #[path = "snippets.rs"]
    mod snippets;
    #[path = "source_map.rs"]
    mod source_map;
    #[path = "source_map_bodies.rs"]
    mod source_map_bodies;
    #[path = "storage_layout_marker.rs"]
    mod storage_layout_marker;
    #[path = "store_checker.rs"]
    mod store_checker;
    #[path = "store_op_registry.rs"]
    mod store_op_registry;
    #[path = "suggestions.rs"]
    mod suggestions;
    #[path = "suite_diagnostics_attributed.rs"]
    mod suite_diagnostics_attributed;
    #[path = "wire_shape_drift.rs"]
    mod wire_shape_drift;
    #[path = "write_output_reconciliation_behaviour.rs"]
    mod write_output_reconciliation_behaviour;
    #[path = "ws_open_key_target.rs"]
    mod ws_open_key_target;
    #[path = "ws_open_split_target.rs"]
    mod ws_open_split_target;

    /// Every `*.rs` file in a tier directory is listed below that tier, once.
    /// With `autotests = false` cargo no longer discovers test files, so a file
    /// added without its `mod` line would compile into nothing and its tests
    /// would silently never run. A `.rs` file left directly in `tests/`, outside
    /// every tier, is the same mistake and fails too.
    ///
    /// A file counts as listed only by a live `#[path = "x.rs"]` line (not one
    /// commented out) followed directly by `mod x;`, inside its tier's `mod`.
    #[test]
    fn it_lists_every_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
        let root = std::fs::read_to_string(dir.join("it.rs")).expect("read it.rs");
        let lines: Vec<&str> = root.lines().map(str::trim).collect();

        // The tier each listing sits under: the nearest preceding `mod <tier> {`.
        let mut tier = "";
        let mut listed: Vec<(String, String)> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if let Some(name) = line.strip_prefix("mod ").and_then(|r| r.strip_suffix(" {")) {
                tier = name;
            }
            let Some(file) = line
                .strip_prefix("#[path = \"")
                .and_then(|r| r.strip_suffix("\"]"))
            else {
                continue;
            };
            let Some(stem) = file.strip_suffix(".rs") else {
                continue;
            };
            if lines.get(i + 1) == Some(&format!("mod {stem};").as_str()) {
                listed.push((tier.to_string(), file.to_string()));
            }
        }
        assert!(
            !listed.is_empty(),
            "no `#[path]`/`mod` listings parsed from tests/it.rs: the parse is broken, not the tree"
        );

        let mut problems: Vec<String> = Vec::new();
        for tier in ["integration", "system"] {
            let mut on_disk: Vec<String> = std::fs::read_dir(dir.join(tier))
                .unwrap_or_else(|e| panic!("read tests/{tier}/: {e}"))
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".rs"))
                .collect();
            on_disk.sort();
            assert!(!on_disk.is_empty(), "tests/{tier}/ holds no test files");
            for file in &on_disk {
                let n = listed
                    .iter()
                    .filter(|(t, f)| t == tier && f == file)
                    .count();
                if n != 1 {
                    problems.push(format!(
                        "{tier}/{file}: listed {n} times under `mod {tier}`"
                    ));
                }
            }
            for (t, f) in &listed {
                if t == tier && !on_disk.contains(f) {
                    problems.push(format!("{tier}/{f}: listed but not on disk"));
                }
            }
        }
        let known = ["integration", "system"];
        for (t, f) in &listed {
            if !known.contains(&t.as_str()) {
                problems.push(format!("{f}: listed under `mod {t}`, which is not a tier"));
            }
        }
        for entry in std::fs::read_dir(&dir).expect("read tests/").flatten() {
            let n = entry.file_name().to_string_lossy().into_owned();
            if n.ends_with(".rs") && n != "it.rs" {
                problems.push(format!("{n}: a test file outside every tier directory"));
            }
        }
        problems.sort();
        assert!(
            problems.is_empty(),
            "tests/ files not placed and listed in exactly one tier, so their tests \
             would run twice or never:\n  {}",
            problems.join("\n  ")
        );
    }
}

mod system {
    #[path = "accepted_contracts_behaviour.rs"]
    mod accepted_contracts_behaviour;
    #[path = "adapter_flattened_capability_stub_behaviour.rs"]
    mod adapter_flattened_capability_stub_behaviour;
    #[path = "agent_serialise_workerd.rs"]
    mod agent_serialise_workerd;
    #[path = "agent_state_schema_lock.rs"]
    mod agent_state_schema_lock;
    #[path = "alias_service_workerd.rs"]
    mod alias_service_workerd;
    #[path = "asset_check_behaviour.rs"]
    mod asset_check_behaviour;
    #[path = "bearer_auth.rs"]
    mod bearer_auth;
    #[path = "behaviour_fixtures.rs"]
    mod behaviour_fixtures;
    #[path = "binding_behaviour.rs"]
    mod binding_behaviour;
    #[path = "binding_scaffold.rs"]
    mod binding_scaffold;
    #[path = "binding_workerd.rs"]
    mod binding_workerd;
    #[path = "callee_faulted_workerd.rs"]
    mod callee_faulted_workerd;
    #[path = "case_filter.rs"]
    mod case_filter;
    #[path = "check_paths_behaviour.rs"]
    mod check_paths_behaviour;
    #[path = "check_reports_past_structural_error_behaviour.rs"]
    mod check_reports_past_structural_error_behaviour;
    #[path = "check_warning_reporting_behaviour.rs"]
    mod check_warning_reporting_behaviour;
    #[path = "cli_rooting.rs"]
    mod cli_rooting;
    #[path = "commons_type_in_capability_behaviour.rs"]
    mod commons_type_in_capability_behaviour;
    #[path = "compatibility_flags_cli.rs"]
    mod compatibility_flags_cli;
    #[path = "compile_esm.rs"]
    mod compile_esm;
    #[path = "compile_sources_stdin.rs"]
    mod compile_sources_stdin;
    #[path = "compile_strips_suites.rs"]
    mod compile_strips_suites;
    #[path = "contract_behaviour.rs"]
    mod contract_behaviour;
    #[path = "coverage_behaviour.rs"]
    mod coverage_behaviour;
    #[path = "cross_context_caller.rs"]
    mod cross_context_caller;
    #[path = "cross_file_label_behaviour.rs"]
    mod cross_file_label_behaviour;
    #[path = "debug_inspect.rs"]
    mod debug_inspect;
    #[path = "deterministic_diagnostic_order_behaviour.rs"]
    mod deterministic_diagnostic_order_behaviour;
    #[path = "doc_examples_check_behaviour.rs"]
    mod doc_examples_check_behaviour;
    #[path = "embed_check_behaviour.rs"]
    mod embed_check_behaviour;
    #[path = "events_behaviour.rs"]
    mod events_behaviour;
    #[path = "events_boundary_workerd.rs"]
    mod events_boundary_workerd;
    #[path = "events_envelope_behaviour.rs"]
    mod events_envelope_behaviour;
    #[path = "events_ordering_workerd.rs"]
    mod events_ordering_workerd;
    #[path = "events_pattern_behaviour.rs"]
    mod events_pattern_behaviour;
    #[path = "events_schema_dispatch_behaviour.rs"]
    mod events_schema_dispatch_behaviour;
    #[path = "events_schema_version_behaviour.rs"]
    mod events_schema_version_behaviour;
    #[path = "events_workers_wiring.rs"]
    mod events_workers_wiring;
    #[path = "example_tests_behaviour.rs"]
    mod example_tests_behaviour;
    #[path = "examples.rs"]
    mod examples;
    #[path = "expect_is_narrowing_behaviour.rs"]
    mod expect_is_narrowing_behaviour;
    #[path = "failure_explanation_behaviour.rs"]
    mod failure_explanation_behaviour;
    #[path = "fetch_headers.rs"]
    mod fetch_headers;
    #[path = "float_codec_behaviour.rs"]
    mod float_codec_behaviour;
    #[path = "header_question_lint.rs"]
    mod header_question_lint;
    #[path = "history_behaviour.rs"]
    mod history_behaviour;
    #[path = "http_caching_behaviour.rs"]
    mod http_caching_behaviour;
    #[path = "http_edge_cache_behaviour.rs"]
    mod http_edge_cache_behaviour;
    #[path = "http_limits_behaviour.rs"]
    mod http_limits_behaviour;
    #[path = "http_method_behaviour.rs"]
    mod http_method_behaviour;
    #[path = "http_option_body_behaviour.rs"]
    mod http_option_body_behaviour;
    #[path = "http_plain_json_behaviour.rs"]
    mod http_plain_json_behaviour;
    #[path = "http_raw_body_behaviour.rs"]
    mod http_raw_body_behaviour;
    #[path = "http_raw_caching_behaviour.rs"]
    mod http_raw_caching_behaviour;
    #[path = "http_recursive_body_behaviour.rs"]
    mod http_recursive_body_behaviour;
    #[path = "http_security_behaviour.rs"]
    mod http_security_behaviour;
    #[path = "inline_value_names.rs"]
    mod inline_value_names;
    #[path = "int_binding_arithmetic_behaviour.rs"]
    mod int_binding_arithmetic_behaviour;
    #[path = "integration_contracts_behaviour.rs"]
    mod integration_contracts_behaviour;
    #[path = "json_codec_behaviour.rs"]
    mod json_codec_behaviour;
    #[path = "kv_behaviour.rs"]
    mod kv_behaviour;
    #[path = "let_else_staircase.rs"]
    mod let_else_staircase;
    #[path = "malformed_project_config_behaviour.rs"]
    mod malformed_project_config_behaviour;
    #[path = "multi_actor_sum.rs"]
    mod multi_actor_sum;
    #[path = "mutate_behaviour.rs"]
    mod mutate_behaviour;
    #[path = "observation_behaviour.rs"]
    mod observation_behaviour;
    #[path = "oidc_auth.rs"]
    mod oidc_auth;
    #[path = "package_dependencies.rs"]
    mod package_dependencies;
    #[path = "package_exports.rs"]
    mod package_exports;
    #[path = "par_behaviour.rs"]
    mod par_behaviour;
    #[path = "par_gate_workerd.rs"]
    mod par_gate_workerd;
    #[path = "par_traverse_behaviour.rs"]
    mod par_traverse_behaviour;
    #[path = "property_behaviour.rs"]
    mod property_behaviour;
    #[path = "question_in_case_behaviour.rs"]
    mod question_in_case_behaviour;
    #[path = "recovery_never_emits.rs"]
    mod recovery_never_emits;
    #[path = "recursive_boundary_behaviour.rs"]
    mod recursive_boundary_behaviour;
    #[path = "refinement_auth.rs"]
    mod refinement_auth;
    #[path = "regex_refinement_behaviour.rs"]
    mod regex_refinement_behaviour;
    #[path = "remote_target_behaviour.rs"]
    mod remote_target_behaviour;
    #[path = "render_long_lines.rs"]
    mod render_long_lines;
    #[path = "requires_range_fold_behaviour.rs"]
    mod requires_range_fold_behaviour;
    #[path = "runtime_helpers.rs"]
    mod runtime_helpers;
    #[path = "schema_lock_check_and_test_cli.rs"]
    mod schema_lock_check_and_test_cli;
    #[path = "signature_auth.rs"]
    mod signature_auth;
    #[path = "snapshot_behaviour.rs"]
    mod snapshot_behaviour;
    #[path = "store_behaviour.rs"]
    mod store_behaviour;
    #[path = "stream_behaviour.rs"]
    mod stream_behaviour;
    #[path = "stream_request_body_workerd.rs"]
    mod stream_request_body_workerd;
    #[path = "suite_filter.rs"]
    mod suite_filter;
    #[path = "system_suite_heading_behaviour.rs"]
    mod system_suite_heading_behaviour;
    #[path = "test_json.rs"]
    mod test_json;
    #[path = "test_runner_resolution.rs"]
    mod test_runner_resolution;
    #[path = "tsc_failure_diagnostic.rs"]
    mod tsc_failure_diagnostic;
    #[path = "tsc_verify.rs"]
    mod tsc_verify;
    #[path = "typescript_client.rs"]
    mod typescript_client;
    #[path = "typescript_client_workerd.rs"]
    mod typescript_client_workerd;
    #[path = "unstable_opt_in.rs"]
    mod unstable_opt_in;
    #[path = "wire_reference.rs"]
    mod wire_reference;
    #[path = "within_behaviour.rs"]
    mod within_behaviour;
    #[path = "workers_fault_log.rs"]
    mod workers_fault_log;
    #[path = "workers_runtime_smoke.rs"]
    mod workers_runtime_smoke;
    #[path = "wrangler_prewarm.rs"]
    mod wrangler_prewarm;
    #[path = "ws_behaviour.rs"]
    mod ws_behaviour;
}
