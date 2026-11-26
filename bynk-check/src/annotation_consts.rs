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
//! keeps the reference's span, and [`bynk_syntax::ast::AnnotationArg::from_const`] names the
//! constant, so a diagnostic points at the reference and the IDE can find the
//! use.
//!
//! An argument that does not resolve (not a constant, the wrong declared type,
//! a refined constant, an initialiser that is not a plain literal, a name a
//! local shadows) is left as written, so the annotation's existing "bad
//! argument" diagnostic fires on it unchanged. No new diagnostic code.

use std::collections::{BTreeMap, HashMap, HashSet};

use bynk_syntax::ast::{
    Annotation, BaseType, Commons, CommonsItem, ConstRef, DeliveryPolicy, ExprKind, Handler,
    RecordField, Refinement, SourceUnit, TypeBody, TypeDecl, TypeRef,
};
use bynk_syntax::error::CompileError;

use crate::index::{RefSink, SymbolKind};
use crate::refinement_consts::resolve_refinement;
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

/// The record-field annotations: `@name("…")` (ADR 0812 C). A sum's variants and payload fields
/// take the same one (#3975).
const FIELD_RULES: &[ArgRule] = &[rule("name", None, BaseType::String)];

/// A sum type's annotation: `@discriminator("…")` (#3975).
const TYPE_RULES: &[ArgRule] = &[rule("discriminator", None, BaseType::String)];

/// A `store` field's annotations: `@ttl(d)` and `@retain(d)`.
const STORE_RULES: &[ArgRule] = &[
    rule("ttl", None, BaseType::Duration),
    rule("retain", None, BaseType::Duration),
];

/// An event's annotation: `@schema(n)`.
const EVENT_RULES: &[ArgRule] = &[rule("schema", None, BaseType::Int)];

/// An agent's annotations: `@busyAfter(d)` and `@jurisdiction("eu")`.
const AGENT_RULES: &[ArgRule] = &[
    rule("busyAfter", None, BaseType::Duration),
    rule("jurisdiction", None, BaseType::String),
];

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
pub(crate) struct Scope<'a> {
    pub(crate) unit: &'a str,
    pub(crate) tables: &'a HashMap<String, UnitTable>,
    pub(crate) uses: &'a HashMap<String, Vec<String>>,
    /// The constants declared in the file being rewritten, so a related span at
    /// one is only offered where it indexes the same source.
    pub(crate) file_consts: HashSet<String>,
    /// #3951: constants declared by the file itself, for a suite, whose own
    /// constants are in no unit table. Empty for a commons or context.
    pub(crate) local: Vec<bynk_syntax::ast::ConstDecl>,
}

impl<'a> Scope<'a> {
    pub(crate) fn lookup(&self, u: &str) -> Option<(&'a UnitTable, &'a [String])> {
        let table = self.tables.get(u)?;
        let uses = self.uses.get(u).map(Vec::as_slice).unwrap_or(&[]);
        Some((table, uses))
    }
}

/// Resolve one annotation argument that is a bare constant name to the literal
/// of type `base` it denotes, in place. Returns whether it changed; an argument
/// that does not resolve is left as written.
fn resolve_arg(
    arg: &mut bynk_syntax::ast::AnnotationArg,
    base: BaseType,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    let ExprKind::Ident(id) = &arg.value.kind else {
        return false;
    };
    let Some(found) =
        crate::consts::resolve_const_literal(|u| scope.lookup(u), scope.unit, &id.name, base)
    else {
        return false;
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
    true
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
            let Some(rule) = rules.iter().find(|r| {
                r.annotation == ann.name.name
                    && r.label == arg.label.as_ref().map(|l| l.name.as_str())
            }) else {
                continue;
            };
            changed |= resolve_arg(arg, rule.base, scope, refs);
        }
    }
    changed
}

