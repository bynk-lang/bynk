//! `bynk.Secrets` read-name checking (v0.173, ADR 0196 D1).
//!
//! `Secrets.get` takes an ordinary `String` expression, so a computed name is
//! invisible to any pass. Where one is seen, this module warns
//! (`bynk.secrets.computed_name`) — a non-failing diagnostic (ADR 0117): the
//! program is correct, `bynk deploy` simply cannot know which secret the
//! context reads, and cannot list it in the deploy plan.
//!
//! P5.5 (`design/tracks/semantics-in-the-checker.md` §6, §9): relocated here
//! from `bynk-emit/src/emitter/secrets.rs` — a real, `CompileError::new`-
//! constructed diagnostic, previously raised only from `bynk-driver::project`'s
//! `run_checks`, and (per that call site's own now-stale comment) never
//! reachable from [`crate::analysis::analyse_project`] at all, since
//! `bynk-check` cannot depend on `bynk-emit`. §9 named this an open risk
//! rather than a scoped relocation; this module closes it, per R3.5. The
//! manifest-only half — `declared_secrets`/`emit_secrets_manifest`/`render`,
//! which describe `bynk-secrets.json` rather than diagnose anything — stays
//! in `bynk-emit`, an emission concern this crate must not depend on. Its
//! caller there now reaches [`secret_reads_of`] here, qualified, rather than
//! duplicating the walk (the same dual-use pattern P5.4 used for
//! `bynk-check::test_suites`).

use std::collections::{HashMap, HashSet};

use bynk_syntax::ast::{Block, CapRef, Expr, ExprKind, ProviderDecl};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::symbols::UnitTable;

/// The capability whose `get` names a platform secret. Matched against the
/// capability a context actually resolved, never against the spelling — see
/// [`reads_secrets_of_bynk`].
const SECRETS_CAPABILITY: &str = "Secrets";

/// What a context's handlers read through `bynk.Secrets`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SecretReads {
    /// Literal names, sorted. A census only while `complete` holds.
    pub names: std::collections::BTreeSet<String>,
    /// False when at least one `Secrets.get` argument was not a literal, so no
    /// pass could know the name.
    pub complete: bool,
}

impl SecretReads {
    /// A context that reads nothing: complete by vacuity, and the shape a
    /// non-`bynk.Secrets` context gets without walking a single expression.
    fn none() -> Self {
        Self {
            names: std::collections::BTreeSet::new(),
            complete: true,
        }
    }
}

/// Does this context's `Secrets` resolve to **`bynk`**'s?
///
/// `flattened` maps a context's in-scope capability name to the unit
/// providing it, so this asks the question that matters — *whose* `Secrets`
/// is this? — rather than matching the identifier. A context with no
/// `Secrets` at all answers `false` here and never gets walked.
fn reads_secrets_of_bynk(flattened: &std::collections::HashMap<String, String>) -> bool {
    flattened
        .get(SECRETS_CAPABILITY)
        .is_some_and(|unit| unit == crate::firstparty::BYNK_UNIT)
}

/// The literal `bynk.Secrets` names this context's handlers read, and whether
/// that list is everything.
///
/// Split so callers can scope it differently without the *rule* differing:
/// `bynk-emit`'s manifest wants a context's whole handler set (its names span
/// every file), while this warning wants one file's handlers at a time,
/// because a diagnostic sink attributes a diagnostic to a path and a merged
/// handler set has thrown that away.
pub fn secret_reads_of<'a>(
    handlers: impl Iterator<Item = &'a bynk_syntax::ast::Handler>,
    flattened: &std::collections::HashMap<String, String>,
) -> (SecretReads, Vec<CompileError>) {
    if !reads_secrets_of_bynk(flattened) {
        return (SecretReads::none(), Vec::new());
    }
    let mut reads = SecretReads::none();
    let mut warnings = Vec::new();
    for handler in handlers {
        walk_block(&handler.body, &mut reads, &mut warnings);
    }
    (reads, warnings)
}

/// [`secret_reads_of`] over a provider's operation bodies — the per-file
/// warning path's twin for `provides … given Secrets { … }` (#1976).
pub fn provider_secret_reads_of<'a>(
    providers: impl Iterator<Item = &'a ProviderDecl>,
    flattened: &HashMap<String, String>,
) -> (SecretReads, Vec<CompileError>) {
    if !reads_secrets_of_bynk(flattened) {
        return (SecretReads::none(), Vec::new());
    }
    let mut reads = SecretReads::none();
    let mut warnings = Vec::new();
    for op in providers.flat_map(|p| p.ops.iter()) {
        walk_block(&op.body, &mut reads, &mut warnings);
    }
    (reads, warnings)
}

