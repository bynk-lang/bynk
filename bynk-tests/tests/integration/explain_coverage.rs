//! #3163: every error code is explained, or is on a named exception list with
//! a reason. A new error code that ships with neither fails here, so the
//! backlog of unexplained codes can only shrink.
//!
//! `bynk explain <code>` is where the longer lesson behind a rule lives
//! (`design/proposals/README.md`, "Documentation is part of done"). A code earns
//! an entry in [`EXCEPTIONS`] only when it cannot yet have one, and the reason
//! says why. The explained set is backed separately, by a compiled pair per
//! code (`explain_examples.rs`). Warnings are exempt from the rule.
//!
//! Remove a code's entry here in the PR that explains it: the second test
//! refuses an entry for a code that now has an explanation.

use std::collections::BTreeSet;

use bynk_syntax::diagnostics::{EXPLANATIONS, REGISTRY};
use bynk_syntax::error::Severity;

/// Error codes with no explanation yet, each with the reason.
const EXCEPTIONS: &[(&str, &str)] = &[
    (
        "bynk.capability.not_provided",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.capability.provider_agent_cross_context",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.context.duplicate_binding",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.ws.open_transfer_cross_file",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.ws.open_transfer_key",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.suite.env_binding_clash",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.target.vendor_conflict",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.target.vendor_required",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.target.browser_bundle_only",
        "the example gate compiles for the bundle target only; this code needs the Workers target or a platform flag (#3163, gate work: a per-fixture target)",
    ),
    (
        "bynk.agent.class_conflict",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.agent.class_removed",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.agent.migrate_incomplete",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.agent.migrate_shape_mismatch",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.agent.migrate_unknown_version",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.agent.non_additive_state_change",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.agent.replaces_unknown",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.event.non_additive_schema_change",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.event.schema_version_mismatch",
        "the example gate never reads `bynk.schema.lock`; this code needs a recorded schema history (#3163, gate work: a fixture-supplied lock)",
    ),
    (
        "bynk.project.export_unknown",
        "a manifest- or project-level diagnostic with no source line, which the example gate cannot mark",
    ),
    (
        "bynk.project.unstable_unknown_item",
        "a manifest- or project-level diagnostic with no source line, which the example gate cannot mark",
    ),
    (
        "bynk.project.file_and_directory",
        "a manifest- or project-level diagnostic with no source line, which the example gate cannot mark",
    ),
    (
        "bynk.project.no_sources",
        "a manifest- or project-level diagnostic with no source line, which the example gate cannot mark",
    ),
    (
        "bynk.project.no_root",
        "a manifest- or project-level diagnostic with no source line, which the example gate cannot mark",
    ),
    (
        "bynk.package.dependency_cycle",
        "a manifest- or project-level diagnostic with no source line, which the example gate cannot mark",
    ),
    (
        "bynk.d1.migration_name",
        "a manifest- or project-level diagnostic with no source line, which the example gate cannot mark",
    ),
    (
        "bynk.asset.needs_project",
        "fires only for a single-file build of something that needs a project; the example gate compiles projects",
    ),
    (
        "bynk.embed.needs_project",
        "fires only for a single-file build of something that needs a project; the example gate compiles projects",
    ),
    (
        "bynk.parse.unexpected_adapter",
        "fires only for a single-file build of something that needs a project; the example gate compiles projects",
    ),
    (
        "bynk.parse.unexpected_context",
        "fires only for a single-file build of something that needs a project; the example gate compiles projects",
    ),
    (
        "bynk.parse.unexpected_suite",
        "fires only for a single-file build of something that needs a project; the example gate compiles projects",
    ),
    (
        "bynk.parse.expected_unit_header",
        "fires only for a single-file build of something that needs a project; the example gate compiles projects",
    ),
    (
        "bynk.index.unknown_key",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.locale.multiple_message_bundles",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.test.credential_needs_system",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.lex.multiline_open",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.lex.multiline_indent",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.lex.multiline_in_hole",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.lex.multiline_close_not_own_line",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.lex.unterminated_interpolation",
        "always arrives with a second diagnostic, so the example gate cannot isolate it",
    ),
    (
        "bynk.resolve.duplicate_fn",
        "a test needs it to stay unexplained (#3163)",
    ),
    (
        "bynk.property.restates_refinement",
        "a test needs it to stay unexplained (#3163)",
    ),
    (
        "bynk.package.checksum_mismatch",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.package.git_failed",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.package.lock_invalid",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.package.lock_out_of_date",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.package.not_fetched",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.package.unsafe_source",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.package.version_conflict",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.account_refused",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.bad_archive",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.bad_signature",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.http_failed",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.key_mismatch",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.nonconformant",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.publish_refused",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.source_mismatch",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.registry.unavailable",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.deploy.contract_skew",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.deploy.foreign_contract_skew",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.test.remote_unsupported_advance",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.test.remote_unsupported_asset",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.test.remote_unsupported_call",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.test.remote_unsupported_env",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.test.remote_unsupported_queue",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.test.remote_unsupported_schedule",
        "needs the network, git, a registry, lockfile state or a live deployment: not expressible as a compiled example",
    ),
    (
        "bynk.asset.unreadable_file",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.embed.unreadable_file",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.embed.too_large",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.fixture.too_large",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.fixture.unreadable_file",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.d1.migration_unreadable",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.project.read_failed",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.project.contracts_lock_corrupt",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.project.schema_registry_corrupt",
        "needs an unreadable, oversized or corrupt file: not expressible as a compiled example",
    ),
    (
        "bynk.lex.decimal_literal_too_large",
        "a size or depth limit, with no readable example",
    ),
    (
        "bynk.lex.interpolation_too_deep",
        "a size or depth limit, with no readable example",
    ),
    (
        "bynk.parse.nesting_too_deep",
        "a size or depth limit, with no readable example",
    ),
    (
        "bynk.types.decimal_negation_too_large",
        "a size or depth limit, with no readable example",
    ),
    (
        "bynk.codegen.tsc_failed",
        "a tool failure or compiler bug, not a program error",
    ),
    (
        "bynk.wasm.panic",
        "a tool failure or compiler bug, not a program error",
    ),
    (
        "bynk.wasm.strip_failed",
        "a tool failure or compiler bug, not a program error",
    ),
    (
        "bynk.fmt.comment_loss",
        "a tool failure or compiler bug, not a program error",
    ),
    (
        "bynk.fmt.mixed_comment_syntax",
        "a tool failure or compiler bug, not a program error",
    ),
    (
        "bynk.fmt.roundtrip",
        "a tool failure or compiler bug, not a program error",
    ),
];

