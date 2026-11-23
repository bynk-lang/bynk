use super::{TsDecl, TsExpr, TsType, VerbatimOrigin};
use crate::source_map::SourceMapBuilder;
use bynk_syntax::span::Span;

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
#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
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
        /// `for await (const <binding> of <iter>)` (1A.35a part B: the `Stream`
        /// generators drain an async iterable).
        is_await: bool,
    },
    /// `while (<cond>) <body>` (1A.35a part B). `body` renders as `If`'s and
    /// `ForOf`'s do: braced when it is a [`TsStmtKind::Block`], inline otherwise.
    While {
        cond: TsExpr,
        body: Box<TsStmt>,
    },
    /// `<label>: <body>` (1A.35a part B): a labelled statement, the target of a
    /// labelled [`TsStmtKind::Continue`] or [`TsStmtKind::Break`]. `body` is
    /// usually a loop or a [`TsStmtKind::Block`]; any statement is valid.
    Labelled {
        label: String,
        body: Box<TsStmt>,
    },
    /// `let a, b: T, c = e;` (1A.35a part B): one `let` with several
    /// declarators, each optionally typed, definitely assigned or initialised.
    /// [`TsStmtKind::Let`] stays the form for a destructuring pattern.
    LetList(Vec<TsDeclarator>),
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
    ///
    /// 1A.21 (#3722, Decision C): `catch_block` is optional and a `finally_block` joins
    /// it, so `try { … } finally { … }` (no `catch`) and `try { … } catch (e) { … }
    /// finally { … }` are expressible. At least one of the two must be present (a bare
    /// `try` is a syntax error); the printer asserts it. `catch_param` is meaningful only
    /// with a `catch_block`.
    TryCatch {
        try_block: Box<TsStmt>,
        catch_param: Option<String>,
        catch_block: Option<Box<TsStmt>>,
        finally_block: Option<Box<TsStmt>>,
    },
    /// `{ <stmts> }` — the body container for `If`/`ForOf`/`TryCatch`/
    /// constructor and method bodies.
    Block(Vec<TsStmt>),
    /// A bare `continue;` — a second real gap beyond the accepted
    /// proposal's own variant list: `events_fanout.rs` uses it twice
    /// (`if (!Array.isArray(subs)) continue;` / `if (!binding) continue;`),
    /// load-bearing loop control the accepted proposal's `TsStmt` list
    /// doesn't name. No label — nothing in the grounding file needs one.
    ///
    /// 1A.35a part B: `continue <label>;` when it carries a label, which must
    /// name an enclosing [`TsStmtKind::Labelled`] loop.
    Continue(Option<String>),
    /// A `break;`, ending a `switch` case (1A.32b: the history driver's
    /// `case 0: …; break;`), or `break <label>;` out of a
    /// [`TsStmtKind::Labelled`] statement (1A.35a part B).
    Break(Option<String>),
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
    /// (`// Generated by bynk — do not edit by hand.` / a file-specific
    /// second line), and nothing before this slice could represent one.
    /// Not speculative — a universal need every later Arc C slice hits on
    /// its own first line, closed once here rather than left as a residual
    /// `Verbatim` wrap that would make `verbatim_sites` show zero
    /// improvement for real, substantial conversion work.
    ///
    /// Carries no semantic content the printer or checker ever reads back —
    /// pure, inert text. Printed one `// `-prefixed line per `\n` in `text`
    /// (`bynk-ts/src/printer/stmt.rs`'s own `render_stmt`), so a multi-line
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
    ///
    /// The second field (1A.36a, Decision D) lists the names the fixed text
    /// uses, declared where the text is written; empty for a plain
    /// [`TsStmt::raw`]. `name_closure` counts them as uses.
    Raw(String, Vec<String>),
}

/// One declarator of a `TsStmtKind::LetList`: `name`, `name!: T`, `name: T` or
/// `name: T = init` (1A.35a part B). `definite` prints the `!` assertion, which
/// TypeScript allows only with a type and no initialiser.
#[derive(Debug, Clone, PartialEq)]
pub struct TsDeclarator {
    pub name: String,
    pub definite: bool,
    pub ty: Option<TsType>,
    pub init: Option<TsExpr>,
}