/// Everything a **Worker** reads through `bynk.Secrets`: its own handlers
/// (services and agents) and every provider its composition root builds
/// (#1976).
///
/// The provider set mirrors what `bynk-emit`'s `emit_worker_compose`
/// instantiates — every local provider, every capability a handler or local
/// provider is `given`, every derived instance — closed transitively over each
/// provider's own `given`, resolved in the provider's **own** unit exactly as
/// `instantiate_provider_ts_expr` resolves it. Walking only handler bodies
/// missed a `Secrets.get("X")` one `given` away, while the manifest still said
/// `read_complete: true`.
///
/// An *external* provider — an adapter's binding — `given` `bynk`'s `Secrets`
/// reads names no pass can see: its body is TypeScript. Such a Worker's list
/// is not a census, so `complete` goes false. No warning is raised for it:
/// nothing is computed, the reads are simply out of the compiler's sight.
pub fn worker_secret_reads(
    context: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
) -> SecretReads {
    let empty_flat = HashMap::new();
    let flat_of = |unit: &str| unit_flattened.get(unit).unwrap_or(&empty_flat);
    let Some(table) = unit_tables.get(context) else {
        return SecretReads::none();
    };
    let handlers = table
        .services
        .values()
        .flat_map(|s| s.handlers.iter())
        .chain(table.agents.values().flat_map(|a| a.handlers.iter()));
    let (mut reads, _) = secret_reads_of(handlers.clone(), flat_of(context));

    // Where `unit`'s `given` capability is provided — `instantiate_provider_
    // ts_expr`'s rule: a prefix names a consumed unit, a bare name is the
    // unit it was flattened from, and either falls back to `unit` itself.
    let resolve = |unit: &str, given: &CapRef| -> (String, String) {
        let target = match &given.context {
            Some(prefix) => crate::project_model::resolve_consume_prefix(
                &prefix.joined(),
                unit_consumes.get(unit).map(Vec::as_slice).unwrap_or(&[]),
                unit_consumes_aliases.get(unit).unwrap_or(&HashMap::new()),
            ),
            None => flat_of(unit).get(&given.name.name).cloned(),
        };
        (
            target.unwrap_or_else(|| unit.to_string()),
            given.name.name.clone(),
        )
    };

    let mut pending: Vec<(String, String)> = Vec::new();
    pending.extend(
        table
            .providers
            .keys()
            .map(|cap| (context.to_string(), cap.clone())),
    );
    for given in handlers.flat_map(|h| h.given.iter()) {
        pending.push(resolve(context, given));
    }
    for inst in table.derived.values() {
        pending.push((inst.base_unit.clone(), inst.base_cap.clone()));
    }

    let mut seen: HashSet<(String, String)> = HashSet::new();
    while let Some((unit, cap)) = pending.pop() {
        if !seen.insert((unit.clone(), cap.clone())) {
            continue;
        }
        let Some(provider) = unit_tables.get(&unit).and_then(|t| t.providers.get(&cap)) else {
            continue;
        };
        let deps: Vec<(String, String)> =
            provider.given.iter().map(|g| resolve(&unit, g)).collect();
        if provider.external {
            if deps
                .iter()
                .any(|(u, c)| u == crate::firstparty::BYNK_UNIT && c == SECRETS_CAPABILITY)
            {
                reads.complete = false;
            }
        } else {
            let (found, _) = provider_secret_reads_of(std::iter::once(provider), flat_of(&unit));
            reads.names.extend(found.names);
            reads.complete &= found.complete;
        }
        pending.extend(deps);
    }
    reads
}

/// Every expression in a handler body.
///
/// Reuses `statement_exprs` rather than matching `Statement` here: a
/// `Secrets.get` in a `let`, an `expect`, a `~>` send or a bare `do` is still a
/// read, and re-enumerating the statement kinds would be a second place to
/// forget one the day a new kind lands.
fn walk_block(block: &Block, reads: &mut SecretReads, warnings: &mut Vec<CompileError>) {
    let mut exprs: Vec<&Expr> = Vec::new();
    for statement in &block.statements {
        bynk_syntax::ast::statement_exprs(statement, &mut exprs);
    }
    exprs.push(&block.tail);
    for e in exprs {
        walk_expr(e, reads, warnings);
    }
}

