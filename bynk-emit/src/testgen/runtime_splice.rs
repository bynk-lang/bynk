//! The test runtime as spliced text (back-half paydown 1B.3a, #4047, Decisions A and C).
//!
//! Each fragment a generated test module embeds is authored as a module under
//! `bynk-emit/runtime/src/testing/`, type-checked with the rest of the runtime, and written back
//! by `runtime/scripts/bundle.mjs` to `testgen/test_runtime/<name>.ts` in its splice form. The
//! generated `test_runtime_modules.rs` table is the only place an artifact is `include_str!`'d, so
//! this module is the only way the harness reaches the test runtime: [`splice`] for a fragment as
//! it stands, [`TestRuntimeModule::source`] for one a caller builds on.
//!
//! The generated test module still *splices* the text rather than importing it (Decision A): each
//! module keeps its own clock, retry count, snapshot state and timer queue, and the output stays
//! byte-identical. Moving to imports is a change to [`splice`] once 1B.5 derives the test modules'
//! imports.

use bynk_ts::{TsBindingName, TsExpr, TsLit, TsProgram, TsStmt, TsType};

use super::test_runtime_modules::TEST_RUNTIME_MODULES;

/// One module of the test runtime. Exhaustive: a module added to `runtime/src/testing/` has to be
/// named here (the `every_module_is_in_the_table_both_ways` test holds the two together).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum TestRuntimeModule {
    /// #2137: the harness clock (`__bynkClockNow`, `__bynkAdvanceClock`) and the `Clock` that reads it.
    Clock,
    /// `ExpectationError` and the `expect` helpers every test module carries.
    Expectation,
    /// v0.119: the history-property runtime (`for all run: History[Agent]`).
    History,
    /// #2135: the test-only `Signature` signer.
    HmacSigner,
    /// ADR 0478: the cron and queue drivers.
    Jobs,
    /// #3328: the `from email` driver.
    JobsEmail,
    /// The `Authorization: Bearer …` signer (`__bynkSignHs256`).
    JwtSigner,
    /// v0.117: the observation runtime (`Cap.op called …`, `trace(Cap.op)`).
    Observation,
    /// #3052: the test OIDC issuer an `Oidc` route's drivers mint tokens with.
    OidcIssuer,
    /// The value-property runtime (seeded generation and shrinking).
    Property,
    /// A remote build's Service Bindings (`--against`).
    Remote,
    /// #2138: `__bynkRunId`, the value `run.id` reads in a remote build.
    RunId,
    /// #1994: the execution-context double and the settle helper for `detach`.
    Send,
    /// A remote build's settle loop.
    Settle,
    /// #2842: the snapshot runtime.
    Snapshot,
    /// #2683: the harness clock on virtual time.
    VirtualTime,
}

impl TestRuntimeModule {
    /// Every module, in the table's order.
    pub(super) const ALL: [TestRuntimeModule; 16] = [
        TestRuntimeModule::Clock,
        TestRuntimeModule::Expectation,
        TestRuntimeModule::History,
        TestRuntimeModule::HmacSigner,
        TestRuntimeModule::Jobs,
        TestRuntimeModule::JobsEmail,
        TestRuntimeModule::JwtSigner,
        TestRuntimeModule::Observation,
        TestRuntimeModule::OidcIssuer,
        TestRuntimeModule::Property,
        TestRuntimeModule::Remote,
        TestRuntimeModule::RunId,
        TestRuntimeModule::Send,
        TestRuntimeModule::Settle,
        TestRuntimeModule::Snapshot,
        TestRuntimeModule::VirtualTime,
    ];

    /// The module's name: its file under `runtime/src/testing/`, without the extension.
    pub(super) fn name(self) -> &'static str {
        match self {
            TestRuntimeModule::Clock => "clock",
            TestRuntimeModule::Expectation => "expectation",
            TestRuntimeModule::History => "history",
            TestRuntimeModule::HmacSigner => "hmac_signer",
            TestRuntimeModule::Jobs => "jobs",
            TestRuntimeModule::JobsEmail => "jobs_email",
            TestRuntimeModule::JwtSigner => "jwt_signer",
            TestRuntimeModule::Observation => "observation",
            TestRuntimeModule::OidcIssuer => "oidc_issuer",
            TestRuntimeModule::Property => "property",
            TestRuntimeModule::Remote => "remote",
            TestRuntimeModule::RunId => "run_id",
            TestRuntimeModule::Send => "send",
            TestRuntimeModule::Settle => "settle",
            TestRuntimeModule::Snapshot => "snapshot",
            TestRuntimeModule::VirtualTime => "virtual_time",
        }
    }

    fn row(self) -> &'static (&'static str, &'static [&'static str], &'static str) {
        TEST_RUNTIME_MODULES
            .iter()
            .find(|(name, _, _)| *name == self.name())
            .unwrap_or_else(|| panic!("`{}` is not in TEST_RUNTIME_MODULES", self.name()))
    }

    /// The module's splice text: exactly what a generated test module embeds.
    pub(super) fn source(self) -> &'static str {
        self.row().2
    }

    /// The module's text as one statement, ready to splice into a generated test module.
    pub(super) fn splice(self) -> TsStmt {
        TsStmt::raw(self.source().to_string(), None)
    }

    /// The modules this one's text uses names from, which a test module has to splice first.
    pub(super) fn deps(self) -> Vec<TestRuntimeModule> {
        self.row()
            .1
            .iter()
            .map(|dep| {
                TestRuntimeModule::ALL
                    .into_iter()
                    .find(|m| m.name() == *dep)
                    .unwrap_or_else(|| panic!("`{dep}` is not a test runtime module"))
            })
            .collect()
    }
}

