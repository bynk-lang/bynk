//! #3941: `const X: T = env(NAME[, default])`, a constant whose value comes from
//! `bynk.toml` at build time, per environment.
//!
//! This pass runs right after parsing, in the driver's build and in the
//! editor's analysis alike, and rewrites each such constant's initialiser to
//! the literal it resolves to, so every later phase sees an ordinary constant:
//! a resolved constant is a literal, and the emitter and runtime know nothing of
//! `env`. The value for the environment being built is `[env.<name>.vars]`,
//! then `[vars]`, then the constant's own default (decisions B and C); it is
//! never read from the Worker's run-time environment or the build machine's.
//!
//! The pass refuses what would otherwise fall back silently: a constant whose
//! type is not one a `bynk.toml` value can spell (`bynk.env.unsupported_type`),
//! a value of the wrong kind (`bynk.env.bad_value`), a required variable with
//! no value (`bynk.env.missing`), a `bynk.toml` variable no constant reads
//! (`bynk.env.unknown_variable`), two constants naming one variable with
//! different defaults (`bynk.env.conflicting_default`), and `env(…)` nested in
//! another initialiser (`bynk.env.not_initialiser`). A variable whose name
//! looks like a secret warns (`bynk.env.secret_like_name`).
//!
//! A value that breaks the constant's refinement is left to the ordinary
//! literal check that follows the rewrite, which reports it on the `env(…)`
//! initialiser.

use std::collections::{BTreeMap, BTreeSet};

use bynk_project::ParsedFile;
use bynk_project::vars::{ProjectVars, ResolvedConst, ValueSource, VarValue, is_var_name};
use bynk_syntax::ast::{
    BaseType, CommonsItem, ConstDecl, Expr, ExprKind, SourceUnit, TypeBody, TypeRef, UnaryOp,
    expr_children,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::lexer::MAX_SAFE_INT;

/// What the pass found: errors and warnings in the sources, and errors that
/// belong to `bynk.toml` itself.
#[derive(Default)]
pub struct Outcome {
    pub source: Vec<CompileError>,
    pub manifest: Vec<CompileError>,
    /// Each constant that resolved, with its value and where it came from.
    pub resolved: Vec<ResolvedConst>,
}

/// The variable names that look like secrets: `bynk.toml` is committed.
const SECRET_SUFFIXES: &[&str] = &["_KEY", "_SECRET", "_TOKEN", "_PASSWORD"];

/// Whether `e` is a call of `env`.
fn is_env_call(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Call { name, .. } if name.name == "env")
}

/// Whether `e` is, or contains, a call of `env`.
fn has_env_call(e: &Expr) -> bool {
    is_env_call(e) || expr_children(e).into_iter().any(has_env_call)
}

/// Report every `env(…)` nested inside `e` (not `e` itself), and, where the
/// call has a default and sits in a list, record, `Some`/`Ok`/`Err` or
/// parenthesis, put the default in its place so the refusal does not cascade
/// into an unknown-function error on the same call.
fn refuse_nested(e: &mut Expr, out: &mut Vec<CompileError>) {
    let children: Vec<&mut Expr> = match &mut e.kind {
        ExprKind::ListLit(items, ..) => items.iter_mut().collect(),
        ExprKind::RecordConstruction { fields, .. } => {
            fields.iter_mut().filter_map(|f| f.value.as_mut()).collect()
        }
        ExprKind::Paren(inner)
        | ExprKind::Some(inner)
        | ExprKind::Ok(inner)
        | ExprKind::Err(inner) => vec![&mut **inner],
        _ => {
            // Anywhere else, report the calls but leave them in place.
            let mut calls = Vec::new();
            collect_env_calls(e, &mut calls);
            out.extend(calls.into_iter().map(not_initialiser));
            return;
        }
    };
    for child in children {
        if is_env_call(child) {
            out.push(not_initialiser(child.span));
            if let ExprKind::Call { args, .. } = &child.kind
                && let Some(default) = args.get(1).cloned()
            {
                *child = default;
            }
        } else {
            refuse_nested(child, out);
        }
    }
}

/// The spans of the `env(…)` calls inside `e`, not counting `e` itself.
fn collect_env_calls(e: &Expr, out: &mut Vec<bynk_syntax::span::Span>) {
    for child in expr_children(e) {
        if is_env_call(child) {
            out.push(child.span);
        }
        collect_env_calls(child, out);
    }
}

fn not_initialiser(span: bynk_syntax::span::Span) -> CompileError {
    CompileError::new(
        "bynk.env.not_initialiser",
        span,
        "`env(…)` is only ever a constant's whole initialiser",
    )
    .with_note(
        "give the value its own constant (`const Origin: String = env(APP_ORIGIN)`) and name that constant where the value is used",
    )
}

