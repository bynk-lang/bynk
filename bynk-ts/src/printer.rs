//! The printer — `TsProgram -> Artefacts` (R7.3: "printing is `TsProgram ->
//! Artefacts`. The printer owns the buffer, the indentation and the offset
//! arithmetic. It is the only code in the compiler that writes a
//! character."). Owns one buffer for the whole [`TsProgram`], so unlike
//! `bynk-emit`'s own splice-based lowering (`crate::source_map`'s own module
//! doc), it never needs to rebase a sub-buffer's checkpoints — it records
//! directly from each statement's own `span` as it writes, which is what
//! makes R7.4 ("the source map is produced by the printer from
//! `TsNode.span`. No phase before the printer records an offset") true by
//! construction for this path, from this slice.
//!
//! # Readability policy (R7.5)
//!
//! R7.5: "Readable output is a printer policy with a name and a test, not a
//! property of how carefully strings were typed." P7.7 named the policy's
//! then-whole surface (statement separation); P7.8 (#1313) extends it now
//! that the printer has real structured nodes to have an opinion about
//! (`TsExpr`/`TsType`/`TsDecl`, plus real `TsStmt` variants):
//!
//! - **Every statement starts on its own generated line, so two statements
//!   can never share one.** Unchanged from P7.7 — see [`print()`]'s own
//!   loop. Still true for the new node kinds: every one of their own
//!   renderers ends its own output in `\n`.
//! - **Two-space indentation, one level per nesting depth.** The only
//!   indentation width this printer produces — chosen because it's the one
//!   `events_fanout.rs` (this slice's own grounding file) already uses, not
//!   because of any earlier general readability decision; matches the
//!   depth threaded through [`render_stmt`]/[`render_block_body`](stmt::render_block_body) exactly.
//! - **One blank line separates top-level declarations, except two
//!   consecutive `import`s or two consecutive `Comment`s.** [`print()`]'s
//!   own loop implements this by peeking at the next statement — matching
//!   `events_fanout.rs`'s own real spacing (its two `import` lines sit
//!   adjacent, and so do its own two header-comment lines; every other pair
//!   of top-level declarations has a blank line between). Added for Arc C's
//!   own first real conversion slice (#1317): every `bynk-emit`-generated
//!   file opens with a multi-line header banner, one [`TsStmtKind::Comment`]
//!   per real line.
//! - **[`TsStmt::no_blank_before`] suppresses that same blank immediately
//!   before the statement carrying it**, composing with every rule above
//!   rather than replacing any of them — a property of the *statement*, not
//!   of a pair, so it stays correct however many different kinds end up
//!   adjacent to it. Scoped to exactly [`print()`]'s own top-level loop: a
//!   statement carrying it inside a `Block`/`InlineBlock`/function body, or
//!   printed through [`print_stmt`] / `bynk-emit`'s own `extend_printed`
//!   splicing helpers, is silently ignored there today — harmless only
//!   because none of those paths inserts an automatic blank of its own in
//!   the first place. Added for #1486: `emit_agent`'s own already-shipped
//!   registry-`const`-then-zero-factory pairing needs zero blank lines
//!   between two adjacent real top-level statements, a shape no existing
//!   pair rule above can express without a `TsDecl`-pair special case.
//! - **Inside a `class`, no blank line between fields and the constructor;
//!   one blank line before each method.** [`render_decl_body`](decl::render_decl_body)'s own
//!   `TsDecl::Class` arm — again, this is what `events_fanout.rs` itself
//!   does, not a general rule derived some other way.
//! - **`if`/`for...of`'s own body prints with braces when it's a
//!   `TsStmtKind::Block`, and inline on the same line otherwise** —
//!   `if (!Array.isArray(subs)) continue;` has no braces in the grounding
//!   file; a `for...of` always does. See [`render_branch`](stmt::render_branch).
//! - **A statement's own interior is not the printer's concern for
//!   `Verbatim` content — still.** `Verbatim` text renders exactly as
//!   written (P7.7's own boundary, unchanged); everything above applies
//!   only to the new, real node kinds this slice adds.
//! - **A shorthand async method entry in an object literal
//!   (`TsObjectEntry::Method`) prints exactly like a class method** —
//!   `async name(params) { <body> },` with a trailing comma, body indented
//!   one level deeper than the entry itself, closing `},` back at the
//!   entry's own indent. Added for Arc C's own third slice (#1321,
//!   `workers.rs`): the `compose`-returned surface object's dominant real
//!   shape, one entry per wrapper.
//! - **`?.`/`?.[...]` print with no surrounding spaces** — `object?.property`/
//!   `object?.[index]`, matching plain `Member`/`Index`'s own spacing
//!   exactly, just with the extra `?`. Added for #1321: `workers.rs`'s own
//!   secret-probe idiom.
//! - **An optional field in a type-position object literal prints `key?:
//!   ty`** — the direct type-side counterpart to `TsParam.optional`'s
//!   already-established `name?: ty` for parameters. Added for #1321.
//! - **An arrow function prints `(params) => body` on one line, expression
//!   body only** — no block-body form exists yet (nothing built here needs
//!   one). Added for #1321.
//! - **A nested `Binary` operand of another `Binary` parenthesizes only
//!   when the inner operator's precedence is not strictly higher than the
//!   outer's, or when the same operator nests in itself and that operator
//!   is `||`/`&&`** (`render_binary_operand`) — every other operand
//!   position (`Member`/`Index`/`Call`/`New`/`Await`/`Unary`,
//!   `render_operand`) still always parenthesizes a nested `Binary`/`As`/
//!   `Arrow`/`Conditional`, unchanged. Added for #1321: `workers.rs`'s own
//!   `__authz === null || !__authz.startsWith(...)` has no parens around
//!   the comparison in the byte-golden fixtures, and `??` mixed with
//!   `||`/`&&` still always parenthesizes (TS forbids that combination
//!   unparenthesized). Extended for #1323: a 3+-term `||`/`&&` chain of the
//!   *same* operator (`typeof args !== "object" || args === null ||
//!   Array.isArray(args)`) prints flat, with no parens around the nested
//!   pair — `??` chained with itself keeps the pre-#1321 conservative
//!   choice (still parenthesized), since nothing real needs it flattened.
//! - **`switch (<discriminant>) { <cases> }` — a non-`default` `case` is
//!   always `{ }`-blocked; a `default` case never is.** Added for #1323:
//!   `workers_entry.rs`'s four real `switch` statements are the first real
//!   content this tree represents with a genuinely new statement-grouping
//!   construct, not a single expression/type variant.
//! - **`export default <expr>;` prints the expression through the same
//!   depth-aware multiline-object handling `const`/`let`/`return`/`Assign`
//!   already get.** Added for #1323: `workers_entry.rs`'s own top-level
//!   shape is a default-exported object literal.
//! - **`export { a, b } from "spec";` — a re-export — prints with no blank
//!   line rule of its own** (it is not classified alongside `import`/
//!   `import * as` for the "no blank line between adjacent imports"
//!   exception, so the ordinary "one blank line between top-level
//!   declarations" default applies both before and after it). Added for
//!   #1323: `workers_entry.rs`'s own agent/fan-out-DO re-exports, each
//!   already separated from its neighbours by a blank line in the
//!   pre-conversion output.
//! - **`test ? consequent : alternate` prints on one line, with `test`
//!   parenthesized only when it is itself an `Arrow` or a `Conditional`.**
//!   Added for #1323: `workers_entry.rs`'s own two real ternaries.
//! - **A `readonly`/method-signature/index-signature member of a
//!   type-position object literal prints `readonly name: ty` /
//!   `name(params): ret` / `[key: key_ty]: value_ty`.** Added for #1323:
//!   `workers_entry.rs`'s own `scheduled`/`queue` parameter types and its
//!   multi-param `on call` dispatch's `args as { [k: string]: JsonValue }`.
//! - **An explicit `Paren` always prints its own literal parentheses,
//!   independent of the wrapped expression's own precedence.** Added for
//!   #1323: `workers_entry.rs`'s CORS-preflight guard condition, which the
//!   pre-conversion code always wrapped in `(...)` even when nothing about
//!   the wrapped expression's own precedence required it.
//! - **A bare blank line (`Blank`) prints exactly one empty line**, usable
//!   at any nesting depth — distinct from `print()`'s own top-level-only
//!   blank-line policy above. Added for #1323: three specific points inside
//!   `workers_entry.rs`'s `fetch` method body.
//! - **An `if`'s own `else` prints on its own fresh line, at the `if`'s own
//!   indent, then follows the same block-vs-inline branch rule the `if`
//!   itself does.** Added for #1323: `workers_entry.rs`'s queue-consumer
//!   ack/retry dispatch.
//! - **`InlineBlock` prints `{ stmt; stmt; ... }` on one generated line**,
//!   distinct from `Block`'s always-multi-line form — reachable only as an
//!   `if`/`else` branch in real content today. Added for #1323:
//!   `workers_entry.rs`'s own two real sites (a deserialise-failure guard,
//!   and the ack/retry dispatch's `else`), both hand-written as compact
//!   one-liners in the pre-conversion text.
//! - **An array literal supports the same `multiline` shape an object
//!   literal does** — one item per line, each with its own trailing comma,
//!   closing `]` at the statement's own indent, only through
//!   `render_stmt_level_expr`. Added for #1325: `emit_test_main`'s own
//!   `modules` array.
//! - **A template literal's static parts print verbatim, with no escaping of
//!   their own.** Added for #1325 — see `TsExpr::TemplateLit`'s own doc for
//!   why (a generic escaper would double an already-pre-formed JS unicode
//!   escape's own literal backslash).
//! - **`declare const name: ty;` prints with no initialiser at all** —
//!   distinct from every other `const` form, which always has one. Added
//!   for #1325: `emit_test_main`'s own ambient `process` declaration.
//! - **An `async function` prints `async function name(...)`** — the
//!   top-level sibling to `TsObjectEntry::Method`'s own `is_async` handling.
//!   Added for #1325: `emit_test_main`'s own top-level `main`.
//! - **`<expr>++;` prints a bare postfix increment as a whole statement.**
//!   Added for #1325: `emit_test_main`'s own `passed++;`/`failed++;`
//!   counters.
//! - **An `if`'s own `else` prints `} else {` on the same physical line when
//!   `same_line_else` is set** (only reachable when `then_branch` is a
//!   `Block` or `InlineBlock`) — a second, real convention alongside the
//!   fresh-line default just above, not a replacement for it. Added for
//!   #1325: all three of `emit_test_main`'s own real `if`/`else` sites use
//!   this spacing, none use the fresh-line form `workers_entry.rs`'s own
//!   real content needs — two already-real files disagreeing on the same
//!   construct, the same tension the `Await`-under-`As` correction
//!   (#1323/#1324) found for parenthesisation.
//! - **`export * from "spec";` — a wildcard re-export — groups with itself
//!   AND with an immediately-preceding header `Comment`, no blank line
//!   either way.** A different spacing shape from `ReExport`'s own (no
//!   grouping rule, see above): `emit_commons_barrel`'s own real barrel
//!   module is one header comment followed by one `export *` line per
//!   constituent source file, every one of those lines adjacent with no
//!   blank line anywhere. Added for #1329.
//! - **A `TsStmtKind::DocComment` prints a JSDoc block comment**, one
//!   ` * <line>` per non-blank source line, a blank source line as a bare
//!   ` *`. Distinct from `Comment`'s own `//`-per-line form, and printed
//!   only through [`print_stmt`] (`bynk-emit`'s own `emit_doc_block`, a
//!   shared helper spliced into still-unconverted callers' buffers), never
//!   through [`print()`]'s own `TsProgram` loop — so this shape has no
//!   blank-line grouping rule of its own to name here. Added for #1333.
//! - **A `TsStmtKind::Raw` prints its own text verbatim** — no leading
//!   indent, no added punctuation, the same rendering `Verbatim` gets
//!   (deliberately a distinct kind, not a reuse of it — see
//!   [`crate::program::TsStmtKind::Raw`]'s own doc for why). Never reached
//!   through [`print()`]'s own `TsProgram` loop — no blank-line grouping
//!   rule of its own to name here either. Added for #1337:
//!   `emit_method`'s own body, delegated wholesale to `emitter/lower.rs`'s
//!   `emit_block_as_function_body_with_return` — a permanent Arc C
//!   exclusion (ADR `arc-c-lower-rs-permanent-exclusion`), not residue.
//!   #1339 added a second, differently-reasoned real use:
//!   `emit_refined_type`'s own `of()` guard body, carrying `emit_refined_
//!   checks`'s already-printed output — not a permanent exclusion, just
//!   scope that function's own conversion didn't reach. Both uses carry
//!   text pre-indented at a fixed absolute depth by their own caller, so
//!   `render_multiline_object_entry`'s own `debug_assert!` guards that this
//!   only renders correctly at `depth == 0` — see its doc and
//!   [`print_object_entry`]'s. **Reached through more than a
//!   `TsObjectEntry::Method`'s own `body` now** (review of #1370 caught
//!   this doc drifting stale): `emit_free_fn` (#1352) and `emit_agent`'s
//!   own rehydrate function (#1369) both hold `Raw` statements inside an
//!   ordinary `TsDecl::Function`'s own `body`, rendered through
//!   `render_block_stmts` at whatever `depth` the enclosing `print_stmt`
//!   call used (always 0 for a top-level declaration in every real site
//!   today, satisfying the same depth-0 assumption named above). Since
//!   #1369 also added `TsDecl::Function.inline`, a `Raw`-bearing function
//!   body reached with `inline: true` would route through
//!   `render_inline_block`/`render_compact_stmts` instead — see
//!   [`render_inline_stmt`](compact::render_inline_stmt)'s own `Raw` arm for why that combination stays
//!   deliberately unbuilt.
//!
//! None of the above is claimed as *the* TypeScript style this printer will
//! use forever — it's what this slice's own grounding file needs, named
//! rather than left implicit, the same posture the node algebra itself
//! takes (`program.rs`'s own module doc). A future file with a construct
//! this policy doesn't cover yet (an `else` branch, a multi-field class with
//! blank lines between fields, …) extends it the same way `program.rs`'s
//! own node list grows: file by file, against real content.

