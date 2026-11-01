//! [`TsProgram`]/[`TsStmt`] — the tree. P7.5 built it wide enough only for
//! the `Verbatim` escape hatch (Q2, `design/tracks/the-typescript-tree.md`
//! §3.2). P7.8 (#1313) adds the real node algebra ([`TsExpr`]/[`TsType`]/
//! [`TsDecl`], plus real [`TsStmt`] variants) — not the full §7.1 reference
//! sketch as literally written (a variant-name list with almost no
//! field-level design), but the subset `bynk-emit/src/emitter/
//! events_fanout.rs` (Arc C's real next file — P7.8's own proposal
//! corrected the track doc's stale schedule, see `design/tracks/
//! the-typescript-tree.md` §6/§9) concretely needs, grounded against that
//! file's own real shape rather than guessed. Building the rest of the
//! sketch's unvalidated variants now would repeat the exact "guessing, not
//! designing" risk `bynk-ts`'s own module doc (`lib.rs`) already named for
//! this layer — Arc C's later slices add more variants file by file, the
//! same precedent [`VerbatimOrigin`] already set.

use crate::source_map::SourceMapBuilder;
use bynk_syntax::span::Span;

/// A whole generated TypeScript module, as an ordered sequence of top-level
/// statements. `Vec<TsStmt>`, plain — no richer container yet (P7.6's own
/// `Artefacts { docs: BTreeMap<PathBuf, Document> }` is where a *project's*
/// documents get keyed; this is one document's own tree).
#[derive(Debug, Default)]
pub struct TsProgram {
    pub stmts: Vec<TsStmt>,
}

impl TsProgram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, stmt: TsStmt) {
        self.stmts.push(stmt);
    }

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
    /// surface a caller outside `bynk-ts` (`bynkc`'s own
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

    /// The module specifier of every top-level import declaration (named,
    /// namespace or default), in statement order.
    pub fn import_specifiers(&self) -> Vec<&str> {
        self.stmts
            .iter()
            .filter_map(|s| match &s.kind {
                TsStmtKind::Decl(
                    TsDecl::Import { from, .. }
                    | TsDecl::ImportNamespace { from, .. }
                    | TsDecl::ImportDefault { from, .. },
                ) => Some(from.as_str()),
                _ => None,
            })
            .collect()
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
        } => {
            walk_expr(iter, out);
            walk_stmt(body, out);
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
        } => {
            walk_stmt(try_block, out);
            walk_stmt(catch_block, out);
        }
        TsStmtKind::Block(stmts) | TsStmtKind::InlineBlock(stmts) => {
            for s in stmts {
                walk_stmt(s, out);
            }
        }
        TsStmtKind::Continue => {}
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
                if let Some(t) = &case.test {
                    walk_expr(t, out);
                }
                for s in &case.body {
                    walk_stmt(s, out);
                }
            }
        }
        TsStmtKind::Increment(e) => walk_expr(e, out),
        TsStmtKind::Raw(_) => {}
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
            name: _,
            fields: _,
            constructor,
            methods,
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
        TsArrowBody::Block(stmts) => {
            for s in stmts {
                walk_stmt(s, out);
            }
        }
    }
}

fn walk_object_entry<'a>(entry: &'a TsObjectEntry, out: &mut Vec<(VerbatimOrigin, &'a str)>) {
    match entry {
        TsObjectEntry::Prop(_, v) => walk_expr(v, out),
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
        TsExpr::Ident(_) => {}
        TsExpr::VerbatimExpr(text, origin) => out.push((*origin, text.as_str())),
        TsExpr::Member { object, .. } | TsExpr::OptionalMember { object, .. } => {
            walk_expr(object, out);
        }
        TsExpr::Index { object, index } | TsExpr::OptionalIndex { object, index } => {
            walk_expr(object, out);
            walk_expr(index, out);
        }
        TsExpr::Arrow { body, .. } => walk_arrow_body(body, out),
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
        TsExpr::Array { items, .. } => {
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
        TsExpr::Unary { op: _, expr } => walk_expr(expr, out),
        TsExpr::Binary { op: _, left, right } => {
            walk_expr(left, out);
            walk_expr(right, out);
        }
        TsExpr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            walk_expr(test, out);
            walk_expr(consequent, out);
            walk_expr(alternate, out);
        }
        TsExpr::Paren(e) => walk_expr(e, out),
        TsExpr::Lit(_) => {}
    }
}

/// One name a statement exports ([`TsStmt::exported_names`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportedName<'a> {
    /// The exported name (an `X as Y` entry's `Y`).
    pub name: &'a str,
    /// Whether the statement exports a value under the name, as a `const`,
    /// `function` or `class` does, or only a type, as an `interface`, a
    /// `type` alias or a `type X` list entry does.
    pub has_value: bool,
}

