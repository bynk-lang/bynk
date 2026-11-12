//! #3445 (ADR 0678 C, amended): a constant in a literal-only annotation
//! argument.
//!
//! A bare reference to a constant counts as the literal it is bound to wherever
//! an annotation argument takes a literal, as ADR 0762 already does for SQL
//! text. The resolution is a lookup, not an evaluation: a constant's initialiser
//! is a closed static set ([`crate::consts::static_error`]), so a reference
//! resolves, by following constant-to-constant references, to literal text in
//! the source.
//!
//! The pass runs once, over the parsed declarations, ahead of every validator
//! and reader of annotation arguments (the checker, hover, the emitters, the
//! OpenAPI readers of `wire_name()`), and replaces the argument's value with the
//! resolved literal. Those consumers are syntactic and hold no constant table;
//! resolving once upstream means none of them can miss it. The replaced value
//! keeps the reference's span, and [`AnnotationArg::from_const`] names the
//! constant, so a diagnostic points at the reference and the IDE can find the
//! use.
//!
//! An argument that does not resolve (not a constant, the wrong declared type,
//! a refined constant, an initialiser that is not a plain literal, a name a
//! local shadows) is left as written, so the annotation's existing "bad
//! argument" diagnostic fires on it unchanged. No new diagnostic code.

use std::collections::{BTreeMap, HashMap, HashSet};

use bynk_syntax::ast::{
    Annotation, BaseType, Commons, CommonsItem, ConstRef, ExprKind, Handler, RecordField,
    Refinement, SourceUnit, TypeBody, TypeDecl, TypeRef,
};
use bynk_syntax::error::CompileError;

use crate::index::{RefSink, SymbolKind};
use crate::symbols::{UnitTable, build_unit_table};
use bynk_project::{ParsedFile, UnitKind};

/// One literal-only annotation argument: the annotation, its argument's label
/// (`None` for the positional one), and the base type its constant must be.
#[derive(Clone, Copy)]
struct ArgRule {
    annotation: &'static str,
    label: Option<&'static str>,
    base: BaseType,
}

const fn rule(annotation: &'static str, label: Option<&'static str>, base: BaseType) -> ArgRule {
    ArgRule {
        annotation,
        label,
        base,
    }
}

/// The record-field annotations: `@name("…")` (ADR 0812 C).
const FIELD_RULES: &[ArgRule] = &[rule("name", None, BaseType::String)];

/// A `store` field's annotations: `@ttl(d)` and `@retain(d)`.
const STORE_RULES: &[ArgRule] = &[
    rule("ttl", None, BaseType::Duration),
    rule("retain", None, BaseType::Duration),
];

/// An event's annotation: `@schema(n)`.
const EVENT_RULES: &[ArgRule] = &[rule("schema", None, BaseType::Int)];

/// A handler's annotations: `@cache(maxAge:, immutable:)`, `@limit(…)` and
/// `@raw(contentType:)`. The keyword arguments (`@cache(scope: public)`,
/// `@json(options: plain)`) and `@indexed(by:)` name words or fields, not
/// values, so they have no rule.
const HANDLER_RULES: &[ArgRule] = &[
    rule("cache", Some("maxAge"), BaseType::Duration),
    rule("cache", Some("immutable"), BaseType::Bool),
    rule("limit", Some("maxBody"), BaseType::Int),
    rule("limit", Some("maxFile"), BaseType::Int),
    rule("limit", Some("maxParts"), BaseType::Int),
    rule("raw", Some("contentType"), BaseType::String),
];

/// What the pass reads of the unit it is resolving in.
struct Scope<'a> {
    unit: &'a str,
    tables: &'a HashMap<String, UnitTable>,
    uses: &'a HashMap<String, Vec<String>>,
    /// The constants declared in the file being rewritten, so a related span at
    /// one is only offered where it indexes the same source.
    file_consts: HashSet<String>,
}

