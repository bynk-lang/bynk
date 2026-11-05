//! #2134: a suite's configuration, `env { NAME = "value" … }`.
//!
//! A suite or case declares the variables and secrets its participants see.
//! At `system` each participant's Worker `env` holds them, as Wrangler would
//! put them there; at `unit`/`integration` they are what the `Secrets` test
//! double answers. One map applies to every participant, and a case's `env`
//! layers over its suite's, name by name.
//!
//! The checks here are tier-independent: a name given twice at one scope
//! (`bynk.suite.duplicate_env`) and a value that is neither a `String`
//! literal nor a `fixture` of the same suite (`bynk.suite.env_not_literal`). The `system`-only binding clash
//! (`bynk.suite.env_binding_clash`) needs each Worker's binding names, which
//! the emitter computes, so it is [`check_env_binding_clash`], called from
//! there with them.

use std::collections::{BTreeMap, HashMap};

use bynk_syntax::ast::{Case, EnvClause, Expr, ExprKind, FixtureDecl, SuiteDecl};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// `bynk.suite.duplicate_env` and `bynk.suite.env_not_literal` for one suite
/// declaration: its own `env` clauses as one scope, and each case's as
/// another.
pub fn check_env_clauses(decl: &SuiteDecl, errors: &mut Vec<CompileError>) {
    check_scope(decl, &decl.envs, "suite", errors);
    for case in &decl.cases {
        check_scope(decl, &case.envs, "case", errors);
    }
}

/// #2956: what an `env` entry's value is. A `fixture` of the same suite is a
/// compile-time `String` constant, so it is as static as a literal; its text
/// stays in the test module once, and the entry refers to it by path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    /// A `String` literal, verbatim.
    Literal(String),
    /// The suite's `fixture` of that normalised path.
    Fixture(String),
}

/// The fixture of `decl` that `value` names, when it is a bare name of one.
fn fixture_named<'a>(decl: &'a SuiteDecl, value: &Expr) -> Option<&'a FixtureDecl> {
    let ExprKind::Ident(id) = &value.kind else {
        return None;
    };
    decl.fixtures.iter().find(|f| f.name.name == id.name)
}

/// The [`EnvValue`] of one entry's value, or `None` when it is neither (already
/// `bynk.suite.env_not_literal`).
fn env_value(decl: &SuiteDecl, value: &Expr) -> Option<EnvValue> {
    if let ExprKind::StrLit(v) = &value.kind {
        return Some(EnvValue::Literal(v.clone()));
    }
    fixture_named(decl, value)
        .map(|f| EnvValue::Fixture(bynk_project::assets::normalise_asset_path(&f.path)))
}

fn check_scope(decl: &SuiteDecl, envs: &[EnvClause], scope: &str, errors: &mut Vec<CompileError>) {
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
        if env_value(decl, &entry.value).is_none() {
            errors.push(
                CompileError::new(
                    "bynk.suite.env_not_literal",
                    entry.value.span,
                    format!(
                        "the value of `{name}` in an `env` clause is not a `String` literal or a fixture of this suite"
                    ),
                )
                .with_note(
                    "a Worker's variables and secrets are strings, so an `env` entry's value is written as one, `NAME = \"value\"`, or names a `fixture` this suite declares",
                ),
            );
        }
    }
}

/// The variables and secrets `case` runs with, as written: its suite's `env`
/// entries, overlaid by the case's own, name by name. Only a literal or a
/// fixture of the suite is kept; anything else is already
/// `bynk.suite.env_not_literal`.
pub fn case_env_values(decl: &SuiteDecl, case: &Case) -> BTreeMap<String, EnvValue> {
    let mut env = BTreeMap::new();
    for envs in [&decl.envs, &case.envs] {
        for entry in envs.iter().flat_map(|e| e.entries.iter()) {
            if let Some(v) = env_value(decl, &entry.value) {
                env.insert(entry.name.name.clone(), v);
            }
        }
    }
    env
}

