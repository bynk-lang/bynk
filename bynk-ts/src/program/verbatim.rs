#[cfg(doc)]
use super::TsType;
use super::{TsArrowBody, TsDecl, TsExpr, TsObjectEntry, TsProgram, TsStmt, TsStmtKind};

impl TsProgram {
    /// Every `TsStmtKind::Verbatim`/[`TsExpr::VerbatimExpr`] leaf's own
    /// text, tagged by its [`VerbatimOrigin`] — found by walking every
    /// statement/expression container in the tree, not just the top level.
    /// #1538's own real gap: nothing before this walked a whole `TsProgram`
    /// to find every opaque leaf nested inside real structured content (a
    /// `Verbatim` inside an `if` branch, a `VerbatimExpr` inside a `Call`'s
    /// arguments, …) — the only prior way to inspect a `Verbatim`'s content
    /// was scanning the *whole printed document* text
    /// ([`crate::verbatim_violations`] doesn't distinguish opaque text from
    /// a real node's own printed output), which also flags real, structured
    /// nodes that happen to print the substring `any` (`TsType::named("any")`
    /// residue, tracked separately by `xtask`'s own `ts_any` probe) — a
    /// false positive for *this* check, whose whole point is the opaque
    /// escape hatch specifically. `TsStmtKind` is `pub(crate)`, so only code
    /// inside this crate can walk it directly; this method is the public
    /// surface a caller outside `bynk-ts` (`bynk`'s own
    /// `tests/tsc_verify.rs`) uses instead.
    ///
    /// Every container is walked exhaustively (`match`, no wildcard arm) so
    /// a future `TsStmt`/`TsExpr` variant fails to compile here until this
    /// walker accounts for it — the same "compile-time construct, not a
    /// grep" discipline [`VerbatimOrigin`]'s own doc already states for
    /// itself. `TsStmtKind::Raw` is deliberately not walked into any
    /// deeper than skipped: it carries no `VerbatimOrigin` at all (ADR
    /// `arc-c-lower-rs-permanent-exclusion`'s own permanent exclusion, a
    /// different bucket from `Verbatim`'s temporary conversion residue), so
    /// it has nothing this method could report. [`TsType`] positions are
    /// never walked — no `TsType` variant can hold a `Verbatim`/
    /// `VerbatimExpr` leaf, so there is nothing to find there.
    ///
    /// **A real gap this walker exposed (#1538):** no `VerbatimOrigin::Emit`
    /// leaf can currently reach a `TsProgram` returned from `bynk-emit` at
    /// all. Every real `bynk-emit/src/emitter/emit.rs` construction site
    /// builds a [`TsExpr::VerbatimExpr`], immediately prints it
    /// (`crate::print_expr`/`crate::print_stmt`), and splices the resulting
    /// text into a plain `String` a caller further up wraps in
    /// `TsStmtKind::Raw` — not tagged with a `VerbatimOrigin` at all, and
    /// so invisible to this walker by design (see `TsStmtKind::Raw`'s own
    /// doc). `VerbatimExpr`'s own doc promises this content is "visible to
    /// `crate::verbatim_violations`"; today that's only true at the instant
    /// of construction, not once the surrounding document is returned —
    /// closing it needs a real Arc C conversion slice, not a change here.
    pub fn verbatim_content(&self) -> Vec<(VerbatimOrigin, &str)> {
        let mut out = Vec::new();
        for stmt in &self.stmts {
            walk_stmt(stmt, &mut out);
        }
        out
    }
}

