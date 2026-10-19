//! #2134: a suite's configuration, `env { NAME = "value" … }`.
//!
//! A suite or case declares the variables and secrets its participants see.
//! At `system` each participant's Worker `env` holds them, as Wrangler would
//! put them there; at `unit`/`integration` they are what the `Secrets` test
//! double answers. One map applies to every participant, and a case's `env`
//! layers over its suite's, name by name.
//!
//! The checks here are tier-independent: a name given twice at one scope
//! (`bynk.suite.duplicate_env`) and a value that is not a `String` literal
//! (`bynk.suite.env_not_literal`). The `system`-only binding clash
//! (`bynk.suite.env_binding_clash`) needs each Worker's binding names, which
//! the emitter computes, so it is [`check_env_binding_clash`], called from
//! there with them.

use std::collections::{BTreeMap, HashMap};

use bynk_syntax::ast::{Case, EnvClause, ExprKind, SuiteDecl};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// `bynk.suite.duplicate_env` and `bynk.suite.env_not_literal` for one suite
/// declaration: its own `env` clauses as one scope, and each case's as
/// another.
pub fn check_env_clauses(decl: &SuiteDecl, errors: &mut Vec<CompileError>) {
    check_scope(&decl.envs, "suite", errors);
    for case in &decl.cases {
        check_scope(&case.envs, "case", errors);
    }
}

fn check_scope(envs: &[EnvClause], scope: &str, errors: &mut Vec<CompileError>) {
    let mut seen: HashMap<&str, Span> = HashMap::new();
    for entry in envs.iter().flat_map(|e| e.entries.iter()) {
        let name = entry.name.name.as_str();
        if let Some(prev) = seen.get(name) {
            errors.push(
                CompileError::new(
                    "bynk.suite.duplicate_env",
                    entry.name.span,
                    format!("`{name}` is given more than once in this {scope}'s `env`"),
                )
                .with_label(*prev, "first given here")
                .with_note(
                    "each name has one value per scope; a case's `env` may override a name its suite gives",
                ),
            );
        } else {
            seen.insert(name, entry.name.span);
        }
        if !matches!(entry.value.kind, ExprKind::StrLit(_)) {
            errors.push(
                CompileError::new(
                    "bynk.suite.env_not_literal",
                    entry.value.span,
                    format!("the value of `{name}` in an `env` clause is not a `String` literal"),
                )
                .with_note(
                    "a Worker's variables and secrets are strings, so an `env` entry's value is written as one: `NAME = \"value\"`",
                ),
            );
        }
    }
}

/// The variables and secrets `case` runs with: its suite's `env` entries,
/// overlaid by the case's own, name by name. Only literal values are kept;
/// a non-literal is already `bynk.suite.env_not_literal`.
pub fn case_env(decl: &SuiteDecl, case: &Case) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for envs in [&decl.envs, &case.envs] {
        for entry in envs.iter().flat_map(|e| e.entries.iter()) {
            if let ExprKind::StrLit(v) = &entry.value.kind {
                env.insert(entry.name.name.clone(), v.clone());
            }
        }
    }
    env
}

/// Whether `decl` gives any `env` entry, at the suite or on a case.
pub fn has_env(decl: &SuiteDecl) -> bool {
    !decl.envs.is_empty() || decl.cases.iter().any(|c| !c.envs.is_empty())
}

/// `bynk.suite.env_binding_clash`: at `system`, an `env` entry lands on each
/// participant's Worker `env`, so a name one of them already binds (a Service
/// Binding, KV namespace or Durable Object namespace), or a key the harness
/// sets there itself (a call edge's key, a foreign caller's secret), is
/// refused. `bindings` maps each such name to what holds it, as a phrase
/// ("a binding of `shop.ledger`'s Worker").
pub fn check_env_binding_clash(
    decl: &SuiteDecl,
    bindings: &BTreeMap<String, String>,
    errors: &mut Vec<CompileError>,
) {
    let entries = decl
        .envs
        .iter()
        .chain(decl.cases.iter().flat_map(|c| c.envs.iter()))
        .flat_map(|e| e.entries.iter());
    for entry in entries {
        let name = &entry.name.name;
        let Some(owner) = bindings.get(name) else {
            continue;
        };
        errors.push(
            CompileError::new(
                "bynk.suite.env_binding_clash",
                entry.name.span,
                format!("`{name}` is {owner}, so it cannot be given in `env`"),
            )
            .with_note(
                "at `system`, an `env` entry is put on every participant's Worker `env`, where it would clash with what is already there",
            )
            .with_note("rename the variable"),
        );
    }
}
