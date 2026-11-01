//! #1824: where a unit's own code meets an imported declaration's position
//! that names a type the unit shadows.
//!
//! A unit's own `type Repo` shadows the `Repo` of a commons it `uses`
//! directly (`compose_unit_symbols`: local first), and that stays legal:
//! `1697_derived_names_collide` declares its own `Message` beside
//! `uses bynk.locale.types`. But type names are bare strings to the checker,
//! so an imported declaration that names `Repo` (`t.model`'s
//! `type Run = { repo: Repo }`) is checked here against the *local* `Repo`.
//! `Run { repo: 42 }` checks, and the emitted TypeScript, typed against
//! `t.core`'s `Repo`, fails `tsc`.
//!
//! [`crate::project_model::close_reachable_types`] records each such reach
//! ([`ShadowedReaches`]); this pass, run after a file's bodies are checked,
//! reports `bynk.uses.name_conflict` where a value crosses between the unit's
//! own code and one of those *shadowed positions*. A shadowed position is a
//! position of an imported declaration whose declared type names a shadowed
//! type, resolved in the declaring unit's scope:
//!
//! - a value goes **in** at a record field (`Run { repo: … }`, and the
//!   overrides of `Run { ...r, repo: … }`), a fn, method or static parameter,
//!   or a variant payload (`Text(…)`);
//! - a value comes **out** of a field read (`r.repo`), a call's result, or a
//!   payload a `match` arm or `is` test binds.
//!
//! A value that comes out of one shadowed position straight into another
//! naming the same types never meets the unit's own code: both sides mean the
//! imported type, in the checker and in the emitted TypeScript alike. That is
//! 1697's `render(tag, message("greeting"))`, which stays legal. Every other
//! crossing is a conflict, including one that goes through a `let` first:
//! the rule reads the expression in the position, not where its value came
//! from, so it errs towards rejecting.
//!
//! Not covered: a reference to an imported fn as a value (`list.map(f)`), and
//! a payload bound by a nested pattern (`Some(Text(v))`). Test suites are
//! checked by `crate::test_suites`, which does not run this pass.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::checker::{Callee, Ty, TypedCommons};
use crate::project_model::{ShadowedReaches, ShadowedType};
use crate::resolver::MethodTable as ResolverMethodTable;
use bynk_syntax::ast::*;
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// The unit-level tables the pass resolves an imported declaration through.
pub struct ShadowScope<'a> {
    /// The unit being checked.
    pub unit: &'a str,
    pub shadowed: &'a ShadowedReaches,
    pub combined_types: &'a HashMap<String, Arc<TypeDecl>>,
    pub combined_fns: &'a HashMap<String, Arc<FnDecl>>,
    pub combined_methods: &'a HashMap<String, ResolverMethodTable>,
    pub imported_from: &'a HashMap<String, String>,
}

/// A shadowed position of an imported declaration: what it is called in a
/// diagnostic, the unit that declares it, and the shadowed types it names.
struct Position<'a> {
    what: String,
    declared_in: &'a str,
    types: Vec<&'a ShadowedType>,
}

/// Report every crossing in `typed`'s bodies: fns and methods (with their
/// contracts), service and agent handlers, agent invariants and transitions,
/// and provider operations. Empty when no imported declaration reaches a
/// shadowed type, which is the common case and costs one check.
pub fn check_shadowed_uses(typed: &TypedCommons, scope: &ShadowScope<'_>) -> Vec<CompileError> {
    if scope.shadowed.is_empty() {
        return Vec::new();
    }
    let mut walk = Walk {
        typed,
        scope,
        accepted: HashSet::new(),
        errors: Vec::new(),
    };
    for item in &typed.commons.items {
        match item {
            CommonsItem::Fn(f) => {
                walk.block(&f.body);
                for c in f.requires.iter().chain(&f.ensures) {
                    walk.expr(&c.predicate);
                }
            }
            CommonsItem::Service(s) => {
                for h in &s.handlers {
                    walk.block(&h.body);
                }
            }
            CommonsItem::Agent(a) => {
                for h in &a.handlers {
                    walk.block(&h.body);
                }
                for p in a
                    .invariants
                    .iter()
                    .map(|i| &i.predicate)
                    .chain(a.transitions.iter().map(|t| &t.predicate))
                {
                    walk.expr(p);
                }
            }
            CommonsItem::Provider(p) => {
                for op in &p.ops {
                    walk.block(&op.body);
                }
            }
            _ => {}
        }
    }
    walk.errors
}