/// #3942: a derived capability instance's options (`Service @worker`, `Queue
/// @name`, `Email @to`, `RateLimit @limit` and `@period`, …) take a constant
/// by the same rule. The option's kind comes from the base kind's own table, so
/// each option resolves to the literal its validator then reads. A list option
/// (`@allowedRecipients`) takes constants for its string elements.
fn rewrite_instance_options(
    derived: &mut bynk_syntax::ast::DerivedCapability,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    use bynk_std::OptionKind;
    let Some(kind) =
        crate::firstparty::instantiable(crate::firstparty::CLOUDFLARE_UNIT, &derived.base.name)
    else {
        return false;
    };
    let mut changed = false;
    for ann in &mut derived.annotations {
        let Some(option) = kind.option_kind(&ann.name.name) else {
            continue;
        };
        for arg in &mut ann.args {
            match option {
                OptionKind::Str => changed |= resolve_arg(arg, BaseType::String, scope, refs),
                OptionKind::PositiveInt => changed |= resolve_arg(arg, BaseType::Int, scope, refs),
                OptionKind::Duration => {
                    changed |= resolve_arg(arg, BaseType::Duration, scope, refs)
                }
                OptionKind::StrList => {
                    if let ExprKind::ListLit(items, ..) = &mut arg.value.kind {
                        for item in items {
                            if let Some(c) = resolve_element(item, BaseType::String, scope, refs) {
                                arg.element_consts.push(c);
                                changed = true;
                            }
                        }
                    }
                }
            }
        }
    }
    changed
}

/// [`resolve_arg`] for a list element, which has no argument of its own to carry
/// the constant: the reference is recorded as a use, and returned for the list
/// argument's [`bynk_syntax::ast::AnnotationArg::element_consts`].
fn resolve_element(
    item: &mut bynk_syntax::ast::Expr,
    base: BaseType,
    scope: &Scope,
    refs: &mut RefSink,
) -> Option<ConstRef> {
    let ExprKind::Ident(id) = &item.kind else {
        return None;
    };
    let found =
        crate::consts::resolve_const_literal(|u| scope.lookup(u), scope.unit, &id.name, base)?;
    let same_file = found.owner.is_none() && scope.file_consts.contains(&id.name);
    let reference = ConstRef {
        name: id.clone(),
        decl_span: same_file.then_some(found.decl.name.span),
    };
    refs.record(id.span, SymbolKind::Const, &id.name);
    item.kind = found.literal.kind.clone();
    Some(reference)
}

/// #3925: a `cors { origins: … }` value may name constants: the whole list as
/// one `List[String]` constant (`origins: AppOrigins`), or any element of a
/// written list as a `String` constant (`origins: [AppOrigin, "https://b.example"]`).
/// Each resolves to the literal it is bound to, as an annotation argument does
/// (ADR 0826), through the same helpers, so the checks for a wildcard and for
/// credentials, and the emitter, read literal text. A name that does not resolve
/// is left as written, so `bynk.http.cors_invalid_origins` fires on it.
fn rewrite_cors(
    policy: Option<&mut bynk_syntax::ast::CorsPolicy>,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    let Some(policy) = policy else {
        return false;
    };
    let mut changed = false;
    for field in policy
        .fields
        .iter_mut()
        .filter(|f| f.name.name == "origins")
    {
        match &mut field.value.kind {
            ExprKind::Ident(id) => {
                let Some(found) = crate::consts::resolve_string_list_const(
                    |u| scope.lookup(u),
                    scope.unit,
                    &id.name,
                ) else {
                    continue;
                };
                refs.record(id.span, SymbolKind::Const, &id.name);
                field.value.kind = found.literal.kind.clone();
                changed = true;
            }
            ExprKind::ListLit(items, ..) => {
                for item in items {
                    changed |= resolve_element(item, BaseType::String, scope, refs).is_some();
                }
            }
            _ => {}
        }
    }
    changed
}

/// The base type each `delivery { }` key's constant must have (#3877).
const DELIVERY_KEYS: &[(&str, BaseType)] = &[
    ("retries", BaseType::Int),
    ("retryDelay", BaseType::Duration),
    ("deadLetter", BaseType::String),
    ("batchSize", BaseType::Int),
    ("batchWait", BaseType::Duration),
    ("concurrency", BaseType::Int),
];