impl<'a> Scope<'a> {
    fn lookup(&self, u: &str) -> Option<(&'a UnitTable, &'a [String])> {
        let table = self.tables.get(u)?;
        let uses = self.uses.get(u).map(Vec::as_slice).unwrap_or(&[]);
        Some((table, uses))
    }
}

/// Rewrite the arguments of `annotations` named by `rules`. Returns whether any
/// argument changed.
fn rewrite_annotations(
    annotations: &mut [Annotation],
    rules: &[ArgRule],
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    let mut changed = false;
    for ann in annotations {
        for arg in &mut ann.args {
            let ExprKind::Ident(id) = &arg.value.kind else {
                continue;
            };
            let Some(rule) = rules.iter().find(|r| {
                r.annotation == ann.name.name
                    && r.label == arg.label.as_ref().map(|l| l.name.as_str())
            }) else {
                continue;
            };
            let Some(found) = crate::consts::resolve_const_literal(
                |u| scope.lookup(u),
                scope.unit,
                &id.name,
                rule.base,
            ) else {
                continue;
            };
            let same_file = found.owner.is_none() && scope.file_consts.contains(&id.name);
            arg.from_const = Some(ConstRef {
                name: id.clone(),
                decl_span: same_file.then_some(found.decl.name.span),
            });
            // The reference is a use of the constant: go-to-definition, hover,
            // find-references and rename see it as one (#3445 H).
            refs.record(id.span, SymbolKind::Const, &id.name);
            arg.value.kind = found.literal.kind.clone();
            changed = true;
        }
    }
    changed
}

/// `@example(lit)` after a refinement: one literal of the refinement's base.
fn rewrite_example(
    refinement: &mut Option<Refinement>,
    base: BaseType,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    let Some(r) = refinement else {
        return false;
    };
    rewrite_annotations(
        &mut r.annotations,
        &[rule("example", None, base)],
        scope,
        refs,
    )
}

/// The base a record field's inline refinement applies to (as
/// `checker::refinements` reads it): the field's own base type, or that of a
/// refined type it names in this unit.
fn field_base(field: &RecordField, scope: &Scope) -> Option<BaseType> {
    match &field.type_ref {
        TypeRef::Base(b, _) => Some(*b),
        TypeRef::Named(id) => match &scope.tables.get(scope.unit)?.types.get(&id.name)?.body {
            TypeBody::Refined { base, .. } => Some(*base),
            _ => None,
        },
        _ => None,
    }
}

fn rewrite_handlers(handlers: &mut [Handler], scope: &Scope, refs: &mut RefSink) -> bool {
    let mut changed = false;
    for h in handlers {
        changed |= rewrite_annotations(&mut h.annotations, HANDLER_RULES, scope, refs);
    }
    changed
}

fn rewrite_type(decl: &mut TypeDecl, scope: &Scope, refs: &mut RefSink) -> bool {
    let mut changed = false;
    match &mut decl.body {
        TypeBody::Refined {
            base, refinement, ..
        }
        | TypeBody::Opaque {
            base, refinement, ..
        } => changed |= rewrite_example(refinement, *base, scope, refs),
        TypeBody::Record(rec) => {
            for field in &mut rec.fields {
                changed |= rewrite_annotations(&mut field.annotations, FIELD_RULES, scope, refs);
                if let Some(base) = field_base(field, scope) {
                    changed |= rewrite_example(&mut field.refinement, base, scope, refs);
                }
            }
        }
        _ => {}
    }
    changed
}

/// Rewrite every literal-only annotation argument of `items`.
fn rewrite_items(items: &mut [CommonsItem], scope: &Scope, refs: &mut RefSink) -> bool {
    let mut changed = false;
    for item in items {
        match item {
            CommonsItem::Type(decl) => changed |= rewrite_type(decl, scope, refs),
            CommonsItem::Event(ev) => {
                changed |= rewrite_annotations(&mut ev.annotations, EVENT_RULES, scope, refs);
            }
            CommonsItem::Service(svc) => {
                changed |= rewrite_handlers(&mut svc.handlers, scope, refs)
            }
            CommonsItem::Agent(agent) => {
                for f in &mut agent.store_fields {
                    changed |= rewrite_annotations(&mut f.annotations, STORE_RULES, scope, refs);
                }
                changed |= rewrite_handlers(&mut agent.handlers, scope, refs);
            }
            _ => {}
        }
    }
    changed
}