mod compact;
mod decl;
mod expr;
mod fragment;
mod multiline;
mod precedence;
mod stmt;
mod types;

#[cfg(test)]
mod tests;

use crate::program::{TsDecl, TsProgram, TsStmt, TsStmtKind};
use crate::source_map::SourceMapBuilder;
use bynk_syntax::span::Span;
pub use fragment::{
    print_class_method, print_class_method_and_merge, print_expr, print_object_entry, print_stmt,
    print_stmt_and_merge, print_stmt_with_spans, print_type,
};
use stmt::render_stmt;

/// The result of printing a [`TsProgram`]: the emitted text, and its source
/// map — `None` when no checkpoint resolved, either because no statement
/// carried a span or every span fell outside `source_text`
/// ([`SourceMapBuilder::to_v3`]'s own "nothing resolves" case).
#[derive(Debug)]
pub struct Printed {
    pub text: String,
    pub source_map: Option<String>,
}

/// Print `program` to TypeScript text. `source_name`/`source_text` register
/// the `.bynk` source every statement's own span is measured against —
/// today always exactly one, since nothing spans two files yet;
/// `output_file` names the generated file in the source map's own `file`
/// field. Every statement's own span is recorded as a checkpoint, at
/// any depth (1A.15; before that only a top-level statement's was, R7.4's
/// original scope) — except inside an arrow body, which carries none yet.
pub fn print(
    program: &TsProgram,
    source_name: &str,
    source_text: &str,
    output_file: &str,
) -> Printed {
    let mut map = SourceMapBuilder::new();
    map.add_source(source_name, source_text);
    print_with_map(program, map, output_file)
}