/// Debug check (Decision C): every fragment spliced into `stmts` comes after each fragment it
/// uses names from, and every such fragment is spliced at all. Today's order, "expectation first",
/// was implicit in where each call sat; this makes it a checked rule.
fn debug_assert_splice_order(stmts: &[TsStmt]) {
    if !cfg!(debug_assertions) {
        return;
    }
    let position = |module: TestRuntimeModule| {
        stmts
            .iter()
            .position(|stmt| stmt.raw_text().is_some_and(|t| t.contains(module.source())))
    };
    for module in TestRuntimeModule::ALL {
        let Some(at) = position(module) else { continue };
        for dep in module.deps() {
            match position(dep) {
                Some(dep_at) => debug_assert!(
                    dep_at < at,
                    "the test runtime `{}` is spliced before `{}`, whose names it uses",
                    module.name(),
                    dep.name()
                ),
                None => debug_assert!(
                    false,
                    "the test runtime `{}` is spliced without `{}`, whose names it uses",
                    module.name(),
                    dep.name()
                ),
            }
        }
    }
}

/// A program of `stmts`, after checking that its spliced fragments are in dependency order.
pub(super) fn checked_program(stmts: Vec<TsStmt>) -> TsProgram {
    debug_assert_splice_order(&stmts);
    TsProgram { stmts }
}

/// #2138: `__bynkRunId`, the value `run.id` reads. A remote build's `system` module reads
/// `BYNK_TEST_RUN_ID` (the runner sets a fresh one); every in-process module is the fixed
/// `"local"`, even when the variable is set (an `--against` run's unit suites share its process
/// environment).
pub(super) fn run_id_stmt(remote: bool) -> TsStmt {
    if remote {
        TestRuntimeModule::RunId.splice()
    } else {
        TsStmt::const_stmt(
            TsBindingName::Ident("__bynkRunId".to_string()),
            Some(TsType::named("string")),
            TsExpr::Lit(TsLit::Str("local".to_string())),
            None,
        )
    }
}

/// #3052: `const __bynkOidcRemote: boolean = <remote>;`, the flag the issuer's text reads.
pub(super) fn oidc_remote_flag(remote: bool) -> TsStmt {
    TsStmt::const_stmt(
        TsBindingName::Ident("__bynkOidcRemote".to_string()),
        Some(TsType::named("boolean")),
        TsExpr::Lit(TsLit::Bool(remote)),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The enum and the generated table name the same modules, both ways, so neither can
    /// gain or lose one without the other.
    #[test]
    fn every_module_is_in_the_table_both_ways() {
        let enum_names: BTreeSet<&str> = TestRuntimeModule::ALL.iter().map(|m| m.name()).collect();
        let table_names: BTreeSet<&str> = TEST_RUNTIME_MODULES.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(enum_names, table_names);
        assert_eq!(enum_names.len(), TestRuntimeModule::ALL.len());
        assert_eq!(TestRuntimeModule::ALL.len(), TEST_RUNTIME_MODULES.len());
    }

    /// `ALL` follows the table's order, so a reader can lay one beside the other.
    #[test]
    fn all_follows_the_tables_order() {
        let names: Vec<&str> = TestRuntimeModule::ALL.iter().map(|m| m.name()).collect();
        let table: Vec<&str> = TEST_RUNTIME_MODULES.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(names, table);
    }

    /// A fragment is a script: nothing in it imports, and the only line-initial `export` is
    /// the one named in `bundle.mjs`'s `SPLICE_EXPORTS`.
    #[test]
    fn a_spliced_fragment_is_a_script() {
        for module in TestRuntimeModule::ALL {
            for line in module.source().lines() {
                assert!(
                    !line.starts_with("import "),
                    "{} splices an import: {line}",
                    module.name()
                );
                if line.starts_with("export ") {
                    assert_eq!(
                        (module, line.contains("__bynkSnapshotReport")),
                        (TestRuntimeModule::Snapshot, true),
                        "{} splices an export: {line}",
                        module.name()
                    );
                }
            }
        }
    }

    /// Every dependency of a module is a module, and none depends on itself.
    #[test]
    fn dependencies_are_modules_and_acyclic() {
        for module in TestRuntimeModule::ALL {
            let deps = module.deps();
            assert!(
                !deps.contains(&module),
                "{} depends on itself",
                module.name()
            );
            for dep in deps {
                assert!(
                    !dep.deps().contains(&module),
                    "{} and {} depend on each other",
                    module.name(),
                    dep.name()
                );
            }
        }
    }

    /// The generated declaration in `harness.ts` is the name `snapshot_harness` writes.
    #[test]
    fn the_harness_contract_matches_the_generator() {
        let contract = include_str!("../../runtime/src/testing/harness.ts");
        let written =
            crate::testgen::test_module::snapshot_harness(&std::collections::BTreeMap::new(), None);
        assert!(
            contract.contains(
                "declare const __bynkSnapshotFiles: globalThis.Record<string, __SnapshotFile>;"
            ),
            "harness.ts does not declare `__bynkSnapshotFiles` as the generator writes it"
        );
        assert!(
            written.contains(
                "const __bynkSnapshotFiles: globalThis.Record<string, __SnapshotFile> = {"
            ),
            "the generator no longer writes the declaration harness.ts mirrors"
        );
    }
}