struct Walk<'a, 's> {
    typed: &'a TypedCommons,
    scope: &'a ShadowScope<'s>,
    /// Values that come out of a shadowed position straight into one naming
    /// the same types: not a crossing.
    accepted: HashSet<ExprId>,
    errors: Vec<CompileError>,
}

impl<'a, 's> Walk<'a, 's>
where
    's: 'a,
{
    fn block(&mut self, b: &Block) {
        let mut exprs = Vec::new();
        for s in &b.statements {
            statement_exprs(s, &mut exprs);
        }
        exprs.push(&b.tail);
        for e in exprs {
            self.expr(e);
        }
    }

    /// Pre-order, so a value an enclosing position accepts is marked before
    /// the value itself is visited.
    fn expr(&mut self, e: &Expr) {
        match &e.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
                for (arg, pos) in self.params_of(e, args.len()) {
                    if let Some(pos) = pos {
                        self.value_in(&args[arg], args[arg].span, &pos);
                    }
                }
            }
            ExprKind::RecordConstruction { type_name, fields } => {
                self.field_inits(&type_name.name, fields);
            }
            ExprKind::RecordSpread {
                type_name: Some(type_name),
                overrides,
                ..
            } => {
                self.field_inits(&type_name.name, overrides);
            }
            ExprKind::Match { discriminant, arms } => {
                for arm in arms {
                    self.pattern(discriminant, &arm.pattern);
                }
            }
            ExprKind::Is { value, pattern } => self.pattern(value, pattern),
            _ => {}
        }
        if !self.accepted.contains(&e.id)
            && let Some(pos) = self.produced_by(e)
        {
            self.report(e.span, &pos, false);
        }
        for child in expr_children(e) {
            self.expr(child);
        }
    }

    /// A value going into a shadowed position.
    fn value_in(&mut self, value: &Expr, span: Span, pos: &Position<'_>) {
        let inner = peel(value);
        if let Some(out) = self.produced_by(inner)
            && out.types == pos.types
        {
            self.accepted.insert(inner.id);
            self.accepted.insert(value.id);
            return;
        }
        self.report(span, pos, true);
    }

    fn field_inits(&mut self, type_name: &str, fields: &[FieldInit]) {
        let Some((decl, unit)) = self.imported_type(type_name) else {
            return;
        };
        let TypeBody::Record(r) = &decl.body else {
            return;
        };
        for init in fields {
            let Some(field) = r.fields.iter().find(|f| f.name.name == init.name.name) else {
                continue;
            };
            let Some(pos) = self.position(
                format!("field `{type_name}.{}`", field.name.name),
                unit,
                &field.type_ref,
                &decl.type_params,
            ) else {
                continue;
            };
            match &init.value {
                Some(v) => self.value_in(v, v.span, &pos),
                // Shorthand: the value is a local binding of the same name.
                None => self.report(init.span, &pos, true),
            }
        }
    }

    /// A payload a top-level variant pattern binds, on a discriminant of an
    /// imported sum type.
    fn pattern(&mut self, discriminant: &Expr, pattern: &Pattern) {
        match pattern {
            Pattern::Or(alts, _) => {
                for p in alts {
                    self.pattern(discriminant, p);
                }
            }
            Pattern::Variant {
                variant, bindings, ..
            } => {
                let Some(name) = self.named_type_of(discriminant) else {
                    return;
                };
                let Some((decl, unit)) = self.imported_type(&name) else {
                    return;
                };
                let TypeBody::Sum(s) = &decl.body else {
                    return;
                };
                let Some(v) = s.variants.iter().find(|v| v.name.name == variant.name) else {
                    return;
                };
                for (i, b) in bindings.iter().enumerate() {
                    if b.is_wildcard() {
                        continue;
                    }
                    let field = match &b.kind {
                        PatternBindingKind::Positional { .. } => v.payload.get(i),
                        PatternBindingKind::Named { field, .. } => {
                            v.payload.iter().find(|p| p.name.name == field.name)
                        }
                    };
                    let Some(field) = field else {
                        continue;
                    };
                    if let Some(pos) = self.position(
                        format!("payload `{}` of `{name}.{}`", field.name.name, v.name.name),
                        unit,
                        &field.type_ref,
                        &decl.type_params,
                    ) {
                        self.report(b.span, &pos, false);
                    }
                }
            }
            _ => {}
        }
    }

    /// The shadowed position `e` reads a value out of, if any: a field of an
    /// imported record, or an imported call's result.
    fn produced_by(&self, e: &Expr) -> Option<Position<'s>> {
        match &e.kind {
            ExprKind::FieldAccess { receiver, field } => {
                let name = self.named_type_of(receiver)?;
                let (decl, unit) = self.imported_type(&name)?;
                let TypeBody::Record(r) = &decl.body else {
                    return None;
                };
                let f = r.fields.iter().find(|f| f.name.name == field.name)?;
                self.position(
                    format!("field `{name}.{}`", f.name.name),
                    unit,
                    &f.type_ref,
                    &decl.type_params,
                )
            }
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                let (f, unit, vars) = self.imported_fn(e)?;
                self.position(
                    format!("the result of `{}`", fn_display(f)),
                    unit,
                    &f.return_type,
                    &vars,
                )
            }
            // #2617: an imported constant, typed by its annotation in the
            // declaring unit's scope.
            ExprKind::Ident(id) => {
                let c = self.typed.consts.get(&id.name)?;
                let unit = self.scope.imported_from.get(&id.name)?;
                if unit == self.scope.unit {
                    return None;
                }
                self.position(format!("constant `{}`", id.name), unit, &c.ty, &[])
            }
            _ => None,
        }
    }

    /// Each argument of the call `e` paired with its parameter's shadowed
    /// position, if the callee is an imported declaration and the parameter
    /// is one.
    fn params_of(&self, e: &Expr, arity: usize) -> Vec<(usize, Option<Position<'s>>)> {
        if let Some((f, unit, vars)) = self.imported_fn(e) {
            // `params` holds no `self` (the parser keeps it as `has_self`),
            // so a method's arguments line up with them one to one (#1879
            // review).
            return f
                .params
                .iter()
                .take(arity)
                .enumerate()
                .map(|(i, p)| {
                    let what = format!("parameter `{}` of `{}`", p.name.name, fn_display(f));
                    (i, self.position(what, unit, &p.type_ref, &vars))
                })
                .collect();
        }
        if let Some(Callee::Ctor { sum, tag }) = self.typed.callee(e.id)
            && let Some((decl, unit)) = self.imported_type(&sum.name.name)
            && decl.span == sum.span
            && let TypeBody::Sum(s) = &decl.body
            && let Some(v) = s.variants.iter().find(|v| &v.name.name == tag)
        {
            return v
                .payload
                .iter()
                .take(arity)
                .enumerate()
                .map(|(i, p)| {
                    let what = format!(
                        "payload `{}` of `{}.{}`",
                        p.name.name, decl.name.name, v.name.name
                    );
                    (i, self.position(what, unit, &p.type_ref, &decl.type_params))
                })
                .collect();
        }
        Vec::new()
    }

    /// The imported fn, method or static the call `e` dispatched to, with its
    /// declaring unit and the type variables its signature binds.
    fn imported_fn(&self, e: &Expr) -> Option<(&'s FnDecl, &'s str, Vec<TypeParam>)> {
        let sc = self.scope;
        match self.typed.callee(e.id)? {
            Callee::Fn(f) => {
                let FnName::Free(n) = &f.name else {
                    return None;
                };
                // The bundle's own `render` declares `bynk.locale`'s signature
                // (1697's `render(tag, message("greeting"))`).
                if crate::symbols::is_synthetic_render(f) {
                    let decl = sc.combined_fns.get(&n.name)?;
                    return Some((decl.as_ref(), "bynk.locale", Vec::new()));
                }
                let unit = sc.imported_from.get(&n.name)?;
                let decl = sc.combined_fns.get(&n.name)?;
                (decl.span == f.span)
                    .then(|| (decl.as_ref(), unit.as_str(), decl.type_params.clone()))
            }
            Callee::Method(f) | Callee::Static(f) => {
                let FnName::Method {
                    type_name,
                    method_name,
                } = &f.name
                else {
                    return None;
                };
                let unit = sc.imported_from.get(&type_name.name)?;
                let table = sc.combined_methods.get(&type_name.name)?;
                let decl = table
                    .instance
                    .get(&method_name.name)
                    .into_iter()
                    .chain(table.statics.get(&method_name.name))
                    .find(|d| d.span == f.span)?;
                let mut vars = decl.type_params.clone();
                if let Some(t) = sc.combined_types.get(&type_name.name) {
                    vars.extend(t.type_params.iter().cloned());
                }
                Some((decl.as_ref(), unit.as_str(), vars))
            }
            _ => None,
        }
    }

    /// An imported type declaration by name, with its declaring unit. A local
    /// declaration of the name is not imported.
    fn imported_type(&self, name: &str) -> Option<(&'s TypeDecl, &'s str)> {
        let unit = self.scope.imported_from.get(name)?;
        let decl = self.scope.combined_types.get(name)?;
        Some((decl.as_ref(), unit.as_str()))
    }

    /// The name of the declared type `e` was checked as, if it is one.
    fn named_type_of(&self, e: &Expr) -> Option<String> {
        match &*self.typed.expr_ty(e.id)? {
            Ty::Named { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// `r` as a shadowed position, when it names a shadowed type in `unit`'s
    /// scope.
    fn position(
        &self,
        what: String,
        unit: &'s str,
        r: &TypeRef,
        vars: &[TypeParam],
    ) -> Option<Position<'s>> {
        let types = self.scope.shadowed.in_type_ref(unit, r, vars);
        (!types.is_empty()).then_some(Position {
            what,
            declared_in: unit,
            types,
        })
    }

    fn report(&mut self, span: Span, pos: &Position<'_>, into: bool) {
        let t = pos.types[0];
        let here = self.scope.unit;
        let verb = if into { "takes" } else { "gives" };
        let shadow = if t.bound == here {
            format!("`{here}` declares its own `{}`", t.name)
        } else {
            format!("`{}` here names `{}`'s declaration", t.name, t.bound)
        };
        self.errors.push(
            CompileError::new(
                "bynk.uses.name_conflict",
                span,
                format!(
                    "{what} from `{decl}` {verb} `{owner}`'s `{name}`, but `{name}` in `{here}` \
                     names `{bound}`'s type",
                    what = pos.what,
                    decl = pos.declared_in,
                    owner = t.owner,
                    name = t.name,
                    bound = t.bound,
                ),
            )
            .with_note(format!(
                "{shadow}, which shadows `{owner}`'s; a value that crosses here cannot be \
                 checked against both, so rename one of the two types",
                owner = t.owner,
            )),
        );
    }
}

/// An argument without its parentheses, which carry no value of their own.
fn peel(e: &Expr) -> &Expr {
    match &e.kind {
        ExprKind::Paren(inner) => peel(inner),
        _ => e,
    }
}

fn fn_display(f: &FnDecl) -> String {
    match &f.name {
        FnName::Free(n) => n.name.clone(),
        FnName::Method {
            type_name,
            method_name,
        } => format!("{}.{}", type_name.name, method_name.name),
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    const CORE: &str = "commons t.core\n\ntype Repo = String where NonEmpty\n\n\
                        type Tag = | Named(value: Repo) | Anon\n";
    const MODEL: &str = "commons t.model\n\nuses t.core\n\n\
                         type Run = { repo: Repo, n: Int }\n\n\
                         fn keep(r: Repo) -> Repo { r }\n\n\
                         fn first(r: Run) -> Repo { r.repo }\n\n\
                         fn count(r: Run) -> Int { r.n }\n\n\
                         fn Run.repoOf(self) -> Repo { self.repo }\n\n\
                         fn Run.withRepo(self, repo: Repo, k: Int) -> Run { Run { repo: repo, n: k } }\n\n\
                         fn Run.make(repo: Repo) -> Run { Run { repo: repo, n: 0 } }\n";

    /// The `bynk.uses.name_conflict` messages for a `t.app` that shadows
    /// `t.core`'s `Repo` and declares `body`. Any other error fails the test.
    fn conflicts(body: &str) -> Vec<String> {
        let app =
            format!("commons t.app\n\nuses t.core\nuses t.model\n\ntype Repo = Int\n\n{body}\n");
        let a = analyse(&[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            ("t/app.bynk", &app),
        ]);
        let other: Vec<_> = a
            .error_categories()
            .into_iter()
            .filter(|c| *c != "bynk.uses.name_conflict")
            .collect();
        assert!(other.is_empty(), "unexpected diagnostics:\n{}", a.render());
        a.analysis
            .errors
            .iter()
            .filter(|e| e.error.category == "bynk.uses.name_conflict")
            .map(|e| e.error.message.clone())
            .collect()
    }

    fn assert_one(body: &str, what: &str) {
        let got = conflicts(body);
        assert!(
            got.len() == 1 && got[0].starts_with(what),
            "expected one conflict at {what}, got {got:#?}"
        );
    }

    #[test]
    fn a_shadow_the_code_never_meets_is_legal() {
        assert_eq!(
            conflicts("fn size(r: Run) -> Int { r.n }\n\nfn mine() -> Repo { 7 }"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_value_moved_between_imported_positions_is_legal() {
        assert_eq!(
            conflicts(
                "fn retag(r: Run) -> Int { count(Run { repo: keep((r.repo)), n: 1 }) }\n\n\
                 fn again(r: Run) -> Run { Run { repo: first(r), n: 2 } }"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_local_value_into_a_record_field_conflicts() {
        assert_one(
            "fn make() -> Run { Run { repo: 42, n: 1 } }",
            "field `Run.repo` from `t.model` takes `t.core`'s `Repo`",
        );
    }

    #[test]
    fn a_shorthand_field_conflicts() {
        assert_one(
            "fn make(repo: Repo) -> Run { Run { repo, n: 1 } }",
            "field `Run.repo` from `t.model` takes",
        );
    }

    #[test]
    fn a_field_read_into_local_code_conflicts() {
        assert_one(
            "fn read(r: Run) -> Int { r.repo + 1 }",
            "field `Run.repo` from `t.model` gives `t.core`'s `Repo`",
        );
    }

    #[test]
    fn a_call_result_into_local_code_conflicts() {
        assert_one(
            "fn read(r: Run) -> Int { first(r) }",
            "the result of `first` from `t.model` gives",
        );
    }

    #[test]
    fn a_method_result_into_local_code_conflicts() {
        assert_one(
            "fn read(r: Run) -> Int { r.repoOf() }",
            "the result of `Run.repoOf` from `t.model` gives",
        );
    }

    #[test]
    fn a_local_argument_conflicts() {
        assert_one(
            "fn pass(x: Repo) -> Int { count(Run { repo: keep(x), n: 1 }) }",
            "parameter `r` of `keep` from `t.model` takes",
        );
    }

    #[test]
    fn a_value_through_a_let_conflicts() {
        // The rule reads the expression in the position, not where its value
        // came from: both ends of the `let` are crossings.
        let got = conflicts(
            "fn via(r: Run) -> Int {\n  let x = first(r)\n  count(Run { repo: x, n: 1 })\n}",
        );
        assert_eq!(got.len(), 2, "{got:#?}");
    }

    #[test]
    fn a_bound_payload_conflicts() {
        assert_one(
            "fn tag(t: Tag) -> Int {\n  match t {\n    Named(v) => 1,\n    Anon => 0,\n  }\n}",
            "payload `value` of `Tag.Named` from `t.core` gives",
        );
    }

    #[test]
    fn a_discarded_payload_is_legal() {
        assert_eq!(
            conflicts(
                "fn tag(t: Tag) -> Int {\n  match t {\n    Named(_) => 1,\n    Anon => 0,\n  }\n}"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_local_payload_conflicts() {
        assert_one(
            "fn tag() -> Tag { Named(5) }",
            "payload `value` of `Tag.Named` from `t.core` takes",
        );
    }

    /// #1879 review: a method's explicit parameters line up with its
    /// arguments (`params` holds no `self`), so the first is checked too.
    #[test]
    fn a_method_parameter_is_checked() {
        let got = conflicts("fn re(r: Run) -> Run { r.withRepo(42, 1) }");
        assert!(
            got.len() == 1 && got[0].contains("parameter `repo`"),
            "expected one conflict at `repo`, got {got:#?}"
        );
    }

    #[test]
    fn a_static_parameter_is_checked() {
        let got = conflicts("fn mk() -> Run { Run.make(42) }");
        assert!(
            got.len() == 1 && got[0].contains("parameter `repo`"),
            "expected one conflict at `repo`, got {got:#?}"
        );
    }

    #[test]
    fn a_handler_body_is_checked() {
        let a = analyse(&[
            ("t/core.bynk", CORE),
            ("t/model.bynk", MODEL),
            (
                "src/t/web.bynk",
                "context t.web\n\nuses t.core\nuses t.model\n\ntype Repo = Int\n\n\
                 service read {\n  on call(r: Run) -> Effect[Int] {\n    r.repo + 1\n  }\n}\n",
            ),
        ]);
        a.assert_reports("bynk.uses.name_conflict");
    }

    /// #1879 review: provider operations and agent handlers are walked too.
    #[test]
    fn provider_ops_and_agent_handlers_are_checked() {
        for (name, body) in [
            (
                "provider op",
                "capability Reader {\n  fn read(r: Run) -> Effect[Int]\n}\n\n\
                 provides Reader = Plain {\n  fn read(r: Run) -> Effect[Int] {\n    r.repo + 1\n  }\n}\n",
            ),
            (
                "agent handler",
                "agent Holder {\n  key id: String\n\n  store hits: Cell[Int]\n\n  \
                 on call read(r: Run) -> Effect[Int] {\n    r.repo + 1\n  }\n}\n",
            ),
        ] {
            let src =
                format!("context t.web\n\nuses t.core\nuses t.model\n\ntype Repo = Int\n\n{body}");
            let a = analyse(&[
                ("t/core.bynk", CORE),
                ("t/model.bynk", MODEL),
                ("src/t/web.bynk", &src),
            ]);
            assert!(
                a.error_categories().contains(&"bynk.uses.name_conflict"),
                "{name}: expected a conflict:\n{}",
                a.render()
            );
        }
    }
}