/// [`case_env_values`] with each fixture replaced by its text, from
/// `fixtures` (each declared file's text by normalised path). A fixture whose
/// text is not there was refused when it was read, so it is left out.
pub fn case_env(
    decl: &SuiteDecl,
    case: &Case,
    fixtures: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    case_env_values(decl, case)
        .into_iter()
        .filter_map(|(k, v)| match v {
            EnvValue::Literal(s) => Some((k, s)),
            EnvValue::Fixture(path) => fixtures.get(&path).map(|t| (k, t.clone())),
        })
        .collect()
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

/// #2138 (end-to-end testing track §3.7): a case run against a named target.
/// A deployed target already runs with its own variables, so the runner can't
/// apply the case's `env`; the case runs only if its `env` and the target's
/// declared `vars` agree on every name either mentions. Refused
/// (`bynk.test.remote_unsupported_env`), first match reported:
/// - an entry naming one of the target's `secrets` — a secret comes from the
///   environment, never from a suite;
/// - an entry the target doesn't declare, or declares with another value;
/// - a declared variable the case doesn't set, which would read as `None` in
///   process, so the case wouldn't promote unchanged.
pub fn remote_env_disagreement(
    case_name: &str,
    span: Span,
    env: &BTreeMap<String, String>,
    vars: &BTreeMap<String, String>,
    secrets: &[String],
) -> Option<CompileError> {
    let refusal = |what: String, note: &str| {
        Some(
            CompileError::new(
                "bynk.test.remote_unsupported_env",
                span,
                format!(
                    "case `\"{case_name}\"` {what}, so it can't run against this target unchanged"
                ),
            )
            .with_note(note.to_string()),
        )
    };
    for (name, value) in env {
        if secrets.contains(name) {
            return refusal(
                format!("sets the secret `{name}` in its `env`"),
                "against a named target a secret comes from the environment, never from a suite; drop it from the case's `env` for this run",
            );
        }
        match vars.get(name) {
            None => {
                return refusal(
                    format!("sets `{name}` in its `env`, which the target doesn't declare"),
                    "declare the value the target was deployed with under its `vars`, or run the case in process",
                );
            }
            Some(deployed) if deployed != value => {
                return refusal(
                    format!(
                        "sets `{name} = \"{value}\"`, but the target was deployed with `\"{deployed}\"`"
                    ),
                    "a suite written for one configuration must not run against another; run it in process, or against a target deployed as it expects",
                );
            }
            Some(_) => {}
        }
    }
    if let Some(name) = vars.keys().find(|n| !env.contains_key(*n)) {
        return refusal(
            format!("doesn't set `{name}`, which the target declares"),
            "in process an unset name reads as `None`, so the target is a different environment; pin the value in the case's `env`",
        );
    }
    None
}

#[cfg(test)]
mod remote_env_tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn check(env: &[(&str, &str)], vars: &[(&str, &str)], secrets: &[&str]) -> Option<String> {
        let secrets: Vec<String> = secrets.iter().map(|s| s.to_string()).collect();
        remote_env_disagreement("c", Span::default(), &map(env), &map(vars), &secrets)
            .map(|e| e.message)
    }

    #[test]
    fn agreeing_env_and_vars_run() {
        assert_eq!(check(&[], &[], &[]), None);
        assert_eq!(check(&[("P", "fake")], &[("P", "fake")], &["S"]), None);
    }

    #[test]
    fn every_disagreement_is_refused() {
        let listed = check(&[("S", "x")], &[], &["S"]).expect("a listed name is refused");
        assert!(listed.contains("sets the secret `S`"), "{listed}");
        let undeclared = check(&[("P", "fake")], &[], &[]).expect("undeclared is refused");
        assert!(undeclared.contains("doesn't declare"), "{undeclared}");
        let differs = check(&[("P", "fake")], &[("P", "real")], &[]).expect("differs");
        assert!(differs.contains("deployed with `\"real\"`"), "{differs}");
        let unset = check(&[], &[("P", "real")], &[]).expect("unset is refused");
        assert!(unset.contains("doesn't set `P`"), "{unset}");
    }
}
