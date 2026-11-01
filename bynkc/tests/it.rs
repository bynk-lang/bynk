//! Every `bynkc` integration test, in ONE test binary.
//!
//! Each file in this directory used to be its own test crate, so `cargo` built
//! and linked ~130 binaries, each carrying the whole compiler. Linking them
//! dominated the Windows CI leg's build phase. The files are unchanged and
//! stay where they are; this root includes each as a module, and
//! `autotests = false` in `bynkc/Cargo.toml` stops cargo from also building
//! them one by one.
//!
//! A test's name gains its file's stem as a prefix: `tsc_verify.rs`'s
//! `emitted_typescript_passes_tsc_strict` is now
//! `tsc_verify::emitted_typescript_passes_tsc_strict`. Run one file's tests
//! with `cargo nextest run -p bynkc --test it -E 'test(/^tsc_verify::/)'`, or
//! `cargo test -p bynkc --test it tsc_verify::`.
//!
//! Adding a test file: add its `mod` line below, in order. `it_lists_every_file`
//! fails if a file in this directory is missing from the list.

// Each file keeps declaring the helper modules it uses (`mod require;`,
// `mod wrangler;`, …), so a helper is compiled once per file that uses it,
// exactly as it was when each file was its own crate. Sharing one copy would
// mean editing every file's `mod`/`use` lines; the duplication is harmless.
#![allow(clippy::duplicate_mod)]