/// #3877: a `delivery { }` value that is a bare constant counts as the literal it
/// is bound to, as an annotation argument does (ADR 0826). An unresolved name is
/// left as written, so the key's own diagnostic (`bynk.queue.delivery_bad_option`)
/// fires on it.
fn rewrite_delivery(
    policy: Option<&mut DeliveryPolicy>,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    let Some(policy) = policy else {
        return false;
    };
    let mut changed = false;
    for field in &mut policy.fields {
        let ExprKind::Ident(id) = &field.value.kind else {
            continue;
        };
        let Some((_, base)) = DELIVERY_KEYS.iter().find(|(k, _)| *k == field.name.name) else {
            continue;
        };
        let Some(found) =
            crate::consts::resolve_const_literal(|u| scope.lookup(u), scope.unit, &id.name, *base)
        else {
            continue;
        };
        refs.record(id.span, SymbolKind::Const, &id.name);
        field.value.kind = found.literal.kind.clone();
        changed = true;
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
        // The unit's own refined type, or one it gets through `uses` (#3951):
        // the checker composes the same view.
        TypeRef::Named(id) => std::iter::once(scope.unit)
            .chain(
                scope
                    .uses
                    .get(scope.unit)
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            )
            .find_map(|u| match &scope.tables.get(u)?.types.get(&id.name)?.body {
                TypeBody::Refined { base, .. } => Some(*base),
                _ => None,
            }),
        _ => None,
    }
}

/// #3951: the refinements of a record body's fields (an event's fields are the
/// same shape as a record type's).
fn rewrite_fields(fields: &mut [RecordField], scope: &Scope, refs: &mut RefSink) -> bool {
    let mut changed = false;
    for field in fields {
        if let Some(base) = field_base(field, scope) {
            changed |= resolve_refinement(&mut field.refinement, base, None, scope, refs);
        }
    }
    changed
}

/// #3889: `from email(Support)`: resolve the named constant to the literal it denotes
/// and fill the protocol's address with it, so the address checks, the duplicate
/// check, the route table, `wrangler.toml` and the deploy ledger all see the literal.
/// A constant declared exactly `String` resolves; anything else is left with its
/// refusal recorded for `check_email_services` to report. The literal is checked as
/// an address afterwards, where a literal address is.
fn rewrite_email_address(
    protocol: &mut bynk_syntax::ast::ServiceProtocol,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    use bynk_syntax::ast::{AddressRefusal, ServiceProtocol};
    let ServiceProtocol::Email {
        address,
        address_const: Some(ac),
    } = protocol
    else {
        return false;
    };
    let name = ac.name.name.clone();
    match crate::consts::resolve_address_literal(|u| scope.lookup(u), scope.unit, &name) {
        Ok(found) => {
            let ExprKind::StrLit(text) = &found.literal.kind else {
                return false;
            };
            let same_file = found.owner.is_none() && scope.file_consts.contains(&name);
            ac.decl_span = same_file.then_some(found.decl.name.span);
            refs.record(ac.name.span, SymbolKind::Const, &name);
            *address = text.clone();
            true
        }
        Err(declared) => {
            ac.refusal = Some(match declared {
                None => AddressRefusal::Unknown,
                Some(d) => {
                    AddressRefusal::Declared(crate::context_checks::ts_type_ref_display(&d.ty))
                }
            });
            // Changed: the unit's table is rebuilt, so the refusal reaches the checker
            // with the service.
            true
        }
    }
}

fn rewrite_handlers(handlers: &mut [Handler], scope: &Scope, refs: &mut RefSink) -> bool {
    let mut changed = false;
    for h in handlers {
        changed |= rewrite_annotations(&mut h.annotations, HANDLER_RULES, scope, refs);
        changed |= rewrite_schedule(h, scope, refs);
    }
    changed
}

/// #3875: `on schedule(Nightly)` — resolve the named constant to the literal it
/// denotes and fill the handler's schedule expression with it, so the checker,
/// the trigger table, dispatch and every reader see the literal text. A constant
/// declared `String` or exactly `CronSchedule` resolves; anything else is left
/// with its refusal recorded for `bynk.cron.invalid_schedule` (or the unknown
/// name) to report. The literal is checked against `Cron` afterwards, where a
/// literal schedule is.
fn rewrite_schedule(h: &mut Handler, scope: &Scope, refs: &mut RefSink) -> bool {
    use bynk_syntax::ast::{HandlerKind, ScheduleRefusal};
    let Some(sc) = &mut h.schedule_const else {
        return false;
    };
    let name = sc.name.name.clone();
    match crate::consts::resolve_schedule_literal(|u| scope.lookup(u), scope.unit, &name) {
        Ok(found) => {
            let ExprKind::StrLit(text) = &found.literal.kind else {
                return false;
            };
            let same_file = found.owner.is_none() && scope.file_consts.contains(&name);
            sc.decl_span = same_file.then_some(found.decl.name.span);
            refs.record(sc.name.span, SymbolKind::Const, &name);
            h.kind = HandlerKind::Cron { expr: text.clone() };
            true
        }
        Err(declared) => {
            sc.refusal = Some(match declared {
                None => ScheduleRefusal::Unknown,
                Some(d) => {
                    ScheduleRefusal::Declared(crate::context_checks::ts_type_ref_display(&d.ty))
                }
            });
            // Changed: the unit's table is rebuilt, so the refusal reaches the
            // checker with the handler.
            true
        }
    }
}