fn walk_stmt<'a>(stmt: &'a TsStmt, out: &mut Vec<(VerbatimOrigin, &'a str)>) {
    match &stmt.kind {
        TsStmtKind::Verbatim { origin, text } => out.push((*origin, text.as_str())),
        TsStmtKind::Decl(decl) => walk_decl(decl, out),
        TsStmtKind::Const { init, .. } => walk_expr(init, out),
        TsStmtKind::Let { init, .. } => {
            if let Some(e) = init {
                walk_expr(e, out);
            }
        }
        TsStmtKind::ExprStmt(e) => walk_expr(e, out),
        TsStmtKind::Return(e) => {
            if let Some(e) = e {
                walk_expr(e, out);
            }
        }
        TsStmtKind::Throw(e) => walk_expr(e, out),
        TsStmtKind::If {
            cond,
            then_branch,
            else_branch,
            same_line_else: _,
        } => {
            walk_expr(cond, out);
            walk_stmt(then_branch, out);
            if let Some(e) = else_branch {
                walk_stmt(e, out);
            }
        }
        TsStmtKind::ForOf {
            binding: _,
            iter,
            body,
            is_await: _,
        } => {
            walk_expr(iter, out);
            walk_stmt(body, out);
        }
        TsStmtKind::While { cond, body } => {
            walk_expr(cond, out);
            walk_stmt(body, out);
        }
        TsStmtKind::Labelled { label: _, body } => walk_stmt(body, out),
        TsStmtKind::LetList(decls) => {
            for d in decls {
                if let Some(e) = &d.init {
                    walk_expr(e, out);
                }
            }
        }
        TsStmtKind::For {
            name: _,
            init,
            test,
            body,
        } => {
            walk_expr(init, out);
            walk_expr(test, out);
            walk_stmt(body, out);
        }
        TsStmtKind::TryCatch {
            try_block,
            catch_param: _,
            catch_block,
            finally_block,
        } => {
            walk_stmt(try_block, out);
            if let Some(c) = catch_block {
                walk_stmt(c, out);
            }
            if let Some(f) = finally_block {
                walk_stmt(f, out);
            }
        }
        TsStmtKind::Block(stmts) | TsStmtKind::InlineBlock(stmts) => {
            for s in stmts {
                walk_stmt(s, out);
            }
        }
        TsStmtKind::Continue(_) | TsStmtKind::Break(_) => {}
        TsStmtKind::Assign { target, value } => {
            walk_expr(target, out);
            walk_expr(value, out);
        }
        TsStmtKind::Comment(_) | TsStmtKind::DocComment(_) | TsStmtKind::Blank => {}
        TsStmtKind::Switch {
            discriminant,
            cases,
        } => {
            walk_expr(discriminant, out);
            for case in cases {
                for t in &case.labels {
                    walk_expr(t, out);
                }
                for s in &case.body {
                    walk_stmt(s, out);
                }
            }
        }
        TsStmtKind::Increment(e) => walk_expr(e, out),
        TsStmtKind::Raw(..) => {}
    }
}

fn walk_decl<'a>(decl: &'a TsDecl, out: &mut Vec<(VerbatimOrigin, &'a str)>) {
    match decl {
        TsDecl::Import { .. }
        | TsDecl::ImportNamespace { .. }
        | TsDecl::ImportDefault { .. }
        | TsDecl::ReExport { .. }
        | TsDecl::ExportNames { .. }
        | TsDecl::ReExportAll { .. }
        | TsDecl::Interface { .. }
        | TsDecl::TypeAlias { .. }
        | TsDecl::DeclareConst { .. } => {}
        TsDecl::Export(inner) => walk_decl(inner, out),
        TsDecl::ConstDecl { init, .. } => walk_expr(init, out),
        TsDecl::Class {
            constructor,
            methods,
            ..
        } => {
            if let Some(ctor) = constructor {
                for s in &ctor.body {
                    walk_stmt(s, out);
                }
            }
            for m in methods {
                for s in &m.body {
                    walk_stmt(s, out);
                }
            }
        }
        TsDecl::Function { body, .. } => {
            for s in body {
                walk_stmt(s, out);
            }
        }
        TsDecl::ExportDefault(e) => walk_expr(e, out),
    }
}