#[path = "accepted_contracts_behaviour.rs"]
mod accepted_contracts_behaviour;
#[path = "adapter_flattened_capability_stub_behaviour.rs"]
mod adapter_flattened_capability_stub_behaviour;
#[path = "agent_state_corpus.rs"]
mod agent_state_corpus;
#[path = "agent_state_schema_lock.rs"]
mod agent_state_schema_lock;
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
#[path = "by_on_agent.rs"]
mod by_on_agent;
#[path = "call_hierarchy_index.rs"]
mod call_hierarchy_index;
#[path = "callee_faulted_workerd.rs"]
mod callee_faulted_workerd;
#[path = "capability_paths.rs"]
mod capability_paths;
#[path = "case_filter.rs"]
mod case_filter;
#[path = "check_format_short.rs"]
mod check_format_short;
#[path = "check_paths_behaviour.rs"]
mod check_paths_behaviour;
#[path = "check_reports_past_structural_error_behaviour.rs"]
mod check_reports_past_structural_error_behaviour;
#[path = "check_warning_reporting_behaviour.rs"]
mod check_warning_reporting_behaviour;
#[path = "cli_reference.rs"]
mod cli_reference;
#[path = "cli_rooting.rs"]
mod cli_rooting;
#[path = "comment_syntax_fixes.rs"]
mod comment_syntax_fixes;
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
#[path = "contract_hash.rs"]
mod contract_hash;
#[path = "coverage_behaviour.rs"]
mod coverage_behaviour;
#[path = "cross_context_caller.rs"]
mod cross_context_caller;
#[path = "cross_file_label_behaviour.rs"]
mod cross_file_label_behaviour;
#[path = "debug_inspect.rs"]
mod debug_inspect;
#[path = "debug_metadata.rs"]
mod debug_metadata;
#[path = "decisions_index.rs"]
mod decisions_index;
#[path = "deterministic_diagnostic_order_behaviour.rs"]
mod deterministic_diagnostic_order_behaviour;
#[path = "diagnose_project.rs"]
mod diagnose_project;
#[path = "diagnostics_registry.rs"]
mod diagnostics_registry;
#[path = "doc_diagnostics.rs"]
mod doc_diagnostics;
#[path = "doc_examples.rs"]
mod doc_examples;
#[path = "doc_examples_check_behaviour.rs"]
mod doc_examples_check_behaviour;
#[path = "doc_version.rs"]
mod doc_version;
#[path = "duration.rs"]
mod duration;
#[path = "e2e.rs"]
mod e2e;
#[path = "embed_check_behaviour.rs"]
mod embed_check_behaviour;
#[path = "eq_bound_migration.rs"]
mod eq_bound_migration;
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
#[path = "events_schema_registry_behaviour.rs"]
mod events_schema_registry_behaviour;
#[path = "events_schema_version_behaviour.rs"]
mod events_schema_version_behaviour;
#[path = "events_workers_wiring.rs"]
mod events_workers_wiring;
#[path = "example_tests_behaviour.rs"]
mod example_tests_behaviour;
#[path = "examples.rs"]
mod examples;
#[path = "expr_types_capture.rs"]
mod expr_types_capture;
#[path = "expression_depth_limit.rs"]
mod expression_depth_limit;
#[path = "failure_explanation_behaviour.rs"]
mod failure_explanation_behaviour;
#[path = "fetch_headers.rs"]
mod fetch_headers;
#[path = "field_op_index.rs"]
mod field_op_index;
#[path = "firstparty_sources.rs"]
mod firstparty_sources;
#[path = "float_codec_behaviour.rs"]
mod float_codec_behaviour;
#[path = "fuzz_smoke.rs"]
mod fuzz_smoke;
#[path = "generated_site_json.rs"]
mod generated_site_json;
#[path = "grammar_coverage.rs"]
mod grammar_coverage;
#[path = "grammar_reference.rs"]
mod grammar_reference;
#[path = "history_behaviour.rs"]
mod history_behaviour;
#[path = "host_globals_qualified.rs"]
mod host_globals_qualified;
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
#[path = "http_raw_body_behaviour.rs"]
mod http_raw_body_behaviour;
#[path = "http_raw_caching_behaviour.rs"]
mod http_raw_caching_behaviour;
#[path = "http_security_behaviour.rs"]
mod http_security_behaviour;
#[path = "implementation_index.rs"]
mod implementation_index;
#[path = "import_closure.rs"]
mod import_closure;
#[path = "inlay_hints.rs"]
mod inlay_hints;
#[path = "inline_value_names.rs"]
mod inline_value_names;
#[path = "int_binding_arithmetic_behaviour.rs"]
mod int_binding_arithmetic_behaviour;
#[path = "integration_contracts_behaviour.rs"]
mod integration_contracts_behaviour;
#[path = "interpolation.rs"]
mod interpolation;
#[path = "json_codec_behaviour.rs"]
mod json_codec_behaviour;
#[path = "kernel_registry.rs"]
mod kernel_registry;
#[path = "keywords_reference.rs"]
mod keywords_reference;
#[path = "kv_behaviour.rs"]
mod kv_behaviour;
#[path = "locals_capture.rs"]
mod locals_capture;
#[path = "malformed_project_config_behaviour.rs"]
mod malformed_project_config_behaviour;
#[path = "method_index.rs"]
mod method_index;
#[path = "multi_actor_sum.rs"]
mod multi_actor_sum;
#[path = "multi_file_commons_barrel.rs"]
mod multi_file_commons_barrel;
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
#[path = "property_behaviour.rs"]
mod property_behaviour;
#[path = "provider_agent_cross_context_target.rs"]
mod provider_agent_cross_context_target;
#[path = "query_kernel_registry.rs"]
mod query_kernel_registry;
#[path = "readme_layering.rs"]
mod readme_layering;
#[path = "recovery_never_emits.rs"]
mod recovery_never_emits;
#[path = "reference_index.rs"]
mod reference_index;
#[path = "refinement_auth.rs"]
mod refinement_auth;
#[path = "refinement_index.rs"]
mod refinement_index;
#[path = "regex_refinement_behaviour.rs"]
mod regex_refinement_behaviour;
#[path = "remote_target_behaviour.rs"]
mod remote_target_behaviour;
#[path = "render_long_lines.rs"]
mod render_long_lines;
#[path = "requirements_ledger.rs"]
mod requirements_ledger;
#[path = "requires_range_fold_behaviour.rs"]
mod requires_range_fold_behaviour;
#[path = "reserved_builtin_type_name.rs"]
mod reserved_builtin_type_name;
#[path = "runtime_helpers.rs"]
mod runtime_helpers;
#[path = "schema_lock_check_and_test_cli.rs"]
mod schema_lock_check_and_test_cli;
#[path = "semantic_tokens_index.rs"]
mod semantic_tokens_index;
#[path = "signature_auth.rs"]
mod signature_auth;
#[path = "snippets.rs"]
mod snippets;
#[path = "source_map.rs"]
mod source_map;
#[path = "source_map_bodies.rs"]
mod source_map_bodies;
#[path = "store_behaviour.rs"]
mod store_behaviour;
#[path = "store_checker.rs"]
mod store_checker;
#[path = "store_op_registry.rs"]
mod store_op_registry;
#[path = "stream_behaviour.rs"]
mod stream_behaviour;
#[path = "suggestions.rs"]
mod suggestions;
#[path = "suite_diagnostics_attributed.rs"]
mod suite_diagnostics_attributed;
#[path = "suite_filter.rs"]
mod suite_filter;
#[path = "system_suite_heading_behaviour.rs"]
mod system_suite_heading_behaviour;
#[path = "test_json.rs"]
mod test_json;
#[path = "test_runner_resolution.rs"]
mod test_runner_resolution;
#[path = "textmate_keywords.rs"]
mod textmate_keywords;
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
#[path = "wire_shape_drift.rs"]
mod wire_shape_drift;
#[path = "workers_fault_log.rs"]
mod workers_fault_log;
#[path = "workers_runtime_smoke.rs"]
mod workers_runtime_smoke;
#[path = "wrangler_prewarm.rs"]
mod wrangler_prewarm;
#[path = "write_output_reconciliation_behaviour.rs"]
mod write_output_reconciliation_behaviour;
#[path = "ws_behaviour.rs"]
mod ws_behaviour;
#[path = "ws_open_split_target.rs"]
mod ws_open_split_target;

/// Every `*.rs` file in this directory, other than this root, is listed above.
/// With `autotests = false` cargo no longer discovers test files, so a file
/// added without its `mod` line would compile into nothing and its tests
/// would silently never run.
///
/// A file counts as listed only by a live `#[path = "x.rs"]` line (not one
/// commented out) followed directly by `mod x;`.
#[test]
fn it_lists_every_file() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let root = std::fs::read_to_string(dir.join("it.rs")).expect("read it.rs");
    let lines: Vec<&str> = root.lines().map(str::trim).collect();
    let listed: std::collections::HashSet<String> = lines
        .windows(2)
        .filter_map(|pair| {
            let file = pair[0].strip_prefix("#[path = \"")?.strip_suffix("\"]")?;
            let stem = file.strip_suffix(".rs")?;
            (pair[1] == format!("mod {stem};")).then(|| file.to_string())
        })
        .collect();
    let mut missing: Vec<String> = std::fs::read_dir(&dir)
        .expect("read tests/")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".rs") && n != "it.rs")
        .filter(|n| !listed.contains(n))
        .collect();
    missing.sort();
    assert!(
        missing.is_empty(),
        "tests/ files not included in tests/it.rs, so their tests never run: {missing:?}"
    );
}