/// #1486's own multi-source sibling of [`print()`]: a test/integration
/// module's own aggregate map spans one source per fragment file a case's
/// body was spliced in from (`emit_test_module`/`emit_integration_module`,
/// `project/tests_emit.rs`), not the single `.bynk` file `print`'s own
/// signature assumes — see [`TsStmt::nested_map_source_id`]'s own doc for
/// why a per-statement source selector needs a caller that can register
/// more than one source in the first place. Takes the caller's own already-
/// built `map` directly (rather than a `(name, text)` list built fresh
/// here) so a caller can keep registering sources incrementally as it
/// discovers them — `emit_test_module`'s own per-case loop, one `add_source`
/// call per case, exactly as it already did before this function existed —
/// and get each call's own returned id back immediately, to set on that
/// case's own top-level statement's `nested_map_source_id`, instead of
/// having to pre-compute the full source list up front. Every statement's
/// own `nested_map_source_id` selects which of `map`'s already-registered
/// sources (by id) its own `nested_map` merges against; a statement with no
/// `span` of its own (this whole module's own shape — an aggregate with no
/// single "primary" source of its own) never records a top-level
/// checkpoint, so `map`'s own id-`0` source (if any) needs no special
/// "primary" meaning here the way [`print()`]'s single source does.
pub fn print_multi_source(
    program: &TsProgram,
    map: SourceMapBuilder,
    output_file: &str,
) -> Printed {
    print_with_map(program, map, output_file)
}