/// The kind of value a constant of type `ty` takes from `bynk.toml`, when it is
/// one of the supported types.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Str,
    Int,
    Bool,
    Strings,
}

fn kind_of(ty: &TypeRef, items: &[CommonsItem]) -> Option<Kind> {
    match ty {
        TypeRef::Base(BaseType::String, _) => Some(Kind::Str),
        TypeRef::Base(BaseType::Int, _) => Some(Kind::Int),
        TypeRef::Base(BaseType::Bool, _) => Some(Kind::Bool),
        TypeRef::List(inner, _) => {
            matches!(**inner, TypeRef::Base(BaseType::String, _)).then_some(Kind::Strings)
        }
        // A refinement over a supported base, declared in the same unit.
        TypeRef::Named(id) => items.iter().find_map(|item| match item {
            CommonsItem::Type(t) if t.name.name == id.name => match &t.body {
                TypeBody::Refined { base, .. } => match base {
                    BaseType::String => Some(Kind::Str),
                    BaseType::Int => Some(Kind::Int),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }),
        _ => None,
    }
}

/// `value` as a literal expression of `kind`, in place of `at`, or `None` when
/// the value is the wrong kind.
fn literal(value: &VarValue, kind: Kind, at: &Expr) -> Option<Expr> {
    let mk = |kind: ExprKind| Expr {
        id: at.id,
        kind,
        span: at.span,
    };
    let str_lit = |s: &str| Expr {
        // Elements of a list share one synthetic id: they are never looked up
        // by it before the checker has typed the list as a whole.
        id: bynk_syntax::ast::ExprId::SYNTHETIC,
        kind: ExprKind::StrLit(s.to_string()),
        span: at.span,
    };
    Some(match (kind, value) {
        (Kind::Str, VarValue::Str(s)) => mk(ExprKind::StrLit(s.clone())),
        (Kind::Int, VarValue::Int(n)) if n.unsigned_abs() <= MAX_SAFE_INT as u64 => {
            mk(ExprKind::IntLit {
                value: *n,
                lexeme: n.to_string(),
            })
        }
        (Kind::Bool, VarValue::Bool(b)) => mk(ExprKind::BoolLit(*b)),
        (Kind::Strings, VarValue::List(items)) => mk(ExprKind::ListLit(
            items.iter().map(|s| str_lit(s)).collect(),
            None,
        )),
        _ => return None,
    })
}

fn expected(kind: Kind) -> &'static str {
    match kind {
        Kind::Str => "a string",
        Kind::Int => "an integer",
        Kind::Bool => "a boolean",
        Kind::Strings => "a list of strings",
    }
}

/// Rewrite every `env(…)` constant of `parsed` for `environment`.
pub fn resolve(parsed: &mut [ParsedFile], vars: &ProjectVars, environment: &str) -> Outcome {
    let mut out = Outcome::default();
    let mut read: BTreeSet<String> = BTreeSet::new();
    // Each variable's first default, to compare a second constant's against.
    let mut defaults: BTreeMap<String, String> = BTreeMap::new();
    for pf in parsed.iter_mut() {
        // #3941 decision I: a package consumed as a dependency takes values as
        // parameters; reading the consuming project's deployment configuration
        // is the application's business.
        let in_dependency = pf.package().is_some();
        let items = match pf.unit_mut() {
            SourceUnit::Commons(c) => &mut c.items,
            SourceUnit::Context(c) => &mut c.items,
            SourceUnit::Adapter(a) => &mut a.items,
            SourceUnit::Suite(_) => continue,
        };
        // Nothing to do for a unit with no `env(…)` anywhere in a constant.
        if !items
            .iter()
            .any(|i| matches!(i, CommonsItem::Const(c) if has_env_call(&c.init)))
        {
            continue;
        }
        // The unit's type declarations, for a refined type a constant names.
        let snapshot: Vec<CommonsItem> = items
            .iter()
            .filter(|i| matches!(i, CommonsItem::Type(_)))
            .cloned()
            .collect();
        for item in items.iter_mut() {
            if let CommonsItem::Const(c) = item {
                if in_dependency {
                    refuse_in_dependency(c, &mut out);
                } else {
                    resolve_const(
                        c,
                        &snapshot,
                        vars,
                        environment,
                        &mut read,
                        &mut defaults,
                        &mut out,
                    );
                }
            }
        }
    }
    for name in vars.names().collect::<BTreeSet<_>>() {
        if !read.contains(name) {
            out.manifest.push(
                CompileError::new(
                    "bynk.env.unknown_variable",
                    bynk_syntax::span::Span::new(0, 0),
                    format!("`bynk.toml` sets `{name}`, but no `env({name})` constant reads it"),
                )
                .with_note("a variable no constant reads is a typo until proven otherwise: it could not change anything, so the default would stay in force"),
            );
        }
    }
    out
}

/// A constant of a dependency package: `env(…)` is refused, and the default (if
/// there is one) stands in so the refusal does not cascade.
fn refuse_in_dependency(c: &mut ConstDecl, out: &mut Outcome) {
    if !is_env_call(&c.init) {
        // A nested call is just as much a read of the consumer's configuration.
        let mut calls = Vec::new();
        collect_env_calls(&c.init, &mut calls);
        for span in calls {
            out.source.push(in_dependency(&c.name.name, span));
        }
        return;
    }
    out.source.push(in_dependency(&c.name.name, c.init.span));
    if let ExprKind::Call { args, .. } = &c.init.kind
        && let Some(default) = args.get(1).cloned()
    {
        c.init = default;
    }
}

fn in_dependency(name: &str, span: bynk_syntax::span::Span) -> CompileError {
    CompileError::new(
        "bynk.env.in_dependency",
        span,
        format!(
            "constant `{name}` reads a `bynk.toml` variable, but a package consumed as a dependency cannot"
        ),
    )
    .with_note(
        "reading deployment configuration is the application's business: take the value as a parameter, and let the application read it with its own `env(…)` constant",
    )
}

#[allow(clippy::too_many_arguments)]
fn resolve_const(
    c: &mut ConstDecl,
    items: &[CommonsItem],
    vars: &ProjectVars,
    environment: &str,
    read: &mut BTreeSet<String>,
    defaults: &mut BTreeMap<String, String>,
    out: &mut Outcome,
) {
    // `env(…)` anywhere but as the whole initialiser.
    if is_env_call(&c.init) {
        if let ExprKind::Call { args, .. } = &mut c.init.kind {
            for a in args.iter_mut() {
                if is_env_call(a) {
                    out.source.push(not_initialiser(a.span));
                }
                refuse_nested(a, &mut out.source);
            }
        }
    } else {
        refuse_nested(&mut c.init, &mut out.source);
    }
    if !is_env_call(&c.init) {
        return;
    }
    let ExprKind::Call { args, .. } = &c.init.kind else {
        return;
    };
    let name_arg = args.first().and_then(|a| match &a.kind {
        ExprKind::Ident(id) if is_var_name(&id.name) => Some((id.name.clone(), id.span)),
        _ => None,
    });
    let (Some((var, var_span)), true) = (name_arg, (1..=2).contains(&args.len())) else {
        out.source.push(
            CompileError::new(
                "bynk.env.malformed_call",
                c.init.span,
                "`env` takes a variable name, bare, and an optional default: `env(NAME)` or `env(NAME, default)`",
            )
            .with_note("the name is capital letters, digits and `_`, starting with a capital letter (`APP_ORIGIN`)"),
        );
        return;
    };
    read.insert(var.clone());
    if SECRET_SUFFIXES.iter().any(|s| var.ends_with(s)) {
        out.source.push(
            CompileError::new(
                "bynk.env.secret_like_name",
                var_span,
                format!("`{var}` looks like a secret, but `bynk.toml` is committed"),
            )
            .with_note("keep secrets in `Secrets`, which a deployment sets outside the repository"),
        );
    }
    let default = args.get(1).cloned();
    if let Some(d) = &default {
        compare_defaults(&var, d, defaults, &mut out.source);
    }
    let Some(kind) = kind_of(&c.ty, items) else {
        out.source.push(
            CompileError::new(
                "bynk.env.unsupported_type",
                c.ty.span(),
                format!(
                    "constant `{}` is read from `bynk.toml`, which can spell a `String`, an `Int`, a `Bool` or a `List[String]` (or a refinement over the first two), not this type",
                    c.name.name
                ),
            ),
        );
        return;
    };
    // The default is checked every time, not only when it is the value used:
    // otherwise its type error would surface only in an environment that
    // leaves the variable unset.
    if let Some(d) = &default
        && let Some(found) = default_mismatch(d, kind)
    {
        out.source.push(CompileError::new(
            "bynk.env.bad_value",
            d.span,
            format!(
                "the default of `env({var})` is {found}, but constant `{}` needs {}",
                c.name.name,
                expected(kind)
            ),
        ));
    }
    match vars.resolve(environment, &var) {
        Some((value, origin)) => match literal(value, kind, &c.init) {
            Some(lit) => {
                out.resolved.push(ResolvedConst {
                    constant: c.name.name.clone(),
                    variable: var.clone(),
                    value: value.shown(),
                    source: origin.into(),
                });
                c.init = lit;
            }
            None => out.source.push(
                CompileError::new(
                    "bynk.env.bad_value",
                    c.init.span,
                    if kind == Kind::Int && matches!(value, VarValue::Int(_)) {
                        format!(
                            "`{var}` in {} is outside the range of an `Int` (±{MAX_SAFE_INT}), the largest a JavaScript number holds exactly",
                            origin.table()
                        )
                    } else {
                        format!(
                            "`{var}` is {} in {}, but constant `{}` needs {}",
                            value.kind(),
                            origin.table(),
                            c.name.name,
                            expected(kind)
                        )
                    },
                ),
            ),
        },
        None => match default {
            // The default stands in as the initialiser, with its own ids, and is
            // checked as a literal of the declared type like any other.
            Some(d) => {
                out.resolved.push(ResolvedConst {
                    constant: c.name.name.clone(),
                    variable: var.clone(),
                    value: default_shown(&d),
                    source: ValueSource::Default,
                });
                c.init = d;
            }
            None => out.source.push(
                CompileError::new(
                    "bynk.env.missing",
                    var_span,
                    format!(
                        "`{var}` has no value in the `{environment}` environment, and `env({var})` has no default"
                    ),
                )
                .with_note(format!(
                    "set it under `[env.{environment}.vars]` (or `[vars]` for every environment) in `bynk.toml`, or give `env` a default"
                )),
            ),
        },
    }
}

/// Record `d` as `var`'s default, or report it when another constant gave a
/// different one. Only defaults that can be compared are: a literal, a negated
/// integer, or a list of those. Anything else is left to the ordinary check.
fn compare_defaults(
    var: &str,
    d: &Expr,
    defaults: &mut BTreeMap<String, String>,
    out: &mut Vec<CompileError>,
) {
    let Some(key) = literal_text(d) else {
        return;
    };
    match defaults.get(var) {
        Some(first) if *first != key => out.push(
            CompileError::new(
                "bynk.env.conflicting_default",
                d.span,
                format!("`{var}` is given a different default here than by another constant"),
            )
            .with_note(
                "two constants reading one variable must agree on its default, or neither has one",
            ),
        ),
        Some(_) => {}
        None => {
            defaults.insert(var.to_string(), key);
        }
    }
}

/// A default's literal text, for comparing two defaults, or `None` when it is
/// not a literal.
fn literal_text(e: &Expr) -> Option<String> {
    Some(match &e.kind {
        ExprKind::StrLit(s) => format!("s:{s}"),
        ExprKind::IntLit { value, .. } => format!("i:{value}"),
        ExprKind::BoolLit(b) => format!("b:{b}"),
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => match &inner.kind {
            ExprKind::IntLit { value, .. } => format!("i:-{value}"),
            _ => return None,
        },
        ExprKind::ListLit(items, ..) => {
            let parts: Option<Vec<String>> = items.iter().map(literal_text).collect();
            format!("l:[{}]", parts?.join(","))
        }
        ExprKind::Paren(inner) => return literal_text(inner),
        _ => return None,
    })
}

/// What a default is when it is a literal of the wrong kind for `kind`
/// (`"a string"`, …), or `None` when it fits or is not a literal (the ordinary
/// check then sees it, as the initialiser, when it is the value in use).
fn default_mismatch(d: &Expr, kind: Kind) -> Option<&'static str> {
    let found = match &d.kind {
        ExprKind::StrLit(_) => "a string",
        ExprKind::IntLit { .. } => "an integer",
        ExprKind::UnaryOp(UnaryOp::Neg, inner) if matches!(inner.kind, ExprKind::IntLit { .. }) => {
            "an integer"
        }
        ExprKind::BoolLit(_) => "a boolean",
        ExprKind::ListLit(items, ..) => {
            if items.iter().all(|i| matches!(i.kind, ExprKind::StrLit(_))) {
                "a list of strings"
            } else {
                "a list"
            }
        }
        _ => return None,
    };
    (found != expected(kind)).then_some(found)
}

/// A default as it is written, for the plan: a literal's own text, else `…`.
fn default_shown(e: &Expr) -> String {
    match &e.kind {
        ExprKind::StrLit(s) => format!("{s:?}"),
        ExprKind::IntLit { lexeme, .. } => lexeme.clone(),
        ExprKind::BoolLit(b) => b.to_string(),
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => format!("-{}", default_shown(inner)),
        ExprKind::ListLit(items, ..) => {
            let parts: Vec<String> = items.iter().map(default_shown).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprKind::Paren(inner) => default_shown(inner),
        _ => "…".to_string(),
    }
}