/// Phase 5c: resolve a constant named by a literal-only annotation argument to
/// the literal it denotes, in every production unit. Run after `uses` resolution
/// (a used commons' public constants are in scope) and before anything reads an
/// annotation argument. A unit whose declarations changed has its symbol table
/// rebuilt from the rewritten files, so the table holds the resolved literals
/// too; the rebuild's own diagnostics were already reported by the first build.
pub fn resolve_annotation_consts(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &mut [ParsedFile],
    unit_tables: &mut HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    refs: &mut RefSink,
) {
    let mut changed_units = Vec::new();
    for (name, indices) in groups {
        if !matches!(kinds.get(name), Some(UnitKind::Context | UnitKind::Commons)) {
            continue;
        }
        let mut unit_changed = false;
        for &i in indices {
            let pf = &mut parsed[i];
            refs.enter_file(&pf.identity_path(), name, pf.is_synthetic());
            let items = match pf.unit_mut() {
                SourceUnit::Commons(c) => &mut c.items,
                SourceUnit::Context(c) => &mut c.items,
                _ => continue,
            };
            let scope = Scope {
                unit: name,
                tables: &*unit_tables,
                uses: unit_uses,
                file_consts: items
                    .iter()
                    .filter_map(|it| match it {
                        CommonsItem::Const(c) => Some(c.name.name.clone()),
                        _ => None,
                    })
                    .collect(),
            };
            unit_changed |= rewrite_items(items, &scope, refs);
        }
        if unit_changed {
            changed_units.push(name.clone());
        }
    }
    for name in changed_units {
        let kind = kinds[&name];
        let mut discarded = Vec::new();
        let table = build_unit_table(&name, kind, &groups[&name], parsed, &mut discarded);
        unit_tables.insert(name, table);
    }
}

/// Attach the constant's declaration to `err` as a related span, when the
/// argument was a reference to a constant in the same file.
pub fn with_const_origin(err: CompileError, arg: &bynk_syntax::ast::AnnotationArg) -> CompileError {
    let Some(c) = &arg.from_const else {
        return err;
    };
    let err = match c.decl_span {
        Some(span) => err.with_label(span, format!("`{}` is declared here", c.name.name)),
        None => err,
    };
    err.with_note(format!("the value comes from `const {}`", c.name.name))
}

/// The single-file form of [`resolve_annotation_consts`], for a commons checked
/// on its own (`bynk check file.bynk`, the playground, the editor's single-file
/// mode): no `uses`, so only the file's own constants are in scope. It runs the
/// same pass over a one-unit project, so the rules cannot differ.
pub fn resolve_single_file(commons: &mut Commons) {
    use std::path::PathBuf;
    let name = commons.name.joined();
    let file = ParsedFile::synthetic(
        PathBuf::from("single.bynk"),
        PathBuf::from("single.bynk"),
        String::new(),
        SourceUnit::Commons(commons.clone()),
        UnitKind::Commons,
    );
    let mut parsed = [file];
    let groups = BTreeMap::from([(name.clone(), vec![0])]);
    let kinds = BTreeMap::from([(name.clone(), UnitKind::Commons)]);
    let mut discarded = Vec::new();
    let table = build_unit_table(&name, UnitKind::Commons, &[0], &parsed, &mut discarded);
    let mut tables = HashMap::from([(name, table)]);
    resolve_annotation_consts(
        &groups,
        &kinds,
        &mut parsed,
        &mut tables,
        &HashMap::new(),
        &mut RefSink::new(),
    );
    if let SourceUnit::Commons(rewritten) = parsed[0].unit() {
        *commons = rewritten.clone();
    }
}