fn walk_arrow_body<'a>(body: &'a TsArrowBody, out: &mut Vec<(VerbatimOrigin, &'a str)>) {
    match body {
        TsArrowBody::Expr(e) => walk_expr(e, out),
        TsArrowBody::Block(stmts) | TsArrowBody::Multiline(stmts) | TsArrowBody::Flat(stmts) => {
            for s in stmts {
                walk_stmt(s, out);
            }
        }
    }
}

fn walk_object_entry<'a>(entry: &'a TsObjectEntry, out: &mut Vec<(VerbatimOrigin, &'a str)>) {
    match entry {
        TsObjectEntry::Prop { value, .. } => walk_expr(value, out),
        TsObjectEntry::Shorthand(_) => {}
        TsObjectEntry::Method { body, .. } => {
            for s in body {
                walk_stmt(s, out);
            }
        }
        TsObjectEntry::Spread(e) => walk_expr(e, out),
    }
}

fn walk_expr<'a>(expr: &'a TsExpr, out: &mut Vec<(VerbatimOrigin, &'a str)>) {
    match expr {
        TsExpr::Ident(_) | TsExpr::RawUsing(..) => {}
        TsExpr::VerbatimExpr(text, origin) => out.push((*origin, text.as_str())),
        TsExpr::Member { object, .. } | TsExpr::OptionalMember { object, .. } => {
            walk_expr(object, out);
        }
        TsExpr::TypeApp { callee, .. } => walk_expr(callee, out),
        TsExpr::Index { object, index } | TsExpr::OptionalIndex { object, index } => {
            walk_expr(object, out);
            walk_expr(index, out);
        }
        TsExpr::Arrow { body, .. } => walk_arrow_body(body, out),
        TsExpr::DefaultIife { default, body, .. } => {
            walk_expr(default, out);
            for s in body {
                walk_stmt(s, out);
            }
        }
        TsExpr::Call { callee, args } | TsExpr::New { callee, args } => {
            walk_expr(callee, out);
            for a in args {
                walk_expr(a, out);
            }
        }
        TsExpr::Object { entries, .. } => {
            for e in entries {
                walk_object_entry(e, out);
            }
        }
        TsExpr::Array { items, .. } | TsExpr::LinesArray(items) => {
            for i in items {
                walk_expr(i, out);
            }
        }
        TsExpr::TemplateLit { parts: _, exprs } => {
            for e in exprs {
                walk_expr(e, out);
            }
        }
        TsExpr::Await(e) => walk_expr(e, out),
        TsExpr::As { expr, ty: _ } => walk_expr(expr, out),
        TsExpr::AsConst(expr) => walk_expr(expr, out),
        TsExpr::Unary { op: _, expr } => walk_expr(expr, out),
        TsExpr::Binary { op: _, left, right } => {
            walk_expr(left, out);
            walk_expr(right, out);
        }
        TsExpr::Conditional {
            test,
            consequent,
            alternate,
        }
        | TsExpr::MultilineConditional {
            test,
            consequent,
            alternate,
        } => {
            walk_expr(test, out);
            walk_expr(consequent, out);
            walk_expr(alternate, out);
        }
        TsExpr::Paren(e) | TsExpr::Spread(e) | TsExpr::NonNull(e) | TsExpr::Yield(e) => {
            walk_expr(e, out)
        }
        TsExpr::Assign { target, value }
        | TsExpr::AddAssign { target, value }
        | TsExpr::NullishAssign { target, value } => {
            walk_expr(target, out);
            walk_expr(value, out);
        }
        TsExpr::Sequence(items) => {
            for i in items {
                walk_expr(i, out);
            }
        }
        TsExpr::OptionalCall { callee, args } => {
            walk_expr(callee, out);
            for a in args {
                walk_expr(a, out);
            }
        }
        TsExpr::Function { body, .. } => {
            for s in body {
                walk_stmt(s, out);
            }
        }
        TsExpr::Lit(_) => {}
    }
}