/// Visit `e` and everything under it, recording each `Secrets.get` call.
///
/// Recurses through [`bynk_syntax::ast::expr_children`] rather than re-matching
/// every `ExprKind`: a `Secrets.get` inside a `match` arm, a lambda, or an
/// interpolation hole is still a read, and a hand-rolled visitor would be a
/// second place to forget that.
fn walk_expr(e: &Expr, reads: &mut SecretReads, warnings: &mut Vec<CompileError>) {
    if let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &e.kind
        && method.name == "get"
        && matches!(&receiver.kind, ExprKind::Ident(name) if name.name == SECRETS_CAPABILITY)
    {
        // Arity is the checker's (`bynk.capability.op_arity`); this reads the
        // one argument when it is there and stays quiet when it is not, rather
        // than reporting a second diagnostic about the same call.
        match args.first().map(|a| &a.kind) {
            Some(ExprKind::StrLit(name)) => {
                reads.names.insert(name.clone());
            }
            Some(_) => {
                reads.complete = false;
                warnings.push(computed_name_warning(e.span));
            }
            None => {}
        }
    }
    for child in bynk_syntax::ast::expr_children(e) {
        walk_expr(child, reads, warnings);
    }
}

/// The one thing that can tell an author `deploy` has lost sight of a secret.
///
/// A warning, not an error ([DECISION A]): `Secrets.get(pickName())` is legal
/// and sometimes reasonable, and making it a compile failure to serve a driver's
/// convenience would be the language spending expressiveness it does not need to
/// spend. The severity is carried by the code — `Severity::for_error` classifies
/// it, and the diagnostic sink routes on that.
fn computed_name_warning(span: Span) -> CompileError {
    CompileError::new(
        "bynk.secrets.computed_name",
        span,
        "`Secrets.get` is called with a computed name, so `bynk deploy` cannot know which secret this context reads"
            .to_string(),
    )
    .with_note(
        "the deploy plan will not list it, and will say its list of read secrets is incomplete; \
         pass a string literal if you want it planned",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // `secret_reads_of`'s own walk (`walk_block`/`walk_expr`) is pinned
    // end-to-end by `bynk-tests/tests/fixtures/positive/374_secrets_computed_name`
    // (`target.txt` = `workers`) — an earlier version of this comment pointed
    // at `differential_analysis.rs`/`analysis_residual_gap.rs` instead, which
    // was wrong: this PR's own additions to both establish that neither can
    // observe this walk (both new sites are gap-in-name-only under the
    // hardcoded `BuildTarget::Bundle`/`SchemaLock::Off` both analysis paths
    // share — see their own doc comments). The tests below cover the same
    // branches directly, now that `secret_reads_of` is `bynk-check`'s public
    // API, by parsing real source rather than hand-building a `Handler`.

    fn parse_context_handlers(src: &str) -> Vec<bynk_syntax::ast::Handler> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let unit = bynk_syntax::parser::parse_unit(&tokens, src).expect("parse");
        let bynk_syntax::ast::SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit");
        };
        ctx.items
            .into_iter()
            .filter_map(|item| match item {
                bynk_syntax::ast::CommonsItem::Service(s) => Some(s.handlers.into_iter()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn flattened_to_bynk() -> std::collections::HashMap<String, String> {
        [(
            SECRETS_CAPABILITY.to_string(),
            crate::firstparty::BYNK_UNIT.to_string(),
        )]
        .into_iter()
        .collect()
    }

    const LITERAL_READ: &str = "context net.probe\n\nconsumes bynk { Secrets }\n\nservice probe {\n  on call() -> Effect[Option[String]] given Secrets {\n    Secrets.get(\"API_KEY\")\n  }\n}\n";

    #[test]
    fn a_literal_argument_is_collected_and_warns_nothing() {
        let handlers = parse_context_handlers(LITERAL_READ);
        let flattened = flattened_to_bynk();
        let (reads, warnings) = secret_reads_of(handlers.iter(), &flattened);
        assert_eq!(reads.names, ["API_KEY".to_string()].into());
        assert!(reads.complete);
        assert!(warnings.is_empty());
    }

    const COMPUTED_READ: &str = "context net.probe\n\nconsumes bynk { Secrets }\n\nservice probe {\n  on call(key: String) -> Effect[Option[String]] given Secrets {\n    Secrets.get(key)\n  }\n}\n";

    #[test]
    fn a_computed_argument_warns_once_and_marks_the_read_set_incomplete() {
        let handlers = parse_context_handlers(COMPUTED_READ);
        let flattened = flattened_to_bynk();
        let (reads, warnings) = secret_reads_of(handlers.iter(), &flattened);
        assert!(reads.names.is_empty());
        assert!(!reads.complete);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].category, "bynk.secrets.computed_name");
    }

    const NESTED_IN_MATCH_ARM: &str = "context net.probe\n\nconsumes bynk { Secrets }\n\ntype Choice = enum { A, B }\n\nservice probe {\n  on call(key: String, choice: Choice) -> Effect[Option[String]] given Secrets {\n    match choice {\n      A => Secrets.get(key)\n      B => Secrets.get(\"FIXED\")\n    }\n  }\n}\n";

    #[test]
    fn a_call_nested_inside_a_match_arm_is_still_found() {
        // `expr_children` is trusted to recurse into constructs `walk_expr`
        // never names explicitly — a `match` arm's body is the case this test
        // pins, since `walk_expr` only ever matches `MethodCall` directly.
        let handlers = parse_context_handlers(NESTED_IN_MATCH_ARM);
        let flattened = flattened_to_bynk();
        let (reads, warnings) = secret_reads_of(handlers.iter(), &flattened);
        assert_eq!(reads.names, ["FIXED".to_string()].into());
        assert!(
            !reads.complete,
            "the other arm's computed argument still counts"
        );
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_context_whose_secrets_is_not_bynks_is_never_walked() {
        // Same computed-argument source as above — if this resolved to
        // `bynk`'s `Secrets`, it would warn; walking nothing is the point.
        let handlers = parse_context_handlers(COMPUTED_READ);
        let flattened: std::collections::HashMap<String, String> =
            [(SECRETS_CAPABILITY.to_string(), "acme.vault".to_string())]
                .into_iter()
                .collect();
        let (reads, warnings) = secret_reads_of(handlers.iter(), &flattened);
        assert_eq!(reads, SecretReads::none());
        assert!(warnings.is_empty());
    }

    #[test]
    fn reads_secrets_of_bynk_matches_on_the_resolved_unit_not_the_spelling() {
        let to_bynk: std::collections::HashMap<String, String> = [(
            SECRETS_CAPABILITY.to_string(),
            crate::firstparty::BYNK_UNIT.to_string(),
        )]
        .into_iter()
        .collect();
        assert!(reads_secrets_of_bynk(&to_bynk));

        let to_someone_else: std::collections::HashMap<String, String> =
            [(SECRETS_CAPABILITY.to_string(), "acme.vault".to_string())]
                .into_iter()
                .collect();
        assert!(!reads_secrets_of_bynk(&to_someone_else));

        assert!(!reads_secrets_of_bynk(&std::collections::HashMap::new()));
    }

    fn parse_context_providers(src: &str) -> Vec<ProviderDecl> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let unit = bynk_syntax::parser::parse_unit(&tokens, src).expect("parse");
        let bynk_syntax::ast::SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit");
        };
        ctx.items
            .into_iter()
            .filter_map(|item| match item {
                bynk_syntax::ast::CommonsItem::Provider(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    const PROVIDER_READS: &str = "context net.probe\n\nconsumes bynk { Secrets }\n\ncapability Mode {\n  fn current(key: String) -> Effect[Option[String]]\n}\n\nprovides Mode = SecretMode given Secrets {\n  fn current(key: String) -> Effect[Option[String]] {\n    let fixed <- Secrets.get(\"APP_MODE\")\n    Secrets.get(key)\n  }\n}\n";

    #[test]
    fn a_provider_body_is_walked_like_a_handler_body() {
        // #1976: only handler bodies were walked, so a provider's literal read
        // was missing from the manifest and its computed one went unwarned.
        let providers = parse_context_providers(PROVIDER_READS);
        let (reads, warnings) = provider_secret_reads_of(providers.iter(), &flattened_to_bynk());
        assert_eq!(reads.names, ["APP_MODE".to_string()].into());
        assert!(!reads.complete);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].category, "bynk.secrets.computed_name");

        let elsewhere: HashMap<String, String> =
            [(SECRETS_CAPABILITY.to_string(), "acme.vault".to_string())]
                .into_iter()
                .collect();
        let (reads, warnings) = provider_secret_reads_of(providers.iter(), &elsewhere);
        assert_eq!(reads, SecretReads::none());
        assert!(warnings.is_empty());
    }

    #[test]
    fn computed_name_warning_carries_the_registered_code() {
        let w = computed_name_warning(Span::default());
        assert_eq!(w.category, "bynk.secrets.computed_name");
    }
}