fn print_with_map(program: &TsProgram, mut map: SourceMapBuilder, output_file: &str) -> Printed {
    let mut out = String::new();
    for (i, stmt) in program.stmts.iter().enumerate() {
        // The statement's own span, and every nested statement's, are
        // recorded by `render_stmt` as it prints (1A.15, Decision B of #3716).
        // #1477: also merges any nested checkpoints this top-level
        // statement (or anything nested inside it) itself carries — the
        // same mechanism `print_stmt_and_merge`/`print_class_method_and_
        // merge` expose to a caller printing one fragment at a time,
        // applied here automatically for every top-level statement in a
        // real `TsProgram`. `source_id` defaults to `stmt.
        // nested_map_source_id`'s own default of `0`, matching `record`'s
        // own unconditional targeting of the primary source just above for
        // every single-source module; #1486's own multi-source case
        // (`nested_map_source_id`'s own doc comment) sets it per statement.
        render_stmt(
            &mut out,
            stmt,
            0,
            Some(MergeTarget {
                map: &mut map,
                source_id: stmt.nested_map_source_id,
            }),
        );
        // The printer owns line structure (R7.3), so a statement's own text
        // not ending in its own newline can't leave two statements sharing
        // a generated line — review of #1308, finding 2: nothing required
        // `Verbatim` text to be newline-terminated, and two that weren't
        // would jam onto one line *and* silently lose the earlier
        // statement's own checkpoint (`SourceMapBuilder::record`'s
        // same-offset dedup, and `to_v3`'s one-checkpoint-per-line forward
        // pass, both keep only the later one). Every real (non-`Verbatim`)
        // renderer already ends its own output in `\n`, so this is a no-op
        // for them; kept unconditional so `Verbatim`'s own guarantee stays
        // exactly as it was.
        if !out.ends_with('\n') {
            out.push('\n');
        }
        // Readability policy (this module's own doc): one blank line
        // between top-level declarations, except two consecutive
        // `import`s, two consecutive `Comment`s, two consecutive
        // `ReExportAll`s, a `Comment` immediately before a `ReExportAll`,
        // or the next statement's own `no_blank_before` (#1486's own
        // escape hatch — a real, already-shipped multi-statement sequence
        // that genuinely needs no blank between two adjacent real
        // statements, not expressible as a `TsStmtKind`-pair rule the way
        // every case above is) — and never after `Verbatim` (P7.7's own
        // boundary — `Verbatim` content's own spacing is not this
        // printer's decision).
        if let Some(next) = program.stmts.get(i + 1) {
            // #1321: `workers.rs`'s own header has `import { ... } from
            // "runtime"` (named) adjacent to `import * as handlers from
            // "./handlers.js"` (namespace) adjacent to one `import * as
            // {ns} from "..."` per referenced unit — all import-like, all
            // real-content-adjacent with no blank line, matching the same
            // "no blank line between two adjacent imports" rule
            // `events_fanout.rs`'s own two named imports already exercised.
            fn is_import_decl(kind: &TsStmtKind) -> bool {
                matches!(
                    kind,
                    TsStmtKind::Decl(TsDecl::Import { .. })
                        | TsStmtKind::Decl(TsDecl::ImportNamespace { .. })
                        | TsStmtKind::Decl(TsDecl::ImportDefault { .. })
                )
            }
            // #1329: `emit_commons_barrel`'s own real barrel module is one
            // header `Comment` immediately followed by an `export *` line
            // per constituent source file, every one of those lines
            // adjacent to the next with no blank line anywhere — a
            // genuinely different spacing shape from `ReExport`'s own
            // (#1323's `workers_entry.rs` re-exports, each already
            // separated from its neighbours by a blank line in the
            // pre-conversion output, so `ReExport` itself still gets no
            // grouping rule of its own). Scoped to exactly this new
            // adjacency — a `Comment` before an ordinary `Import`/other
            // decl still gets its blank line (see `events_fanout.rs`'s own
            // header-comment-then-blank-then-import shape), unchanged.
            fn is_reexport_all(kind: &TsStmtKind) -> bool {
                matches!(kind, TsStmtKind::Decl(TsDecl::ReExportAll { .. }))
            }
            let both_imports = is_import_decl(&stmt.kind) && is_import_decl(&next.kind);
            let both_comments = matches!(&stmt.kind, TsStmtKind::Comment(_))
                && matches!(&next.kind, TsStmtKind::Comment(_));
            let both_reexport_all = is_reexport_all(&stmt.kind) && is_reexport_all(&next.kind);
            let comment_then_reexport_all =
                matches!(&stmt.kind, TsStmtKind::Comment(_)) && is_reexport_all(&next.kind);
            if !both_imports
                && !both_comments
                && !both_reexport_all
                && !comment_then_reexport_all
                && !next.no_blank_before
                && !matches!(stmt.kind, TsStmtKind::Verbatim { .. })
            {
                out.push('\n');
            }
        }
    }
    let source_map = map.to_v3(&out, output_file);
    Printed {
        text: out,
        source_map,
    }
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

/// #1477's own carrier for a nested-checkpoint merge target: which map to
/// merge a [`crate::program::TsStmt::nested_map`]-bearing statement's own
/// checkpoints into, and which of that map's own registered sources they
/// belong to. A bare `&mut SourceMapBuilder` isn't enough on its own
/// (review of #1488, finding 1): hardcoding source `0` is correct for
/// `print`'s own single-primary-source map and for every real caller today
/// (`emit_free_fn`/`emit_class_method_and_merge_source_map` both already
/// pass `0`), but silently wrong for a multi-source aggregate map — a test
/// module's own map spans several `.bynk` files, and its real per-body
/// merges already pass a non-zero id (`bynk-emit/src/project/tests_emit.rs:
/// 537`/`:1936`) — exactly the shape #1475 found is this arc's own next
/// conversion target. Threading `source_id` alongside `map` here, not
/// hardcoded inside `SourceMapBuilder::merge`'s own caller here, keeps that
/// conversion correct from the start rather than needing a second pass to
/// discover the same gap.
struct MergeTarget<'a> {
    map: &'a mut SourceMapBuilder,
    source_id: usize,
}