/// One statement — a `Verbatim`-tagged escape hatch (still constructible
/// only via [`TsStmt::verbatim`], per #1307's Decision D — the
/// `verbatim_sites` probe needs exactly one string to line-scan for), or,
/// from this slice, a real structured kind. The real kinds have no such
/// sealing: they're normal typed constructors, not a "wrap opaque text"
/// escape hatch, so the `verbatim_sites` concern that motivates `verbatim`'s
/// own single-constructor discipline doesn't apply to them.
#[derive(Debug, Clone)]
pub struct TsStmt {
    pub(crate) kind: TsStmtKind,
    /// Where this statement's content originated in the `.bynk` source, if
    /// known. Only a *top-level* statement's own span is currently recorded
    /// as a source-map checkpoint ([`crate::printer::print`], unchanged
    /// from P7.5/R7.4's own scope) — a nested statement (inside a `Block`,
    /// `If`, `ForOf`, `TryCatch`) still carries this field structurally, for
    /// whichever future slice gives sub-statement source maps real value,
    /// but the printer does not yet record a checkpoint from it. Named here
    /// explicitly (P7.8's own accepted proposal: "an implementation-time
    /// call within this same shape") rather than left ambiguous.
    pub span: Option<Span>,
    /// #1477's own real gap: a body-bearing statement — in practice always
    /// a `TsStmtKind::Raw`/`TsStmtKind::Verbatim` opaque blob standing in
    /// for a lowered function/method body (ADR 0391's own permanent
    /// exclusion) — carries its *own* per-statement source-map checkpoints,
    /// collected by the caller's own body-local `SourceMapBuilder` before
    /// this node existed. Before this field, every real `bynk-emit` caller
    /// that needed to merge those checkpoints into its own module map had to
    /// reverse-engineer this node's own print-time byte offset from the
    /// *outside* — `bynk-emit`'s own `emit_class_method_and_merge_source_map`
    /// (`emitter/emit.rs`) recovers it by subtracting known lengths and
    /// string-matching the printed text's own tail, degrading to a silent
    /// skip if that search fails; still live for `emit_service`/`emit_agent`'s
    /// own not-yet-converted call sites (#1481/#1482). `emit_free_fn` used to
    /// recover it by separate, independent exact arithmetic, guarded by a
    /// `debug_assert!` — #1480 converted it to set this field directly
    /// instead, the first real `bynk-emit` caller to do so. Both existed
    /// only because nothing reported *this* node's own real print-time
    /// offset directly. Setting this field lets the printer itself do the
    /// merge, at the exact offset it is about to write this node's text to
    /// — no reverse-engineering, no silent-skip fallback (see this crate's
    /// own private `printer::render_block_stmts` for the handling). `None`
    /// for every real site that predates this field and every node whose own
    /// text carries no nested checkpoints of its own — the overwhelmingly
    /// common case, and the reason this is an `Option`, not a required
    /// field.
    pub nested_map: Option<SourceMapBuilder>,
    /// #1486's own second real gap, found converting `emit_test_module`/
    /// `emit_integration_module`: [`crate::printer::print`]'s own top-level
    /// loop always merges a statement's [`nested_map`](Self::nested_map)
    /// against parent source id `0` (`crate::printer::MergeTarget`'s own
    /// review-of-#1488 fix threads a `source_id` through every *manual*
    /// merge entry point, but `print`'s own automatic top-level case never
    /// had a per-statement way to say "not 0") — correct for every
    /// single-source module (`emit_project`, every other real caller so
    /// far), but wrong for a test/integration module's own aggregate map,
    /// which registers one source per fragment file a case's body came from
    /// (`SourceMapBuilder::add_source`) and needs each such case's own
    /// top-level statement merged against *that* source's id, not the
    /// module's primary one. `0` (every site predating this field, and
    /// every single-source module after it) keeps the automatic policy
    /// exactly as it already was; a case/property/attack wrapper statement
    /// sets this to its own registered source id before `print` sees it.
    pub nested_map_source_id: usize,
    /// #1486's own real gap, found converting `emit_project`/`emit_test_
    /// module`/`emit_integration_module` off a single opaque top-level
    /// `Verbatim` wrap: [`crate::printer::print`]'s own top-level loop
    /// inserts exactly one blank line between every pair of adjacent
    /// top-level statements (its own "readability policy" doc, right below
    /// this field's own use site) — correct for content that was always
    /// designed against that policy (`emit_composition_root`/`emit_test_
    /// main`/`workers.rs`/`workers_entry.rs`, every one of them zero-diff
    /// from day one), but `emit_agent`'s own already-shipped `Vec<TsStmt>`
    /// (#1482, predating this field) was built against the *previous*
    /// regime — where every real caller printed via `bynk-emit`'s own
    /// `extend_printed`/`extend_printed_and_merged` (a flat per-stmt loop
    /// with no automatic spacing of its own) — and genuinely needs a
    /// same-registry-then-zero-factory pairing with **no** blank line
    /// between them, matching the pre-conversion hand-written text this
    /// crate's own zero-diff discipline requires preserving exactly.
    /// `true` on a statement suppresses the automatic blank line
    /// [`crate::printer::print`] would otherwise insert immediately before
    /// it — the narrow, general escape hatch this real, already-shipped
    /// content needs, not a special case hard-coded into the printer for
    /// one specific pair of `TsDecl` variants. `false` (every site
    /// predating this field, and the overwhelming majority after it) keeps
    /// the automatic policy exactly as it already is.
    pub no_blank_before: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum TsStmtKind {
    Verbatim {
        // Read by `TsProgram::verbatim_content` (#1538) to attribute each
        // opaque leaf's text to its origin file.
        origin: VerbatimOrigin,
        text: String,
    },
    /// A top-level declaration ([`TsDecl`]) printed as a statement — the
    /// bridge between `TsProgram`'s flat `Vec<TsStmt>` and the reference
    /// sketch's separate `TsDecl` enum (an `import`/`interface`/top-level
    /// `const`/`class` *is* one kind of top-level statement in this tree,
    /// not a different container).
    Decl(TsDecl),
    /// A local `const` binding, e.g. `const { events } = ...;` or
    /// `const subs = ...;` — distinct from [`TsDecl::ConstDecl`], which is
    /// the top-level form. Carries a real destructuring [`TsBindingName`]
    /// because `events_fanout.rs`'s own `const { events } = ...` needs one
    /// (a gap beyond the accepted proposal's own variant list — a bare
    /// `String` name cannot represent it; named explicitly as a deviation,
    /// not invented silently).
    Const {
        name: TsBindingName,
        ty: Option<TsType>,
        init: TsExpr,
    },
    /// `let`'s sibling to `Const` — unused by `events_fanout.rs` itself, but
    /// the `const`/`let` distinction is real TypeScript semantics the
    /// printer must preserve once one of the pair exists (the accepted
    /// proposal's own reasoning for keeping it).
    Let {
        name: TsBindingName,
        ty: Option<TsType>,
        init: Option<TsExpr>,
        /// #2617: printed `var` rather than `let`. A module-level `var` is
        /// hoisted and reads as `undefined` before its statement runs, where
        /// a `let` would throw (its temporal dead zone), so a hoisted
        /// function may read it while another module of an import cycle is
        /// still evaluating. Set only by [`TsStmt::var_stmt`].
        hoisted: bool,
    },
    /// An expression used as a whole statement (a bare call, e.g.).
    ExprStmt(TsExpr),
    Return(Option<TsExpr>),
    /// `throw <expr>;` — #1353's own real gap: `emit_contract_guarded_body`'s
    /// own precondition/postcondition guards (`bynk-emit/src/emitter/
    /// emit.rs`) each throw a constructed `Error` on violation — no prior
    /// slice needed a bare `throw` (every other error-signalling site in
    /// this tree so far has been a `Return`). Mirrors `Return`'s own shape
    /// exactly, minus the `Option` (a `throw` always carries a value; there
    /// is no bare `throw;` in JS/TS).
    Throw(TsExpr),
    /// `if (cond) <then_branch>` (optionally `else <else_branch>`).
    /// `then_branch`/`else_branch` may each be a [`TsStmtKind::Block`]
    /// (printed with braces) or any other single statement (printed inline
    /// on the same line, matching `if (!Array.isArray(subs)) continue;`'s
    /// own real, brace-free shape). `else_branch` is #1323's own real gap:
    /// `workers_entry.rs`'s queue-consumer ack/retry dispatch (`if
    /// (result.tag === "Ack") msg.ack(); else { ...; msg.retry(); }`) —
    /// `events_fanout.rs`'s own grounding never used one, so P7.8 named
    /// omitting it a deliberate choice, not an oversight; this slice's own
    /// real content needs it for real.
    ///
    /// `same_line_else` is #1325's own real gap: `workers_entry.rs`'s own
    /// real `else` sits on its own fresh line (`}\nelse {`), but
    /// `emit_test_main`'s own real content wants the conventional `} else
    /// {` — two already-real files disagreeing on the same construct, the
    /// same class of tension the `Await`-under-`As` correction (#1323/
    /// #1324) found for parenthesisation. `false` (fresh line) is the
    /// existing, already-tested default via [`TsStmt::if_else_stmt`]; `true`
    /// (same line) is reached only through
    /// [`TsStmt::if_else_same_line_stmt`].
    If {
        cond: TsExpr,
        then_branch: Box<TsStmt>,
        else_branch: Option<Box<TsStmt>>,
        same_line_else: bool,
    },
    /// `for (const <binding> of <iter>) <body>`.
    ForOf {
        binding: String,
        iter: TsExpr,
        body: Box<TsStmt>,
    },
    /// `for (let <name> = <init>; <test>; <name>++) <body>` — a C-style
    /// indexed loop, `ForOf`'s counterpart for index-based iteration (as
    /// opposed to `for...of`'s element-based iteration over an existing
    /// collection). Arc E slice 7 (#1447)'s own real, narrow gap:
    /// `bynk-emit`'s `serialisation.rs` has two real sites — `ListInst`'s
    /// and `MapInst`'s own deserialise-side element loops, both `for (let i
    /// = 0; i < json.length; i++) { ... }` — that walk a JSON array by
    /// index, a shape `ForOf` cannot represent (its own `binding` names a
    /// per-element destructuring target bound fresh each iteration, not a
    /// counter carrying its own init/test/update clauses across
    /// iterations). Deliberately grounded in only what those two real call
    /// sites need, not the general C-style-for grammar — the same "extend
    /// narrowly" posture this file's own `TsBinaryOp::In`/`LessThan`
    /// additions already took:
    ///   - `name`/`init` are always a plain `let <name> = <init>;`
    ///     declaration — no destructuring, no multi-declarator list, no
    ///     explicit type annotation (neither real site needs one).
    ///   - The update clause always renders as the postfix increment
    ///     `<name>++`, over `name` itself rather than a separate field —
    ///     review of #1448 found an independent `update: TsExpr` field
    ///     structurally redundant with `name` (both real construction sites
    ///     could only ever pass `ident(name)`) with a genuinely dangerous
    ///     failure mode if the two ever disagreed: unlike a wrong `test`
    ///     (which `tsc --strict` or a non-running loop tends to surface), a
    ///     mismatched update clause (`for (let i = 0; i < json.length;
    ///     j++)`) compiles cleanly whenever `j` is merely in scope and hangs
    ///     the generated worker at runtime, with nothing in the type, the
    ///     printer, or a test to catch it. Removing the field removes the
    ///     failure mode entirely rather than merely asserting against it —
    ///     the same "postfix increment has no expression-position
    ///     representation, only a dedicated statement/clause shape"
    ///     restriction `Increment`'s own doc above already states for a
    ///     bare `<expr>++;` statement (#1325), applied here too.
    ///   - `body` is the loop body; both real sites pass a `Block`, though
    ///     the same brace-vs-inline rendering `If`/`ForOf` already share
    ///     applies to any other shape too.
    For {
        name: String,
        init: TsExpr,
        test: TsExpr,
        body: Box<TsStmt>,
    },
    /// `try <try_block> catch (<catch_param>) <catch_block>` — a real gap
    /// beyond the reference sketch (`design/bynk-greenfield-compiler.md`'s
    /// §7.1 has no `TryCatch` at all), found and named by P7.8's own
    /// accepted proposal: `events_fanout.rs`'s subscriber-failure-isolation
    /// `try`/`catch` (ADR 0284) is load-bearing control flow, not
    /// decorative. `catch_param: None` prints the bare `catch { ... }`
    /// form (ES2019's optional catch binding) — #1321's own real gap:
    /// `workers.rs`'s `emit_http_sum_wrapper` reads the raw request body in
    /// a `try`/`catch` that never uses the caught error (`} catch {`, no
    /// `(e)` at all).
    TryCatch {
        try_block: Box<TsStmt>,
        catch_param: Option<String>,
        catch_block: Box<TsStmt>,
    },
    /// `{ <stmts> }` — the body container for `If`/`ForOf`/`TryCatch`/
    /// constructor and method bodies.
    Block(Vec<TsStmt>),
    /// A bare `continue;` — a second real gap beyond the accepted
    /// proposal's own variant list: `events_fanout.rs` uses it twice
    /// (`if (!Array.isArray(subs)) continue;` / `if (!binding) continue;`),
    /// load-bearing loop control the accepted proposal's `TsStmt` list
    /// doesn't name. No label — nothing in the grounding file needs one.
    Continue,
    /// `target = value;` — a third real gap, found in review of the
    /// implementing PR (#1313): `events_fanout.rs`'s own constructor body is
    /// exactly one statement, `this.env = (env ?? {}) as
    /// Record<string, ServiceBinding>;` (a field assignment, not a `const`/
    /// `let` binding), which the accepted proposal's own grounding
    /// catalogue missed — it catalogued the `fetch` method's body in detail
    /// but not the constructor's. `target` is deliberately a full `TsExpr`
    /// (not a narrower "assignable" type) so it can hold `this.env` (a
    /// `Member` expression) without a second binding-target type; nothing
    /// about `Assign` validates that `target` is actually assignable
    /// (`bynk-check` already does that on the `.bynk` side before emission
    /// ever runs).
    Assign {
        target: TsExpr,
        value: TsExpr,
    },
    /// `// <text>` — a bare line comment. Added for Arc C's own first real
    /// conversion slice (#1317): every `bynk-emit`-generated `.ts` file
    /// opens with the same two-line header banner
    /// (`// Generated by bynkc — do not edit by hand.` / a file-specific
    /// second line), and nothing before this slice could represent one.
    /// Not speculative — a universal need every later Arc C slice hits on
    /// its own first line, closed once here rather than left as a residual
    /// `Verbatim` wrap that would make `verbatim_sites` show zero
    /// improvement for real, substantial conversion work.
    ///
    /// Carries no semantic content the printer or checker ever reads back —
    /// pure, inert text. Printed one `// `-prefixed line per `\n` in `text`
    /// (`bynk-ts/src/printer.rs`'s own `render_stmt`), so a multi-line
    /// comment is representable as one statement, though
    /// `events_fanout.rs`'s own two-line header is built as two separate
    /// `Comment` statements instead — matching how its two adjacent
    /// `import` lines are two separate `TsDecl::Import` statements, not one
    /// with an embedded line break, and exercising the printer's own new
    /// "no blank line between adjacent `Comment`s" rule (the same
    /// exception already established for adjacent `import`s).
    Comment(String),
    /// `/** ... */` — a JSDoc block comment, distinct from
    /// [`TsStmtKind::Comment`]'s own `//`-per-line form. #1333's own real
    /// need: `emit_doc_block` (`bynk-emit/src/emitter/emit.rs`) renders a
    /// Bynk `///` doc comment as a real JSDoc block — printed via
    /// [`crate::printer::print_stmt`], not [`crate::printer::print`], since
    /// every real call site today is a shared helper spliced into a
    /// still-unconverted caller's own buffer, the same P7.9 pattern
    /// `ts_type_ref`/`ts_ty` already used (keep the caller's own signature,
    /// build a real node internally, print just that fragment). Escaping
    /// (a literal `*/` inside the text becomes `*\/`, so it can't
    /// prematurely close the comment and let trailing text land as
    /// executable top-level TypeScript — issue #720) and the blank-line
    /// convention (a blank source line prints as a bare ` *`, no trailing
    /// space) live in the printer (`render_stmt`'s own `DocComment` arm,
    /// `crate::printer`'s own private renderer), matching
    /// where every other statement kind's rendering rules live.
    DocComment(String),
    /// A bare blank line — no statement content. #1323's own real, narrow
    /// need: `workers_entry.rs`'s `fetch` method body has an unconditional
    /// blank line after its internal Service-Binding dispatch block, and a
    /// conditional one after the WebSocket-upgrade dispatch (when present)
    /// and the Events dispatch (when present) — three specific points the
    /// pre-conversion `writeln!(out).unwrap()` wrote directly, nested
    /// *inside* the `try` block's own statement list. Distinct from
    /// [`crate::printer::print`]'s own top-level blank-line policy (this
    /// module's own printer doc), which only separates `TsProgram`'s own
    /// top-level statements — nothing before this slice needed a blank line
    /// anywhere inside a nested block, so that policy was never generalized
    /// to every nesting depth. A single, narrow statement kind represents
    /// exactly this, rather than widening the top-level-only policy to a
    /// depth it has no other real content to justify.
    Blank,
    /// `switch (<discriminant>) { <cases> }` — #1323's own largest gap: the
    /// first genuinely new statement-*grouping* construct in this tree
    /// (every prior addition was a single expression/type variant or a
    /// straightforward one-block wrapper). `workers_entry.rs` has four real
    /// `switch` statements (the internal `/_bynk/call/` and `/_bynk/event/`
    /// dispatches, the `scheduled` handler's `event.cron` dispatch, the
    /// `queue` handler's `batch.queue` dispatch); every real `case` there is
    /// a `{ ... }`-blocked body ending in a terminal statement (`return`/
    /// `continue`), so no fallthrough/shared-body grouping is represented —
    /// extend narrowly, the same posture every other addition here takes.
    /// Arc E slice 6 (#1445) widens this one further, narrowly: a
    /// non-`default` case's own bracing is now per-case
    /// ([`TsSwitchCase::case_braced`]) rather than unconditional —
    /// `emit_sum_codec`'s own payload-free-variant case is real, unbraced
    /// content, not a hypothetical.
    Switch {
        discriminant: TsExpr,
        cases: Vec<TsSwitchCase>,
    },
    /// `{ stmt; stmt; ... }` — a braced, multi-statement block printed on
    /// ONE generated line, distinct from [`TsStmtKind::Block`] (always
    /// multi-line). #1323's own real gap: `workers_entry.rs`'s queue
    /// consumer has two real sites (`if (__r.tag === "Err") { console.
    /// error(...); msg.retry(); continue; }`; the `else` branch of the
    /// ack/retry dispatch) where the pre-conversion `writeln!` code
    /// deliberately packed several short statements onto one physical
    /// line — a real, distinct formatting choice from every other
    /// multi-statement body in this tree, which always prints one
    /// statement per line. Only reachable through [`TsStmt::if_stmt`]/
    /// [`TsStmt::if_else_stmt`]'s own branches in real content today.
    InlineBlock(Vec<TsStmt>),
    /// `<expr>++;` — a postfix increment used as a whole statement. #1325's
    /// own real, narrow gap: `emit_test_main`'s own `passed++;`/`failed++;`
    /// counters. No prefix form, no decrement, no expression-position use
    /// (every real site increments a bare counter as its own statement) —
    /// [`TsExpr::Unary`] stays prefix-only (`!`/`typeof`), since a postfix
    /// operator used as a *statement* is a different grammatical position
    /// from a prefix operator used as an *operand*, not a shape `Unary`
    /// could represent by adding a variant.
    Increment(TsExpr),
    /// A pre-rendered, unconditional-passthrough statement blob — printed
    /// exactly as given, with no leading indent, no added semicolon, no
    /// added braces (matching [`TsStmtKind::Verbatim`]'s own `out.push_str(
    /// text)` rendering, not the ordinary per-kind `indent(depth)`-prefixed
    /// shape every other variant gets). #1337's own real need:
    /// `emit_method`'s own body is delegated wholesale to
    /// `emit_block_as_function_body_with_return` (`bynk-emit`'s
    /// `emitter/lower.rs:201`) — the one splice boundary ADR
    /// `arc-c-lower-rs-permanent-exclusion` names as a *permanent*,
    /// deliberate exclusion from this tree (`lower.rs` is the compiler's
    /// own second code-generation pass, comprehensive language-surface
    /// work Arc C was never scoped to cover), not a not-yet-converted
    /// residue [`TsStmtKind::Verbatim`] would misrepresent it as.
    ///
    /// Deliberately a SEPARATE variant from `Verbatim`, not a reuse of it,
    /// even though both render identically (`out.push_str(text)`): the
    /// `verbatim_sites`/`verbatim_origins` probes exist specifically to
    /// track Arc C's own *temporary* conversion residue trending toward
    /// zero — a `Raw` site is never counted there (it has no
    /// `VerbatimOrigin` at all), because using `Verbatim` here would make
    /// this permanent exclusion look like unfinished Arc C work a future
    /// slice is expected to close, which it structurally cannot.
    ///
    /// #1339's own second real use, broadening (not narrowing) the
    /// above: `emit_refined_type`'s own `of()` guard block splices
    /// `emit_refined_checks`'s own output this same way — that function
    /// keeps its exact `out: &mut String` signature (the P7.9/step-1
    /// pattern applied one level down, not a `lower.rs`-style permanent
    /// exclusion), but its real content is ALREADY built and printed from
    /// real `bynk_ts::TsStmt`/`print_stmt` calls internally — genuinely
    /// statement-shaped pre-rendered text, the same mechanical fit `Raw`
    /// already provides, just for a different underlying reason than
    /// `lower.rs`'s own permanent exclusion.
    ///
    /// #1445's own third real use (Arc E slice 6, `serialisation.rs`'s
    /// `emit_sum_codec`, via that file's own `raw_stmts_at_depth_one`
    /// helper): a real, currently-shipped BYTE-LEVEL quirk, not a scope
    /// boundary. `emit_field_deserialise_wire`'s own field guards, spliced
    /// into a sum's deserialise-side payload case, have always printed at
    /// depth 1 (`splice_stmts`'s own fixed indent) even though they sit
    /// structurally inside a `TsStmtKind::Switch` case's own body (which
    /// this printer would otherwise render two levels deeper, at depth 3) —
    /// confirmed byte-for-byte against `212_json_codec`/
    /// `407_workers_generic_sum_boundary`'s own real fixtures. Letting the
    /// switch case render these at their structurally-correct depth instead
    /// would be a real, deliberate formatting change with no fixture-corpus
    /// backing either way, which that slice's own zero-diff mandate ruled
    /// out choosing unprompted.
    ///
    /// All three real uses share the one property that actually matters for
    /// this variant's own existence: *real, already-correctly-indented (or,
    /// for the third, real-but-deliberately-NOT-restructured) statement text
    /// this call site cannot turn into a properly-nested `Vec<TsStmt>`
    /// without changing scope or bytes it isn't the one authorised to
    /// change* — not whether the reason is permanent, temporary, or a
    /// preserved historical quirk.
    Raw(String),
}

/// One `case`/`default` arm of a `TsStmtKind::Switch`. `test: None` is the
/// `default:` case — every real `default` in `workers_entry.rs` prints its
/// body directly under `default:` with no `{ }` block, while every real
/// non-`default` `case` (regardless of `test`) always prints a `{ }`-blocked
/// body.
///
/// `default_braced` (Arc C slice 33, `tests_emit.rs` slice C, #1401) —
/// `emit_stub_class`'s own `ReturnsEach` dispatch is the first real site
/// with a BRACED `default: { ... }`, a genuinely different convention from
/// `workers_entry.rs`'s own unbraced one; only meaningful when `test` is
/// `None` (a non-`default` case was unconditionally braced before
/// `case_braced` existed, below). Kept a per-case flag rather than changing
/// the existing unbraced-default rendering, since that would risk
/// `workers_entry.rs`'s own real, already-zero-diff content for no benefit —
/// the same "don't touch working, unrelated content" judgment this track
/// makes repeatedly.
///
/// `case_braced` (Arc E slice 6, #1445) — the mirror-image gap `default_braced`
/// left open: `serialisation.rs`'s `emit_sum_codec` (this tree's first real
/// non-`default` case that is NOT `{ }`-blocked) needs `case "Pending":
/// return { kind: "Pending" };` — unbraced — right beside a sibling
/// payload-carrying `case "Shipped": { ... }` — braced — in the *same*
/// switch (`212_json_codec`'s own mixed `Status` fixture, confirmed by
/// direct grep before this field was added: a payload-free variant's case
/// is always unbraced, a payload-carrying variant's is always braced, and a
/// real fixture exercises both side by side). Every prior real non-`default`
/// case (`workers_entry.rs`'s dispatches, `tests_emit.rs`'s own sequential-
/// outcome cases) already wants braces, so those call sites all set this
/// `true` — the same "narrow, argued, backward-compatible extension"
/// shape `default_braced` itself set as precedent. Only meaningful when
/// `test` is `Some(..)` (a `default` case's bracing is `default_braced`'s
/// business, not this field's — the inverse of that field's own scoping).
#[derive(Debug, Clone)]
pub struct TsSwitchCase {
    pub test: Option<TsExpr>,
    pub body: Vec<TsStmt>,
    pub default_braced: bool,
    pub case_braced: bool,
}

impl TsStmt {
    /// The one constructor for a `Verbatim`-kinded statement.
    pub fn verbatim(origin: VerbatimOrigin, text: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Verbatim {
                origin,
                text: text.into(),
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// Whether this is a named `import { … } from "…"` declaration (#1817): a
    /// module assembler hoists those into its import block.
    pub fn is_named_import(&self) -> bool {
        matches!(self.kind, TsStmtKind::Decl(TsDecl::Import { .. }))
    }

    /// The names this statement exports (#1820): an `export` of a named
    /// declaration, or an `export { … }` list, with or without `from`. A
    /// module assembler re-exporting another module's names reads them here.
    /// A `Raw` or `Verbatim` statement's text is not parsed, so an export
    /// written that way is not reported.
    ///
    /// Each name says whether this statement exports a value under it (#1862
    /// review): an `interface` or `type` alias exports only a type, and under
    /// `isolatedModules` a type-only name is re-exported only as `type X`
    /// (TS1205). A list entry follows [`TsDecl::Import`]'s spelling: `type X`
    /// is type-only, and `X as Y` exports `Y`. Every other entry is taken to
    /// carry a value, the form that re-exports both meanings.
    pub fn exported_names(&self) -> Vec<ExportedName<'_>> {
        let TsStmtKind::Decl(decl) = &self.kind else {
            return Vec::new();
        };
        match decl {
            TsDecl::ReExport { names, .. } | TsDecl::ExportNames { names } => names
                .iter()
                .map(|spec| {
                    let (spec, has_value) = match spec.trim().strip_prefix("type ") {
                        Some(rest) => (rest, false),
                        None => (spec.trim(), true),
                    };
                    let name = spec.rsplit(" as ").next().unwrap_or(spec).trim();
                    ExportedName { name, has_value }
                })
                .collect(),
            TsDecl::Export(inner) => match inner.as_ref() {
                TsDecl::Interface { name, .. } | TsDecl::TypeAlias { name, .. } => {
                    vec![ExportedName {
                        name,
                        has_value: false,
                    }]
                }
                TsDecl::ConstDecl { name, .. }
                | TsDecl::Class { name, .. }
                | TsDecl::Function { name, .. } => vec![ExportedName {
                    name,
                    has_value: true,
                }],
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    pub fn decl(decl: TsDecl, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Decl(decl),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn const_stmt(
        name: TsBindingName,
        ty: Option<TsType>,
        init: TsExpr,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::Const { name, ty, init },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn let_stmt(
        name: TsBindingName,
        ty: Option<TsType>,
        init: Option<TsExpr>,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::Let {
                name,
                ty,
                init,
                hoisted: false,
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// #2617: `var name: ty;` — a hoisted binding with no initialiser, which
    /// reads as `undefined` (rather than throwing) before its statement runs.
    /// The cache behind each constant's function (`bynk-emit`'s
    /// `emit_consts`), which an import cycle may call before the declaring
    /// module has evaluated.
    pub fn var_stmt(name: impl Into<String>, ty: TsType, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Let {
                name: TsBindingName::Ident(name.into()),
                ty: Some(ty),
                init: None,
                hoisted: true,
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn expr_stmt(expr: TsExpr, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::ExprStmt(expr),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn return_stmt(expr: Option<TsExpr>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Return(expr),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn throw_stmt(expr: TsExpr, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Throw(expr),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn if_stmt(cond: TsExpr, then_branch: TsStmt, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::If {
                cond,
                then_branch: Box::new(then_branch),
                else_branch: None,
                same_line_else: false,
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn if_else_stmt(
        cond: TsExpr,
        then_branch: TsStmt,
        else_branch: TsStmt,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::If {
                cond,
                then_branch: Box::new(then_branch),
                else_branch: Some(Box::new(else_branch)),
                same_line_else: false,
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// [`TsStmt::if_else_stmt`]'s own sibling with `} else {` on one line —
    /// #1325's own real gap, `emit_test_main`'s own real `else` spacing. See
    /// `TsStmtKind::If`'s own doc for why this needs to be a distinct
    /// constructor rather than a change to the existing default.
    pub fn if_else_same_line_stmt(
        cond: TsExpr,
        then_branch: TsStmt,
        else_branch: TsStmt,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::If {
                cond,
                then_branch: Box::new(then_branch),
                else_branch: Some(Box::new(else_branch)),
                same_line_else: true,
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn for_of(
        binding: impl Into<String>,
        iter: TsExpr,
        body: TsStmt,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::ForOf {
                binding: binding.into(),
                iter,
                body: Box::new(body),
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// `for (let <name> = <init>; <test>; <update>++) <body>` — see
    /// `TsStmtKind::For`'s own doc for exactly what this construct does
    /// and does not represent.
    pub fn for_stmt(
        name: impl Into<String>,
        init: TsExpr,
        test: TsExpr,
        body: TsStmt,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::For {
                name: name.into(),
                init,
                test,
                body: Box::new(body),
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn try_catch(
        try_block: TsStmt,
        catch_param: Option<impl Into<String>>,
        catch_block: TsStmt,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::TryCatch {
                try_block: Box::new(try_block),
                catch_param: catch_param.map(Into::into),
                catch_block: Box::new(catch_block),
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn block(stmts: Vec<TsStmt>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Block(stmts),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn continue_stmt(span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Continue,
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn assign(target: TsExpr, value: TsExpr, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Assign { target, value },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn comment(text: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Comment(text.into()),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn doc_comment(text: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::DocComment(text.into()),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn blank(span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Blank,
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn switch_stmt(discriminant: TsExpr, cases: Vec<TsSwitchCase>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Switch {
                discriminant,
                cases,
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn inline_block(stmts: Vec<TsStmt>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::InlineBlock(stmts),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn increment(expr: TsExpr, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Increment(expr),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// The one constructor for a `Raw`-kinded statement — `text` is printed
    /// verbatim, exactly as given (see `TsStmtKind::Raw`'s own doc for
    /// why this is a distinct kind from `Verbatim`, not a reuse of it).
    pub fn raw(text: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Raw(text.into()),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }
}

/// A binding's own name, in either of the two shapes `events_fanout.rs`
/// itself uses: a plain identifier (`const subs = ...`), or an
/// object-destructuring pattern (`const { events } = ...`) — naming only
/// the destructured properties themselves (`{ a, b }`), not the renamed
/// (`{ a: renamed }`) or nested (`{ a: { b } }`) forms, since nothing in
/// the grounding file needs either.
#[derive(Debug, Clone)]
pub enum TsBindingName {
    Ident(String),
    ObjectPattern(Vec<String>),
}

/// An expression. Only the shapes `events_fanout.rs` concretely uses
/// (Decision B) — not the reference sketch's full `TsExpr` list (`Arrow`,
/// `Cond`, `TemplateLit`, `Spread` are all unused in the grounding file and
/// deliberately not built here). Arc C slice 3 (#1321, `workers.rs`) adds
/// `Arrow`, `OptionalMember`/`OptionalIndex` (Decision A, gaps 3/4) — this
/// slice's own real, grounded needs.
#[derive(Debug, Clone)]
pub enum TsExpr {
    /// A real identifier — the only content this variant is for. #1539
    /// (review of `design/reviews/2026-08-30-post-restructuring-review.md`
    /// Part 3/Part 5 §1): before this doc existed, `bynk-emit` also smuggled
    /// non-identifier text through it at 15 call sites in `emit.rs` alone —
    /// a quoted string literal, `this.<method>` member access, a complete
    /// `!(<pred>)` unary expression — an untagged `Verbatim`-shaped escape
    /// hatch [`TsStmt::verbatim`]'s own doc already named the concern for
    /// (Q2's "makes the ratchet a compile-time construct, not a grep"), just
    /// with no tag and no probe watching it. Every one of those call sites
    /// now either builds the matching real node (`Lit(TsLit::Str)`, `Member`,
    /// `Unary`) or routes through [`TsExpr::VerbatimExpr`] instead — this
    /// variant no longer holds anything but a genuine identifier.
    Ident(String),
    /// A residual, not-yet-converted expression carrying a [`VerbatimOrigin`]
    /// tag — [`TsStmt::verbatim`]'s own discipline, closed for expressions
    /// too (#1539). Printed exactly as [`TsExpr::Ident`] always was (raw
    /// text, no escaping of its own — the caller is responsible for
    /// producing valid, already-escaped TypeScript, the same contract
    /// `TsStmt::verbatim`'s own text carries), but now visible to
    /// [`crate::verbatim_violations`] and `xtask`'s own `verbatim_origins`/
    /// `verbatim_sites` gated probes, which had nothing to watch at the
    /// expression level before this variant existed. What ends up here after
    /// #1539's own conversion pass: a generic-typed method/constructor
    /// callee (`Call`/`New` have no `type_args` field), a nested
    /// `As`-under-`As` cast chain the printer's own operand-parenthesisation
    /// rule would otherwise mis-wrap (review of #1390), a block-bodied IIFE
    /// with no matching `TsArrowBody` shape, and a `pred_condition_and_
    /// message`-style message that is already pre-escaped or unescaped by
    /// contract (a second escaping pass through `TsLit::Str` would corrupt
    /// it) — none of these has a real node to convert to without expanding
    /// this crate's own type algebra well past what #1539 asks for.
    VerbatimExpr(String, VerbatimOrigin),
    /// `object.property`.
    Member {
        object: Box<TsExpr>,
        property: String,
    },
    /// `object?.property` — the optional-chaining form of [`TsExpr::Member`].
    /// A distinct variant, not an `optional: bool` flag on `Member` itself
    /// (#1321's own Decision A, gap 4, left the mechanism to the
    /// implementation): a flag would touch every one of `Member`'s
    /// already-many real call sites (`events_fanout.rs`, this crate's own
    /// printer tests) that never need one, where a separate variant touches
    /// none of them — narrower, matching this track's own repeated
    /// "smallest correct scope" judgment. `workers.rs`'s own secret-probe
    /// idiom (`emit_websocket_upgrade`/`emit_http_wrapper`/
    /// `emit_secret_lookup`, three identical real sites) is the only
    /// grounding.
    OptionalMember {
        object: Box<TsExpr>,
        property: String,
    },
    /// `object[index]`.
    Index {
        object: Box<TsExpr>,
        index: Box<TsExpr>,
    },
    /// `object?.[index]` — the optional-chaining form of [`TsExpr::Index`],
    /// the same real grounding and the same "separate variant, not a flag"
    /// reasoning as [`TsExpr::OptionalMember`].
    OptionalIndex {
        object: Box<TsExpr>,
        index: Box<TsExpr>,
    },
    /// `(params) => body` (`is_async: false`) or `async (params) => body`
    /// (`is_async: true`) — an arrow function, expression- or block-bodied
    /// (see [`TsArrowBody`]). `is_async` added by #1327: `emit_composition_root`'s
    /// own `__eventsDispatch` closure (`async (events: Array<...>) => {...}`)
    /// is the first real `Arrow` site that's async — mirrors
    /// `TsDecl::Function`'s own `is_async` field (#1325), added for the same
    /// reason. `generics`/`return_type` added by #1339: `emit_sum_type`'s
    /// own generic payload-constructor arrows (`<T>(name: T): Sum<T> =>
    /// (...)`) need both — bare generic names only (empty for every
    /// non-generic site, the overwhelming majority), matching
    /// `TsObjectEntry::Method.generics`'s own (#1337) identical convention;
    /// `return_type` mirrors `TsDecl::Function.return_type`'s own existing
    /// `Option<TsType>` shape — a real gap the accepted proposal's own
    /// grounding named `generics` for but missed: every one of this file's
    /// own real generic-payload arrows carries an explicit return-type
    /// annotation the arrow itself owns (`: {name}{params}`), not something
    /// the body's own type alone determines.
    ///
    /// `body`'s type became [`TsArrowBody`] (#1435, Arc E slice 1): from
    /// P7.8 through #1434 this field was a bare `Box<TsExpr>` — expression-
    /// bodied only, "extend narrowly" (the same posture `TsBinaryOp` takes
    /// for its own operator table), since every real site up to and
    /// including #1339's generic payload-constructor arrows had an
    /// expression body. `serialisation.rs`'s `serialise_field_expr_wire`
    /// (`bynk-emit`) is the first real site that doesn't: its `Float`
    /// non-finite guard is a genuine statement-bodied IIFE (`((v: number) =>
    /// { if (!Number.isFinite(v)) throw new Error(...); return v as
    /// JsonValue; })(value)`), not reducible to one expression. Widening the
    /// existing field (`TsArrowBody::Expr(Box<TsExpr>)` |
    /// `TsArrowBody::Block(Vec<TsStmt>)`) rather than adding a second
    /// sibling `TsExpr` variant matches this file's own repeated
    /// "extend the existing variant when every real site still needs the
    /// same node kind, only the body shape differs" precedent
    /// (`TsObjectEntry::Method.inline`, #1337) — every prior `Arrow`
    /// construction site across the workspace wraps its already-correct
    /// expression body in `TsArrowBody::Expr(..)`, a mechanical change with
    /// no behavior difference.
    Arrow {
        params: Vec<TsParam>,
        is_async: bool,
        generics: Vec<String>,
        return_type: Option<TsType>,
        body: Box<TsArrowBody>,
    },
    Call {
        callee: Box<TsExpr>,
        args: Vec<TsExpr>,
    },
    New {
        callee: Box<TsExpr>,
        args: Vec<TsExpr>,
    },
    /// A value object literal, e.g. `{ status: 204 }` — comma-separated.
    /// `multiline: false` (the ordinary case, via [`TsExpr::object`])
    /// always prints on one line, matching [`TsType::Object`]'s own
    /// (semicolon-separated) single-line convention for the *type*-position
    /// shape. `multiline: true` (via [`TsExpr::multiline_object`]) is a
    /// real, grounded gap found implementing Arc C's first slice (#1317):
    /// `events_fanout.rs`'s own `__eventRoutes` table is a top-level
    /// `const` initializer with one entry per line, each with its own
    /// trailing comma, closing brace at the *statement's* own indent —
    /// TypeScript's ordinary multi-line object-literal convention, which
    /// nothing in this crate could represent before this addition. Only
    /// statement/declaration-level renderers (which already carry `depth`)
    /// can render this correctly — `printer.rs`'s own `render_stmt_level_
    /// expr`, and (#1355) `render_multiline_object_entry`'s own `Prop` arm,
    /// for a `multiline: true` object nested one level inside ANOTHER
    /// multiline object as one of its own entries' values —
    /// `emit_messages_bundle`'s own real doubly-nested `{ locale: { code:
    /// expr, ... }, ... }` table. A `multiline: true` object nested any
    /// OTHER way (an array element, a call argument, a `Prop`'s value
    /// inside a non-multiline object, …) still renders via the ordinary
    /// depth-unaware `render_expr` recursion, which cannot honour
    /// `multiline` — not reachable from any real `bynk-emit` call site
    /// today, but worth knowing before nesting one that way.
    Object {
        entries: Vec<TsObjectEntry>,
        multiline: bool,
    },
    /// An array literal, e.g. `[{ binding: "x", service: "y" }]`.
    /// `multiline: false` (the ordinary case, via [`TsExpr::array`]) always
    /// prints on one line, comma-separated — every real site before this
    /// slice. `multiline: true` (via [`TsExpr::multiline_array`]) is #1325's
    /// own real gap: `emit_test_main`'s own `modules` array (one `{ name,
    /// run }` entry per test, one per line, each with its own trailing
    /// comma, closing `]` at the statement's own indent) — the exact same
    /// shape [`TsExpr::Object`]'s own `multiline` field already represents
    /// for object literals, just for an array. Same reachability boundary as
    /// `Object`'s own `multiline` field (see its own doc, updated by
    /// #1355): `render_stmt_level_expr` and `render_multiline_object_
    /// entry`'s own `Prop` arm both honour it; nested any other way, a
    /// `multiline: true` array falls back to single-line via the ordinary
    /// depth-unaware `render_expr` recursion.
    Array {
        items: Vec<TsExpr>,
        multiline: bool,
    },
    /// `` `text${expr}more text` `` — a template literal. `parts.len()` is
    /// always `exprs.len() + 1` (`parts[0]` before the first substitution,
    /// `parts[i+1]` after `exprs[i]`, …). #1325's own real, first grounding
    /// for this shape (`bynk-ts`'s own module doc named `TemplateLit`
    /// explicitly "unused in the grounding file" until now):
    /// `emit_test_main`'s own `` `${m.name}:` ``/`` `${passed} passed,
    /// ${failed} failed.` `` lines.
    ///
    /// **`parts` are printed verbatim — the printer applies no escaping of
    /// its own.** The same "the field is already a raw-text slot" reasoning
    /// [`TsDecl::Import`]'s own `names` field doc already uses, not a new
    /// pattern: `emit_test_main`'s own two real ✓/✗ substitution lines embed
    /// a literal `✓`/`✗` JS unicode escape as pre-formed ASCII
    /// text (six literal characters, not the actual glyph) — an escaper
    /// mirroring [`TsLit::Str`]'s own (which escapes every `\` it sees)
    /// would double that literal backslash into `\\u2713`, corrupting the
    /// exact byte-golden output this slice must match. Every real part in
    /// `emit_test_main` is static, compiler-authored text (never Bynk user
    /// data), so there is no real content this boundary loses safety on
    /// today — a future caller passing untrusted/dynamic text into `parts`
    /// is responsible for pre-escaping backtick/`` ${ ``/`\` itself before
    /// constructing one.
    TemplateLit {
        parts: Vec<String>,
        exprs: Vec<TsExpr>,
    },
    Await(Box<TsExpr>),
    /// `expr as ty`.
    As {
        expr: Box<TsExpr>,
        ty: TsType,
    },
    Unary {
        op: TsUnaryOp,
        expr: Box<TsExpr>,
    },
    Binary {
        op: TsBinaryOp,
        left: Box<TsExpr>,
        right: Box<TsExpr>,
    },
    /// `test ? consequent : alternate` — #1323's own real gap: `method ===
    /// "HEAD" ? headResponse(__response) : __response` (once per `GET`
    /// route) and `method === "OPTIONS" ? 204 : 405` (the method-fallthrough
    /// path). The lowest-precedence expression form after `Arrow` — needs
    /// the same parenthesization-rule coverage `Arrow` got in review of
    /// #1322 (`needs_parens_as_operand`/`render_binary_operand`/`As`'s own
    /// local rule), added proactively in this same slice rather than left
    /// for a review round to re-find.
    Conditional {
        test: Box<TsExpr>,
        consequent: Box<TsExpr>,
        alternate: Box<TsExpr>,
    },
    /// An explicit, printer-preserved parenthesization — distinct from every
    /// other variant's own precedence-*derived* parens (`render_operand`/
    /// `render_binary_operand`), which the printer adds or omits based on
    /// what the wrapped expression *is*. `Paren` instead always prints
    /// `(<inner>)` regardless of `inner`'s own shape or precedence. #1323's
    /// own real, narrow need: `workers_entry.rs`'s CORS-preflight guard
    /// wraps its own path-match condition in unconditional parens
    /// (`&& ({cond})`) even when `cond` reduces to a single equality check
    /// with no operator lower-precedence than the outer `&&` — a real case
    /// the precedence-derived rules correctly do *not* parenthesize (they're
    /// answering "is this needed for correctness", not "did the source
    /// always write parens here"). Not a general escape hatch: every other
    /// real site in this file's own content still goes through the ordinary
    /// precedence machinery unchanged.
    Paren(Box<TsExpr>),
    Lit(TsLit),
}

/// [`TsExpr::Arrow`]'s own `body` shape (#1435, Arc E slice 1) — see that
/// field's own doc for why this is a widened field rather than a second
/// `TsExpr` variant. `Expr` is every real site before this slice (and the
/// overwhelming majority after it): the arrow's body is one expression,
/// printed with no surrounding braces. `Block` is `serialisation.rs`'s own
/// `Float` non-finite guard, the first real statement-bodied arrow anywhere
/// in this tree — printed as a real braced block, reusing this crate's
/// own printer's existing compact-statement-list renderer
/// (`render_compact_stmts`, the same one `TsStmtKind::InlineBlock` already
/// shares with `render_branch`'s own same-line `if`/`else`) rather than a
/// third copy of that "one physical line, semicolon-separated" logic. Every
/// real `Block` site today is exactly this one-line IIFE shape (an arrow
/// with no other real use of a genuinely multi-line block body has been
/// found); a future multi-line block-bodied arrow is a real, separate gap
/// this variant does not yet cover.
#[derive(Debug, Clone)]
pub enum TsArrowBody {
    Expr(Box<TsExpr>),
    Block(Vec<TsStmt>),
}

impl TsExpr {
    /// The ordinary, single-line object literal, `Prop`-only — every entry
    /// is `key: value`. `events_fanout.rs`'s own entries (and this crate's
    /// own printer tests) are all this shape; kept taking a plain
    /// `Vec<(String, TsExpr)>` rather than `Vec<TsObjectEntry>` so none of
    /// those existing call sites needed to change when `TsObjectEntry` was
    /// added (#1321) — see [`TsExpr::object_entries`] for the mixed-entry
    /// form `workers.rs` itself needs (shorthand/spread/method entries).
    pub fn object(entries: Vec<(String, TsExpr)>) -> Self {
        TsExpr::Object {
            entries: entries
                .into_iter()
                .map(|(k, v)| TsObjectEntry::Prop(k, v))
                .collect(),
            multiline: false,
        }
    }

    /// One entry per line, each with its own trailing comma — see
    /// [`TsExpr::Object`]'s own doc for the real shape and the
    /// depth-awareness this needs at the print site. `Prop`-only, the same
    /// convenience [`TsExpr::object`]'s own doc explains.
    pub fn multiline_object(entries: Vec<(String, TsExpr)>) -> Self {
        TsExpr::Object {
            entries: entries
                .into_iter()
                .map(|(k, v)| TsObjectEntry::Prop(k, v))
                .collect(),
            multiline: true,
        }
    }

    /// The single-line object literal, taking [`TsObjectEntry`] directly —
    /// for a mixed entry list (shorthand/spread/method alongside `Prop`),
    /// which [`TsExpr::object`]'s own `Vec<(String, TsExpr)>` convenience
    /// can't represent. #1321's own real grounding: `workers.rs`'s local
    /// capability-provider `deps` object mixes shorthand names
    /// (`{ cap1, cap2 }`) with explicit `key: value` entries
    /// (`__exec: exec`) in one literal.
    pub fn object_entries(entries: Vec<TsObjectEntry>) -> Self {
        TsExpr::Object {
            entries,
            multiline: false,
        }
    }

    /// [`TsExpr::multiline_object`]'s own `TsObjectEntry` sibling —
    /// #1321's own real grounding: `workers.rs`'s `compose`-returned
    /// surface object is one shorthand-async-`Method` entry per wrapper,
    /// one per line.
    pub fn multiline_object_entries(entries: Vec<TsObjectEntry>) -> Self {
        TsExpr::Object {
            entries,
            multiline: true,
        }
    }

    /// The ordinary, single-line array literal — every real site before
    /// #1325.
    pub fn array(items: Vec<TsExpr>) -> Self {
        TsExpr::Array {
            items,
            multiline: false,
        }
    }

    /// One entry per line, each with its own trailing comma — see
    /// [`TsExpr::Array`]'s own doc for the real shape and the
    /// depth-awareness this needs at the print site. #1325's own real
    /// grounding: `emit_test_main`'s own `modules` array.
    pub fn multiline_array(items: Vec<TsExpr>) -> Self {
        TsExpr::Array {
            items,
            multiline: true,
        }
    }

    /// A template literal — see [`TsExpr::TemplateLit`]'s own doc for the
    /// real shape and its no-escaping boundary. The sole caller-facing
    /// invariant (`parts.len() == exprs.len() + 1`) is asserted here rather
    /// than left to `render_expr`'s own `parts`-driven loop, which would
    /// otherwise silently drop trailing `exprs` on a malformed tree instead
    /// of failing loudly (review of #1326, finding 1).
    pub fn template_lit(parts: Vec<String>, exprs: Vec<TsExpr>) -> Self {
        debug_assert_eq!(
            parts.len(),
            exprs.len() + 1,
            "TsExpr::template_lit: parts.len() must be exprs.len() + 1 \
             (parts[0] before the first substitution, parts[i+1] after \
             exprs[i], …) — got {} parts and {} exprs",
            parts.len(),
            exprs.len()
        );
        TsExpr::TemplateLit { parts, exprs }
    }
}

/// One entry of a [`TsExpr::Object`] literal. Only `Prop` existed before
/// #1321 (`events_fanout.rs`'s own grounding never needed the other three) —
/// `workers.rs`'s own dominant shape (Decision A, gap 1) is a literal
/// object whose entries are shorthand async methods (every `on call`/`on
/// http`/… wrapper attaches this way), its local capability-`deps` object
/// mixes bare shorthand names with explicit `key: value` pairs, and three
/// real sites spread another object into one (gap 2, folded in here rather
/// than as its own top-level `TsExpr` variant — a spread only ever appears
/// as an object- or array-literal entry in this file, never as a
/// standalone expression, so scoping it to entry position is the narrower
/// correct change).
#[derive(Debug, Clone)]
pub enum TsObjectEntry {
    /// `key: value`.
    Prop(String, TsExpr),
    /// `key` alone — object-literal property shorthand, e.g. `{ cap1,
    /// cap2 }`. Distinct from `Prop(name, TsExpr::Ident(name))`, which
    /// prints `name: name`, not the shorthand form real TypeScript
    /// property-shorthand text actually is.
    Shorthand(String),
    /// A shorthand async method entry, e.g. `async foo(a: T) { ... },` —
    /// `workers.rs`'s own dominant shape (Decision A, gap 1): every
    /// wrapper helper (`emit_call_wrapper`, `emit_event_wrapper`, …)
    /// attaches to the returned `compose` surface this way — none of
    /// `workers.rs`'s own real sites annotates a return type, so
    /// `return_type` stayed unneeded until #1323's own real gap:
    /// `workers_entry.rs`'s `export default { fetch, scheduled?, queue? }`
    /// entries all carry one (`: Promise<Response>`/`: Promise<void>`), the
    /// same field [`TsClassMethod::return_type`] already has for the
    /// declaration-position sibling this mirrors.
    ///
    /// `generics` and `doc` (#1337's own real gaps, both found only by the
    /// zero-diff fixture check — not the accepted proposal's own citation,
    /// which searched the project-form fixture corpus only;
    /// `402_generic_instance_method` is single-file form, and while
    /// `65_money_uses_time`/`64_full_time_commons` ARE project form, the
    /// citation's own doc-comment search used the wrong marker, `///`
    /// rather than this language's real `---`-delimited doc block —
    /// both gaps outside what that first pass actually checked):
    ///
    /// - `generics`: a method on a generic type erases to
    ///   `{name}<{generics}>(self: {Type}<{generics}>, …)` — `Box.map<A,
    ///   U>`, the type's own `A` plus the method's own `U`. Bare names
    ///   only, no bounds/defaults — every real generic parameter list this
    ///   tree ever builds is (matching [`crate::printer::print_type`]'s
    ///   own `TsType::Named`-only-name convention for the identical
    ///   reason), so a full `Vec<TsParam>` (with its own unneeded
    ///   `ty`/`optional` fields) would be the wrong shape here.
    /// - `doc`: a JSDoc block immediately preceding the method entry, at
    ///   the same indent — `Timestamp.diff`/`Timestamp.add`
    ///   (`65_money_uses_time`) both carry one. Printed the identical way
    ///   `TsStmtKind::DocComment` is (same escaping, same blank-line
    ///   convention), reusing that one renderer rather than a second copy
    ///   — this field exists because an object-entry method has no
    ///   `TsStmt` slot of its own to hold a preceding statement in.
    Method {
        name: String,
        is_async: bool,
        generics: Vec<String>,
        params: Vec<TsParam>,
        return_type: Option<TsType>,
        doc: Option<String>,
        /// `false` (every method entry landed before #1337): `body` renders
        /// as an ordinary braced, multi-line block. `true`: `body` renders
        /// compactly on the SAME line as the signature — `{ <stmts>; }`,
        /// reusing this crate's own established compact-statement
        /// machinery (`TsStmtKind::InlineBlock`'s own sibling shape). A
        /// real gap #1337's own zero-diff check found:
        /// `emit_forwarded_methods`'s own pre-conversion `writeln!` always
        /// built the WHOLE entry — signature and one-statement body alike —
        /// on one physical line (`"  {method}({params}): {ret} {{ return
        /// …; }},"`), a genuinely different real shape from every other
        /// `Method` entry in this tree, all of which are multi-line.
        inline: bool,
        body: Vec<TsStmt>,
    },
    /// `...expr` — an object-spread entry (Decision A, gap 2), e.g. `{
    /// ...deps, identity: __caller }`.
    Spread(TsExpr),
}

/// `!x` (`events_fanout.rs`'s `!Array.isArray(subs)`, `!binding`) and
/// `typeof x` (#1321, `workers.rs`'s own secret-probe idiom: `typeof
/// __secret !== "string"`) — the two unary operators real content uses, not
/// the full JS/TS table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsUnaryOp {
    Not,
    Typeof,
}

/// `??` (`events_fanout.rs`'s `env ?? {}`), plus three more real operators
/// #1321 (`workers.rs`) grounds: `||`/`&&` (the Bearer-header presence
/// checks, `__authz === null || !__authz.startsWith(...)` /
/// `__authz !== null && __authz.startsWith(...)`) and `===`/`!==`
/// (pervasive throughout the file's own tagged-result and header checks).
/// #1323 (`workers_entry.rs`) grounds one more: `>` (the request-body-
/// ceiling guard's own `Number(__contentLength) > <cap>` — the one real
/// site anywhere in `bynk-emit` that needs a relational, not equality,
/// comparison). Not the
/// full JS/TS operator table (Decision B's own "extend narrowly" posture) —
/// see the printer's own `binary_precedence` (`bynk-ts/src/printer.rs`,
/// private) for why a nested `Binary` operand's parenthesisation needed to
/// become precedence-aware once more than one operator existed. Arc C, step
/// (11) (#1388) grounds one more: `+` (string concatenation) — the
/// ICU-formatting cluster's own dominant structural pattern, every
/// literal/placeholder segment in a message template joins this way. Real
/// JS/TS precedence (binds tighter than every comparison/logical operator
/// this table already has) and real left-associativity (`"a" + "b" + "c"`
/// needs no parens, the same way a same-operator `||`/`&&` chain already
/// prints flat) both matter here, not just the operator symbol itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsBinaryOp {
    NullishCoalescing,
    Or,
    And,
    StrictEq,
    StrictNotEq,
    GreaterThan,
    /// `instanceof` — Arc C slice 34 (`tests_emit.rs` slice D, #1403)'s own
    /// real gap: `emit_test_case_function`'s own catch clause
    /// (`e instanceof ExpectationError`) is the first real `instanceof`
    /// anywhere in `bynk-emit`'s own content. Real JS/TS precedence puts it
    /// at the same tier as the relational comparisons (`<`/`>`) — sharing
    /// [`TsBinaryOp::LessThan`]'s own tier, not a new one — rendered as the
    /// keyword `" instanceof "`, textually the same shape as `&&`/`||`
    /// rather than symbol punctuation.
    InstanceOf,
    /// `<` — Arc C slice 33 (`tests_emit.rs` slice C, #1401)'s own real gap:
    /// `emit_stub_class`'s `ReturnsEach` sequence-cursor guard
    /// (`this.__seq_N < <bound>`) is the first real `<` comparison anywhere
    /// in `bynk-emit`'s own content. Same precedence tier as
    /// [`TsBinaryOp::GreaterThan`] (real JS/TS relational operators all
    /// share one level) — added alongside it rather than folding into a
    /// single "relational" variant, matching this enum's own existing
    /// one-variant-per-real-operator convention.
    LessThan,
    Add,
    /// `in` — Arc E slice 5 (`serialisation.rs`, #1443)'s own real gap:
    /// `emit_record_codec`'s per-field default-value prevalidation line
    /// (`"<field>" in obj ? obj["<field>"] : <default>`) needs a real "is
    /// this wire key present at all" test, distinct from `!== undefined`
    /// (Events slice 3a, #972's own Decision D: a wire key present with an
    /// explicit `null`/`{"kind":"None"}` value must NOT fall through to the
    /// default, only a genuinely *absent* key may). Real JS/TS grammar puts
    /// `in` at the exact same `RelationalExpression` precedence tier as
    /// `<`/`>`/`instanceof` (all five — plus `<=`/`>=`, unmodelled here —
    /// share one level in the spec), so it joins [`TsBinaryOp::LessThan`]'s
    /// tier rather than a new one, the same "share, don't multiply, tiers"
    /// convention `InstanceOf`/`LessThan` already set. Rendered as the
    /// keyword `" in "`, the same textual-keyword-operator shape
    /// `InstanceOf` already established (not symbol punctuation).
    In,
    /// `>=` — #1471's own real gap: `pred_condition_and_message`'s
    /// (`bynk-emit`) `NonNegative`/`InRange`/`InRangeF`/`MinLength` arms
    /// each build a real "at least" comparison (`{receiver} >= 0`,
    /// `{receiver} >= {a}`, `{receiver}.length >= {n}`) that this enum had
    /// no home for — [`TsBinaryOp::In`]'s own doc already named this exact
    /// gap ("plus `<=`/`>=`, unmodelled here"). Same `RelationalExpression`
    /// precedence tier as [`TsBinaryOp::GreaterThan`]/[`TsBinaryOp::
    /// LessThan`]/[`TsBinaryOp::InstanceOf`]/[`TsBinaryOp::In`] (real
    /// JS/TS puts all six at one level), rendered `" >= "` — symbol
    /// punctuation, matching `GreaterThan`/`LessThan`'s own convention,
    /// not a keyword like `InstanceOf`/`In`.
    GreaterThanEq,
    /// `<=` — the same gap's other half: `InRange`/`InRangeF`/`MaxLength`'s
    /// own "at most" comparison (`{receiver} <= {b}`, `{receiver}.length
    /// <= {n}`). Same tier and rendering convention as
    /// [`TsBinaryOp::GreaterThanEq`], symmetric with it the way
    /// `GreaterThan`/`LessThan` already are.
    LessThanEq,
}

/// A literal — the three kinds `events_fanout.rs` uses (a string, a number,
/// `null`), plus `Bool` (#1323's own real gap: `workers_entry.rs`'s
/// `CorsPolicy.credentials`/`SecurityPolicy.nosniff` object-literal fields
/// are real booleans, e.g. `credentials: true`, `nosniff: false` — nothing
/// before this slice's own grounding ever built one).
#[derive(Debug, Clone)]
pub enum TsLit {
    Str(String),
    /// Rendered verbatim, as text — TypeScript's own numeric-literal
    /// grammar is not this crate's problem to re-derive; the caller passes
    /// the exact digits it wants printed.
    Num(String),
    Null,
    Bool(bool),
    /// The complete literal text, printed exactly as given — no quoting, no
    /// escaping, nothing added or removed. #1325's own real, narrow gap:
    /// `emit_test_main`'s own `const PREFIX = "system · ";` embeds
    /// a literal JS unicode escape (six ASCII characters, `·`) as
    /// pre-formed text — [`TsLit::Str`]'s own escaper (which turns every `\`
    /// it sees into `\\`) would double that literal backslash, corrupting
    /// this specific byte-golden output. The same "already a raw-text slot"
    /// boundary [`TsExpr::TemplateLit`]'s own `parts` field documents, just
    /// for a whole literal (quotes included) rather than a template
    /// literal's own static segments. One real site — not a general
    /// escaping bypass for ordinary string content, which stays on
    /// `TsLit::Str`.
    Raw(String),
}

/// A type-position node. `Named` (extended with type arguments — a real gap
/// the reference sketch left unaddressed: `Record<string,
/// Array<{...}>>`/`Promise<Response>` both need one, and a bare `Named`
/// with no type-argument slot cannot represent either), `Array` (extended
/// with a `readonly` modifier — P7.9's own real gap: every `List`/`Query`
/// element type `bynk-emit`'s `ts_type_ref*`/`ts_ty` families build is
/// `readonly T[]`, not plain `T[]`), `Object`, and `Fn` (P7.9's own second
/// real gap — the query-thunk wrapper `(() => readonly T[])` and a real
/// parametered function type `(a0: T0, …) => Ret` both need one) — not the
/// sketch's `Union`/`Intersection`/`Literal`/`TypeParam`/`Readonly` (still
/// unused; `readonly` here is a modifier on `Array`, not the sketch's own
/// separate `Readonly` wrapper variant).
#[derive(Debug, Clone)]
pub enum TsType {
    /// A named type, optionally generic — `string`/`unknown` (no type
    /// arguments) or `Record<string, T>`/`Promise<Response>` (some).
    Named {
        name: String,
        type_args: Vec<TsType>,
    },
    /// `T[]` (`readonly: false`) or `readonly T[]` (`readonly: true`) —
    /// TypeScript's own postfix array-type syntax (not the equivalent
    /// `Array<T>`/`ReadonlyArray<T>` generic spelling either family uses).
    ///
    /// The printer parenthesises `element` wherever a bare postfix `[]`
    /// would bind to the wrong thing (#1898): a readonly array (`readonly
    /// T[][]` is TS1354), a [`TsType::Fn`] (whose `[]` would bind to its
    /// return type), a [`TsType::Union`] and a [`TsType::Intersection`]
    /// (`A | B[]` reads as `A | (B[])`). So `Array` over any element prints
    /// correctly as built — a caller must not wrap the element itself, or
    /// it double-parenthesises. The rule is `needs_parens_as_array_element`
    /// in `printer.rs`. This closes the hazard the review of #1315/#1316
    /// recorded here.
    Array {
        element: Box<TsType>,
        readonly: bool,
    },
    /// A type-position object shape, e.g. `{ type: string; payload: unknown }`
    /// — semicolon-separated, always printed on one line (see
    /// [`TsExpr::Object`]'s own doc for the value-position contrast). An
    /// `interface`'s own *members* are each printed on their own line by
    /// [`TsDecl::Interface`]'s printer, independent of this variant's own
    /// (always-inline) rendering — the two only look different because
    /// `TsDecl::Interface` is the thing choosing to put each member on its
    /// own line, not because `Object` has two rendering modes.
    ///
    /// #1323 replaced the plain `Vec<(String, TsType, bool)>` (`optional`
    /// only, #1321) with [`TsTypeMember`] — `workers_entry.rs`'s own real
    /// gap needed both a `readonly` modifier (every structural type this
    /// file builds is `readonly`-qualified, e.g. `{ readonly cron: string;
    /// readonly scheduledTime: number }`) and a method-signature member
    /// (`ack(): void`, `retry(): void`, `waitUntil(promise: Promise<
    /// unknown>): void`), neither representable by one more anonymous
    /// tuple `bool` — a fourth positional `bool` next to `optional` would
    /// be genuinely ambiguous at call sites (`readonly` vs. `optional`, ordered
    /// how?), the same reasoning [`TsObjectEntry`] already used over a raw
    /// tuple for the value-position sibling.
    Object(Vec<TsTypeMember>),
    /// `(a0: T0, a1: T1, …) => Ret` — a function type. Parameters carry no
    /// name of their own (just their type); the printer numbers them
    /// positionally (`a0`, `a1`, …), matching the exact convention
    /// `bynk-emit`'s own pre-P7.9 `ts_type_ref_with`/`ts_ty` already used
    /// (TypeScript requires *some* name in function-type syntax, and
    /// nothing about a `TypeRef::Fn`/`Ty::Fn` parameter carries a real one
    /// to use instead). A zero-`params` `Fn` is the query-thunk wrapper
    /// shape, `() => Ret`. Inside a [`TsType::Array`] the printer
    /// parenthesises it (`readonly ((a0: T) => U)[]`); see that variant's
    /// own doc.
    Fn {
        params: Vec<TsType>,
        ret: Box<TsType>,
    },
    /// `A | B | C` — a type-position union. Added in review of #1315:
    /// `bynk-emit`'s `ts_ty` builds a real union type for a resolved
    /// multi-actor sum (`Ty::ActorSum`, discriminated-union members tagged
    /// by actor name), a shape none of `Named`/`Array`/`Object`/`Fn` can
    /// represent — a real, grounded gap the same way `readonly`/`Fn`
    /// themselves were (P7.9's own accepted proposal), not a speculative
    /// addition. Each member prints through the ordinary `render_type`
    /// recursion; a member that is itself a `Union` is legal to construct
    /// but nothing in `bynk-emit` builds one today. Inside a
    /// [`TsType::Array`] the printer parenthesises it (`(A | B)[]`); see that
    /// variant's own doc.
    ///
    /// `multiline` (#1339): `false` (via [`TsType::union`]) is this variant's
    /// original single-line `A | B | C` form, unchanged. `true` (via
    /// [`TsType::multiline_union`]) is `emit_sum_type`'s own real
    /// discriminated-union shape — one variant per line, a leading `|` on
    /// every line *except* the first (which gets equivalent spacing
    /// instead), the closing `;` appended directly to the last variant's own
    /// line. Mirrors [`TsExpr::Object`]'s own `multiline` field precedent
    /// (#1317), but — unlike that one — needs no depth-aware wrapper: the
    /// pre-conversion `writeln!` code this reproduces always used a fixed
    /// two-space indent regardless of nesting (this shape is only ever a
    /// top-level `export type` alias body, never nested inside another
    /// type), so `render_type` stays entirely depth-unaware here too. Only
    /// rendered correctly from [`TsDecl::TypeAlias`]'s own top-level
    /// render — not reachable, and not given a defined rendering, from any
    /// nested position (an array element, a type argument, …), the same
    /// named boundary [`TsExpr::Object`]'s own `multiline` doc already
    /// draws for its own nested case.
    Union {
        members: Vec<TsType>,
        multiline: bool,
    },
    /// `A & B` — a type-position intersection. #1339's own real gap:
    /// `emit_refined_type`'s own branded-type alias,
    /// `{base} & { readonly __brand: "..." }`, has no representation among
    /// `Named`/`Array`/`Object`/`Fn`/`Union` — mirrors `Union`'s own
    /// shape/precedent exactly (a flat `Vec`, each member printed through
    /// the ordinary `render_type` recursion, joined by ` & `), single-line
    /// only (nothing in `bynk-emit` builds a multi-line intersection).
    Intersection(Vec<TsType>),
}

impl TsType {
    /// A plain named type with no type arguments — `string`, `unknown`,
    /// `Request`, …
    pub fn named(name: impl Into<String>) -> Self {
        TsType::Named {
            name: name.into(),
            type_args: Vec::new(),
        }
    }

    /// A generic named type — `Record<K, V>`, `Promise<T>`, …
    pub fn named_with_args(name: impl Into<String>, type_args: Vec<TsType>) -> Self {
        TsType::Named {
            name: name.into(),
            type_args,
        }
    }

    /// `T[]` — the non-`readonly` array shape.
    pub fn array(element: TsType) -> Self {
        TsType::Array {
            element: Box::new(element),
            readonly: false,
        }
    }

    /// `readonly T[]`.
    pub fn readonly_array(element: TsType) -> Self {
        TsType::Array {
            element: Box::new(element),
            readonly: true,
        }
    }

    /// `A | B | C` — the original single-line union form, unchanged.
    pub fn union(members: Vec<TsType>) -> Self {
        TsType::Union {
            members,
            multiline: false,
        }
    }

    /// `emit_sum_type`'s own real multi-line discriminated-union shape — see
    /// [`TsType::Union`]'s own doc for the exact rendering rules and why
    /// this needs no depth parameter.
    pub fn multiline_union(members: Vec<TsType>) -> Self {
        TsType::Union {
            members,
            multiline: true,
        }
    }

    /// `A & B` — an intersection type.
    pub fn intersection(members: Vec<TsType>) -> Self {
        TsType::Intersection(members)
    }
}

/// One member of a [`TsType::Object`] structural type — a property (`Prop`)
/// or a method signature (`Method`, #1323's own real gap: `ack(): void`,
/// `retry(): void`, `waitUntil(promise: Promise<unknown>): void` — no body,
/// a type-position sibling to [`TsObjectEntry::Method`], which does carry
/// one). `Method`'s own parameters reuse [`TsParam`] directly (not a bare
/// `Vec<TsType>` the way [`TsType::Fn`]'s anonymous, positionally-numbered
/// parameters do) — `waitUntil`'s own real parameter has a real name
/// (`promise`) the printed text must show, unlike `Fn`'s callers, none of
/// which have one to show.
#[derive(Debug, Clone)]
pub enum TsTypeMember {
    Prop {
        name: String,
        ty: TsType,
        optional: bool,
        readonly: bool,
    },
    /// `generics`/`doc` added by #1357: `emit_capability`'s own interface
    /// methods are genuinely generic (`op<T>(...): ret;`, no
    /// monomorphisation) and doc-commented — bare generic names, matching
    /// every other real generics-list precedent in this crate; `doc`
    /// mirrors `TsObjectEntry::Method.doc`'s own identical field (#1337).
    /// Both default empty/`None` via [`TsTypeMember::method`]'s own
    /// existing constructor — every one of its 6 real pre-#1357 callers is
    /// unaffected.
    ///
    /// `doc` renders from exactly one of this variant's two reachable
    /// positions: `TsDecl::Interface`'s own render arm (a real, multi-line
    /// declaration body with `depth` available, so it calls
    /// `render_doc_comment` before a documented member's own line — the
    /// only place `doc` is honoured). A `Method` reached through
    /// `TsType::Object`'s own inline, single-line shape (a type-position
    /// object literal, e.g. `{ a: X; b(): Y }`) has no line budget for a
    /// JSDoc block at all — `doc: Some(_)` there is a real, `debug_assert`-
    /// guarded misuse (`render_type`'s own `TsType::Object` arm), the same
    /// "loud, not silently dropped" precedent review of #1338 already
    /// established for `render_object_entry_inline`'s identical
    /// `TsObjectEntry::Method.doc` case.
    Method {
        name: String,
        generics: Vec<String>,
        params: Vec<TsParam>,
        ret: TsType,
        doc: Option<String>,
    },
    /// `[key_name: key_ty]: value_ty` — a TypeScript index signature.
    /// #1323's own real gap: `workers_entry.rs`'s multi-param `on call`
    /// dispatch casts its raw JSON args object through `{ [k: string]:
    /// JsonValue }` before indexing it by field name — textually distinct
    /// from the semantically-equivalent `Record<string, JsonValue>` (a
    /// `Named` type with type arguments), which `bynk-emit`'s own
    /// pre-conversion `writeln!` never wrote here. The one real site.
    Index {
        key_name: String,
        key_ty: TsType,
        value_ty: TsType,
    },
}

impl TsTypeMember {
    /// `name: ty` — the ordinary, non-`optional`, non-`readonly` case.
    pub fn prop(name: impl Into<String>, ty: TsType) -> Self {
        TsTypeMember::Prop {
            name: name.into(),
            ty,
            optional: false,
            readonly: false,
        }
    }

    /// `name?: ty`.
    pub fn optional_prop(name: impl Into<String>, ty: TsType) -> Self {
        TsTypeMember::Prop {
            name: name.into(),
            ty,
            optional: true,
            readonly: false,
        }
    }

    /// `readonly name: ty`.
    pub fn readonly_prop(name: impl Into<String>, ty: TsType) -> Self {
        TsTypeMember::Prop {
            name: name.into(),
            ty,
            optional: false,
            readonly: true,
        }
    }

    /// `name(params): ret` — no body, no generics, no doc comment.
    pub fn method(name: impl Into<String>, params: Vec<TsParam>, ret: TsType) -> Self {
        TsTypeMember::Method {
            name: name.into(),
            generics: Vec::new(),
            params,
            ret,
            doc: None,
        }
    }

    /// `[key_name: key_ty]: value_ty`.
    pub fn index(key_name: impl Into<String>, key_ty: TsType, value_ty: TsType) -> Self {
        TsTypeMember::Index {
            key_name: key_name.into(),
            key_ty,
            value_ty,
        }
    }
}

/// One function/method/constructor parameter.
#[derive(Debug, Clone)]
pub struct TsParam {
    pub name: String,
    pub ty: Option<TsType>,
    /// `name?: ty` — `events_fanout.rs`'s own `env?: unknown` constructor
    /// parameter needs this; nothing in the grounding file needs a default
    /// value, so only optionality is represented, not defaults.
    pub optional: bool,
}

/// One `class` field.
#[derive(Debug, Clone)]
pub struct TsClassField {
    pub name: String,
    pub ty: TsType,
    /// `private` — the one visibility modifier `events_fanout.rs` uses
    /// (`private env: ...`). Not a decorator or a constructor parameter
    /// property (R7.1 forbids both categorically — there is no variant
    /// shape here that could construct either).
    pub private: bool,
}

/// A class's own constructor.
#[derive(Debug, Clone)]
pub struct TsClassCtor {
    pub params: Vec<TsParam>,
    pub body: Vec<TsStmt>,
}

/// One class method.
#[derive(Debug, Clone)]
pub struct TsClassMethod {
    pub name: String,
    /// `private {name}(...)`, e.g. `loadState`/`commitState` — the
    /// grounding pass's own predicted gap (#1366), closed by Arc C's own
    /// `emit_agent` class-scaffold slice: every real `TsClassMethod` site
    /// before this one (`emit_provider` #1359's own op methods) was
    /// public-only. Rendered before `async`, matching the one real site's
    /// own modifier order (`private async loadState()`, not `async private
    /// loadState()`).
    pub private: bool,
    pub is_async: bool,
    pub params: Vec<TsParam>,
    pub return_type: Option<TsType>,
    /// A JSDoc block immediately preceding the method — the grounding
    /// pass's own second predicted gap (#1366), closed alongside `private`'s
    /// own sibling site: `emit_agent`'s own per-handler methods each carry
    /// a doc comment (`emit_doc_block`'s pre-conversion standalone call),
    /// the same need `TsObjectEntry::Method.doc` (#1337) and
    /// `TsTypeMember::Method.doc` (#1357) already solved for their own node
    /// kinds. Rendered the identical way those two already are — a
    /// `TsStmtKind::DocComment`-shaped block at the method's own indent,
    /// immediately before its header line.
    pub doc: Option<String>,
    pub body: Vec<TsStmt>,
}

/// A top-level declaration. `Import`, `Export`, `Interface`, `ConstDecl`,
/// and `Class` were `events_fanout.rs`'s own grounding (P7.8's own note:
/// "not the sketch's `Function`/`TypeAlias` — unused in the grounding
/// file"). #1321 (`workers.rs`) needed both after all — a real gap the
/// accepted proposal's own Framing didn't name (its own "no other gap
/// surfaced" checked `TsStmt`/`TsExpr`/`TsType` shapes, not `TsDecl` ones):
/// `compose.ts`'s own `export function compose(env: Env, …) { … }` is a
/// top-level function declaration, and (when the Worker has agents or
/// publishes events) a `type DurableObjectNamespace = { … };` fallback
/// alias sits alongside it. Named explicitly as a deviation from the
/// accepted proposal's own catalogue, not silently added — both are
/// mechanical, direct `TsDecl` siblings of `ConstDecl`/`Class` already
/// here, the same "small, grounded, same class of gap" this track's own
/// history repeatedly found and closed (P7.8's `Assign`/`Continue`/
/// `TryCatch`, Arc C slice 1's `Comment`).
#[derive(Debug, Clone)]
pub enum TsDecl {
    /// `import { a, b } from "spec";` (`type_only: false`) or
    /// `import type { a, b } from "spec";` (`type_only: true`). Only the
    /// named-imports form — `events_fanout.rs` never uses a default
    /// import, so none is represented. `names` entries are pushed verbatim
    /// (`printer.rs`'s own `names.join(", ")`), so a renamed import
    /// (`__messagesLocales as __locale_declaredLocales`, `workers.rs`'s own
    /// locale-negotiation import) is one opaque `String` entry, not a
    /// structured rename — the same "the field is already a raw-text slot"
    /// reasoning that also covers a `type`-prefixed single specifier
    /// (`"type KVNamespace"`) inside an otherwise non-`type_only` import.
    Import {
        type_only: bool,
        names: Vec<String>,
        from: String,
    },
    /// `import * as alias from "spec";` — a namespace import, structurally
    /// different from [`TsDecl::Import`]'s braced named-imports form (no
    /// `{ }`, one bound name for the whole module). #1321's own real gap:
    /// `workers.rs`'s `compose.ts` imports `handlers.js` and each
    /// referenced unit's binding module this way; `events_fanout.rs` never
    /// used one. `type_only` (Arc C, step (10), #1392) mirrors
    /// [`TsDecl::Import`]'s own identical field, a parallel gap by
    /// omission, not deliberate design — `emit_cross_context_namespace_
    /// imports`'s own real `import type * as ns from "...";` form (a
    /// Workers-mode consumed-context import reaching the callee's types
    /// only, #661) had no way to represent the `type` keyword until now.
    ImportNamespace {
        type_only: bool,
        alias: String,
        from: String,
    },
    /// `import name from "spec";` — a default import, structurally
    /// different from both [`TsDecl::Import`] (braced named-imports form)
    /// and [`TsDecl::ImportNamespace`] (`* as` form) — no braces, no `* as`,
    /// one bound local name for the module's own default export. No
    /// `type_only` form — nothing in the grounding file default-imports a
    /// type. Arc C, slice 37 (#1409, `tests_emit.rs`'s own slice G): a
    /// per-participant `import worker_{ns} from "../workers/{dir}/
    /// index.js";` (the participant's own Worker entry module's default
    /// export) is the first real default import anywhere in `bynk-emit`'s
    /// own converted content — every prior import site is either named
    /// ([`TsDecl::Import`]) or namespace ([`TsDecl::ImportNamespace`]).
    ImportDefault { alias: String, from: String },
    /// `export { a, b } from "spec";` — a re-export, structurally distinct
    /// from both [`TsDecl::Import`] (which binds locally, carries no
    /// `export` keyword) and [`TsDecl::Export`] (which wraps a whole
    /// declaration, not a braced name list re-pointed at another module).
    /// #1323's own real gap: `workers_entry.rs` re-exports each agent's
    /// Durable Object class from `./handlers.js`, and (when the context
    /// publishes events) the fan-out DO's class from `./events_fanout.js`.
    /// No `type_only` form — nothing in the grounding file re-exports a
    /// type-only name.
    ReExport { names: Vec<String>, from: String },
    /// `export { a, b };` — exports names already bound in this module (an
    /// import's, typically), with no `from` clause, which [`TsDecl::ReExport`]
    /// always carries. A Workers context re-exports the boundary codecs it
    /// imports from a `uses`d commons this way. #1820: a structured variant,
    /// not opaque text, so [`TsStmt::exported_names`] reports what the module
    /// exports to a barrel that must find the names two modules both export.
    ExportNames { names: Vec<String> },
    /// `export * from "spec";` — a wildcard re-export, structurally distinct
    /// from [`TsDecl::ReExport`] (which always carries a braced name list —
    /// an empty `names` there would render the ill-formed `export {  }
    /// from "spec";`, not this). Mirrors [`TsDecl::ImportNamespace`]'s own
    /// "no braces, no name list, one bound target" shape, on the export
    /// side. #1329's own real gap: `emit_commons_barrel`
    /// (`bynk-emit/src/project/tests_emit.rs`) builds a multi-file
    /// `commons` unit's barrel module as one `export *` line per
    /// constituent source file.
    ReExportAll { from: String },
    /// Marks the wrapped declaration `export`ed — `export class Foo { .. }`
    /// is `Export(Box::new(Class { .. }))`. A wrapper, not a per-variant
    /// `exported: bool` field, matching the reference sketch's own naming
    /// (`TsDecl::Export` is a peer variant, not a modifier on each other
    /// one).
    Export(Box<TsDecl>),
    /// `type_params`/per-member `readonly` (#1339's own real gap):
    /// `emit_record_type`'s own `export interface {name}{params} {
    /// readonly {field}: {ty}; ... }` — bare generic names (matching
    /// `ts_type_params`'s/`TsObjectEntry::Method.generics`'s own
    /// convention) and a `readonly` modifier every real field here
    /// carries. `members` reuses [`TsTypeMember`] (the exact same shape
    /// [`TsType::Object`]'s own structural members already use) rather
    /// than a bespoke tuple, since `readonly` is already that type's own
    /// field — this interface's real content has never needed
    /// `Method`/`Index`, but nothing about reusing the shared type
    /// forecloses a future slice that does.
    Interface {
        name: String,
        type_params: Vec<String>,
        members: Vec<TsTypeMember>,
    },
    /// A top-level `const` — distinct from `TsStmtKind::Const` (private —
    /// reachable through [`TsStmt::const_stmt`]), the local
    /// form.
    ConstDecl {
        name: String,
        ty: Option<TsType>,
        init: TsExpr,
    },
    Class {
        name: String,
        fields: Vec<TsClassField>,
        constructor: Option<TsClassCtor>,
        methods: Vec<TsClassMethod>,
    },
    /// `function name(params): ret { body }` (`is_async: false`) or `async
    /// function name(params): ret { body }` (`is_async: true`) — a top-level
    /// function declaration. `is_async` added by #1325: `workers.rs`'s own
    /// one real site (`compose`) is never async, so the field didn't exist
    /// until `emit_test_main`'s own top-level `async function main() {...}`
    /// needed it — the exact "extend narrowly, add it when a future slice's
    /// own grounding needs it" deferral this variant's own history already
    /// named. `generics` added by #1351: `emit_free_fn`'s own v0.20a erased
    /// generics (`export function foo<T>(...)`) — bare names only, empty
    /// for every non-generic site (`compose`/`main`), matching every other
    /// real generics-list precedent in this crate
    /// (`TsObjectEntry::Method.generics` #1337, `TsExpr::Arrow.generics`
    /// #1339). `inline` added by #1369 (Arc C, slice 20, step (9)'s own
    /// second sub-slice): `emit_agent`'s own zero-factory function
    /// (`function __zeroOf{Name}State(): {Name}State { return {...}; }`) is
    /// a genuinely single-physical-line declaration — braces and body share
    /// the header's own line, not `render_block_stmts`'s always-multi-line
    /// shape. `false` (every site landed before #1369) renders the ordinary
    /// multi-line body; `true` reuses `render_inline_block`'s own compact
    /// `{ stmt; stmt; }` shape directly at the declaration's own header
    /// line, the same "one more bool, mirroring an existing precedent"
    /// scope `TsObjectEntry::Method.inline` (#1337) already set for the
    /// identical single-line-vs-multi-line tension at a different node kind.
    Function {
        name: String,
        generics: Vec<String>,
        params: Vec<TsParam>,
        return_type: Option<TsType>,
        body: Vec<TsStmt>,
        is_async: bool,
        inline: bool,
    },
    /// `type name = ty;` (non-generic) or `type name<T, U> = ty;` (via
    /// `type_params`, #1339's own real gap: `emit_sum_type`'s own generic
    /// sum types erase to `export type Foo<T> = ...`) — a top-level type
    /// alias. `workers.rs`'s own real site: the `DurableObjectNamespace`
    /// local fallback type (emitted only when the Worker has agents or
    /// publishes events), never generic — `type_params` is empty there.
    TypeAlias {
        name: String,
        type_params: Vec<String>,
        ty: TsType,
    },
    /// `export default <expr>;` — a default export of an *expression*, not
    /// a declaration. #1323's own real gap (`workers_entry.rs`'s own
    /// top-level shape, `export default { fetch, scheduled?, queue? }`):
    /// [`TsDecl::Export`] wraps a [`TsDecl`] (a `Class`/`ConstDecl`/etc.,
    /// something with its own name), which cannot represent exporting a
    /// bare, unnamed object-literal expression — so this is its own
    /// variant, not `Export(Box::new(ConstDecl { .. }))` with a synthetic
    /// name that would print wrong.
    ExportDefault(TsExpr),
    /// `declare const name: ty;` — an ambient binding with no initialiser,
    /// narrowing a global TypeScript otherwise doesn't know about. #1325's
    /// own real, narrow site: `emit_test_main`'s own `declare const process:
    /// { exit(code: number): never; env: { [k: string]: string | undefined
    /// } };`, narrowing Node's `process` global without a `@types/node`
    /// dependency. Distinct from [`TsDecl::ConstDecl`] (always has a real
    /// `init` expression) — a `declare const` has none, by definition; a
    /// `TsDecl::ConstDecl` with a synthetic placeholder `init` would print
    /// wrong (`= <something>;` where nothing should appear at all).
    DeclareConst { name: String, ty: TsType },
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_carries_its_own_span() {
        let span = Span::new(3, 8);
        let stmt = TsStmt::verbatim(VerbatimOrigin::RuntimeUse, "x", Some(span));
        assert_eq!(stmt.span, Some(span));
    }

    #[test]
    fn a_program_prints_its_statements_in_push_order() {
        // Kept as a construction-order check (not a print check — printer.rs
        // owns that) since `TsStmtKind` is `pub(crate)` and no longer
        // exposes a uniform `text()` accessor once non-`Verbatim` kinds
        // exist; span order is what's left to check at this layer.
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(VerbatimOrigin::Contracts, "a", None));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            "b",
            Some(Span::new(0, 1)),
        ));
        assert_eq!(program.stmts[0].span, None);
        assert_eq!(program.stmts[1].span, Some(Span::new(0, 1)));
    }

    /// Review of #1326, finding 1: `TsExpr::template_lit`'s own
    /// `debug_assert_eq!` must actually fire on a malformed tree, not just
    /// exist as documentation — proves the guard guards, the same "prove
    /// it" discipline this crate applies to every other invariant.
    #[test]
    #[should_panic(expected = "parts.len() must be exprs.len() + 1")]
    fn template_lit_rejects_a_parts_exprs_count_mismatch() {
        let _ = TsExpr::template_lit(
            vec!["a".to_string()],
            vec![TsExpr::Ident("too_many".to_string())],
        );
    }
}