impl TsDeclarator {
    /// `name`, declared and not yet assigned.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            definite: false,
            ty: None,
            init: None,
        }
    }

    /// `name: ty`.
    #[must_use]
    pub fn typed(mut self, ty: TsType) -> Self {
        self.ty = Some(ty);
        self
    }

    /// `name!: ty`: a definite-assignment assertion, for a variable assigned
    /// where the compiler cannot see it.
    #[must_use]
    pub fn definite(mut self, ty: TsType) -> Self {
        self.definite = true;
        self.ty = Some(ty);
        self
    }

    /// `name = init`.
    #[must_use]
    pub fn init(mut self, init: TsExpr) -> Self {
        self.init = Some(init);
        self
    }
}

/// One `case`/`default` arm of a `TsStmtKind::Switch`. empty `labels` is the
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
/// it has a label (a `default` case's bracing is `default_braced`'s
/// business, not this field's — the inverse of that field's own scoping).
#[derive(Debug, Clone, PartialEq)]
pub struct TsSwitchCase {
    /// The `case` labels: empty is `default`; more than one prints stacked,
    /// `case a:` / `case b: {`, sharing the one body (1A.13, #3714: the
    /// or-pattern's shape, widened from the single `test` it replaced).
    pub labels: Vec<TsExpr>,
    pub body: Vec<TsStmt>,
    pub default_braced: bool,
    pub case_braced: bool,
    /// The source span recorded by `print_stmt_with_spans` at the start of the
    /// case's first label line (1A.13, #3714). Plain printing ignores it.
    pub span: Option<Span>,
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
                TsDecl::Import { .. }
                | TsDecl::ImportNamespace { .. }
                | TsDecl::ImportDefault { .. }
                | TsDecl::ReExport { .. }
                | TsDecl::ExportNames { .. }
                | TsDecl::ReExportAll { .. }
                | TsDecl::Export(_)
                | TsDecl::ExportDefault(_)
                | TsDecl::DeclareConst { .. } => Vec::new(),
            },
            TsDecl::Import { .. }
            | TsDecl::ImportNamespace { .. }
            | TsDecl::ImportDefault { .. }
            | TsDecl::ReExportAll { .. }
            | TsDecl::Interface { .. }
            | TsDecl::ConstDecl { .. }
            | TsDecl::Class { .. }
            | TsDecl::Function { .. }
            | TsDecl::TypeAlias { .. }
            | TsDecl::ExportDefault(_)
            | TsDecl::DeclareConst { .. } => Vec::new(),
        }
    }

    /// This statement with its `export` modifier dropped, when it is an exported declaration
    /// ([`TsDecl::Export`]); any other statement is returned as it was. For a declaration that
    /// moves into a function body, where `export` is not legal.
    #[must_use]
    pub fn without_export(mut self) -> Self {
        if let TsStmtKind::Decl(TsDecl::Export(inner)) = self.kind {
            self.kind = TsStmtKind::Decl(*inner);
        }
        self
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
                is_await: false,
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
                catch_block: Some(Box::new(catch_block)),
                finally_block: None,
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// `try <try_block> finally <finally_block>`, with no `catch` (1A.21, #3722).
    pub fn try_finally(try_block: TsStmt, finally_block: TsStmt, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::TryCatch {
                try_block: Box::new(try_block),
                catch_param: None,
                catch_block: None,
                finally_block: Some(Box::new(finally_block)),
            },
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// `try <try_block> catch (<catch_param>) <catch_block> finally <finally_block>`
    /// (1A.21, #3722).
    pub fn try_catch_finally(
        try_block: TsStmt,
        catch_param: Option<impl Into<String>>,
        catch_block: TsStmt,
        finally_block: TsStmt,
        span: Option<Span>,
    ) -> Self {
        Self {
            kind: TsStmtKind::TryCatch {
                try_block: Box::new(try_block),
                catch_param: catch_param.map(Into::into),
                catch_block: Some(Box::new(catch_block)),
                finally_block: Some(Box::new(finally_block)),
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

    pub fn break_stmt(span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Break(None),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    pub fn continue_stmt(span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Continue(None),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// `break <label>;`.
    pub fn break_label_stmt(label: impl Into<String>, span: Option<Span>) -> Self {
        Self::plain(TsStmtKind::Break(Some(label.into())), span)
    }

    /// `continue <label>;`.
    pub fn continue_label_stmt(label: impl Into<String>, span: Option<Span>) -> Self {
        Self::plain(TsStmtKind::Continue(Some(label.into())), span)
    }

    /// `while (<cond>) <body>`.
    pub fn while_stmt(cond: TsExpr, body: TsStmt, span: Option<Span>) -> Self {
        Self::plain(
            TsStmtKind::While {
                cond,
                body: Box::new(body),
            },
            span,
        )
    }

    /// `<label>: <body>`.
    pub fn labelled(label: impl Into<String>, body: TsStmt, span: Option<Span>) -> Self {
        Self::plain(
            TsStmtKind::Labelled {
                label: label.into(),
                body: Box::new(body),
            },
            span,
        )
    }

    /// `let a, b: T, c = e;`.
    pub fn let_list(decls: Vec<TsDeclarator>, span: Option<Span>) -> Self {
        debug_assert!(!decls.is_empty(), "TsStmt::let_list needs a declarator");
        Self::plain(TsStmtKind::LetList(decls), span)
    }

    /// `for await (const <binding> of <iter>) <body>`.
    pub fn for_await_of(
        binding: impl Into<String>,
        iter: TsExpr,
        body: TsStmt,
        span: Option<Span>,
    ) -> Self {
        Self::plain(
            TsStmtKind::ForOf {
                binding: binding.into(),
                iter,
                body: Box::new(body),
                is_await: true,
            },
            span,
        )
    }

    /// A statement of `kind` with no source-map extras.
    fn plain(kind: TsStmtKind, span: Option<Span>) -> Self {
        Self {
            kind,
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

    /// The text of a `Raw` statement, `None` for every other kind.
    pub fn raw_text(&self) -> Option<&str> {
        if let TsStmtKind::Raw(text, _) = &self.kind {
            Some(text)
        } else {
            None
        }
    }

    /// The one constructor for a `Raw`-kinded statement — `text` is printed
    /// verbatim, exactly as given (see `TsStmtKind::Raw`'s own doc for
    /// why this is a distinct kind from `Verbatim`, not a reuse of it).
    pub fn raw(text: impl Into<String>, span: Option<Span>) -> Self {
        Self {
            kind: TsStmtKind::Raw(text.into(), Vec::new()),
            span,
            nested_map: None,
            no_blank_before: false,
            nested_map_source_id: 0,
        }
    }

    /// 1A.36a (Decision D): a `Raw` statement that also declares the names its
    /// fixed text uses. `text` prints exactly as [`TsStmt::raw`]'s does; `names`
    /// are the identifiers (types or values) the text refers to without
    /// declaring, which `name_closure` counts as uses. The names are written
    /// where the text is, never scanned out of it: a scan over-matches on
    /// string literals and comments and under-matches on formatting.
    pub fn raw_using(text: impl Into<String>, names: &[&str]) -> Self {
        Self {
            kind: TsStmtKind::Raw(
                text.into(),
                names.iter().map(|n| (*n).to_string()).collect(),
            ),
            span: None,
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
#[derive(Debug, Clone, PartialEq)]
pub enum TsBindingName {
    Ident(String),
    ObjectPattern(Vec<String>),
    /// An array-destructuring pattern (`const [a, b] = …`, back-half paydown
    /// 1A.14, for `par`'s join). An empty name is a hole (`[, b]`), which is how
    /// a `do` part binds nothing.
    ArrayPattern(Vec<String>),
}