impl MergeTarget<'_> {
    /// A fresh, independently-usable borrow of the same target — `Option<
    /// MergeTarget>` isn't `Copy` (it holds a `&mut`), so a caller handing
    /// the same target to more than one child (a `Vec<TsStmt>`'s own loop,
    /// `TsDecl::Class`'s own constructor-then-methods sequence) reborrows
    /// once per child via this, the same shape `Option::as_deref_mut` gives
    /// a bare `&mut`.
    fn reborrow(&mut self) -> MergeTarget<'_> {
        MergeTarget {
            map: self.map,
            source_id: self.source_id,
        }
    }
}

/// Record `stmt`'s span at `offset` in the target's map, if the print has one
/// and the statement carries a span (1A.15, Decision B of #3716). The same
/// statement can be reached twice at one offset (an inline branch is noted by
/// `render_branch` and again by a `render_stmt` fallback); the map keeps the
/// later span at an offset, which is the same one.
fn note_span(offset: usize, stmt: &TsStmt, map: Option<&mut MergeTarget<'_>>) {
    if let Some(span) = stmt.span {
        note_span_at(offset, span, map);
    }
}

/// [`note_span`] for a span that is not a statement's own: a `switch` case's.
fn note_span_at(offset: usize, span: Span, map: Option<&mut MergeTarget<'_>>) {
    if let Some(target) = map {
        target.map.record_in(offset, target.source_id, span);
    }
}