/// Which family of residual, not-yet-converted emission a [`TsStmt::verbatim`]
/// statement came from. A closed enum, deliberately — "makes the ratchet a
/// compile-time construct, not a grep" (Q2's own settling text). Named
/// file-by-file as Arc C actually needs them (`ast_importers`'s own five-file
/// floor is the precedent for how this track names residue), not
/// pre-populated for the whole ~19-slice Arc C schedule up front.
///
/// Deliberately **not** `#[non_exhaustive]`: a `match` over every variant —
/// in this crate, or in `bynk-emit` once Arc C reads it — must fail to
/// compile the moment a new variant is added, forcing every consumer to
/// account for it explicitly. A non-exhaustive enum would let a wildcard arm
/// silently absorb a new residue family instead, exactly the "grep, not a
/// compile-time construct" Q2's own settling text rejected. P7.8's own
/// grounding work found that `Contracts`/`Secrets`/`RuntimeUse` were seeded
/// against files that turn out not to need `bynk-ts` conversion at all
/// (`design/tracks/the-typescript-tree.md` §9) — recorded there, not fixed
/// by removing the variants here, since that's separate follow-on work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbatimOrigin {
    /// `bynk-emit/src/emitter/contracts.rs`.
    Contracts,
    /// `bynk-emit/src/emitter/secrets.rs`.
    Secrets,
    /// `bynk-emit/src/emitter/runtime_use.rs`.
    RuntimeUse,
    /// P7.6's own transitional wrap (#1309): the whole of a still-`String`-
    /// producing `bynk-emit` document (an entry point, `compose.ts`, the
    /// runtime module, an adapter binding, a test module, …), carried into
    /// `Document::Ts` so `Artefacts` never stores a bare `String` for TS
    /// output (R7.8) even before Arc C converts the function that built it.
    /// Deliberately **not** file-specific like the three variants above —
    /// this is Arc B's own infrastructure slice, not an Arc C conversion, so
    /// it covers everything Arc C hasn't reached yet, one call site per
    /// document `bynk-emit/src/project.rs`/`project/tests_emit.rs`
    /// constructs (not funnelled through a shared helper: a shared wrap
    /// point would collapse every one of those call sites to a single
    /// textual `TsStmt::verbatim(` occurrence, defeating `verbatim_sites`'
    /// own purpose of counting how much is genuinely still unconverted).
    /// Retires site by site as Arc C converts each underlying emitter
    /// function to return a real `TsProgram` directly — at which point that
    /// document's own construction site stops calling `TsStmt::verbatim` at
    /// all, not by this variant being deleted first.
    NotYetConverted,
    /// `bynk-emit/src/emitter/emit.rs`'s own residual [`TsExpr::VerbatimExpr`]
    /// leaves (#1539) — a generic-typed callee, a printer-unsafe nested `As`
    /// chain, a block-bodied ICU IIFE, and a pre-escaped/unescaped predicate
    /// message, none reducible to a real node without expanding this crate's
    /// own type algebra past what #1539 asks for. Expression-level, not
    /// statement-level like the three file-specific variants above (this is
    /// the first `VerbatimOrigin` a `TsExpr` carries, not a `TsStmt`), but
    /// named the same file-by-file way.
    Emit,
    /// The `__formDecode` / `__formString` field-spec table (1A.35a part C, #3736 Decision E): the
    /// array literal of `{ name, kind, list, option }` records the router's `@form` path reads a
    /// body by, which `workers_entry::field_spec_source` returns as TypeScript *source text*. The
    /// text is foreign (it is the router's own spelling, shared with the entry point), so a
    /// `Form.decode[T]` or `Form.encode` call carries it as one expression leaf rather than
    /// re-spelling it as nodes. Built at the one site `form_field_specs`
    /// (`bynk-emit/src/emitter/lower_call/codec_statics.rs`).
    FormFieldSpecs,
}