#[test]
fn every_error_code_is_explained_or_a_named_exception() {
    let explained: BTreeSet<&str> = EXPLANATIONS.iter().map(|e| e.code).collect();
    let excepted: BTreeSet<&str> = EXCEPTIONS.iter().map(|(code, _)| *code).collect();
    let missing: Vec<&str> = REGISTRY
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code)
        .filter(|code| !explained.contains(code) && !excepted.contains(code))
        .collect();
    assert!(
        missing.is_empty(),
        "error codes with no `bynk explain` entry (add one in `EXPLANATIONS` in \
         bynk-syntax/src/diagnostics.rs, with its fixture pair under \
         bynk-tests/tests/fixtures/explain/, or, if it cannot have a compiled \
         example, name it with a reason in EXCEPTIONS in this file):\n  {}",
        missing.join("\n  ")
    );
}

/// The list names real, unexplained error codes, each with a reason.
#[test]
fn the_exception_list_names_unexplained_error_codes() {
    let explained: BTreeSet<&str> = EXPLANATIONS.iter().map(|e| e.code).collect();
    let mut seen = BTreeSet::new();
    for (code, reason) in EXCEPTIONS {
        assert!(seen.insert(*code), "`{code}` is listed twice");
        let info = REGISTRY
            .iter()
            .find(|d| d.code == *code)
            .unwrap_or_else(|| panic!("`{code}` is not a registry code"));
        assert_eq!(info.severity, Severity::Error, "`{code}` is not an error");
        assert!(
            !explained.contains(code),
            "`{code}` now has an explanation: drop it from EXCEPTIONS"
        );
        assert!(!reason.trim().is_empty(), "`{code}` has no reason");
    }
}