pub(crate) fn rewrite_type(decl: &mut TypeDecl, scope: &Scope, refs: &mut RefSink) -> bool {
    let mut changed = rewrite_annotations(&mut decl.annotations, TYPE_RULES, scope, refs);
    match &mut decl.body {
        TypeBody::Refined {
            base, refinement, ..
        }
        | TypeBody::Opaque {
            base, refinement, ..
        } => {
            changed |= resolve_refinement(refinement, *base, Some(&decl.name.name), scope, refs);
            changed |= rewrite_example(refinement, *base, scope, refs);
        }
        TypeBody::Record(rec) => {
            changed |= rewrite_fields(&mut rec.fields, scope, refs);
            for field in &mut rec.fields {
                changed |= rewrite_annotations(&mut field.annotations, FIELD_RULES, scope, refs);
                if let Some(base) = field_base(field, scope) {
                    changed |= rewrite_example(&mut field.refinement, base, scope, refs);
                }
            }
        }
        // #3975: a variant's and a payload field's `@name`.
        TypeBody::Sum(sum) => {
            for v in &mut sum.variants {
                changed |= rewrite_annotations(&mut v.annotations, FIELD_RULES, scope, refs);
                for p in &mut v.payload {
                    changed |= rewrite_annotations(&mut p.annotations, FIELD_RULES, scope, refs);
                }
            }
        }
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
                changed |= rewrite_fields(&mut ev.body.fields, scope, refs);
                // #3975: an event field's `@name`.
                for field in &mut ev.body.fields {
                    changed |=
                        rewrite_annotations(&mut field.annotations, FIELD_RULES, scope, refs);
                }
                changed |= rewrite_annotations(&mut ev.annotations, EVENT_RULES, scope, refs);
            }
            CommonsItem::Service(svc) => {
                changed |= rewrite_email_address(&mut svc.protocol, scope, refs);
                changed |= rewrite_delivery(svc.delivery.as_mut(), scope, refs);
                changed |= rewrite_cors(svc.cors.as_mut(), scope, refs);
                changed |= rewrite_handlers(&mut svc.handlers, scope, refs)
            }
            CommonsItem::Agent(agent) => {
                changed |= rewrite_annotations(&mut agent.annotations, AGENT_RULES, scope, refs);
                for f in &mut agent.store_fields {
                    changed |= rewrite_annotations(&mut f.annotations, STORE_RULES, scope, refs);
                }
                changed |= rewrite_handlers(&mut agent.handlers, scope, refs);
            }
            CommonsItem::Capability(cap) => {
                if let Some(derived) = cap.derived.as_mut() {
                    changed |= rewrite_instance_options(derived, scope, refs);
                }
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
                local: Vec::new(),
            };
            unit_changed |= rewrite_items(items, &scope, refs);
        }
        if unit_changed {
            changed_units.push(name.clone());
        }
    }
    crate::refinement_consts::resolve_suite_refinements(parsed, unit_tables, unit_uses, refs);
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
    // A list argument's element is found by the reference's span, which the
    // resolved element keeps (#3942).
    let Some(c) = arg
        .from_const
        .as_ref()
        .or_else(|| arg.element_consts.iter().find(|c| c.name.span == err.span))
    else {
        // A check that reports on the whole list, not an element: say which
        // constants the list took values from.
        if arg.element_consts.is_empty() {
            return err;
        }
        let names: Vec<String> = arg
            .element_consts
            .iter()
            .map(|c| format!("`const {}`", c.name.name))
            .collect();
        return err.with_note(format!("some of the values come from {}", names.join(", ")));
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
