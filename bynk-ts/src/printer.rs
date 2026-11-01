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
//!   depth threaded through [`render_stmt`]/[`render_block_body`] exactly.
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
//!   one blank line before each method.** [`render_decl_body`]'s own
//!   `TsDecl::Class` arm — again, this is what `events_fanout.rs` itself
//!   does, not a general rule derived some other way.
//! - **`if`/`for...of`'s own body prints with braces when it's a
//!   `TsStmtKind::Block`, and inline on the same line otherwise** —
//!   `if (!Array.isArray(subs)) continue;` has no braces in the grounding
//!   file; a `for...of` always does. See [`render_branch`].
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
//!   [`render_inline_stmt`]'s own `Raw` arm for why that combination stays
//!   deliberately unbuilt.
//!
//! None of the above is claimed as *the* TypeScript style this printer will
//! use forever — it's what this slice's own grounding file needs, named
//! rather than left implicit, the same posture the node algebra itself
//! takes (`program.rs`'s own module doc). A future file with a construct
//! this policy doesn't cover yet (an `else` branch, a multi-field class with
//! blank lines between fields, …) extends it the same way `program.rs`'s
//! own node list grows: file by file, against real content.

use crate::program::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsClassMethod, TsDecl, TsExpr, TsLit, TsObjectEntry,
    TsParam, TsProgram, TsStmt, TsStmtKind, TsType, TsTypeMember, TsUnaryOp,
};
use crate::source_map::SourceMapBuilder;

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
/// field. Only a *top-level* statement's own span is recorded as a
/// checkpoint (R7.4's existing scope, unchanged by P7.8 — see
/// [`TsStmt::span`]'s own doc for why a nested statement's span isn't
/// recorded yet).
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
        if let Some(span) = stmt.span {
            map.record(out.len(), span);
        }
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

/// `emit_doc_block`'s own real rendering, byte-for-byte (#1333): `/**`
/// opens; one ` * <line>` per non-blank source line, a literal `*/`
/// escaped to `*\/` (issue #720 — otherwise it would close the comment
/// early and let trailing text land as executable top-level TypeScript); a
/// blank source line prints as a bare ` *`, no trailing space; `*/`
/// closes. All at `depth`'s own 2-space-per-level indent.
fn render_doc_comment(out: &mut String, text: &str, depth: usize) {
    let ind = indent(depth);
    out.push_str(&ind);
    out.push_str("/**\n");
    for line in text.lines() {
        let trimmed = line.trim_end();
        out.push_str(&ind);
        if trimmed.is_empty() {
            out.push_str(" *\n");
        } else {
            out.push_str(" * ");
            out.push_str(&trimmed.replace("*/", "*\\/"));
            out.push('\n');
        }
    }
    out.push_str(&ind);
    out.push_str(" */\n");
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

/// Render one statement, including its own leading indent.
///
/// `map`: #1477's own thread-through, checked *here*, once, rather than
/// re-checked at every one of this function's own many call sites (review
/// of #1488, finding 2: the first version of this mechanism only checked a
/// `Vec<TsStmt>` body's own *direct* children, inside `render_block_stmts`
/// — silently dropping a `nested_map` set on a statement handed to `print`/
/// `print_stmt_and_merge` directly, or on a statement nested one level
/// deeper still, inside a `Block`/`Switch` case/`try`/`catch` body within a
/// real function body). Checking unconditionally at this one entry point,
/// before dispatching on `stmt.kind`, means every caller — however deep —
/// gets the merge for free by simply forwarding `map` on, the same way
/// `depth` already threads through unconditionally; a caller that has
/// nothing to merge just passes `None`, as most still do.
fn render_stmt(out: &mut String, stmt: &TsStmt, depth: usize, mut map: Option<MergeTarget<'_>>) {
    if let Some(nested) = &stmt.nested_map
        && let Some(target) = map.as_mut()
    {
        let base = out.len();
        render_stmt_kind(out, stmt, depth, None);
        target
            .map
            .merge(nested, &out[base..], out, base, target.source_id);
        return;
    }
    render_stmt_kind(out, stmt, depth, map);
}

/// [`render_stmt`]'s own per-`kind` dispatch, split out so the nested-
/// checkpoint check above runs exactly once per statement regardless of
/// which arm below ends up recursing back into `render_stmt` for a child.
fn render_stmt_kind(
    out: &mut String,
    stmt: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    match &stmt.kind {
        TsStmtKind::Verbatim { text, .. } => {
            out.push_str(text);
        }
        TsStmtKind::Decl(decl) => render_decl(out, decl, depth, map),
        TsStmtKind::Const { name, ty, init } => {
            out.push_str(&indent(depth));
            out.push_str("const ");
            render_binding_name(out, name);
            if let Some(ty) = ty {
                out.push_str(": ");
                render_type(out, ty);
            }
            out.push_str(" = ");
            render_stmt_level_expr(out, init, depth);
            out.push_str(";\n");
        }
        TsStmtKind::Let {
            name,
            ty,
            init,
            hoisted,
        } => {
            out.push_str(&indent(depth));
            out.push_str(if *hoisted { "var " } else { "let " });
            render_binding_name(out, name);
            if let Some(ty) = ty {
                out.push_str(": ");
                render_type(out, ty);
            }
            if let Some(init) = init {
                out.push_str(" = ");
                render_stmt_level_expr(out, init, depth);
            }
            out.push_str(";\n");
        }
        TsStmtKind::ExprStmt(expr) => {
            out.push_str(&indent(depth));
            render_expr(out, expr);
            out.push_str(";\n");
        }
        TsStmtKind::Return(expr) => {
            out.push_str(&indent(depth));
            out.push_str("return");
            if let Some(expr) = expr {
                out.push(' ');
                // #1321: `return { ... }` needs the same depth-aware
                // multiline-object handling `Const`/`Let`/`Assign` already
                // get — `workers.rs`'s own `emit_worker_compose` returns
                // its whole compose surface this way (one shorthand async
                // `Method` entry per wrapper, one per line), a shape
                // `render_expr`'s plain recursion can't render correctly
                // (see `TsExpr::Object`'s own doc).
                render_stmt_level_expr(out, expr, depth);
            }
            out.push_str(";\n");
        }
        TsStmtKind::Throw(expr) => {
            out.push_str(&indent(depth));
            out.push_str("throw ");
            render_stmt_level_expr(out, expr, depth);
            out.push_str(";\n");
        }
        TsStmtKind::If {
            cond,
            then_branch,
            else_branch,
            same_line_else,
        } => {
            out.push_str(&indent(depth));
            out.push_str("if (");
            render_expr(out, cond);
            out.push(')');
            // `} else {` on one line — #1325's own real gap, all three of
            // `emit_test_main`'s own real `if`/`else` sites (one `Block`,
            // one `InlineBlock`). Only reachable when `then_branch` is
            // itself braced (`Block` or `InlineBlock` — every real
            // `same_line_else` site is one or the other); a brace-free
            // `then_branch` has no closing `}` for `else` to sit against,
            // so this falls back to the ordinary fresh-line rendering
            // rather than producing `<inline-stmt> else {`, which nothing
            // real needs. Review of #1326, finding 2: the two-variant
            // check needs no `Option<()>`/re-match/`unreachable!()` — a
            // plain `matches!` guard plus one `if let ... else` dispatch
            // says the same thing with no wildcard arm.
            let then_is_braced = matches!(
                &then_branch.kind,
                TsStmtKind::Block(_) | TsStmtKind::InlineBlock(_)
            );
            if *same_line_else
                && then_is_braced
                && let Some(else_branch) = else_branch
            {
                // `render_inline_block`'s own `{ stmts }` shape, minus its
                // own trailing `\n` and leading space (added here instead,
                // matching `render_branch`'s own InlineBlock arm) so
                // `else` continues on the same physical line;
                // `render_block_body`'s own `{ stmts }` (no trailing
                // newline, the exact shape `TryCatch`'s own `} catch (e) {`
                // continuation already reuses) for the `Block` case,
                // guaranteed by `then_is_braced` above.
                if let TsStmtKind::InlineBlock(stmts) = &then_branch.kind {
                    out.push_str(" { ");
                    render_compact_stmts(out, stmts);
                    out.push_str(" }");
                } else {
                    render_block_body(out, then_branch, depth, None);
                }
                out.push_str(" else");
                render_branch(out, else_branch, depth);
            } else {
                render_branch(out, then_branch, depth);
                if let Some(else_branch) = else_branch {
                    out.push_str(&indent(depth));
                    out.push_str("else");
                    render_branch(out, else_branch, depth);
                }
            }
        }
        TsStmtKind::ForOf {
            binding,
            iter,
            body,
        } => {
            out.push_str(&indent(depth));
            out.push_str("for (const ");
            out.push_str(binding);
            out.push_str(" of ");
            render_expr(out, iter);
            out.push(')');
            render_branch(out, body, depth);
        }
        TsStmtKind::For {
            name,
            init,
            test,
            body,
        } => {
            out.push_str(&indent(depth));
            out.push_str("for (let ");
            out.push_str(name);
            out.push_str(" = ");
            render_expr(out, init);
            out.push_str("; ");
            render_expr(out, test);
            out.push_str("; ");
            out.push_str(name);
            out.push_str("++)");
            render_branch(out, body, depth);
        }
        TsStmtKind::TryCatch {
            try_block,
            catch_param,
            catch_block,
        } => {
            out.push_str(&indent(depth));
            out.push_str("try");
            render_block_body(out, try_block, depth, map.as_mut().map(|t| t.reborrow()));
            out.push_str(" catch");
            if let Some(p) = catch_param {
                out.push_str(" (");
                out.push_str(p);
                out.push(')');
            }
            render_block_body(out, catch_block, depth, map.as_mut().map(|t| t.reborrow()));
            out.push('\n');
        }
        TsStmtKind::Block(stmts) => {
            out.push_str(&indent(depth));
            out.push_str("{\n");
            for s in stmts {
                render_stmt(out, s, depth + 1, map.as_mut().map(|t| t.reborrow()));
            }
            out.push_str(&indent(depth));
            out.push_str("}\n");
        }
        TsStmtKind::Continue => {
            out.push_str(&indent(depth));
            out.push_str("continue;\n");
        }
        TsStmtKind::Assign { target, value } => {
            out.push_str(&indent(depth));
            render_expr(out, target);
            out.push_str(" = ");
            render_stmt_level_expr(out, value, depth);
            out.push_str(";\n");
        }
        TsStmtKind::Comment(text) => {
            for line in text.split('\n') {
                out.push_str(&indent(depth));
                out.push_str("// ");
                out.push_str(line);
                out.push('\n');
            }
        }
        TsStmtKind::DocComment(text) => render_doc_comment(out, text, depth),
        TsStmtKind::Blank => out.push('\n'),
        TsStmtKind::Switch {
            discriminant,
            cases,
        } => {
            out.push_str(&indent(depth));
            out.push_str("switch (");
            render_expr(out, discriminant);
            out.push_str(") {\n");
            for case in cases {
                debug_assert!(
                    !(case.test.is_some() && case.default_braced),
                    "TsSwitchCase.default_braced only means something on the default (test: None) case — review of #1402's own nit"
                );
                // Review of #1446: the mirror invariant for `case_braced`
                // (Arc E slice 6, #1445) — it only means something on a
                // non-`default` (`test: Some(..)`) case; every real
                // `test: None` call site sets it `true` with no effect
                // (`default_braced` owns that arm's bracing instead), which
                // would silently swallow a future caller's `false` there.
                debug_assert!(
                    case.test.is_some() || case.case_braced,
                    "TsSwitchCase.case_braced only means something on a non-default (test: Some(..)) case"
                );
                out.push_str(&indent(depth + 1));
                match &case.test {
                    Some(test) => {
                        out.push_str("case ");
                        render_expr(out, test);
                        // Arc E slice 6 (#1445): `case_braced` is the
                        // non-`default` mirror of `default_braced` just
                        // below — `emit_sum_codec`'s own payload-free
                        // variant case (`case "Pending":\n  return { kind:
                        // "Pending" };`) is real, unbraced content sitting
                        // right beside a braced payload-carrying sibling in
                        // the same switch (`212_json_codec`'s own `Status`
                        // fixture).
                        if case.case_braced {
                            out.push_str(": {\n");
                            for s in &case.body {
                                render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
                            }
                            out.push_str(&indent(depth + 1));
                            out.push_str("}\n");
                        } else {
                            out.push_str(":\n");
                            for s in &case.body {
                                render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
                            }
                        }
                    }
                    None => {
                        if case.default_braced {
                            out.push_str("default: {\n");
                            for s in &case.body {
                                render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
                            }
                            out.push_str(&indent(depth + 1));
                            out.push_str("}\n");
                        } else {
                            out.push_str("default:\n");
                            for s in &case.body {
                                render_stmt(out, s, depth + 2, map.as_mut().map(|t| t.reborrow()));
                            }
                        }
                    }
                }
            }
            out.push_str(&indent(depth));
            out.push_str("}\n");
        }
        TsStmtKind::InlineBlock(stmts) => {
            out.push_str(&indent(depth));
            render_inline_block(out, stmts);
        }
        TsStmtKind::Increment(expr) => {
            out.push_str(&indent(depth));
            render_expr(out, expr);
            out.push_str("++;\n");
        }
        // Same rendering as `Verbatim` above — the text is printed exactly
        // as given, no `indent(depth)` prefix, no added punctuation (see
        // `TsStmtKind::Raw`'s own doc for why this is a distinct kind, not
        // a reuse of `Verbatim`).
        TsStmtKind::Raw(text) => out.push_str(text),
    }
}

/// `if`/`for...of`'s own body: braces (and a nested block) when `branch` is
/// itself a `Block`, otherwise printed inline on the same line — matching
/// `events_fanout.rs`'s own `if (!Array.isArray(subs)) continue;` (no
/// braces) alongside its always-braced `for...of` bodies.
fn render_branch(out: &mut String, branch: &TsStmt, depth: usize) {
    match &branch.kind {
        TsStmtKind::Block(_) => {
            render_block_body(out, branch, depth, None);
            out.push('\n');
        }
        TsStmtKind::InlineBlock(stmts) => {
            out.push(' ');
            render_inline_block(out, stmts);
        }
        _ => {
            out.push(' ');
            render_inline_stmt(out, branch);
        }
    }
}

/// `{ stmt; stmt; ... }` on one generated line — [`TsStmtKind::InlineBlock`]'s
/// own renderer, shared by [`render_branch`] (an `if`/`else` branch) and
/// [`render_stmt`]'s own top-level/nested-statement-list arm (not reachable
/// from any real content at that position today, but every `TsStmtKind`
/// variant still needs a real rendering, not a wildcard). Each statement
/// renders through [`render_inline_stmt`] (reusing its own exhaustive
/// per-kind handling) with its own trailing newline trimmed, then a single
/// space — matching the real, hand-written `{ a; b; c; }` shape exactly.
fn render_inline_block(out: &mut String, stmts: &[TsStmt]) {
    out.push_str("{ ");
    render_compact_stmts(out, stmts);
    out.push_str(" }\n");
}

/// `stmt1; stmt2; ...` — every statement in `stmts` rendered on the current
/// line, semicolon-and-space-separated, no leading/trailing space or
/// surrounding braces of its own. The shared core both real compact shapes
/// need: [`TsStmtKind::InlineBlock`]'s own single-line-*braced* form
/// (`render_inline_block`, braces on the same line as the content) and
/// [`render_block_body`]'s own special case for a `TryCatch` block whose
/// body is an `InlineBlock` (braces on their own lines, per that shape's
/// usual convention, but the body itself still one compact line) —
/// `workers_entry.rs`'s own two real shapes, distinguished by whether the
/// pre-conversion `writeln!` code put the opening brace on the same
/// physical line as the content or not.
fn render_compact_stmts(out: &mut String, stmts: &[TsStmt]) {
    for (i, s) in stmts.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut piece = String::new();
        render_inline_stmt(&mut piece, s);
        // Review of #1324, finding 2: only `trim_end_matches('\n')`'s own
        // trailing newline is stripped — an *embedded* newline (a genuinely
        // multi-line statement, e.g. `Block`/`Switch`/`TryCatch`, rendered
        // via `render_inline_stmt`'s own `render_stmt(out, stmt, 0)`
        // fallback) would break the one-line shape this function promises.
        // Not reachable today — `workers_entry.rs`'s own three real
        // `InlineBlock` sites are `ExprStmt`/`Continue` only — but worth a
        // loud check rather than a silent multi-line break if a future
        // slice's `InlineBlock` ever holds one.
        debug_assert!(
            !piece.trim_end_matches('\n').contains('\n'),
            "render_compact_stmts: a genuinely multi-line statement can't render on one line"
        );
        out.push_str(piece.trim_end_matches('\n'));
    }
}

/// A statement rendered without its own leading indent, for the "no braces"
/// half of [`render_branch`] — only the shapes `events_fanout.rs` actually
/// needs inline (`continue`, `return`, a bare expression) get dedicated
/// handling; anything else falls back to a normal indented render (depth 0)
/// rather than panicking, so an unanticipated future shape still prints
/// something plausible instead of crashing.
fn render_inline_stmt(out: &mut String, stmt: &TsStmt) {
    match &stmt.kind {
        TsStmtKind::Continue => out.push_str("continue;\n"),
        TsStmtKind::Return(expr) => {
            out.push_str("return");
            if let Some(expr) = expr {
                out.push(' ');
                render_expr(out, expr);
            }
            out.push_str(";\n");
        }
        TsStmtKind::ExprStmt(expr) => {
            render_expr(out, expr);
            out.push_str(";\n");
        }
        // Review of #1317/#1318, finding 2: every other variant in the
        // fallback group below is safe to render via `render_stmt`'s own
        // top-level `//`-line-comment form — `Comment` is not. A `//`
        // comment run through the generic fallback (`if (cond) // text`)
        // comments out the rest of the physical line, leaving the `if`
        // with no body at all — a TypeScript parse error, not merely an
        // unlikely shape. Not reachable today (`events_fanout.rs` never
        // puts a bare `Comment` in a brace-free `if`/`for...of` body), but
        // the fallback group exists precisely so a future slice trips over
        // a missing case at compile time, not at parse time in emitted
        // output — so `Comment` needs its own real inline shape now, a
        // block comment (`/* text */`), which cannot swallow anything
        // after it. Embedded newlines flatten to spaces, keeping this
        // strictly one generated line like every other inline shape here.
        // Review of #1324, finding 2: `Blank`'s own top-level rendering is a
        // bare `'\n'` (see `render_stmt`), which the generic fallback below
        // would inherit unchanged — reaching `render_branch`'s brace-free
        // arm (`if (cond) <inline>`), that prints `if (cond) \n`, and since
        // JS/TS statement grammar doesn't care about the newline, the very
        // next statement in the enclosing block silently becomes the `if`'s
        // own body. Exactly the same swallowing-bug class the #1317/#1318
        // review already fixed for `Comment` just above — an honest empty
        // statement (`;`) is what a "no code here" body actually means in an
        // inline TS position, and cannot swallow anything after it.
        TsStmtKind::Blank => out.push_str(";\n"),
        TsStmtKind::Comment(text) => {
            out.push_str("/* ");
            out.push_str(&text.replace('\n', " "));
            out.push_str(" */\n");
        }
        // No inline shape of its own — `events_fanout.rs` never puts one of
        // these in a brace-free `if`/`for...of` body — but listed by name,
        // not a wildcard (review of #1314, finding 3): a future Arc C
        // slice's new `TsStmtKind` variant must fail to compile here rather
        // than silently inherit this fallback, the same exhaustiveness
        // discipline `VerbatimOrigin`'s own doc states for `bynk-emit`.
        TsStmtKind::Verbatim { .. }
        | TsStmtKind::Decl(_)
        | TsStmtKind::Const { .. }
        | TsStmtKind::Let { .. }
        | TsStmtKind::If { .. }
        | TsStmtKind::ForOf { .. }
        // Arc E slice 7 (#1447): not reachable today — `ListInst`/
        // `MapInst`'s two real `For` sites both use a braced `Block` body,
        // which `render_branch` dispatches to its own multi-line arm before
        // this fallback group is ever consulted — but safe for the same
        // reason `ForOf` already is: a brace-free `For` body renders through
        // `render_stmt`'s own indented, newline-terminated form, which is
        // exactly what this fallback provides.
        | TsStmtKind::For { .. }
        | TsStmtKind::TryCatch { .. }
        | TsStmtKind::Block(_)
        | TsStmtKind::Assign { .. }
        | TsStmtKind::Switch { .. }
        | TsStmtKind::InlineBlock(_)
        // #1325: `passed++;`/`failed++;` inside `emit_test_main`'s own
        // compact `{ passed++; console.log(...); }` shape — always complete,
        // single-line, semicolon-terminated content (never a bare newline
        // the way `Blank`'s own top-level form is), so the same fallback
        // that's safe for `Const`/`If`/etc. above is safe here too.
        | TsStmtKind::Increment(_)
        // #1353: `emit_contract_guarded_body`'s own precondition/
        // postcondition guards are exactly this fallback's own target case —
        // a real, reachable `InlineBlock` site (`if (!(pred)) { const __e =
        // ...; __e.name = ...; throw __e; }`), not a defensive placeholder.
        // Safe for the same reason as `Const`/`Assign` above, with the same
        // caveat those two already carry: `render_stmt_level_expr` renders a
        // `multiline: true` operand with embedded newlines, which would
        // break this fallback's one-line contract — untested today only
        // because the one real call site (`throw __e;`, a bare `Ident`)
        // never reaches that case, not because `Throw` structurally cannot.
        | TsStmtKind::Throw(_)
        // #1333: not reachable today — every real `DocComment` reaches the
        // printer only via `print_stmt`, never through a `TsProgram`'s own
        // tree — which is what actually makes this fallback safe: unlike
        // `Comment`'s own `//` form, a `/** ... */` block always closes
        // itself before any subsequent text, so it cannot swallow the next
        // statement the way an unterminated `//` line comment could — but
        // the fallback's own `render_stmt(out, stmt, 0)` still emits the
        // block's embedded newlines, which would break an `InlineBlock`'s
        // single-line contract if this arm ever became reachable through
        // one. Listed by name per this group's own exhaustiveness
        // discipline, not folded into a wildcard.
        | TsStmtKind::DocComment(_)
        // #1337: not reachable today — every real `Raw`-bearing body
        // (`TsObjectEntry::Method`, and since #1352/#1369 also
        // `TsDecl::Function`) renders through `render_block_stmts`, never
        // through `render_inline_stmt`'s own call path (an `if`/`for...of`
        // branch or an `InlineBlock`). Unsafe if it ever became reachable
        // there for the same reason `DocComment` is: `Raw`'s own text is
        // typically a whole multi-statement function body, never
        // single-line, so the fallback's embedded newlines would break an
        // `InlineBlock`'s single-line contract. Since #1369 added
        // `TsDecl::Function.inline`, this is now one field flip away from
        // live rather than purely hypothetical — `render_compact_stmts`'s
        // own `debug_assert!` is the actual backstop if a future call site
        // ever combines `inline: true` with a `Raw` body statement;
        // `emit_agent`'s own rehydrate function (#1369) deliberately keeps
        // `inline: false` specifically to avoid exercising this arm.
        // Listed by name, not folded into a wildcard, for the same reason
        // as every other arm in this group.
        | TsStmtKind::Raw(_) => render_stmt(out, stmt, 0, None),
    }
}

/// `{ <stmts> }`, with the closing brace at `depth`'s own indent and no
/// trailing newline — the shared shape [`TsStmtKind::TryCatch`] needs to
/// keep writing on the same generated line (`} catch (e) {`) and
/// [`render_branch`]'s own `Block` case needs with a `\n` appended after.
/// `block` is expected to be a [`TsStmtKind::Block`]; anything else is
/// still rendered sensibly (as a single-statement body) rather than
/// panicking.
///
/// `map`: forwarded into `stmts` when `block` is a real `Block` (#1477) —
/// `TryCatch`'s own `try`/`catch` bodies are the one real site this covers
/// (a `nested_map`-bearing statement inside either); `render_branch`'s own
/// `If`/`ForOf`/`For` call sites always pass `None`, since nothing threads
/// a map into `render_branch` itself (out of scope — ADR 0391's own
/// permanently-opaque content is always a whole function/method/constructor
/// body, never embedded directly inside an `if`/loop branch).
fn render_block_body(
    out: &mut String,
    block: &TsStmt,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str(" {\n");
    if let TsStmtKind::Block(stmts) = &block.kind {
        for s in stmts {
            render_stmt(out, s, depth + 1, map.as_mut().map(|t| t.reborrow()));
        }
    } else if let TsStmtKind::InlineBlock(stmts) = &block.kind {
        // The braces sit on their own lines here (this function's own usual
        // shape, e.g. `TryCatch`'s `catch (e) { ... }`), but the body itself
        // is one compact line — `workers_entry.rs`'s own real queue-consumer
        // catch clause (`console.error(...); msg.retry();`).
        out.push_str(&indent(depth + 1));
        render_compact_stmts(out, stmts);
        out.push('\n');
    } else {
        render_stmt(out, block, depth + 1, None);
    }
    out.push_str(&indent(depth));
    out.push('}');
}

fn render_binding_name(out: &mut String, name: &TsBindingName) {
    match name {
        TsBindingName::Ident(s) => out.push_str(s),
        TsBindingName::ObjectPattern(names) => {
            out.push_str("{ ");
            for (i, n) in names.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(n);
            }
            out.push_str(" }");
        }
    }
}

/// Whether `expr`, printed in a position member access, indexing, a call
/// callee, `await`, or unary `!`/`typeof` binds tighter than, needs its own
/// parens to preserve the built tree's meaning — `Binary`/`As`/`Arrow`/
/// `Conditional` all bind looser than any of those (review of #1314, finding
/// 2: `!a ?? b`/`a ?? b.c`/`await a ?? b` all silently changed meaning
/// without this; review of #1322, finding 1: `Arrow` was missing from this
/// same rule — `(x) => x(1)` is a call whose *body* is `x(1)`, not the IIFE
/// `((x) => x)(1)` a `Call { callee: Arrow, .. }` node means; #1323 adds
/// `Conditional` proactively, the same class of gap, before any real content
/// or review round hits it). Deliberately conservative for *these* contexts
/// specifically: a nested `Binary`/`Arrow`/`Conditional` is always
/// parenthesized here regardless of which operator/shape it is — correct in
/// every case that class of bug can hit (none of these
/// six positions is ever real, grounded content in this file today), at
/// the cost of an occasional pair a real precedence table could omit.
///
/// **Not** used for `Binary`'s own left/right operands — see
/// [`render_binary_operand`]/[`binary_precedence`] for why that context
/// needed to become precedence-aware once #1321 added more than one binary
/// operator (this function's own always-parenthesize rule would print
/// `(__authz === null) || …` for `workers.rs`'s own real
/// `__authz === null || !__authz.startsWith(...)`, which the byte-golden
/// fixtures don't have parens around).
fn needs_parens_as_operand(expr: &TsExpr) -> bool {
    matches!(
        expr,
        TsExpr::Binary { .. }
            | TsExpr::As { .. }
            | TsExpr::Arrow { .. }
            | TsExpr::Conditional { .. }
    )
}

fn render_operand(out: &mut String, expr: &TsExpr) {
    if needs_parens_as_operand(expr) {
        out.push('(');
        render_expr(out, expr);
        out.push(')');
    } else {
        render_expr(out, expr);
    }
}

/// Standard JS/TS relative precedence for the operators [`TsBinaryOp`]
/// currently has — only used to decide a nested `Binary` operand's own
/// parenthesisation (see [`render_binary_operand`]); higher binds tighter.
fn binary_precedence(op: TsBinaryOp) -> u8 {
    match op {
        TsBinaryOp::NullishCoalescing => 1,
        TsBinaryOp::Or => 2,
        TsBinaryOp::And => 3,
        TsBinaryOp::StrictEq | TsBinaryOp::StrictNotEq => 4,
        TsBinaryOp::GreaterThan
        | TsBinaryOp::LessThan
        | TsBinaryOp::InstanceOf
        | TsBinaryOp::In
        | TsBinaryOp::GreaterThanEq
        | TsBinaryOp::LessThanEq => 5,
        TsBinaryOp::Add => 6,
    }
}

/// Render `expr` as one side of a `Binary { op: outer_op, .. }` — #1321's
/// own real gap (`workers.rs`'s own `__authz === null ||
/// !__authz.startsWith(...)` / `__authz !== null &&
/// __authz.startsWith(...)`): with only `??` in the algebra,
/// [`render_operand`]'s "always parenthesize a nested `Binary`" rule never
/// mismatched real content; a comparison (`===`/`!==`) nested inside `||`/
/// `&&` needs *no* parens to preserve meaning (`===` binds tighter), and
/// the byte-golden fixtures have none — so this context needed to become
/// precedence-aware rather than reusing [`render_operand`] unchanged.
/// `??` mixed with `||`/`&&` is a real TS syntax error without explicit
/// parens (not just a precedence question), so that combination always
/// parenthesizes regardless of the numeric table above; nothing in
/// `bynk-emit` builds that combination today, but a future caller that did
/// still gets correct, parseable output rather than a silent `tsc` error.
///
/// Equal precedence still parenthesizes for two *different* operators
/// (`<=`, not `<`) — deliberately preserving [`render_operand`]'s own
/// pre-#1321 "always parenthesize a same/lower-precedence nested chain,
/// even where associativity would read the same without it" conservatism
/// for that case; only a *strictly higher*-precedence nested operator — the
/// one real, grounded case #1321 needs — omits parens there. #1323 adds one
/// more real case: the exact *same* operator nested in itself omits parens
/// too, for `||`/`&&` specifically (both associative, and real content —
/// `emit_call_handler_dispatch`'s 3-term `||` chain — needs the flat,
/// unparenthesized reading) — `??` keeps the pre-#1321 conservatism
/// unchanged (pinned by `parenthesises_a_nested_binary_operand_of_another_
/// binary`'s own test, still `a ?? (b ?? c)`), since nothing in real
/// content needs a flattened `??` chain and this slice's own grounding
/// gives no reason to widen that boundary opportunistically.
fn render_binary_operand(out: &mut String, outer_op: TsBinaryOp, expr: &TsExpr, is_left: bool) {
    let needs_parens = match expr {
        // `Arrow`/`Conditional` are the two lowest-precedence expression
        // forms in JS/TS — `(x) => y || z` as a binary operand must print
        // `(x) => (y || z)`, never the bare `(x) => y || z` (which reparses
        // as the arrow's own *body* being `y || z`, not the arrow itself
        // being one operand of `||`); a `Conditional` operand needs the
        // identical protection for the identical reason (review of #1322,
        // finding 1, for `Arrow`; #1323 adds `Conditional` proactively, the
        // same class of gap).
        TsExpr::As { .. } | TsExpr::Arrow { .. } | TsExpr::Conditional { .. } => true,
        TsExpr::Binary { op: inner_op, .. } => {
            let mixes_nullish = (outer_op == TsBinaryOp::NullishCoalescing)
                != (*inner_op == TsBinaryOp::NullishCoalescing);
            if mixes_nullish {
                true
            } else if *inner_op == outer_op {
                // Same operator chained with itself: only `||`/`&&` are
                // associative, so only they may print flat with no parens —
                // #1323's own `typeof args !== "object" || args === null ||
                // Array.isArray(args)` (`emit_call_handler_dispatch`), a
                // genuine 3-term chain the pre-#1323 "always parenthesize
                // equal precedence" rule would have wrongly printed as
                // `(typeof args !== "object" || args === null) ||
                // Array.isArray(args)`. Every other operator — including
                // `??` (kept at its pre-#1321 conservative choice; nothing in
                // real content needs a flattened `??` chain, and
                // `parenthesises_a_nested_binary_operand_of_another_binary`
                // pins that `a ?? (b ?? c)` still gets its parens) and, as of
                // this fix, the equality/relational operators (`===`/`!==`/
                // `>`) — is NOT associative: `a === b === c` parses as
                // `(a === b) === c`, not the tree's real `a === (b === c)`,
                // so a same-operator nesting of any of those still needs its
                // parens regardless of which side it's nested on (review of
                // #1324, finding 1 — the original fix wrongly generalized
                // the `||`/`&&` exemption to every operator). `+` (Arc C,
                // step (11), #1388) does NOT join that exemption on both
                // sides — review of #1389, finding 1: `||`/`&&` are
                // SEMANTICALLY associative (`(a || b) || c` and
                // `a || (b || c)` always agree), but `+` is only
                // GRAMMATICALLY left-associative — with a mixed number/string
                // chain, `1 + (2 + "3")` (`"123"`) and `(1 + 2) + "3"`
                // (`"33"`) disagree, so flattening a RIGHT-nested `Add` would
                // silently change the value. `join_plus`
                // (`bynk-emit/src/emitter/emit.rs`) only ever left-folds, so
                // this asymmetry never showed up in that caller's own
                // zero-diff fixture run — but `bynk-ts` is a shared
                // primitive, not scoped to that one caller's own usage.
                match outer_op {
                    TsBinaryOp::Or | TsBinaryOp::And => false,
                    TsBinaryOp::Add => !is_left,
                    _ => true,
                }
            } else {
                binary_precedence(*inner_op) <= binary_precedence(outer_op)
            }
        }
        _ => false,
    };
    if needs_parens {
        out.push('(');
        render_expr(out, expr);
        out.push(')');
    } else {
        render_expr(out, expr);
    }
}

/// Render `expr` as a statement or declaration's own top-level expression
/// (a `const`/`let` initialiser, an assignment's own value, …) — one of the
/// two places `depth` is available to render a `multiline: true`
/// [`TsExpr::Object`]/[`TsExpr::Array`] correctly (see `Object`'s own doc);
/// the other is [`render_multiline_object_entry`]'s own `Prop` arm (#1355 —
/// a `Prop`'s own value can itself be a nested multiline object/array,
/// `emit_messages_bundle`'s own real doubly-nested table). Every other
/// shape defers to the ordinary, depth-unaware [`render_expr`] unchanged;
/// this exists only to intercept the one shape that needs `depth` before it
/// gets there.
fn render_stmt_level_expr(out: &mut String, expr: &TsExpr, depth: usize) {
    if let TsExpr::Object {
        entries,
        multiline: true,
    } = expr
    {
        render_multiline_object(out, entries, depth);
    } else if let TsExpr::Array {
        items,
        multiline: true,
    } = expr
    {
        render_multiline_array(out, items, depth);
    } else {
        render_expr(out, expr);
    }
}

/// `{ <newline> ("  "*depth+1)<key>: <value>,<newline> ... ("  "*depth)}` —
/// one entry per line, each with its own trailing comma (including the
/// last), closing brace back at `depth`'s own indent. `events_fanout.rs`'s
/// own `__eventRoutes` table is the real, grounded shape this exists for
/// (#1317) — TypeScript's ordinary multi-line object-literal convention.
/// #1321 (`workers.rs`): entries may now be [`TsObjectEntry`], not just
/// `Prop` — the `compose`-returned surface object is one shorthand async
/// `Method` entry per wrapper, one per line.
fn render_multiline_object(out: &mut String, entries: &[TsObjectEntry], depth: usize) {
    // Deliberately no empty-entries shortcut (review of #1317/#1318, finding
    // 1): `events_fanout.rs`'s own `__eventRoutes` table is reachable with
    // zero entries (a context can `ctx_uses_emit` while publishing only
    // events nobody subscribes to — `own_event_routes` filters down to
    // exactly that empty case, `bynk-emit/src/project.rs`'s own
    // `own_event_routes` computation), and the pre-conversion `writeln!`
    // code always wrote the open-brace line and the closing `};` line
    // unconditionally, regardless of how many times its own `for` loop
    // iterated — an empty table printed `{\n};`, not the tight `{}` a
    // single-line object's own empty case uses. Matching that byte-for-byte
    // means never taking the single-line shortcut here, not even for zero
    // entries.
    out.push_str("{\n");
    for entry in entries {
        render_multiline_object_entry(out, entry, depth, None);
    }
    out.push_str(&indent(depth));
    out.push('}');
}

/// One [`TsObjectEntry`], as [`render_multiline_object`]'s own per-entry
/// loop body — factored out so [`print_object_entry`] can render exactly
/// one entry without a whole object's own opening/closing braces, sharing
/// this one real per-kind dispatch rather than a second copy (the same
/// "one document-fragment entry point, no duplicated rendering" posture
/// [`print_stmt`]/[`print_type`] already established).
/// `map`: see [`render_block_stmts`]'s own doc (#1477) — reaches a `Method`
/// entry's own body unchanged; every other entry kind ignores it (no body
/// of its own).
fn render_multiline_object_entry(
    out: &mut String,
    entry: &TsObjectEntry,
    depth: usize,
    map: Option<MergeTarget<'_>>,
) {
    // Review of #1340, finding 1: a `Raw`-bodied `Method` entry's own text
    // is captured pre-indented at a fixed absolute depth by its caller (see
    // `print_object_entry`'s own doc) — correct only when `depth` is `0`.
    // Originally guarded only in `print_object_entry`, the one entry point
    // #1337 had in mind; #1339's `emit_refined_type` reaches this function
    // directly (via `TsExpr::multiline_object_entries`/`render_expr`, never
    // through `print_object_entry`), which the original guard's placement
    // silently missed. Asserted here instead, where both callers converge,
    // so neither path can bypass it again.
    if let TsObjectEntry::Method { body, .. } = entry {
        debug_assert!(
            depth == 0 || !contains_raw(body),
            "render_multiline_object_entry: a Raw-bodied Method entry's own baked-in indent only matches depth 0"
        );
    }
    match entry {
        TsObjectEntry::Prop(k, v) => {
            out.push_str(&indent(depth + 1));
            out.push_str(k);
            out.push_str(": ");
            // #1355: a `Prop`'s own value sits one level deeper than the
            // entry itself (matching this function's own `indent(depth +
            // 1)` above) — the same depth `render_stmt_level_expr`'s own
            // doc names as "the only place `depth` is available to render a
            // `multiline: true` `TsExpr::Object`/`Array` correctly."
            // `emit_messages_bundle`'s own real, doubly-nested `{ locale: {
            // code: expr, ... }, ... }` table is this gap's real grounding
            // — a nested multiline object value previously fell back to
            // single-line silently (the plain, depth-unaware `render_expr`
            // recursion ignores `multiline` entirely).
            render_stmt_level_expr(out, v, depth + 1);
            out.push_str(",\n");
        }
        TsObjectEntry::Shorthand(name) => {
            out.push_str(&indent(depth + 1));
            out.push_str(name);
            out.push_str(",\n");
        }
        TsObjectEntry::Spread(e) => {
            out.push_str(&indent(depth + 1));
            out.push_str("...");
            render_expr(out, e);
            out.push_str(",\n");
        }
        // `workers.rs`'s own dominant real shape (Decision A, gap 1):
        // `async {name}({params}) { <body> },`, body indented one
        // level deeper still — the same shape a class method's own
        // body gets (`render_decl_body`'s `TsDecl::Class` arm), here
        // for one object-literal entry instead.
        TsObjectEntry::Method {
            name,
            is_async,
            generics,
            params,
            return_type,
            doc,
            inline,
            body,
        } => {
            if let Some(text) = doc {
                render_doc_comment(out, text, depth + 1);
            }
            out.push_str(&indent(depth + 1));
            if *is_async {
                out.push_str("async ");
            }
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            if *inline {
                out.push_str(" { ");
                render_compact_stmts(out, body);
                out.push_str(" }");
            } else {
                render_block_stmts(out, body, depth + 1, map);
            }
            out.push_str(",\n");
        }
    }
}

/// `<{names.join(", ")}>`, or nothing at all when `names` is empty — bare
/// names only, matching `bynk-emit`'s own `ts_type_params` rendering
/// exactly. Originally `TsObjectEntry::Method`'s own generic parameter list
/// (#1337); generalised by #1339 into the one shared renderer for every
/// bare-name generics/type-params list this crate builds
/// (`TsDecl::Interface.type_params`, `TsDecl::TypeAlias.type_params`,
/// `TsExpr::Arrow.generics`) rather than four near-identical copies.
fn render_bare_generics(out: &mut String, names: &[String]) {
    if names.is_empty() {
        return;
    }
    out.push('<');
    for (i, g) in names.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(g);
    }
    out.push('>');
}

/// `[ <newline> ("  "*depth+1)<item>,<newline> ... ("  "*depth)]` — one
/// item per line, each with its own trailing comma, closing bracket back at
/// `depth`'s own indent. [`render_multiline_object`]'s own sibling for array
/// literals — #1325's own real, grounded shape: `emit_test_main`'s own
/// `modules` array (one `{ name, run }` entry per test). Same "no
/// empty-items shortcut" discipline as `render_multiline_object` — nothing
/// in `emit_test_main`'s own real content reaches this with zero tests, but
/// matching the pre-conversion `writeln!` code's own unconditional
/// open/close-bracket-on-separate-lines shape (rather than assuming an
/// untested empty case) is the same precedent review of #1317/#1318 set.
fn render_multiline_array(out: &mut String, items: &[TsExpr], depth: usize) {
    out.push_str("[\n");
    for item in items {
        out.push_str(&indent(depth + 1));
        render_expr(out, item);
        out.push_str(",\n");
    }
    out.push_str(&indent(depth));
    out.push(']');
}

/// One [`TsObjectEntry`] rendered inline (no leading indent, no trailing
/// comma/newline) — [`render_expr`]'s own `TsExpr::Object` arm's per-entry
/// renderer, shared so the single-line and multi-line forms agree on what
/// each entry kind looks like. A `Method` entry has no real single-line
/// call site today (`workers.rs`'s own method entries are always the
/// multi-line `return { ... }` shape), but is still rendered plausibly
/// (not panicking) rather than left unreachable, matching this crate's own
/// "no wildcard, but still sensible on an unanticipated shape" posture
/// elsewhere (e.g. `render_block_body`'s own non-`Block` fallback).
fn render_object_entry_inline(out: &mut String, entry: &TsObjectEntry) {
    match entry {
        TsObjectEntry::Prop(k, v) => {
            out.push_str(k);
            out.push_str(": ");
            render_expr(out, v);
        }
        TsObjectEntry::Shorthand(name) => out.push_str(name),
        TsObjectEntry::Spread(e) => {
            out.push_str("...");
            render_expr(out, e);
        }
        TsObjectEntry::Method {
            name,
            is_async,
            generics,
            params,
            return_type,
            doc,
            inline,
            body,
        } => {
            // Review of #1338, finding 2: a JSDoc block has no single-line
            // form — not reachable through this function today: every real
            // `Method` entry (`inline` or not) is placed via
            // `render_multiline_object_entry`, never here (this function
            // renders one ENTRY inline for when the whole ENCLOSING OBJECT
            // is single-line, a different concept from a method's own body
            // being compact). Silently dropping a JSDoc block would be a
            // strictly worse failure mode than a loud check — the same
            // "not reachable today, but worth a loud check" posture
            // `render_compact_stmts`'s own `debug_assert!` (above) already
            // established for an analogous inline-rendering hazard.
            debug_assert!(
                doc.is_none(),
                "render_object_entry_inline: a Method entry's own doc comment has no single-line form"
            );
            if *is_async {
                out.push_str("async ");
            }
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            if *inline {
                out.push_str(" { ");
                render_compact_stmts(out, body);
                out.push_str(" }");
            } else {
                render_block_stmts(out, body, 0, None);
            }
        }
    }
}

fn render_expr(out: &mut String, expr: &TsExpr) {
    match expr {
        TsExpr::Ident(name) => out.push_str(name),
        // #1539: printed exactly as `Ident` always was — raw text, no
        // escaping of its own. The origin exists for `verbatim_origins`/
        // `verbatim_sites` (xtask's gated probes) to line-scan, not for the
        // printer to branch on.
        TsExpr::VerbatimExpr(text, _origin) => out.push_str(text),
        TsExpr::Member { object, property } => {
            render_operand(out, object);
            out.push('.');
            out.push_str(property);
        }
        TsExpr::OptionalMember { object, property } => {
            render_operand(out, object);
            out.push_str("?.");
            out.push_str(property);
        }
        TsExpr::Index { object, index } => {
            render_operand(out, object);
            out.push('[');
            render_expr(out, index);
            out.push(']');
        }
        TsExpr::OptionalIndex { object, index } => {
            render_operand(out, object);
            out.push_str("?.[");
            render_expr(out, index);
            out.push(']');
        }
        TsExpr::Arrow {
            params,
            is_async,
            generics,
            return_type,
            body,
        } => {
            if *is_async {
                out.push_str("async ");
            }
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            out.push_str(" => ");
            render_arrow_body(out, body);
        }
        TsExpr::Call { callee, args } => {
            render_operand(out, callee);
            out.push('(');
            render_expr_list(out, args);
            out.push(')');
        }
        TsExpr::New { callee, args } => {
            out.push_str("new ");
            render_operand(out, callee);
            out.push('(');
            render_expr_list(out, args);
            out.push(')');
        }
        // `multiline` is ignored here deliberately: this recursion has no
        // `depth` to render a multi-line object correctly against (see
        // `TsExpr::Object`'s own doc). Every real `bynk-emit` call site
        // that needs `multiline: true` reaches it through
        // `render_stmt_level_expr` instead, which intercepts the shape
        // before it gets here.
        TsExpr::Object { entries, .. } => {
            if entries.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{ ");
                for (i, entry) in entries.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    render_object_entry_inline(out, entry);
                }
                out.push_str(" }");
            }
        }
        // `multiline` is ignored here deliberately, for the same reason
        // `TsExpr::Object`'s own arm just above ignores it: this recursion
        // has no `depth` to render a multi-line array correctly against.
        // Every real `bynk-emit` call site that needs `multiline: true`
        // reaches it through `render_stmt_level_expr` instead.
        TsExpr::Array { items, .. } => {
            out.push('[');
            render_expr_list(out, items);
            out.push(']');
        }
        TsExpr::TemplateLit { parts, exprs } => {
            out.push('`');
            for (i, part) in parts.iter().enumerate() {
                out.push_str(part);
                if let Some(e) = exprs.get(i) {
                    out.push_str("${");
                    render_expr(out, e);
                    out.push('}');
                }
            }
            out.push('`');
        }
        TsExpr::Await(inner) => {
            out.push_str("await ");
            render_operand(out, inner);
        }
        TsExpr::As { expr, ty } => {
            // `(a ?? b) as T` / `((x) => y) as T` / `((a ? b : c)) as T` — a
            // binary expression, an arrow, or a conditional all need parens
            // before `as` (`(x) => y as T` binds the cast to the arrow's own
            // *body*, not the whole arrow — review of #1322, finding 1;
            // #1323 adds `Conditional` proactively, the same class of gap).
            // `Await` is deliberately NOT in this list (corrected by #1323):
            // `as` binds looser than `await`, so `await x as T` already
            // parses as `(await x) as T` with no parens needed — P7.8's
            // original reasoning conflated "this file's real text has
            // parens" with "parens are grammatically required"; `workers_
            // entry.rs`'s own real `await request.json() as JsonValue` has
            // none. A caller whose own real content *does* want them (like
            // `events_fanout.rs`'s own historical text) wraps its own
            // `Await` in an explicit [`TsExpr::Paren`] instead of relying on
            // an implicit per-shape rule here.
            let needs_parens = matches!(
                **expr,
                TsExpr::Binary { .. } | TsExpr::Arrow { .. } | TsExpr::Conditional { .. }
            );
            if needs_parens {
                out.push('(');
                render_expr(out, expr);
                out.push(')');
            } else {
                render_expr(out, expr);
            }
            out.push_str(" as ");
            render_type(out, ty);
        }
        TsExpr::Unary { op, expr } => {
            out.push_str(match op {
                TsUnaryOp::Not => "!",
                TsUnaryOp::Typeof => "typeof ",
            });
            render_operand(out, expr);
        }
        TsExpr::Binary { op, left, right } => {
            render_binary_operand(out, *op, left, true);
            out.push_str(match op {
                TsBinaryOp::NullishCoalescing => " ?? ",
                TsBinaryOp::Or => " || ",
                TsBinaryOp::And => " && ",
                TsBinaryOp::StrictEq => " === ",
                TsBinaryOp::StrictNotEq => " !== ",
                TsBinaryOp::GreaterThan => " > ",
                TsBinaryOp::LessThan => " < ",
                TsBinaryOp::InstanceOf => " instanceof ",
                TsBinaryOp::Add => " + ",
                TsBinaryOp::In => " in ",
                TsBinaryOp::GreaterThanEq => " >= ",
                TsBinaryOp::LessThanEq => " <= ",
            });
            render_binary_operand(out, *op, right, false);
        }
        TsExpr::Conditional {
            test,
            consequent,
            alternate,
        } => {
            // `test` prints like a `ShortCircuitExpression` (the real JS/TS
            // grammar position) — an `Arrow` or a nested `Conditional` there
            // needs parens (grammar), a `Binary`/`As`/anything else doesn't.
            // `consequent`/`alternate` are `AssignmentExpression` positions
            // and admit a nested `Arrow`/`Conditional` with no parens at all
            // (right-associative chaining, e.g. `a ? b : c ? d : e`, is
            // already the correct reading) — neither needs the operand
            // machinery the way `test` does.
            let test_needs_parens =
                matches!(**test, TsExpr::Arrow { .. } | TsExpr::Conditional { .. });
            if test_needs_parens {
                out.push('(');
                render_expr(out, test);
                out.push(')');
            } else {
                render_expr(out, test);
            }
            out.push_str(" ? ");
            render_expr(out, consequent);
            out.push_str(" : ");
            render_expr(out, alternate);
        }
        TsExpr::Paren(inner) => {
            out.push('(');
            render_expr(out, inner);
            out.push(')');
        }
        TsExpr::Lit(lit) => render_lit(out, lit),
    }
}

/// [`TsExpr::Arrow`]'s own `body` renderer — see [`TsArrowBody`]'s own doc
/// for why there are exactly two shapes. `Expr` is the ordinary case, no
/// braces, the same plain `render_expr` recursion every arrow used before
/// #1435. `Block` reuses [`render_compact_stmts`] (the same shared core
/// `TsStmtKind::InlineBlock`'s own renderer already reduces to), not a
/// fresh copy of its "one physical line, semicolon-separated" logic — every
/// real `Block` site today (`serialisation.rs`'s `Float` guard) is exactly
/// this one-line-IIFE shape, so no trailing newline is added here (unlike
/// `render_inline_block`'s own statement-position form): this text is
/// embedded mid-expression, not standing alone as a statement.
fn render_arrow_body(out: &mut String, body: &TsArrowBody) {
    match body {
        TsArrowBody::Expr(expr) => render_expr(out, expr),
        TsArrowBody::Block(stmts) => {
            out.push_str("{ ");
            render_compact_stmts(out, stmts);
            out.push_str(" }");
        }
    }
}

fn render_expr_list(out: &mut String, exprs: &[TsExpr]) {
    for (i, e) in exprs.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        render_expr(out, e);
    }
}

fn render_lit(out: &mut String, lit: &TsLit) {
    match lit {
        TsLit::Str(s) => {
            out.push('"');
            // Matches `bynk_check::wire_default::escape_ts_literal`
            // byte-for-byte (review of #1314, finding 1) — the previous
            // version escaped only `"`/`\`, so a `\n`/`\t`/`\r` inside a
            // literal printed raw into the output, an unterminated string
            // literal `tsc` can't parse. `bynk-ts` can't depend on
            // `bynk-check` to share that function directly (this crate's
            // own module doc: `bynk-syntax` only) — inlined instead, kept
            // deliberately identical rather than reinvented, since both
            // this printer and `bynk-emit`'s `escape_ts_string` splice into
            // generated TypeScript and must never disagree.
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\t' => out.push_str("\\t"),
                    '\r' => out.push_str("\\r"),
                    // #2619: `\0`, other control characters and the JS line terminators
                    // U+2028/U+2029 print as `\u{…}` escapes (valid in strings and templates).
                    c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                        out.push_str(&format!("\\u{{{:X}}}", c as u32));
                    }
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        TsLit::Num(n) => out.push_str(n),
        TsLit::Null => out.push_str("null"),
        TsLit::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        // The whole literal, printed exactly as given — see `TsLit::Raw`'s
        // own doc for why: this is the one place in this printer that must
        // NOT apply `Str`'s own escaping.
        TsLit::Raw(text) => out.push_str(text),
    }
}

fn render_type(out: &mut String, ty: &TsType) {
    match ty {
        TsType::Named { name, type_args } => {
            out.push_str(name);
            if !type_args.is_empty() {
                out.push('<');
                for (i, arg) in type_args.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    render_type(out, arg);
                }
                out.push('>');
            }
        }
        TsType::Array { element, readonly } => {
            if *readonly {
                out.push_str("readonly ");
            }
            if needs_parens_as_array_element(element) {
                out.push('(');
                render_type(out, element);
                out.push(')');
            } else {
                render_type(out, element);
            }
            out.push_str("[]");
        }
        TsType::Object(members) => {
            // Review of #1358, finding 1: a `Method` member's own `doc` has
            // no single-line form — this inline shape has nowhere to put a
            // JSDoc block, unlike `TsDecl::Interface`'s own render arm
            // (which calls `render_doc_comment` before a documented
            // member's own line, since it has `depth` and a real newline
            // budget to work with). The identical case #1338's own review
            // already ruled on for `TsObjectEntry::Method.doc` in
            // `render_object_entry_inline` — a loud check, not a silent
            // drop, since silently dropping a JSDoc block is a strictly
            // worse failure mode than a loud one. Not reachable today
            // (`workers_entry.rs`'s own one real `TsType::Object` method
            // member goes through `TsTypeMember::method`, so `doc` is
            // always `None`) — the #1337 case was equally unreachable,
            // which is exactly why it got the assert.
            debug_assert!(
                !members
                    .iter()
                    .any(|m| matches!(m, TsTypeMember::Method { doc: Some(_), .. })),
                "render_type: a Method member's own doc comment has no single-line form"
            );
            if members.is_empty() {
                out.push_str("{}");
            } else {
                out.push_str("{ ");
                for (i, m) in members.iter().enumerate() {
                    if i > 0 {
                        out.push_str("; ");
                    }
                    render_type_member(out, m);
                }
                out.push_str(" }");
            }
        }
        TsType::Fn { params, ret } => {
            out.push('(');
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push('a');
                out.push_str(&i.to_string());
                out.push_str(": ");
                render_type(out, p);
            }
            out.push_str(") => ");
            render_type(out, ret);
        }
        TsType::Union {
            members,
            multiline: false,
        } => {
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push_str(" | ");
                }
                render_type(out, m);
            }
        }
        // #1339: `emit_sum_type`'s own real shape — one variant per line, a
        // leading `|` on every line except the first (which gets equivalent
        // spacing instead, matching the pre-conversion `writeln!` code's own
        // `let pipe = if i == 0 { " " } else { "|" };` exactly), the closing
        // `;` appended by `TsDecl::TypeAlias`'s own caller directly after
        // the last member's own line — see `TsType::Union`'s own doc for
        // why this needs no depth parameter despite the multi-line shape.
        TsType::Union {
            members,
            multiline: true,
        } => {
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push('\n');
                }
                out.push_str("  ");
                out.push_str(if i == 0 { " " } else { "|" });
                out.push(' ');
                render_type(out, m);
            }
        }
        TsType::Intersection(members) => {
            for (i, m) in members.iter().enumerate() {
                if i > 0 {
                    out.push_str(" & ");
                }
                render_type(out, m);
            }
        }
    }
}

/// Whether `element` must be parenthesised before a postfix `[]`. `[]` binds
/// tighter than every prefix or infix type operator, so a readonly array
/// (`readonly T[][]` reads as `readonly (T[][])`, which tsc rejects with
/// TS1354), a function type (`() => T[]` returns the array) and a union or
/// intersection (`A | B[]`) would each be misread bare. A mutable array, a
/// named type and an object literal are postfix-safe as they are.
fn needs_parens_as_array_element(element: &TsType) -> bool {
    matches!(
        element,
        TsType::Array { readonly: true, .. }
            | TsType::Fn { .. }
            | TsType::Union { .. }
            | TsType::Intersection(_)
    )
}

/// One [`TsTypeMember`] rendered inline (no leading indent, no trailing
/// separator) — [`render_type`]'s own `TsType::Object` arm's per-member
/// renderer.
fn render_type_member(out: &mut String, member: &TsTypeMember) {
    match member {
        TsTypeMember::Prop {
            name,
            ty,
            optional,
            readonly,
        } => {
            if *readonly {
                out.push_str("readonly ");
            }
            out.push_str(name);
            if *optional {
                out.push('?');
            }
            out.push_str(": ");
            render_type(out, ty);
        }
        TsTypeMember::Method {
            name,
            generics,
            params,
            ret,
            doc: _,
        } => {
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push_str("): ");
            render_type(out, ret);
        }
        TsTypeMember::Index {
            key_name,
            key_ty,
            value_ty,
        } => {
            out.push('[');
            out.push_str(key_name);
            out.push_str(": ");
            render_type(out, key_ty);
            out.push_str("]: ");
            render_type(out, value_ty);
        }
    }
}

/// Print a single [`TsType`] on its own — the real callers this closes R7.2
/// for (`bynk-emit`'s `ts_type_ref*`/`ts_ty` families, P7.9, #1315) each
/// want one type fragment to interpolate into a larger, still-hand-built
/// line (a field's own type annotation, a parameter list, …), not a whole
/// [`TsProgram`]. No source-map/buffer machinery — [`print()`] owns that
/// for a whole document; this is the printer's other, narrower entry
/// point, sharing the same internal recursion rather than a second copy.
pub fn print_type(ty: &TsType) -> String {
    let mut out = String::new();
    render_type(&mut out, ty);
    out
}

/// Print a single [`TsExpr`] on its own — the expression-level sibling of
/// [`print_type`]'s own "one fragment, not a whole document" entry point.
/// Arc C, step (11) (#1388) need: `emit_icu_placeholder`'s own `Select` arm
/// stays one opaque, hand-built block-bodied IIFE (at the time, `TsExpr::
/// Arrow` had no block-body variant at all, and extending it for that one
/// site was rejected as disproportionate) — its own arm VALUES,
/// `emit_sub_message`'s now-real `TsExpr` results, still need stringifying
/// back into that opaque host text. #1435 (Arc E slice 1) later added
/// [`TsArrowBody::Block`] for a genuinely different real site
/// (`serialisation.rs`'s `Float` guard); `emit_icu_placeholder`'s own
/// `Select` arm was not reconverted along with it — out of that slice's own
/// scope — so this call site's opaque text stays exactly as it was. No
/// source-map/buffer machinery, matching [`print_type`]/[`print_stmt`]'s
/// own scope exactly.
pub fn print_expr(expr: &TsExpr) -> String {
    let mut out = String::new();
    render_expr(&mut out, expr);
    out
}

/// Print a single [`TsStmt`] on its own, at `depth` — the statement-level
/// sibling of [`print_type`]'s own "one fragment, not a whole document"
/// entry point. `bynk-emit`'s own #1333 need (`emit_doc_block`, a shared
/// helper spliced into ~14 still-unconverted callers' own buffers) wants
/// one statement's own printed text at a caller-supplied depth, not a
/// whole [`TsProgram`] — no source-map/buffer machinery, matching
/// [`print_type`]'s own scope exactly, and reusing `render_stmt`'s own
/// exhaustive per-kind dispatch (this module's own private renderer)
/// rather than a second copy.
/// Review of #1402: a `TsStmtKind::Raw` nested inside a `Switch` case's own
/// body (`emit_stub_rhs`'s own `ReturnsEach` dispatch, Arc C slice 33,
/// `tests_emit.rs` slice C) carries the identical "no indent of its own,
/// pre-indented at a fixed absolute depth" hazard `render_class_method`'s
/// and `render_multiline_object_entry`'s own `debug_assert!`s already guard
/// — `stmt_contains_raw` already recurses into `Switch` cases, so the same
/// check applies here, this fragment entry point's own first `Raw`-bearing
/// `Switch` caller. A bare `Raw` passed directly as `stmt` itself is exempt
/// (not a false negative — `render_stmt`'s own `Raw` arm never reads
/// `depth` at all, so calling `print_stmt` on a bare `Raw` is safe at any
/// depth, the established `print_stmt_renders_raw_text_verbatim_with_no_
/// added_indent_or_punctuation` contract below): only a Raw *nested inside*
/// a depth-using wrapper (like this `Switch` case) is the real hazard.
pub fn print_stmt(stmt: &TsStmt, depth: usize) -> String {
    debug_assert!(
        depth == 0 || matches!(stmt.kind, TsStmtKind::Raw(_)) || !stmt_contains_raw(stmt),
        "print_stmt: a Raw-bearing statement's own baked-in indent only matches depth 0"
    );
    let mut out = String::new();
    render_stmt(&mut out, stmt, depth, None);
    out
}

/// [`print_stmt`]'s own sibling (#1477), the `TsDecl::Function`-body
/// counterpart to [`print_class_method_and_merge`]: identical rendering,
/// but merges any `nested_map` `stmt` carries — on `stmt` itself, or on
/// anything nested inside it (a function's body, a class's constructor/
/// methods, …) — into `map` as it prints, at the real print-time offset —
/// the same offset `bynk-emit`'s own `emit_free_fn` (`emitter/emit.rs`) used
/// to recover by exact arithmetic over the returned text, guarded by a
/// `debug_assert!`, before #1480 converted it to set `nested_map` directly
/// instead. Kept separate from `print_stmt` itself so every existing
/// caller's own call sites are untouched. `source_id` is `map`'s
/// own registered source this statement's content belongs to (see
/// this crate's own private `MergeTarget`'s own doc for why this can't just be hardcoded).
///
/// **Appends to the caller's own `out`, unlike `print_stmt`'s own
/// return-a-fresh-`String` shape** — the merge computes each checkpoint's
/// absolute offset from `out.len()` *as this statement's own text is
/// written*, so it must be the caller's real, already-populated buffer, not
/// a throwaway local one starting at 0. Returning a fresh string here (the
/// first shape this function took, caught before merge) would silently
/// record every checkpoint at the wrong offset the moment the caller
/// spliced the returned text anywhere but the very start of its own buffer.
pub fn print_stmt_and_merge(
    out: &mut String,
    stmt: &TsStmt,
    depth: usize,
    map: &mut SourceMapBuilder,
    source_id: usize,
) {
    debug_assert!(
        depth == 0 || matches!(stmt.kind, TsStmtKind::Raw(_)) || !stmt_contains_raw(stmt),
        "print_stmt_and_merge: a Raw-bearing statement's own baked-in indent only matches depth 0"
    );
    render_stmt(out, stmt, depth, Some(MergeTarget { map, source_id }));
}

/// Print a single [`TsObjectEntry`] on its own, at `depth` — the
/// object-entry sibling of [`print_stmt`]'s own "one fragment, not a whole
/// document" entry point, `depth` meaning the SAME thing it does for this
/// crate's own internal multi-line-object renderer: the *object's* own
/// depth, so the entry itself lands one level deeper, matching an object built by that
/// renderer exactly. #1337's own real need: `emit_attached_methods` (a
/// shared helper spliced into `emit_refined_type`/`emit_record_type`/
/// `emit_sum_type`'s own still-unconverted `&mut String` buffers) now
/// returns `Vec<TsObjectEntry>` instead of writing text directly — each
/// caller renders the returned entries one at a time through this, the
/// same P7.9/#1333 "keep the caller's own signature, print just the
/// fragment" pattern applied to an object-entry-shaped fragment instead of
/// a whole statement or type.
///
/// Review of #1338, finding 3: a `TsStmtKind::Raw` body statement (e.g.
/// `emit_method`'s own opaque `lower.rs`-sourced body, or `emit_refined_
/// type`'s own opaque `emit_refined_checks`-sourced guard body, #1339's
/// second real use) carries NO indent of its own — its text is captured
/// pre-indented at a fixed absolute depth by its own caller — so it only
/// renders correctly when `depth` is `0`. The guard for this now lives in
/// `render_multiline_object_entry` itself (a private renderer, so named
/// here in text rather than linked — moved there by review of #1340,
/// finding 1: this function's own copy missed the `TsExpr::
/// multiline_object_entries`/`render_expr` call path #1339 added, which
/// reaches that renderer directly, never through this one), so it fires
/// for every caller, not just this entry point.
pub fn print_object_entry(entry: &TsObjectEntry, depth: usize) -> String {
    let mut out = String::new();
    render_multiline_object_entry(&mut out, entry, depth, None);
    out
}

/// [`print_object_entry`]'s own sibling (#1477), the `TsObjectEntry::Method`
/// counterpart to [`print_class_method_and_merge`]/[`print_stmt_and_merge`]:
/// identical rendering, but merges a `Method` entry's own body `nested_map`
/// into `map` as it prints, at the real print-time offset. Kept separate
/// from `print_object_entry` itself so every existing caller's own call
/// sites are untouched.
///
/// Appends to the caller's own `out`, the same "must be the real buffer,
/// not a throwaway local one" reasoning [`print_stmt_and_merge`]'s own doc
/// explains in full. `source_id`: see this crate's own private `MergeTarget`'s own doc.
pub fn print_object_entry_and_merge(
    out: &mut String,
    entry: &TsObjectEntry,
    depth: usize,
    map: &mut SourceMapBuilder,
    source_id: usize,
) {
    render_multiline_object_entry(out, entry, depth, Some(MergeTarget { map, source_id }));
}

/// Whether `stmts`, or any block/branch/case nested inside them at any
/// depth, contains a [`TsStmtKind::Raw`] — the recursive check
/// [`render_class_method`]'s and [`render_multiline_object_entry`]'s own
/// `debug_assert!`s need. Review of #1374: a top-level-only scan
/// (`body.iter().any(...)`) is blind to a `Raw` buried inside an `If`'s own
/// `Block` — `emit_agent`'s own `commitState` (#1373) is exactly this shape,
/// a transition's own hoisted-statement `Raw`s sitting inside
/// `if (__prior !== undefined) { ... }` — so the two guards this exists for
/// would have silently missed exactly the case they exist to catch. Every
/// `TsStmtKind` variant is named explicitly, not a wildcard, matching this
/// crate's own exhaustiveness discipline.
fn contains_raw(stmts: &[TsStmt]) -> bool {
    stmts.iter().any(stmt_contains_raw)
}

fn stmt_contains_raw(stmt: &TsStmt) -> bool {
    match &stmt.kind {
        TsStmtKind::Raw(_) => true,
        TsStmtKind::Verbatim { .. }
        | TsStmtKind::Decl(_)
        | TsStmtKind::Const { .. }
        | TsStmtKind::Let { .. }
        | TsStmtKind::ExprStmt(_)
        | TsStmtKind::Return(_)
        | TsStmtKind::Throw(_)
        | TsStmtKind::Continue
        | TsStmtKind::Assign { .. }
        | TsStmtKind::Comment(_)
        | TsStmtKind::DocComment(_)
        | TsStmtKind::Blank
        | TsStmtKind::Increment(_) => false,
        TsStmtKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            stmt_contains_raw(then_branch) || else_branch.as_deref().is_some_and(stmt_contains_raw)
        }
        TsStmtKind::ForOf { body, .. } | TsStmtKind::For { body, .. } => stmt_contains_raw(body),
        TsStmtKind::TryCatch {
            try_block,
            catch_block,
            ..
        } => stmt_contains_raw(try_block) || stmt_contains_raw(catch_block),
        TsStmtKind::Block(stmts) | TsStmtKind::InlineBlock(stmts) => contains_raw(stmts),
        TsStmtKind::Switch { cases, .. } => cases.iter().any(|c| contains_raw(&c.body)),
    }
}

/// Print a single [`TsClassMethod`] on its own, at `depth` — the
/// class-method sibling of [`print_object_entry`]'s own "one fragment, not
/// a whole document" entry point. `depth` means the *class's* own depth
/// (matching `print_object_entry`'s own convention exactly), so the method
/// itself lands at `depth + 1`, its own body at `depth + 2`. #1359's own
/// real need: `emit_provider`'s own class wrapper stays hand-written text
/// (each method's own body needs a per-method source-map sub-builder/
/// `merge`, and the wrapper's own real spacing — no blank line between
/// methods — genuinely differs from [`TsDecl::Class`]'s own "one blank
/// line before each method" policy, `events_fanout.rs`'s real convention,
/// #1317), so each real method prints through here directly into that
/// still-hand-written wrapper. Deliberately no automatic blank-line
/// insertion of its own — the same "caller controls spacing" contract
/// `print_object_entry` already established.
pub fn print_class_method(method: &TsClassMethod, depth: usize) -> String {
    let mut out = String::new();
    render_class_method(&mut out, method, depth, None);
    out
}

/// [`print_class_method`]'s own sibling (#1477): identical rendering, but
/// merges any of `method.body`'s own direct-child `nested_map`s into `map`
/// as they print, at the real print-time offset — no reverse-engineering
/// the offset from the returned text afterward, the way `bynk-emit`'s own
/// `emit_class_method_and_merge_source_map` (`emitter/emit.rs`) has to
/// today. Kept as a separate function, not an added parameter on
/// `print_class_method` itself, so every existing caller's own call sites
/// are untouched — this is a strict addition.
///
/// Appends to the caller's own `out`, the same "must be the real buffer,
/// not a throwaway local one" reasoning [`print_stmt_and_merge`]'s own doc
/// explains in full. `source_id`: see this crate's own private `MergeTarget`'s own doc.
pub fn print_class_method_and_merge(
    out: &mut String,
    method: &TsClassMethod,
    depth: usize,
    map: &mut SourceMapBuilder,
    source_id: usize,
) {
    render_class_method(out, method, depth, Some(MergeTarget { map, source_id }));
}

/// The one real per-method render, shared by [`print_class_method`] and
/// [`TsDecl::Class`]'s own methods-loop arm — review of #1360, finding 4:
/// keeping two independent copies of this shape was exactly the drift risk
/// this track exists to design against (two printers disagreeing about a
/// formatting convention). `depth` means the *class's* own depth, matching
/// both callers' own convention.
///
/// Review of #1360, finding 1: a `TsStmtKind::Raw` method body has no
/// indent of its own — its text is captured pre-indented at a fixed
/// absolute depth by its own caller — so it only renders correctly when
/// `depth` is `0`, the same hazard `render_multiline_object_entry`'s own
/// identical `debug_assert!` guards (review of #1338 finding 3, relocated
/// by review of #1340 finding 1). `print_class_method`'s own doc frames it
/// as a general fragment entry point, so a future caller at a non-zero
/// depth is the expected case this guards against, not a hypothetical.
///
/// `map`: see [`render_block_stmts`]'s own doc (#1477) — forwarded
/// unchanged to `method.body`'s own rendering.
fn render_class_method(
    out: &mut String,
    method: &TsClassMethod,
    depth: usize,
    map: Option<MergeTarget<'_>>,
) {
    debug_assert!(
        depth == 0 || !contains_raw(&method.body),
        "render_class_method: a Raw method body's own baked-in indent only matches depth 0"
    );
    if let Some(text) = &method.doc {
        render_doc_comment(out, text, depth + 1);
    }
    out.push_str(&indent(depth + 1));
    if method.private {
        out.push_str("private ");
    }
    if method.is_async {
        out.push_str("async ");
    }
    out.push_str(&method.name);
    out.push('(');
    render_params(out, &method.params);
    out.push(')');
    if let Some(rt) = &method.return_type {
        out.push_str(": ");
        render_type(out, rt);
    }
    render_block_stmts(out, &method.body, depth + 1, map);
    out.push('\n');
}

fn render_params(out: &mut String, params: &[TsParam]) {
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&p.name);
        if p.optional {
            out.push('?');
        }
        if let Some(ty) = &p.ty {
            out.push_str(": ");
            render_type(out, ty);
        }
    }
}

fn render_decl(out: &mut String, decl: &TsDecl, depth: usize, map: Option<MergeTarget<'_>>) {
    // `render_decl_body`'s own `TsDecl::Export` arm already writes
    // `"export "` before recursing into the inner declaration — the
    // `if let Export` special case this replaced produced byte-identical
    // output by duplicating that same logic one layer up (review of
    // #1314, smaller note: the two branches were provably the same for
    // every input, so only one needs to exist).
    out.push_str(&indent(depth));
    render_decl_body(out, decl, depth, map);
}

/// The declaration's own text, with no leading indent — [`render_decl`]
/// writes the indent (and, for `Export`, the `export ` keyword) once, then
/// hands off here so `Export(inner)` doesn't duplicate `inner`'s own
/// leading whitespace.
///
/// `map`: see [`render_block_stmts`]'s own doc (#1477) — reaches a
/// `TsDecl::Function`'s own body, and a `TsDecl::Class`'s own constructor/
/// methods, unchanged; every other arm ignores it (no body of its own to
/// carry a `nested_map`).
fn render_decl_body(
    out: &mut String,
    decl: &TsDecl,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    match decl {
        TsDecl::Import {
            type_only,
            names,
            from,
        } => {
            out.push_str(if *type_only {
                "import type { "
            } else {
                "import { "
            });
            out.push_str(&names.join(", "));
            out.push_str(" } from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ImportNamespace {
            type_only,
            alias,
            from,
        } => {
            out.push_str(if *type_only {
                "import type * as "
            } else {
                "import * as "
            });
            out.push_str(alias);
            out.push_str(" from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ImportDefault { alias, from } => {
            out.push_str("import ");
            out.push_str(alias);
            out.push_str(" from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ReExport { names, from } => {
            out.push_str("export { ");
            out.push_str(&names.join(", "));
            out.push_str(" } from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::ExportNames { names } => {
            out.push_str("export { ");
            out.push_str(&names.join(", "));
            out.push_str(" };\n");
        }
        TsDecl::ReExportAll { from } => {
            out.push_str("export * from \"");
            out.push_str(from);
            out.push_str("\";\n");
        }
        TsDecl::Export(inner) => {
            out.push_str("export ");
            render_decl_body(out, inner, depth, map);
        }
        TsDecl::Interface {
            name,
            type_params,
            members,
        } => {
            out.push_str("interface ");
            out.push_str(name);
            render_bare_generics(out, type_params);
            out.push_str(" {\n");
            for member in members {
                // #1357: a `Method` member's own `doc` renders here, not in
                // `render_type_member` itself — that function has no
                // `depth` to give `render_doc_comment`, the same "doc lives
                // at the call site that carries depth" split
                // `render_multiline_object_entry`'s own `Method` arm
                // already uses for the identical field.
                if let TsTypeMember::Method {
                    doc: Some(text), ..
                } = member
                {
                    render_doc_comment(out, text, depth + 1);
                }
                out.push_str(&indent(depth + 1));
                render_type_member(out, member);
                out.push_str(";\n");
            }
            out.push_str(&indent(depth));
            out.push_str("}\n");
        }
        TsDecl::ConstDecl { name, ty, init } => {
            out.push_str("const ");
            out.push_str(name);
            if let Some(ty) = ty {
                out.push_str(": ");
                render_type(out, ty);
            }
            out.push_str(" = ");
            render_stmt_level_expr(out, init, depth);
            out.push_str(";\n");
        }
        TsDecl::Class {
            name,
            fields,
            constructor,
            methods,
        } => {
            out.push_str("class ");
            out.push_str(name);
            out.push_str(" {\n");
            for f in fields {
                out.push_str(&indent(depth + 1));
                if f.private {
                    out.push_str("private ");
                }
                out.push_str(&f.name);
                out.push_str(": ");
                render_type(out, &f.ty);
                out.push_str(";\n");
            }
            // Readability policy (this module's own doc): no blank line
            // between fields and the constructor; one blank line before
            // each method — `events_fanout.rs`'s own real class spacing.
            let mut wrote_member = !fields.is_empty();
            if let Some(ctor) = constructor {
                out.push_str(&indent(depth + 1));
                out.push_str("constructor(");
                render_params(out, &ctor.params);
                out.push(')');
                render_block_stmts(
                    out,
                    &ctor.body,
                    depth + 1,
                    map.as_mut().map(|t| t.reborrow()),
                );
                out.push('\n');
                wrote_member = true;
            }
            for m in methods {
                if wrote_member {
                    out.push('\n');
                }
                render_class_method(out, m, depth, map.as_mut().map(|t| t.reborrow()));
                wrote_member = true;
            }
            out.push_str(&indent(depth));
            out.push_str("}\n");
        }
        TsDecl::Function {
            name,
            generics,
            params,
            return_type,
            body,
            is_async,
            inline,
        } => {
            if *is_async {
                out.push_str("async ");
            }
            out.push_str("function ");
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            if *inline {
                out.push(' ');
                render_inline_block(out, body);
            } else {
                render_block_stmts(out, body, depth, map);
                out.push('\n');
            }
        }
        TsDecl::TypeAlias {
            name,
            type_params,
            ty,
        } => {
            out.push_str("type ");
            out.push_str(name);
            render_bare_generics(out, type_params);
            // #1339: a multiline Union's own first rendered character is
            // its first member's leading indent, not a newline — the `=`
            // itself must be followed directly by `\n` here (no space
            // before it), matching the pre-conversion `writeln!(out,
            // "export type {name}{params} =")` line's own exact bytes;
            // every other `ty` keeps the ordinary `" = "` (space both
            // sides) ordinary single-line form.
            if matches!(
                ty,
                TsType::Union {
                    multiline: true,
                    ..
                }
            ) {
                out.push_str(" =\n");
            } else {
                out.push_str(" = ");
            }
            render_type(out, ty);
            out.push_str(";\n");
        }
        TsDecl::ExportDefault(expr) => {
            out.push_str("export default ");
            render_stmt_level_expr(out, expr, depth);
            out.push_str(";\n");
        }
        TsDecl::DeclareConst { name, ty } => {
            out.push_str("declare const ");
            out.push_str(name);
            out.push_str(": ");
            render_type(out, ty);
            out.push_str(";\n");
        }
    }
}

/// `{ <stmts> }` for a constructor/method body — same shape as
/// [`render_block_body`] but over a plain `&[TsStmt]` (a `TsClassCtor`/
/// `TsClassMethod`'s own `body` field, not a boxed `TsStmt`), and always
/// followed by the caller's own `\n` (constructor/method declarations, not
/// a same-line `try`/`catch` continuation).
///
/// `map: Some(_)` when the caller wants a `nested_map`-bearing statement
/// reachable from `stmts` merged into a module source map as it prints
/// (#1477) — see [`crate::program::TsStmt::nested_map`]'s own doc for the
/// problem this solves. Reborrowed once per child and forwarded to
/// [`render_stmt`], which does the actual check — this function itself
/// holds none of that logic.
fn render_block_stmts(
    out: &mut String,
    stmts: &[TsStmt],
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    out.push_str(" {\n");
    for s in stmts {
        render_stmt(out, s, depth + 1, map.as_mut().map(|t| t.reborrow()));
    }
    out.push_str(&indent(depth));
    out.push('}');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::program::{TsClassCtor, TsClassField, TsClassMethod, TsSwitchCase, VerbatimOrigin};
    use bynk_syntax::span::Span;

    /// Pins the readability policy's statement-separation guarantee (R7.5,
    /// this module's own doc): every statement starts on its own generated
    /// line.
    #[test]
    fn prints_every_statement_in_order() {
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Contracts,
            "const a = 1;\n",
            None,
        ));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            "const b = 2;\n",
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "const a = 1;\nconst b = 2;\n");
    }

    /// Pins the readability policy's statement-separation guarantee (R7.5,
    /// this module's own doc) in its sharpest form: nothing requires
    /// `Verbatim` text to be newline-terminated — the printer owns line
    /// structure (R7.3), so it's the one that guarantees two statements
    /// never share a generated line, not a `TsStmt::verbatim` caller
    /// obligation nobody enforces (review of #1308, finding 2).
    #[test]
    fn a_statement_missing_its_own_trailing_newline_still_gets_its_own_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Contracts,
            "const a = 1;",
            None,
        ));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            "const b = 2;",
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "const a = 1;\nconst b = 2;\n");
    }

    /// Production `Verbatim` content is rarely one line — `VerbatimOrigin::
    /// NotYetConverted` (P7.6) wraps a whole generated document per
    /// statement, so one statement routinely spans dozens of generated
    /// lines. Neither of the two tests above exercised that shape (review
    /// of #1312, finding 1) — this one confirms the readability policy's
    /// real invariant (the *next* statement always starts on a fresh line)
    /// holds for multi-line content too, not just the single-line text
    /// those tests happened to use.
    #[test]
    fn a_multi_line_statement_still_starts_the_next_statement_on_a_fresh_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Contracts,
            "function a() {\n  return 1;\n}\n",
            None,
        ));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            "const b = 2;\n",
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "function a() {\n  return 1;\n}\nconst b = 2;\n"
        );
    }

    /// `print` decides whether to append a trailing newline by checking the
    /// whole *buffer*, not the statement's own text (review of #1312,
    /// finding 2) — an empty statement's text changes nothing, so whether
    /// it gets a line depends entirely on what's already in the buffer.
    /// First in a program, the buffer is still empty, so the check fires
    /// and the output opens with a blank line. Pinned here so this edge
    /// stays a known, deliberate reading of the code rather than an
    /// untested accident.
    #[test]
    fn an_empty_statement_first_in_the_program_yields_a_leading_blank_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(VerbatimOrigin::Contracts, "", None));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            "const b = 2;\n",
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "\nconst b = 2;\n");
    }

    /// The other half of the buffer-vs-statement-text distinction (review
    /// of #1312, finding 2): an empty statement following one that already
    /// ended its own text in `\n` contributes no line at all — the buffer
    /// already ends in `\n`, so the check doesn't fire, and the empty
    /// statement's own (non-)content and the next statement's text end up
    /// on what reads as one generated line for the next statement alone.
    #[test]
    fn an_empty_statement_after_a_newline_terminated_one_contributes_no_line_of_its_own() {
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Contracts,
            "const a = 1;\n",
            None,
        ));
        program.push(TsStmt::verbatim(VerbatimOrigin::Secrets, "", None));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::RuntimeUse,
            "const c = 3;\n",
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "const a = 1;\nconst c = 3;\n");
    }

    #[test]
    fn no_spans_at_all_means_no_source_map() {
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(VerbatimOrigin::Contracts, "x;\n", None));
        let printed = print(&program, "x.bynk", "let x = 1\n", "x.ts");
        assert_eq!(printed.source_map, None);
    }

    /// The property that actually matters (R7.4): the printer's own
    /// buffer-position bookkeeping is correct — a checkpoint recorded from a
    /// later statement's span still resolves to the *generated* line that
    /// statement's own text landed on, not (as the old splice-based
    /// mechanism could get wrong, #4) some other buffer's offset.
    ///
    /// Review of #1308, finding 3: the original version of this test
    /// asserted only that the source map's `sources` array was present —
    /// true even with the second statement's checkpoint silently dropped,
    /// exactly the failure this doc comment claims is ruled out. Asserting
    /// full equality against a map built independently (replicating the
    /// printer's own record-then-write order by hand, not calling `print`)
    /// actually pins both statements' own offsets.
    #[test]
    fn each_statements_span_resolves_to_its_own_generated_line() {
        let source = "let a = 1\nlet b = 2\n";
        let off_a = source.find("let a").unwrap();
        let off_b = source.find("let b").unwrap();
        let span_a = Span::new(off_a, off_a + 5);
        let span_b = Span::new(off_b, off_b + 5);
        let text_a = "const a = 1;\n";
        let text_b = "const b = 2;\n";
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Contracts,
            text_a,
            Some(span_a),
        ));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            text_b,
            Some(span_b),
        ));

        let printed = print(&program, "x.bynk", source, "x.ts");

        let mut expected_map = SourceMapBuilder::new();
        expected_map.add_source("x.bynk", source);
        expected_map.record(0, span_a);
        expected_map.record(text_a.len(), span_b);
        let expected = expected_map.to_v3(&printed.text, "x.ts");

        assert_eq!(printed.source_map, expected);
    }

    // -- #1477: nested source-map checkpoints on an opaque body blob. --

    /// `print_stmt_and_merge` — the `TsDecl::Function` shape (`emit_free_fn`'s
    /// own real site): a `Raw` body statement's own `nested_map` merges into
    /// the caller's map at the real print-time offset, appended into a
    /// buffer that already has *other* content first (`base != 0`) — proving
    /// this reads the caller's own real buffer position, not a fresh
    /// throwaway one starting at 0 (the bug this function's own first draft
    /// had, caught before merge: returning a `String` computes the offset
    /// against that empty local buffer, silently wrong the moment the
    /// caller splices the result anywhere but the very start of its own
    /// `out`).
    #[test]
    fn print_stmt_and_merge_merges_a_function_bodys_own_nested_checkpoint() {
        let source = "fn f() { return 1 }\n";
        let ret_off = source.find("return 1").unwrap();
        let ret_span = Span::new(ret_off, ret_off + 8);

        let body_text = "  return 1;\n";
        let mut nested = SourceMapBuilder::new();
        nested.record(0, ret_span);

        let mut raw = TsStmt::raw(body_text, None);
        raw.nested_map = Some(nested);

        let func = TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body: vec![raw],
                is_async: false,
                inline: false,
            },
            None,
        );

        let mut out = String::from("// header\n");
        let mut map = SourceMapBuilder::new();
        map.add_source("x.bynk", source);

        print_stmt_and_merge(&mut out, &func, 0, &mut map, 0);

        assert_eq!(out, "// header\nfunction f() {\n  return 1;\n}\n");
        let base = out.find(body_text).unwrap();

        let mut expected = SourceMapBuilder::new();
        expected.add_source("x.bynk", source);
        expected.record(base, ret_span);
        assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
    }

    /// `print_class_method_and_merge` — the `TsClassMethod` shape
    /// (`emit_provider`/`emit_agent`'s own real site, today reverse-engineered
    /// by `emit_class_method_and_merge_source_map`'s own string-matching).
    /// Same non-zero-`base` proof as the `TsDecl::Function` test above.
    #[test]
    fn print_class_method_and_merge_merges_a_methods_own_nested_checkpoint() {
        let source = "on call m() { commit }\n";
        let commit_off = source.find("commit").unwrap();
        let commit_span = Span::new(commit_off, commit_off + 6);

        let body_text = "    this.commit();\n";
        let mut nested = SourceMapBuilder::new();
        nested.record(0, commit_span);

        let mut raw = TsStmt::raw(body_text, None);
        raw.nested_map = Some(nested);

        let method = TsClassMethod {
            name: "m".to_string(),
            private: false,
            is_async: false,
            params: Vec::new(),
            return_type: None,
            doc: None,
            body: vec![raw],
        };

        let mut out = String::from("class C {\n");
        let mut map = SourceMapBuilder::new();
        map.add_source("x.bynk", source);

        print_class_method_and_merge(&mut out, &method, 0, &mut map, 0);

        let base = out.find(body_text).unwrap();
        let mut expected = SourceMapBuilder::new();
        expected.add_source("x.bynk", source);
        expected.record(base, commit_span);
        assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
    }

    /// `print_object_entry_and_merge` — the `TsObjectEntry::Method` shape
    /// (`emit_service`'s own real per-handler site): identical mechanism,
    /// exercised through its own, third public entry point.
    #[test]
    fn print_object_entry_and_merge_merges_a_methods_own_nested_checkpoint() {
        let source = "on call h() { reply }\n";
        let reply_off = source.find("reply").unwrap();
        let reply_span = Span::new(reply_off, reply_off + 5);

        let body_text = "    return reply();\n";
        let mut nested = SourceMapBuilder::new();
        nested.record(0, reply_span);

        let mut raw = TsStmt::raw(body_text, None);
        raw.nested_map = Some(nested);

        let entry = TsObjectEntry::Method {
            name: "h".to_string(),
            is_async: false,
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            doc: None,
            inline: false,
            body: vec![raw],
        };

        let mut out = String::from("export const svc = {\n");
        let mut map = SourceMapBuilder::new();
        map.add_source("x.bynk", source);

        print_object_entry_and_merge(&mut out, &entry, 0, &mut map, 0);

        let base = out.find(body_text).unwrap();
        let mut expected = SourceMapBuilder::new();
        expected.add_source("x.bynk", source);
        expected.record(base, reply_span);
        assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
    }

    /// `print()`'s own top-level loop (#1477): a real `TsProgram` containing
    /// a `Decl`-kinded top-level statement whose own body carries a
    /// `nested_map` merges it automatically — no `bynk-emit` change needed
    /// to benefit once a real construction site sets the field, since
    /// `print()` already threads its own module map through `render_stmt`
    /// for every top-level statement.
    #[test]
    fn print_merges_a_top_level_functions_own_nested_checkpoint_automatically() {
        let source = "fn f() { return 1 }\n";
        let ret_off = source.find("return 1").unwrap();
        let ret_span = Span::new(ret_off, ret_off + 8);

        let body_text = "  return 1;\n";
        let mut nested = SourceMapBuilder::new();
        nested.record(0, ret_span);

        let mut raw = TsStmt::raw(body_text, None);
        raw.nested_map = Some(nested);

        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body: vec![raw],
                is_async: false,
                inline: false,
            },
            None,
        ));

        let printed = print(&program, "x.bynk", source, "x.ts");
        let base = printed.text.find(body_text).unwrap();

        let mut expected = SourceMapBuilder::new();
        expected.add_source("x.bynk", source);
        expected.record(base, ret_span);
        assert_eq!(printed.source_map, expected.to_v3(&printed.text, "x.ts"));
    }

    /// Review of #1488, finding 1: `source_id` must reach the merge, not be
    /// hardcoded — a multi-source module map (a test module aggregating
    /// several `.bynk` files, `tests_emit.rs:537`/`:1936`'s own real shape)
    /// tags a nested checkpoint to the *wrong* source silently if the merge
    /// always assumes index 0.
    ///
    /// Decisive by construction, not just an assertion on the map's own
    /// `sources` list (which would pass either way — every registered
    /// source name appears there regardless of which one a checkpoint
    /// actually resolves against, `to_v3`'s own doc/body confirms `sources`/
    /// `sourcesContent` iterate the full registered list unconditionally):
    /// source 0's own text is a single short line, too short to contain the
    /// real checkpoint's span. If the merge resolves the checkpoint against
    /// source 0 (the bug this test exists to catch), `to_v3`'s own
    /// out-of-range guard (`span.start > text.len()`) silently drops it and
    /// the whole map comes back `None` (its only checkpoint). Only
    /// resolving against the real source (id 1, correctly threaded through)
    /// keeps the span in range and produces a real map.
    #[test]
    fn print_stmt_and_merge_tags_a_nested_checkpoint_to_the_given_source_id() {
        let primary = "x\n"; // deliberately shorter than `ret_span`'s own offset
        let secondary = "fn f() { return 1 }\n";
        let ret_off = secondary.find("return 1").unwrap();
        let ret_span = Span::new(ret_off, ret_off + 8);
        assert!(
            ret_span.start > primary.len(),
            "sanity: the span must fall outside source 0's own short text"
        );

        let body_text = "  return 1;\n";
        let mut nested = SourceMapBuilder::new();
        nested.record(0, ret_span);

        let mut raw = TsStmt::raw(body_text, None);
        raw.nested_map = Some(nested);

        let func = TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body: vec![raw],
                is_async: false,
                inline: false,
            },
            None,
        );

        let mut out = String::new();
        let mut map = SourceMapBuilder::new();
        map.add_source("primary.bynk", primary); // id 0
        let secondary_id = map.add_source("secondary.bynk", secondary); // id 1
        assert_eq!(secondary_id, 1);

        print_stmt_and_merge(&mut out, &func, 0, &mut map, secondary_id);

        assert!(
            map.to_v3(&out, "x.ts").is_some(),
            "the checkpoint resolved against the wrong source (id 0) and was silently dropped"
        );
    }

    /// #1486's own sibling gap to the one above, but in [`print_multi_
    /// source`]'s own automatic top-level loop rather than the manual
    /// `print_stmt_and_merge` entry point: a real `TsProgram` built for a
    /// multi-source module (a test/integration module aggregating several
    /// `.bynk` fragment files, one per case) needs each case's own
    /// top-level statement merged against *its own* registered source, not
    /// the module's primary one (id 0) — `nested_map_source_id`'s own doc
    /// comment. Decisive the same way the sibling test above is: source 0's
    /// text is too short to contain the real span, so a wrong-source merge
    /// silently drops the checkpoint (`to_v3`'s own out-of-range guard) and
    /// the map comes back missing it.
    #[test]
    fn print_multi_source_honours_a_top_level_statements_own_nested_map_source_id() {
        let primary = "x\n";
        let secondary = "fn f() { return 1 }\n";
        let ret_off = secondary.find("return 1").unwrap();
        let ret_span = Span::new(ret_off, ret_off + 8);
        assert!(
            ret_span.start > primary.len(),
            "sanity: the span must fall outside source 0's own short text"
        );

        let body_text = "  return 1;\n";
        let mut nested = SourceMapBuilder::new();
        nested.record(0, ret_span);

        let mut raw = TsStmt::raw(body_text, None);
        raw.nested_map = Some(nested);

        let mut func = TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body: vec![raw],
                is_async: false,
                inline: false,
            },
            None,
        );
        func.nested_map_source_id = 1;

        let mut program = TsProgram::new();
        program.push(func);

        let mut map = SourceMapBuilder::new();
        map.add_source("primary.bynk", primary);
        map.add_source("secondary.bynk", secondary);
        let printed = print_multi_source(&program, map, "x.ts");
        assert!(
            printed.source_map.is_some(),
            "the checkpoint resolved against the wrong source (id 0) and was silently dropped"
        );
    }

    /// Review of #1488, finding 2: a `nested_map` set on the statement handed
    /// directly to `print`/`print_stmt_and_merge` itself — not nested inside
    /// a `Decl` body — must still merge, not silently vanish. The first
    /// version of this mechanism only checked a body's own *direct
    /// children* (inside `render_block_stmts`), missing this exact case.
    #[test]
    fn print_stmt_and_merge_merges_a_bare_top_level_raw_statements_own_nested_checkpoint() {
        let source = "contract seal() {}\n";
        let off = source.find("contract").unwrap();
        let span = Span::new(off, off + 8);

        let text = "seal();\n";
        let mut nested = SourceMapBuilder::new();
        nested.record(0, span);

        let mut raw = TsStmt::raw(text, None);
        raw.nested_map = Some(nested);

        let mut out = String::from("// header\n");
        let mut map = SourceMapBuilder::new();
        map.add_source("x.bynk", source);

        print_stmt_and_merge(&mut out, &raw, 0, &mut map, 0);

        let base = out.find(text).unwrap();
        let mut expected = SourceMapBuilder::new();
        expected.add_source("x.bynk", source);
        expected.record(base, span);
        assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
    }

    /// Review of #1488, finding 3 (part 1): the merge's own line-rebasing
    /// arithmetic (`out_line = base_line + body_line`) is exercised for
    /// real, not just the degenerate `body_line == 0` case every other test
    /// here uses — a two-statement body with the checkpoint on the
    /// *second* line.
    #[test]
    fn print_stmt_and_merge_rebases_a_checkpoint_on_a_bodys_second_line() {
        let source = "fn f() { let a = 1\n  return a }\n";
        let ret_off = source.find("return a").unwrap();
        let ret_span = Span::new(ret_off, ret_off + 8);

        let body_text = "  const a = 1;\n  return a;\n";
        let second_line_off = body_text.find("return a;").unwrap();
        let mut nested = SourceMapBuilder::new();
        nested.record(second_line_off, ret_span);

        let mut raw = TsStmt::raw(body_text, None);
        raw.nested_map = Some(nested);

        let func = TsStmt::decl(
            TsDecl::Function {
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body: vec![raw],
                is_async: false,
                inline: false,
            },
            None,
        );

        let mut out = String::from("// header\n");
        let mut map = SourceMapBuilder::new();
        map.add_source("x.bynk", source);

        print_stmt_and_merge(&mut out, &func, 0, &mut map, 0);

        let base_of_body = out.find(body_text).unwrap();
        let base_of_second_line = out.find("  return a;\n").unwrap();
        assert!(
            base_of_second_line > base_of_body,
            "sanity: second line is after the first"
        );

        let mut expected = SourceMapBuilder::new();
        expected.add_source("x.bynk", source);
        expected.record(base_of_second_line, ret_span);
        assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
    }

    /// Review of #1488, finding 3 (part 2): `TsDecl::Class`'s own
    /// constructor-then-methods sequence is the one real place `map` is
    /// reborrowed across more than one consumer — a class with a
    /// `nested_map`-bearing constructor body *and* a `nested_map`-bearing
    /// method body proves both reborrows land correctly, not just the
    /// first one. `emit_provider`/`emit_agent` (both class-shaped) are
    /// exactly the real callers this slice unblocks.
    #[test]
    fn a_classs_constructor_and_method_both_merge_their_own_nested_checkpoints() {
        let source = "on new() { init }\non call m() { commit }\n";
        let init_off = source.find("init").unwrap();
        let init_span = Span::new(init_off, init_off + 4);
        let commit_off = source.find("commit").unwrap();
        let commit_span = Span::new(commit_off, commit_off + 6);

        let ctor_body_text = "    this.init();\n";
        let mut ctor_nested = SourceMapBuilder::new();
        ctor_nested.record(0, init_span);
        let mut ctor_raw = TsStmt::raw(ctor_body_text, None);
        ctor_raw.nested_map = Some(ctor_nested);

        let method_body_text = "    this.commit();\n";
        let mut method_nested = SourceMapBuilder::new();
        method_nested.record(0, commit_span);
        let mut method_raw = TsStmt::raw(method_body_text, None);
        method_raw.nested_map = Some(method_nested);

        let class = TsDecl::Class {
            name: "C".to_string(),
            fields: Vec::new(),
            constructor: Some(TsClassCtor {
                params: Vec::new(),
                body: vec![ctor_raw],
            }),
            methods: vec![TsClassMethod {
                name: "m".to_string(),
                private: false,
                is_async: false,
                params: Vec::new(),
                return_type: None,
                doc: None,
                body: vec![method_raw],
            }],
        };

        let mut out = String::new();
        let mut map = SourceMapBuilder::new();
        map.add_source("x.bynk", source);

        print_stmt_and_merge(&mut out, &TsStmt::decl(class, None), 0, &mut map, 0);

        let ctor_base = out.find(ctor_body_text).unwrap();
        let method_base = out.find(method_body_text).unwrap();

        let mut expected = SourceMapBuilder::new();
        expected.add_source("x.bynk", source);
        expected.record(ctor_base, init_span);
        expected.record(method_base, commit_span);
        assert_eq!(map.to_v3(&out, "x.ts"), expected.to_v3(&out, "x.ts"));
    }

    // -- P7.8 (#1313): real node rendering. --

    #[test]
    fn prints_a_const_statement_with_a_destructured_binding_and_a_type_cast() {
        let mut program = TsProgram::new();
        program.push(TsStmt::const_stmt(
            TsBindingName::ObjectPattern(vec!["events".to_string()]),
            None,
            TsExpr::As {
                // #1323: `Await` no longer auto-parenthesizes under `As` —
                // this file's own real content wraps it explicitly via
                // `Paren` (see `render_expr`'s own `As` arm doc).
                expr: Box::new(TsExpr::Paren(Box::new(TsExpr::Await(Box::new(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Member {
                            object: Box::new(TsExpr::Ident("request".to_string())),
                            property: "json".to_string(),
                        }),
                        args: vec![],
                    },
                ))))),
                ty: TsType::Object(vec![TsTypeMember::prop(
                    "events",
                    TsType::array(TsType::named("FanoutEvent")),
                )]),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "const { events } = (await request.json()) as { events: FanoutEvent[] };\n"
        );
    }

    #[test]
    fn prints_if_without_braces_when_the_body_is_not_a_block() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_stmt(
            TsExpr::Unary {
                op: TsUnaryOp::Not,
                expr: Box::new(TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("Array".to_string())),
                        property: "isArray".to_string(),
                    }),
                    args: vec![TsExpr::Ident("subs".to_string())],
                }),
            },
            TsStmt::continue_stmt(None),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (!Array.isArray(subs)) continue;\n");
    }

    /// Review of #1317/#1318, finding 2: a bare `Comment` as an `if`'s
    /// brace-free body must not fall through to the top-level `//`-line
    /// form — `if (cond) // text` comments out the rest of the physical
    /// line, leaving `if` with no body at all (a parse error). Renders as
    /// a block comment instead, which cannot swallow anything after it.
    #[test]
    fn a_comment_as_an_ifs_brace_free_body_renders_as_a_block_comment() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::comment("annotation", None),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (cond) /* annotation */\n");
    }

    #[test]
    fn prints_a_for_of_loop_with_a_braced_body() {
        let mut program = TsProgram::new();
        program.push(TsStmt::for_of(
            "ev",
            TsExpr::Ident("events".to_string()),
            TsStmt::block(
                vec![TsStmt::return_stmt(
                    Some(TsExpr::Ident("ev".to_string())),
                    None,
                )],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "for (const ev of events) {\n  return ev;\n}\n"
        );
    }

    /// Arc E slice 7 (#1447): `TsStmtKind::For` — pins the exact C-style
    /// header shape `ListInst`/`MapInst`'s own real deserialise-side element
    /// loops need (`for (let i = 0; i < json.length; i++) { ... }`),
    /// including the postfix `++` the printer appends over `name` itself
    /// (see `TsStmtKind::For`'s own doc for why there is no separate
    /// `update` field to pass).
    #[test]
    fn prints_a_c_style_for_loop_with_a_braced_body() {
        let mut program = TsProgram::new();
        program.push(TsStmt::for_stmt(
            "i",
            TsExpr::Lit(TsLit::Num("0".to_string())),
            TsExpr::Binary {
                op: TsBinaryOp::LessThan,
                left: Box::new(TsExpr::Ident("i".to_string())),
                right: Box::new(TsExpr::Member {
                    object: Box::new(TsExpr::Ident("json".to_string())),
                    property: "length".to_string(),
                }),
            },
            TsStmt::block(
                vec![TsStmt::expr_stmt(TsExpr::Ident("item".to_string()), None)],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "for (let i = 0; i < json.length; i++) {\n  item;\n}\n"
        );
    }

    /// A `For` loop's own brace-free body falls back to the same inline
    /// rendering `ForOf`'s own brace-free body already uses — exercised as
    /// the *outer* statement, over a `Continue` body (not reachable from any
    /// real `bynk-emit` content today, but a real, direct shape).
    #[test]
    fn prints_a_c_style_for_loops_brace_free_body_inline() {
        let mut program = TsProgram::new();
        program.push(TsStmt::for_stmt(
            "i",
            TsExpr::Lit(TsLit::Num("0".to_string())),
            TsExpr::Binary {
                op: TsBinaryOp::LessThan,
                left: Box::new(TsExpr::Ident("i".to_string())),
                right: Box::new(TsExpr::Ident("n".to_string())),
            },
            TsStmt::continue_stmt(None),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "for (let i = 0; i < n; i++) continue;\n");
    }

    /// Review of #1448, finding 1: the previous test above pins a brace-free
    /// `For` as the *outer* statement, which renders through `render_stmt`'s
    /// own dedicated `For` arm, not the `render_inline_stmt` fallback this
    /// slice's own `| TsStmtKind::For { .. }` line actually added. That
    /// fallback only fires when a `For` is itself the brace-free body of an
    /// ENCLOSING `if`/`for...of`/`For` — exercised directly here, nesting a
    /// brace-free `For` as an `if`'s own brace-free branch.
    #[test]
    fn prints_a_brace_free_for_loop_nested_as_an_ifs_own_brace_free_body() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::for_stmt(
                "i",
                TsExpr::Lit(TsLit::Num("0".to_string())),
                TsExpr::Binary {
                    op: TsBinaryOp::LessThan,
                    left: Box::new(TsExpr::Ident("i".to_string())),
                    right: Box::new(TsExpr::Ident("n".to_string())),
                },
                TsStmt::continue_stmt(None),
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "if (cond) for (let i = 0; i < n; i++) continue;\n"
        );
    }

    #[test]
    fn prints_a_try_catch_on_the_shared_closing_brace_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::try_catch(
            TsStmt::block(
                vec![TsStmt::expr_stmt(
                    TsExpr::Await(Box::new(TsExpr::Call {
                        callee: Box::new(TsExpr::Ident("deliverEvent".to_string())),
                        args: vec![TsExpr::Ident("binding".to_string())],
                    })),
                    None,
                )],
                None,
            ),
            Some("e"),
            TsStmt::block(
                vec![TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Member {
                            object: Box::new(TsExpr::Ident("console".to_string())),
                            property: "error".to_string(),
                        }),
                        args: vec![TsExpr::Lit(TsLit::Str("failed".to_string()))],
                    },
                    None,
                )],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "try {\n  await deliverEvent(binding);\n} catch (e) {\n  console.error(\"failed\");\n}\n"
        );
    }

    #[test]
    fn prints_an_interface_with_an_inline_nested_object_type() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Interface {
                name: "FanoutEvent".to_string(),
                type_params: Vec::new(),
                members: vec![
                    TsTypeMember::prop("type", TsType::named("string")),
                    TsTypeMember::prop("payload", TsType::named("unknown")),
                    TsTypeMember::prop(
                        "envelope",
                        TsType::Object(vec![
                            TsTypeMember::prop("eventId", TsType::named("string")),
                            TsTypeMember::prop("publisherId", TsType::named("string")),
                        ]),
                    ),
                ],
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "interface FanoutEvent {\n  type: string;\n  payload: unknown;\n  envelope: { eventId: string; publisherId: string };\n}\n"
        );
    }

    #[test]
    fn prints_two_adjacent_imports_with_no_blank_line_between_them() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Import {
                type_only: true,
                names: vec!["A".to_string()],
                from: "../x.js".to_string(),
            },
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: vec!["b".to_string()],
                from: "../x.js".to_string(),
            },
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::ConstDecl {
                name: "c".to_string(),
                ty: None,
                init: TsExpr::Lit(TsLit::Null),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "import type { A } from \"../x.js\";\nimport { b } from \"../x.js\";\n\nconst c = null;\n"
        );
    }

    /// #1486: `no_blank_before` suppresses the automatic blank line before
    /// exactly the statement that carries it — a `const`/`function` pair
    /// neither an import, a comment, a `ReExportAll`, nor `Verbatim`, so
    /// none of the existing exemptions apply; without this field the two
    /// would print with an unwanted blank between them (confirmed by
    /// temporarily reverting the field during this same change — the
    /// printer's own default policy really does insert one here). A third,
    /// ordinary statement after it confirms the field is scoped to the one
    /// carrying it, not sticky for the rest of the program.
    #[test]
    fn no_blank_before_suppresses_exactly_the_next_statements_own_blank() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ConstDecl {
                name: "__FooRegistry".to_string(),
                ty: None,
                init: TsExpr::Lit(TsLit::Null),
            },
            None,
        ));
        let mut zero_fn = TsStmt::decl(
            TsDecl::Function {
                name: "__zeroOfFooState".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body: Vec::new(),
                is_async: false,
                inline: true,
            },
            None,
        );
        zero_fn.no_blank_before = true;
        program.push(zero_fn);
        program.push(TsStmt::decl(
            TsDecl::ConstDecl {
                name: "c".to_string(),
                ty: None,
                init: TsExpr::Lit(TsLit::Null),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "const __FooRegistry = null;\nfunction __zeroOfFooState() {  }\n\nconst c = null;\n"
        );
    }

    /// The required "does the algebra hold against real content" test
    /// (P7.8's own accepted proposal, #1313): builds
    /// `bynk-emit/src/emitter/events_fanout.rs`'s own `EventsFanoutDO`
    /// class as real nodes (its field, constructor, and `fetch` method,
    /// with the exact control flow the real function emits — `for...of`,
    /// nested `if`-without-braces, `try`/`catch`) and asserts the printed
    /// text is byte-identical to what `emit_events_fanout_do` produces
    /// today for that class (transcribed directly from
    /// `bynk-emit/src/emitter/events_fanout.rs`'s own `write!` calls, not
    /// re-derived — including the constructor body, `this.env = (env ??
    /// {}) as Record<string, ServiceBinding>;`, via `TsStmtKind::Assign`,
    /// added in review of #1313: the accepted proposal's own grounding
    /// catalogue detailed the `fetch` method's body but missed the
    /// constructor's one real statement, which turned out to need a
    /// variant Decision B's list didn't name). The leading header comment
    /// and the `__eventRoutes` table's own per-project literal entries are
    /// deliberately not included here — a representative fragment covering
    /// the class is exactly what the accepted proposal's own "Done when"
    /// allows, and this is the shape that actually exercises every new
    /// node kind this slice adds.
    #[test]
    fn prints_events_fanout_dos_own_class_byte_identical_to_the_real_emitter() {
        let real_ctor = TsClassCtor {
            params: vec![
                TsParam {
                    name: "_state".to_string(),
                    ty: Some(TsType::named("DurableObjectState")),
                    optional: false,
                },
                TsParam {
                    name: "env".to_string(),
                    ty: Some(TsType::named("unknown")),
                    optional: true,
                },
            ],
            body: vec![TsStmt::assign(
                TsExpr::Member {
                    object: Box::new(TsExpr::Ident("this".to_string())),
                    property: "env".to_string(),
                },
                TsExpr::As {
                    expr: Box::new(TsExpr::Binary {
                        op: TsBinaryOp::NullishCoalescing,
                        left: Box::new(TsExpr::Ident("env".to_string())),
                        right: Box::new(TsExpr::object(vec![])),
                    }),
                    ty: TsType::named_with_args(
                        "Record",
                        vec![TsType::named("string"), TsType::named("ServiceBinding")],
                    ),
                },
                None,
            )],
        };

        let fetch = TsClassMethod {
            name: "fetch".to_string(),
            private: false,
            is_async: true,
            params: vec![TsParam {
                name: "request".to_string(),
                ty: Some(TsType::named("Request")),
                optional: false,
            }],
            return_type: Some(TsType::named_with_args(
                "Promise",
                vec![TsType::named("Response")],
            )),
            doc: None,
            body: vec![
                TsStmt::const_stmt(
                    TsBindingName::ObjectPattern(vec!["events".to_string()]),
                    None,
                    TsExpr::As {
                        expr: Box::new(TsExpr::Paren(Box::new(TsExpr::Await(Box::new(
                            TsExpr::Call {
                                callee: Box::new(TsExpr::Member {
                                    object: Box::new(TsExpr::Ident("request".to_string())),
                                    property: "json".to_string(),
                                }),
                                args: vec![],
                            },
                        ))))),
                        ty: TsType::Object(vec![TsTypeMember::prop(
                            "events",
                            TsType::array(TsType::named("FanoutEvent")),
                        )]),
                    },
                    None,
                ),
                TsStmt::for_of(
                    "ev",
                    TsExpr::Ident("events".to_string()),
                    TsStmt::block(
                        vec![
                            TsStmt::const_stmt(
                                TsBindingName::Ident("subs".to_string()),
                                None,
                                TsExpr::Index {
                                    object: Box::new(TsExpr::Ident("__eventRoutes".to_string())),
                                    index: Box::new(TsExpr::Member {
                                        object: Box::new(TsExpr::Ident("ev".to_string())),
                                        property: "type".to_string(),
                                    }),
                                },
                                None,
                            ),
                            TsStmt::if_stmt(
                                TsExpr::Unary {
                                    op: TsUnaryOp::Not,
                                    expr: Box::new(TsExpr::Call {
                                        callee: Box::new(TsExpr::Member {
                                            object: Box::new(TsExpr::Ident("Array".to_string())),
                                            property: "isArray".to_string(),
                                        }),
                                        args: vec![TsExpr::Ident("subs".to_string())],
                                    }),
                                },
                                TsStmt::continue_stmt(None),
                                None,
                            ),
                            TsStmt::for_of(
                                "sub",
                                TsExpr::Ident("subs".to_string()),
                                TsStmt::block(
                                    vec![
                                        TsStmt::const_stmt(
                                            TsBindingName::Ident("binding".to_string()),
                                            None,
                                            TsExpr::Index {
                                                object: Box::new(TsExpr::Member {
                                                    object: Box::new(TsExpr::Ident(
                                                        "this".to_string(),
                                                    )),
                                                    property: "env".to_string(),
                                                }),
                                                index: Box::new(TsExpr::Member {
                                                    object: Box::new(TsExpr::Ident(
                                                        "sub".to_string(),
                                                    )),
                                                    property: "binding".to_string(),
                                                }),
                                            },
                                            None,
                                        ),
                                        TsStmt::if_stmt(
                                            TsExpr::Unary {
                                                op: TsUnaryOp::Not,
                                                expr: Box::new(TsExpr::Ident(
                                                    "binding".to_string(),
                                                )),
                                            },
                                            TsStmt::continue_stmt(None),
                                            None,
                                        ),
                                        TsStmt::try_catch(
                                            TsStmt::block(
                                                vec![TsStmt::expr_stmt(
                                                    TsExpr::Await(Box::new(TsExpr::Call {
                                                        callee: Box::new(TsExpr::Ident(
                                                            "deliverEvent".to_string(),
                                                        )),
                                                        args: vec![
                                                            TsExpr::Ident("binding".to_string()),
                                                            TsExpr::Member {
                                                                object: Box::new(TsExpr::Ident(
                                                                    "sub".to_string(),
                                                                )),
                                                                property: "service".to_string(),
                                                            },
                                                            TsExpr::Member {
                                                                object: Box::new(TsExpr::Ident(
                                                                    "ev".to_string(),
                                                                )),
                                                                property: "payload".to_string(),
                                                            },
                                                            TsExpr::Member {
                                                                object: Box::new(TsExpr::Ident(
                                                                    "ev".to_string(),
                                                                )),
                                                                property: "envelope".to_string(),
                                                            },
                                                        ],
                                                    })),
                                                    None,
                                                )],
                                                None,
                                            ),
                                            Some("e"),
                                            TsStmt::block(
                                                vec![TsStmt::expr_stmt(
                                                    TsExpr::Call {
                                                        callee: Box::new(TsExpr::Member {
                                                            object: Box::new(TsExpr::Ident(
                                                                "console".to_string(),
                                                            )),
                                                            property: "error".to_string(),
                                                        }),
                                                        args: vec![
                                                            TsExpr::Lit(TsLit::Str(
                                                                "EventsFanout delivery failed"
                                                                    .to_string(),
                                                            )),
                                                            TsExpr::object(vec![
                                                                (
                                                                    "event".to_string(),
                                                                    TsExpr::Member {
                                                                        object: Box::new(
                                                                            TsExpr::Ident(
                                                                                "ev".to_string(),
                                                                            ),
                                                                        ),
                                                                        property: "type"
                                                                            .to_string(),
                                                                    },
                                                                ),
                                                                (
                                                                    "service".to_string(),
                                                                    TsExpr::Member {
                                                                        object: Box::new(
                                                                            TsExpr::Ident(
                                                                                "sub".to_string(),
                                                                            ),
                                                                        ),
                                                                        property: "service"
                                                                            .to_string(),
                                                                    },
                                                                ),
                                                                (
                                                                    "error".to_string(),
                                                                    TsExpr::Call {
                                                                        callee: Box::new(
                                                                            TsExpr::Ident(
                                                                                "String"
                                                                                    .to_string(),
                                                                            ),
                                                                        ),
                                                                        args: vec![TsExpr::Ident(
                                                                            "e".to_string(),
                                                                        )],
                                                                    },
                                                                ),
                                                            ]),
                                                        ],
                                                    },
                                                    None,
                                                )],
                                                None,
                                            ),
                                            None,
                                        ),
                                    ],
                                    None,
                                ),
                                None,
                            ),
                        ],
                        None,
                    ),
                    None,
                ),
                TsStmt::return_stmt(
                    Some(TsExpr::New {
                        callee: Box::new(TsExpr::Ident("Response".to_string())),
                        args: vec![
                            TsExpr::Lit(TsLit::Null),
                            TsExpr::object(vec![(
                                "status".to_string(),
                                TsExpr::Lit(TsLit::Num("204".to_string())),
                            )]),
                        ],
                    }),
                    None,
                ),
            ],
        };

        let class = TsDecl::Export(Box::new(TsDecl::Class {
            name: "EventsFanoutDO".to_string(),
            fields: vec![TsClassField {
                name: "env".to_string(),
                ty: TsType::named_with_args(
                    "Record",
                    vec![TsType::named("string"), TsType::named("ServiceBinding")],
                ),
                private: true,
            }],
            constructor: Some(real_ctor),
            methods: vec![fetch],
        }));

        let mut program = TsProgram::new();
        program.push(TsStmt::decl(class, None));
        let printed = print(&program, "x.bynk", "", "x.ts");

        // Transcribed directly from `bynk-emit/src/emitter/events_fanout.rs`'s
        // own `write!` calls, byte-for-byte including the constructor body.
        let expected_lines = [
            "export class EventsFanoutDO {",
            "  private env: Record<string, ServiceBinding>;",
            "  constructor(_state: DurableObjectState, env?: unknown) {",
            "    this.env = (env ?? {}) as Record<string, ServiceBinding>;",
            "  }",
            "",
            "  async fetch(request: Request): Promise<Response> {",
            "    const { events } = (await request.json()) as { events: FanoutEvent[] };",
            "    for (const ev of events) {",
            "      const subs = __eventRoutes[ev.type];",
            "      if (!Array.isArray(subs)) continue;",
            "      for (const sub of subs) {",
            "        const binding = this.env[sub.binding];",
            "        if (!binding) continue;",
            "        try {",
            "          await deliverEvent(binding, sub.service, ev.payload, ev.envelope);",
            "        } catch (e) {",
            "          console.error(\"EventsFanout delivery failed\", { event: ev.type, service: sub.service, error: String(e) });",
            "        }",
            "      }",
            "    }",
            "    return new Response(null, { status: 204 });",
            "  }",
            "}",
        ];
        let expected = format!("{}\n", expected_lines.join("\n"));
        assert_eq!(printed.text, expected);
    }

    /// Coverage gap named in review of #1314, finding 4: `Let` had no test
    /// anywhere, including its distinct `init: None` branch (`Const` always
    /// has an initialiser; `Let` is the one new statement kind that can
    /// omit one).
    #[test]
    fn prints_a_let_statement_with_no_initialiser() {
        let mut program = TsProgram::new();
        program.push(TsStmt::let_stmt(
            TsBindingName::Ident("subs".to_string()),
            None,
            None,
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "let subs;\n");
    }

    /// Coverage gap named in review of #1314, finding 4: `TsExpr::Array`
    /// was never constructed in any test.
    #[test]
    fn prints_an_array_literal() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::array(vec![
                TsExpr::Lit(TsLit::Num("1".to_string())),
                TsExpr::Ident("x".to_string()),
            ]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "[1, x];\n");
    }

    /// P7.9 (#1315): `TsType::Array`'s new `readonly` modifier — the shape
    /// every `List`/`Query` element type `bynk-emit`'s `ts_type_ref*`/
    /// `ts_ty` families build (`readonly T[]`), which `TsType::array`'s own
    /// `readonly: false` default cannot represent.
    #[test]
    fn print_type_renders_a_readonly_array() {
        assert_eq!(
            print_type(&TsType::readonly_array(TsType::named("Order"))),
            "readonly Order[]"
        );
        assert_eq!(
            print_type(&TsType::array(TsType::named("Order"))),
            "Order[]"
        );
    }

    /// A readonly array, function, union or intersection element is
    /// parenthesised before `[]`; a bare `readonly readonly T[][]` is TS1354.
    #[test]
    fn print_type_parenthesises_a_non_postfix_safe_array_element() {
        let order = || TsType::named("Order");
        assert_eq!(
            print_type(&TsType::readonly_array(TsType::readonly_array(order()))),
            "readonly (readonly Order[])[]"
        );
        assert_eq!(
            print_type(&TsType::readonly_array(TsType::array(order()))),
            "readonly Order[][]"
        );
        assert_eq!(
            print_type(&TsType::readonly_array(TsType::Fn {
                params: vec![order()],
                ret: Box::new(order()),
            })),
            "readonly ((a0: Order) => Order)[]"
        );
        assert_eq!(
            print_type(&TsType::array(TsType::Union {
                members: vec![order(), TsType::named("null")],
                multiline: false,
            })),
            "(Order | null)[]"
        );
        assert_eq!(
            print_type(&TsType::array(TsType::Intersection(vec![
                order(),
                TsType::named("Tagged"),
            ]))),
            "(Order & Tagged)[]"
        );
    }

    /// P7.9 (#1315): `TsType::Fn`'s zero-`params` form — the query-thunk
    /// wrapper shape, `bynk-emit`'s own `(() => readonly T[])`.
    #[test]
    fn print_type_renders_a_zero_param_function_type() {
        let ty = TsType::Fn {
            params: vec![],
            ret: Box::new(TsType::readonly_array(TsType::named("Order"))),
        };
        assert_eq!(print_type(&ty), "() => readonly Order[]");
    }

    /// P7.9 (#1315): `TsType::Fn`'s real parametered form — positional
    /// `a0`/`a1`/… names, matching `bynk-emit`'s own pre-P7.9 convention
    /// exactly (TypeScript requires *some* name in function-type syntax).
    #[test]
    fn print_type_renders_a_parametered_function_type_with_positional_names() {
        let ty = TsType::Fn {
            params: vec![TsType::named("string"), TsType::named("number")],
            ret: Box::new(TsType::named("void")),
        };
        assert_eq!(print_type(&ty), "(a0: string, a1: number) => void");
    }

    /// `TsType::Union`, added during review of #1315's own implementation —
    /// a real gap `bynk-emit`'s `ts_ty` needed for a resolved multi-actor
    /// sum (`Ty::ActorSum`), beyond the accepted proposal's own `readonly`/
    /// `Fn` gap list. Members print `" | "`-joined, in order.
    #[test]
    fn print_type_renders_a_union_of_named_types() {
        let ty = TsType::union(vec![
            TsType::named("string"),
            TsType::named("number"),
            TsType::Object(vec![TsTypeMember::prop(
                "tag",
                TsType::named("\"literal\""),
            )]),
        ]);
        assert_eq!(print_type(&ty), "string | number | { tag: \"literal\" }");
    }

    /// `TsStmtKind::Comment`, added for Arc C's own first real conversion
    /// slice (#1317). A multi-line comment (embedded `\n`) prints one
    /// `// `-prefixed line per real line.
    #[test]
    fn prints_a_comment_one_prefixed_line_per_embedded_newline() {
        let mut program = TsProgram::new();
        program.push(TsStmt::comment(
            "Generated by bynkc — do not edit by hand.\nSecond line.",
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "// Generated by bynkc — do not edit by hand.\n// Second line.\n"
        );
    }

    /// Two adjacent top-level `Comment` statements get no blank line
    /// between them — the same exception already established for two
    /// adjacent `import`s, matching `events_fanout.rs`'s own real two-line
    /// header banner (built as two separate `Comment` statements, not one
    /// with an embedded newline).
    #[test]
    fn no_blank_line_between_two_adjacent_comment_statements() {
        let mut program = TsProgram::new();
        program.push(TsStmt::comment(
            "Generated by bynkc — do not edit by hand.",
            None,
        ));
        program.push(TsStmt::comment("Second line.", None));
        program.push(TsStmt::decl(
            TsDecl::Interface {
                name: "A".to_string(),
                type_params: Vec::new(),
                members: vec![],
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "// Generated by bynkc — do not edit by hand.\n// Second line.\n\ninterface A {\n}\n"
        );
    }

    /// `TsExpr::multiline_object`, added for Arc C's own first real
    /// conversion slice (#1317): `events_fanout.rs`'s own `__eventRoutes`
    /// table is a top-level `const` initialiser with one entry per line,
    /// each with its own trailing comma (including the last), closing
    /// brace back at the statement's own indent — TypeScript's ordinary
    /// multi-line object-literal convention, which `TsExpr::object`'s
    /// single-line form cannot represent. Only reachable through a
    /// statement/declaration-level renderer that carries `depth`
    /// (`render_stmt_level_expr`) — this exercises it via a top-level
    /// `ConstDecl`, the real shape `events_fanout.rs` itself uses.
    #[test]
    fn prints_a_multiline_object_as_a_top_level_const_initialiser() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ConstDecl {
                name: "table".to_string(),
                ty: None,
                init: TsExpr::multiline_object(vec![
                    ("a".to_string(), TsExpr::Lit(TsLit::Num("1".to_string()))),
                    ("b".to_string(), TsExpr::Lit(TsLit::Num("2".to_string()))),
                ]),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "const table = {\n  a: 1,\n  b: 2,\n};\n");
    }

    /// Review of #1317/#1318, finding 1: an empty `multiline_object` still
    /// prints the open/close-brace-on-separate-lines shape, not the tight
    /// `{}` a single-line empty object uses — matching the pre-conversion
    /// `writeln!` code's own real behaviour (its `for` loop simply not
    /// iterating, while the open-brace and closing `};` lines were written
    /// unconditionally either way). This is a real, reachable shape:
    /// `events_fanout.rs`'s own `__eventRoutes` table can be empty for a
    /// context that publishes only events nobody subscribes to.
    #[test]
    fn an_empty_multiline_object_still_prints_the_open_and_close_brace_on_separate_lines() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ConstDecl {
                name: "table".to_string(),
                ty: None,
                init: TsExpr::multiline_object(vec![]),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "const table = {\n};\n");
    }

    /// A `multiline_object` reached through the ordinary, depth-unaware
    /// `render_expr` recursion (nested inside another expression, not a
    /// statement's own top-level initialiser) falls back to single-line
    /// rendering — documented on `TsExpr::Object` itself as a real,
    /// deliberate boundary, not silently wrong. Pinned here so a future
    /// change to that boundary is a visible, intentional decision.
    #[test]
    fn a_nested_multiline_object_falls_back_to_single_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::array(vec![TsExpr::multiline_object(vec![(
                "a".to_string(),
                TsExpr::Lit(TsLit::Num("1".to_string())),
            )])]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "[{ a: 1 }];\n");
    }

    /// Coverage gap named in review of #1314, finding 4: the blank-line-
    /// between-top-level-declarations policy was never tested for the
    /// mixed `Verbatim` + real-`Decl` case — the case that actually keeps
    /// every P7.6 fixture's zero-diff claim true today, since every P7.6
    /// construction site is all-`Verbatim`. Pins both halves of the rule's
    /// asymmetry: no blank line is added *after* a `Verbatim` statement
    /// (P7.7's own boundary — its content's own trailing spacing isn't the
    /// printer's decision), but a blank line *is* added after a real
    /// `Decl` even when `Verbatim` follows it.
    #[test]
    fn no_blank_line_after_verbatim_but_one_before_it() {
        let mut program = TsProgram::new();
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Contracts,
            "const legacy = 1;\n",
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::Interface {
                name: "A".to_string(),
                type_params: Vec::new(),
                members: vec![],
            },
            None,
        ));
        program.push(TsStmt::verbatim(
            VerbatimOrigin::Secrets,
            "const trailing = 2;\n",
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "const legacy = 1;\ninterface A {\n}\n\nconst trailing = 2;\n"
        );
    }

    /// Coverage gap named in review of #1314, finding 4: nothing exercised
    /// `render_block_body`'s own graceful-degradation path — its doc
    /// comment promises a single non-`Block` statement still renders
    /// sensibly as a one-statement body rather than panicking, but no test
    /// ever passed one. `TryCatch` is the real caller that could hit this
    /// (its `try_block`/`catch_block` are typed as a bare `TsStmt`, not
    /// required to be a `Block`).
    #[test]
    fn a_try_block_that_is_not_a_block_statement_still_prints_as_a_braced_body() {
        let mut program = TsProgram::new();
        program.push(TsStmt::try_catch(
            TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("risky".to_string())),
                    args: vec![],
                },
                None,
            ),
            Some("e"),
            TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("handle".to_string())),
                    args: vec![TsExpr::Ident("e".to_string())],
                },
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "try {\n  risky();\n} catch (e) {\n  handle(e);\n}\n"
        );
    }

    /// Coverage gap named in review of #1314, finding 4: `render_inline_stmt`'s
    /// shared fallback arm (finding 3's fix — explicit variants, not a
    /// wildcard) was never actually exercised by a test; every existing
    /// brace-free `if`/`for...of` body used `Continue`/`Return`/`ExprStmt`,
    /// which each have their own dedicated inline arm.
    #[test]
    fn a_brace_free_if_body_that_is_not_one_of_the_dedicated_inline_kinds_still_prints() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::const_stmt(
                TsBindingName::Ident("x".to_string()),
                None,
                TsExpr::Lit(TsLit::Num("1".to_string())),
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (cond) const x = 1;\n");
    }

    /// Review of #1314, finding 2: nothing checked operator-precedence
    /// parenthesisation outside `As` — a `Binary` nested as the operand of
    /// `Unary`/`Member`/`Await` printed with no parens at all, silently
    /// changing what the text parses back to. Three of the review's own
    /// examples, pinned directly.
    #[test]
    fn parenthesises_a_binary_operand_of_unary_member_and_await() {
        let bin = || TsExpr::Binary {
            op: TsBinaryOp::NullishCoalescing,
            left: Box::new(TsExpr::Ident("a".to_string())),
            right: Box::new(TsExpr::Ident("b".to_string())),
        };

        let mut not_program = TsProgram::new();
        not_program.push(TsStmt::expr_stmt(
            TsExpr::Unary {
                op: TsUnaryOp::Not,
                expr: Box::new(bin()),
            },
            None,
        ));
        assert_eq!(
            print(&not_program, "x.bynk", "", "x.ts").text,
            "!(a ?? b);\n"
        );

        let mut member_program = TsProgram::new();
        member_program.push(TsStmt::expr_stmt(
            TsExpr::Member {
                object: Box::new(bin()),
                property: "c".to_string(),
            },
            None,
        ));
        assert_eq!(
            print(&member_program, "x.bynk", "", "x.ts").text,
            "(a ?? b).c;\n"
        );

        let mut await_program = TsProgram::new();
        await_program.push(TsStmt::expr_stmt(TsExpr::Await(Box::new(bin())), None));
        assert_eq!(
            print(&await_program, "x.bynk", "", "x.ts").text,
            "await (a ?? b);\n"
        );
    }

    /// Review of #1314, finding 2's fourth example: a right-nested `Binary`
    /// (`a ?? (b ?? c)`, as built) must not print as the bare `a ?? b ?? c`
    /// that a naive left-to-right walk would produce — that text parses
    /// back as `(a ?? b) ?? c`, a different tree than what was built.
    #[test]
    fn parenthesises_a_nested_binary_operand_of_another_binary() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::NullishCoalescing,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::NullishCoalescing,
                    left: Box::new(TsExpr::Ident("b".to_string())),
                    right: Box::new(TsExpr::Ident("c".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a ?? (b ?? c);\n");
    }

    /// Review of #1314, finding 1: `TsLit::Str` escaped only `"`/`\`, so a
    /// literal containing a newline, tab, or carriage return printed raw —
    /// unterminated-string-literal TypeScript `tsc` can't parse. Pins the
    /// fix against every character `bynk_check::wire_default::
    /// escape_ts_literal` escapes.
    #[test]
    fn escapes_newline_tab_and_carriage_return_in_a_string_literal() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Lit(TsLit::Str("line one\nline two\ttabbed\r".to_string())),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "\"line one\\nline two\\ttabbed\\r\";\n");
    }

    // -- Arc C slice 3 (#1321, `workers.rs`): the five new shapes Decision A
    // names, plus the real gaps found beyond it (`TsDecl::Function`/
    // `TsDecl::TypeAlias`/`TsDecl::ImportNamespace`, the parameterless
    // `catch {}` form, and the two new `TsUnaryOp`/four new `TsBinaryOp`
    // operators). --

    #[test]
    fn prints_a_shorthand_async_method_entry_in_a_multiline_object() {
        let mut program = TsProgram::new();
        program.push(TsStmt::return_stmt(
            Some(TsExpr::multiline_object_entries(vec![
                TsObjectEntry::Method {
                    name: "foo".to_string(),
                    is_async: true,
                    generics: Vec::new(),
                    params: vec![TsParam {
                        name: "a".to_string(),
                        ty: Some(TsType::named("string")),
                        optional: false,
                    }],
                    return_type: None,
                    doc: None,
                    inline: false,
                    body: vec![TsStmt::return_stmt(
                        Some(TsExpr::Ident("a".to_string())),
                        None,
                    )],
                },
            ])),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "return {\n  async foo(a: string) {\n    return a;\n  },\n};\n"
        );
    }

    #[test]
    fn prints_shorthand_and_spread_entries_inline() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::object_entries(vec![
                TsObjectEntry::Spread(TsExpr::Ident("deps".to_string())),
                TsObjectEntry::Shorthand("cap1".to_string()),
                TsObjectEntry::Prop(
                    "identity".to_string(),
                    TsExpr::Ident("__caller".to_string()),
                ),
            ]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "{ ...deps, cap1, identity: __caller };\n");
    }

    #[test]
    fn prints_an_expression_bodied_arrow() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Arrow {
                params: vec![TsParam {
                    name: "events".to_string(),
                    ty: Some(TsType::named_with_args(
                        "Array",
                        vec![TsType::named("Wire")],
                    )),
                    optional: false,
                }],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("dispatch".to_string())),
                    args: vec![TsExpr::Ident("events".to_string())],
                }))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "(events: Array<Wire>) => dispatch(events);\n");
    }

    /// #1327's own real gap: `emit_composition_root`'s `__eventsDispatch`
    /// closure is the first real `Arrow` site that's async.
    #[test]
    fn prints_an_async_arrow() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Arrow {
                params: vec![TsParam {
                    name: "events".to_string(),
                    ty: Some(TsType::named_with_args(
                        "Array",
                        vec![TsType::named("Wire")],
                    )),
                    optional: false,
                }],
                is_async: true,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident(
                    "{ dispatch(events); }".to_string(),
                )))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "async (events: Array<Wire>) => { dispatch(events); };\n"
        );
    }

    /// #1435 (Arc E slice 1): [`TsArrowBody::Block`], the first real
    /// statement-bodied arrow — pins the exact `serialisation.rs` `Float`
    /// non-finite guard shape (`serialise_field_expr_wire`'s own
    /// `WireRef::Base { base: Float, .. }` arm) directly against a hand-built
    /// tree, the same way #1322's own three parenthesisation tests below pin
    /// their own gap. Proves `render_arrow_body`'s `Block` arm reuses
    /// `render_compact_stmts` (an `If` with a brace-free `Throw` then-branch,
    /// followed by a `Return`) correctly: both statements land on the SAME
    /// physical line as the arrow's own `{ ... }`, with no trailing newline
    /// leaking out of the block into the enclosing call's own `;`.
    #[test]
    fn prints_a_block_bodied_arrow_iife() {
        let mut program = TsProgram::new();
        let guard = TsExpr::Call {
            callee: Box::new(TsExpr::Arrow {
                params: vec![TsParam {
                    name: "v".to_string(),
                    ty: Some(TsType::named("number")),
                    optional: false,
                }],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Block(vec![
                    TsStmt::if_stmt(
                        TsExpr::Unary {
                            op: TsUnaryOp::Not,
                            expr: Box::new(TsExpr::Call {
                                callee: Box::new(TsExpr::Member {
                                    object: Box::new(TsExpr::Ident("Number".to_string())),
                                    property: "isFinite".to_string(),
                                }),
                                args: vec![TsExpr::Ident("v".to_string())],
                            }),
                        },
                        TsStmt::throw_stmt(
                            TsExpr::New {
                                callee: Box::new(TsExpr::Ident("Error".to_string())),
                                args: vec![TsExpr::Lit(TsLit::Str(
                                    "non-finite Float at boundary".to_string(),
                                ))],
                            },
                            None,
                        ),
                        None,
                    ),
                    TsStmt::return_stmt(
                        Some(TsExpr::As {
                            expr: Box::new(TsExpr::Ident("v".to_string())),
                            ty: TsType::named("JsonValue"),
                        }),
                        None,
                    ),
                ])),
            }),
            args: vec![TsExpr::Ident("value".to_string())],
        };
        program.push(TsStmt::expr_stmt(guard, None));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "((v: number) => { if (!Number.isFinite(v)) throw new Error(\"non-finite Float at boundary\"); return v as JsonValue; })(value);\n"
        );
    }

    /// Review of #1322, finding 1: `Arrow` was missing from every
    /// parenthesisation rule. Not reachable through any real `bynk-emit`
    /// content today (the one grounded `Arrow` site is an object-property
    /// value, rendered through the depth-unaware `render_expr` with no
    /// operand context) — these three tests pin the fix directly against
    /// hand-built trees, the same way #1314's own precedence tests did
    /// before any real content exercised them.
    #[test]
    fn parenthesises_an_arrow_used_as_a_call_callee() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Call {
                callee: Box::new(TsExpr::Arrow {
                    params: vec![TsParam {
                        name: "x".to_string(),
                        ty: None,
                        optional: false,
                    }],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident("x".to_string())))),
                }),
                args: vec![TsExpr::Lit(TsLit::Num("1".to_string()))],
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "((x) => x)(1);\n");
    }

    #[test]
    fn parenthesises_an_arrow_used_as_a_binary_operand() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::NullishCoalescing,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Arrow {
                    params: vec![TsParam {
                        name: "x".to_string(),
                        ty: None,
                        optional: false,
                    }],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident("y".to_string())))),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a ?? ((x) => y);\n");
    }

    #[test]
    fn parenthesises_an_arrow_used_as_an_as_operand() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::As {
                expr: Box::new(TsExpr::Arrow {
                    params: vec![TsParam {
                        name: "x".to_string(),
                        ty: None,
                        optional: false,
                    }],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Ident("y".to_string())))),
                }),
                ty: TsType::named("Handler"),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "((x) => y) as Handler;\n");
    }

    #[test]
    fn prints_optional_member_and_optional_index_chained() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::OptionalIndex {
                object: Box::new(TsExpr::OptionalMember {
                    object: Box::new(TsExpr::Ident("x".to_string())),
                    property: "process".to_string(),
                }),
                index: Box::new(TsExpr::Lit(TsLit::Str("secret".to_string()))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "x?.process?.[\"secret\"];\n");
    }

    #[test]
    fn print_type_renders_an_optional_object_field() {
        let ty = TsType::Object(vec![
            TsTypeMember::optional_prop("env", TsType::named("unknown")),
            TsTypeMember::prop("name", TsType::named("string")),
        ]);
        assert_eq!(print_type(&ty), "{ env?: unknown; name: string }");
    }

    #[test]
    fn prints_a_top_level_function_declaration() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Function {
                name: "compose".to_string(),
                generics: Vec::new(),
                params: vec![TsParam {
                    name: "env".to_string(),
                    ty: Some(TsType::named("Env")),
                    optional: false,
                }],
                return_type: None,
                body: vec![TsStmt::return_stmt(Some(TsExpr::Lit(TsLit::Null)), None)],
                is_async: false,
                inline: false,
            })),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "export function compose(env: Env) {\n  return null;\n}\n"
        );
    }

    /// #1351's own real gap: `TsDecl::Function` had no `generics` field —
    /// `emit_free_fn`'s own v0.20a erased generics (`export function
    /// foo<A, B>(...)`) needed one. Review of #1352, finding 3: every other
    /// existing `TsDecl::Function` test only threads `generics: Vec::new()`
    /// through; this pins the non-empty case directly, matching #1339's own
    /// precedent of a dedicated test per new field.
    #[test]
    fn prints_a_generic_top_level_function_declaration() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Function {
                name: "identity".to_string(),
                generics: vec!["A".to_string(), "B".to_string()],
                params: vec![TsParam {
                    name: "x".to_string(),
                    ty: Some(TsType::named("A")),
                    optional: false,
                }],
                return_type: Some(TsType::named("A")),
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::Ident("x".to_string())),
                    None,
                )],
                is_async: false,
                inline: false,
            })),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "export function identity<A, B>(x: A): A {\n  return x;\n}\n"
        );
    }

    #[test]
    fn prints_a_top_level_type_alias() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::TypeAlias {
                name: "Foo".to_string(),
                type_params: Vec::new(),
                ty: TsType::named("{ get(id: any): any }"),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "type Foo = { get(id: any): any };\n");
    }

    #[test]
    fn prints_a_namespace_import_adjacent_to_a_named_import_with_no_blank_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: vec!["a".to_string()],
                from: "./x.js".to_string(),
            },
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: "handlers".to_string(),
                from: "./handlers.js".to_string(),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "import { a } from \"./x.js\";\nimport * as handlers from \"./handlers.js\";\n"
        );
    }

    /// Arc C, step (10) (#1392): `ImportNamespace.type_only`, a parallel gap
    /// by omission — `TsDecl::Import` already had this field, but nothing
    /// before `emit_cross_context_namespace_imports`'s own conversion built
    /// a type-only namespace import (`import type * as ns from "...";`, a
    /// Workers-mode consumed-context import reaching the callee's types
    /// only, #661).
    #[test]
    fn prints_a_type_only_namespace_import() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: true,
                alias: "commerce_payment".to_string(),
                from: "../commerce-payment/handlers.js".to_string(),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "import type * as commerce_payment from \"../commerce-payment/handlers.js\";\n"
        );
    }

    /// Arc C, slice 37 (#1409, `tests_emit.rs`'s own slice G): `emit_integration_
    /// module`'s own per-participant `import worker_{ns} from "../workers/{dir}/
    /// index.js";` (the participant's Worker entry module's default export) —
    /// the first real default import anywhere in `bynk-emit`'s own converted
    /// content.
    #[test]
    fn prints_a_default_import() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ImportDefault {
                alias: "worker_shop_api".to_string(),
                from: "../workers/shop-api/index.js".to_string(),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "import worker_shop_api from \"../workers/shop-api/index.js\";\n"
        );
    }

    /// #1321's own real gap beyond the accepted proposal's five: ES2019's
    /// optional catch binding (`emit_http_sum_wrapper`'s own `} catch {`,
    /// no `(e)` at all).
    #[test]
    fn prints_a_parameterless_catch() {
        let mut program = TsProgram::new();
        program.push(TsStmt::try_catch(
            TsStmt::block(vec![], None),
            None::<String>,
            TsStmt::block(vec![], None),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "try {\n} catch {\n}\n");
    }

    #[test]
    fn prints_typeof_and_the_new_comparison_and_logical_operators() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Unary {
                op: TsUnaryOp::Typeof,
                expr: Box::new(TsExpr::Ident("x".to_string())),
            },
            None,
        ));
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            },
            None,
        ));
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::StrictNotEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            },
            None,
        ));
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::And,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            },
            None,
        ));
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Or,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "typeof x;\n\na === b;\n\na !== b;\n\na && b;\n\na || b;\n"
        );
    }

    /// #1321's own real gap: with only `??` in the algebra, a nested
    /// `Binary` operand of another `Binary` was always parenthesized,
    /// regardless of operator (see `parenthesises_a_nested_binary_operand_
    /// of_another_binary`, unchanged). `workers.rs`'s own real content
    /// nests a *strictly higher precedence* comparison inside `||`/`&&`
    /// (`__authz === null || !__authz.startsWith(...)`) and the byte-golden
    /// fixtures have no parens around it — this pins that the printer
    /// omits them precisely in that case, not more broadly.
    #[test]
    fn a_strictly_higher_precedence_comparison_needs_no_parens_inside_or_or_and() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Or,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::StrictEq,
                    left: Box::new(TsExpr::Ident("a".to_string())),
                    right: Box::new(TsExpr::Lit(TsLit::Null)),
                }),
                right: Box::new(TsExpr::Unary {
                    op: TsUnaryOp::Not,
                    expr: Box::new(TsExpr::Ident("b".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a === null || !b;\n");
    }

    // -- #1323 (workers_entry.rs): Switch, ExportDefault, Conditional,
    // TsType::Object readonly/method members, and the smaller mechanical
    // gaps found during implementation (ReExport, Blank, TsLit::Bool,
    // TsExpr::Paren, same-operator-chain flattening). --

    /// #1323's own largest gap: `switch (<discriminant>) { <cases> }`. A
    /// non-`default` `case` is `{ }`-blocked; `default` is not — pinned
    /// against `workers_entry.rs`'s own real internal-dispatch shape.
    #[test]
    fn prints_a_switch_with_a_braced_case_and_an_unbraced_default() {
        let mut program = TsProgram::new();
        program.push(TsStmt::switch_stmt(
            TsExpr::Ident("servicePath".to_string()),
            vec![
                TsSwitchCase {
                    test: Some(TsExpr::Lit(TsLit::Str("orders".to_string()))),
                    body: vec![TsStmt::return_stmt(None, None)],
                    default_braced: false,
                    case_braced: true,
                },
                TsSwitchCase {
                    test: None,
                    body: vec![TsStmt::expr_stmt(
                        TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(TsExpr::Ident("console".to_string())),
                                property: "log".to_string(),
                            }),
                            args: vec![],
                        },
                        None,
                    )],
                    default_braced: false,
                    case_braced: true,
                },
            ],
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "switch (servicePath) {\n  case \"orders\": {\n    return;\n  }\n  default:\n    console.log();\n}\n"
        );
    }

    /// Arc C, slice 33 (`tests_emit.rs` slice C, #1401): `TsSwitchCase.
    /// default_braced` — `emit_stub_class`'s own `ReturnsEach` dispatch
    /// braces its `default` case, unlike `workers_entry.rs`'s own unbraced
    /// one pinned just above.
    #[test]
    fn prints_a_switch_with_a_braced_default() {
        let mut program = TsProgram::new();
        program.push(TsStmt::switch_stmt(
            TsExpr::Ident("__k".to_string()),
            vec![TsSwitchCase {
                test: None,
                body: vec![TsStmt::expr_stmt(TsExpr::Ident("x".to_string()), None)],
                default_braced: true,
                case_braced: true,
            }],
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "switch (__k) {\n  default: {\n    x;\n  }\n}\n"
        );
    }

    /// Arc E slice 6 (#1445): `TsSwitchCase.case_braced` — the mirror-image
    /// gap `default_braced` left open. `emit_sum_codec`'s own payload-free
    /// variant case is unbraced right beside a braced payload-carrying
    /// sibling in the *same* switch, pinned against `212_json_codec`'s own
    /// mixed `Status` fixture (`case "Pending": return { kind: "Pending"
    /// };` then `case "Shipped": { ... }`).
    #[test]
    fn prints_a_switch_with_an_unbraced_case_beside_a_braced_one() {
        let mut program = TsProgram::new();
        program.push(TsStmt::switch_stmt(
            TsExpr::Ident("kind".to_string()),
            vec![
                TsSwitchCase {
                    test: Some(TsExpr::Lit(TsLit::Str("Pending".to_string()))),
                    body: vec![TsStmt::return_stmt(
                        Some(TsExpr::Ident("a".to_string())),
                        None,
                    )],
                    default_braced: false,
                    case_braced: false,
                },
                TsSwitchCase {
                    test: Some(TsExpr::Lit(TsLit::Str("Shipped".to_string()))),
                    body: vec![TsStmt::return_stmt(
                        Some(TsExpr::Ident("b".to_string())),
                        None,
                    )],
                    default_braced: false,
                    case_braced: true,
                },
            ],
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "switch (kind) {\n  case \"Pending\":\n    return a;\n  case \"Shipped\": {\n    return b;\n  }\n}\n"
        );
    }

    /// #1323's own second gap: `export default <expr>;` — a default export
    /// of a bare expression, not a declaration. Uses the same depth-aware
    /// multiline-object handling `const`/`let`/`return` already get.
    #[test]
    fn prints_an_export_default_object_literal_multiline() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ExportDefault(TsExpr::multiline_object(vec![(
                "fetch".to_string(),
                TsExpr::Ident("handler".to_string()),
            )])),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "export default {\n  fetch: handler,\n};\n");
    }

    /// #1355's own real gap: a `multiline: true` `TsExpr::Object` nested as
    /// one `Prop`'s own value inside ANOTHER multiline object —
    /// `emit_messages_bundle`'s own real doubly-nested `{ locale: { code:
    /// expr, ... }, ... }` table. Before this slice, `render_multiline_
    /// object_entry`'s own `Prop` arm rendered its value through the plain,
    /// depth-unaware `render_expr`, which ignores `multiline` entirely —
    /// this would have silently collapsed the inner object to one line.
    #[test]
    fn prints_a_nested_multiline_object_as_a_props_own_value() {
        let mut program = TsProgram::new();
        program.push(TsStmt::const_stmt(
            TsBindingName::Ident("byLocale".to_string()),
            None,
            TsExpr::multiline_object_entries(vec![TsObjectEntry::Prop(
                "\"en\"".to_string(),
                TsExpr::multiline_object_entries(vec![
                    TsObjectEntry::Prop(
                        "\"greeting\"".to_string(),
                        TsExpr::Ident("renderGreeting".to_string()),
                    ),
                    TsObjectEntry::Prop(
                        "\"farewell\"".to_string(),
                        TsExpr::Ident("renderFarewell".to_string()),
                    ),
                ]),
            )]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "const byLocale = {\n  \"en\": {\n    \"greeting\": renderGreeting,\n    \"farewell\": renderFarewell,\n  },\n};\n"
        );
    }

    /// Review of #1356, finding 2: `render_multiline_object_entry`'s own
    /// `Prop` arm routes through `render_stmt_level_expr` for BOTH
    /// `TsExpr::Object`/`Array` (see `TsExpr::Array`'s own doc, updated by
    /// #1355) — the object case is pinned above, this is the array sibling,
    /// making that doc claim self-verifying rather than merely asserted.
    #[test]
    fn prints_a_nested_multiline_array_as_a_props_own_value() {
        let mut program = TsProgram::new();
        program.push(TsStmt::const_stmt(
            TsBindingName::Ident("byGroup".to_string()),
            None,
            TsExpr::multiline_object_entries(vec![TsObjectEntry::Prop(
                "\"a\"".to_string(),
                TsExpr::multiline_array(vec![
                    TsExpr::Ident("one".to_string()),
                    TsExpr::Ident("two".to_string()),
                ]),
            )]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "const byGroup = {\n  \"a\": [\n    one,\n    two,\n  ],\n};\n"
        );
    }

    /// #1323's own third gap: `test ? consequent : alternate`.
    #[test]
    fn prints_a_conditional_expression() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Conditional {
                test: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::StrictEq,
                    left: Box::new(TsExpr::Ident("method".to_string())),
                    right: Box::new(TsExpr::Lit(TsLit::Str("HEAD".to_string()))),
                }),
                consequent: Box::new(TsExpr::Lit(TsLit::Num("1".to_string()))),
                alternate: Box::new(TsExpr::Lit(TsLit::Num("2".to_string()))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "method === \"HEAD\" ? 1 : 2;\n");
    }

    /// Review-anticipated gap (matching #1322 finding 1's own class): a
    /// `Conditional` used as a binary operand, an `as`-operand, or a call
    /// callee all need parens — closed proactively in this same slice
    /// rather than left for a review round to re-find.
    #[test]
    fn parenthesises_a_conditional_used_as_a_binary_operand_and_an_as_operand() {
        let cond = || TsExpr::Conditional {
            test: Box::new(TsExpr::Ident("a".to_string())),
            consequent: Box::new(TsExpr::Ident("b".to_string())),
            alternate: Box::new(TsExpr::Ident("c".to_string())),
        };
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::NullishCoalescing,
                left: Box::new(TsExpr::Ident("x".to_string())),
                right: Box::new(cond()),
            },
            None,
        ));
        program.push(TsStmt::expr_stmt(
            TsExpr::As {
                expr: Box::new(cond()),
                ty: TsType::named("string"),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "x ?? (a ? b : c);\n\n(a ? b : c) as string;\n"
        );
    }

    /// #1323's own real gap: a `Method` member of a type-position object
    /// literal (no body), alongside a `readonly` property.
    #[test]
    fn print_type_renders_a_readonly_field_and_a_method_signature() {
        let ty = TsType::Object(vec![
            TsTypeMember::readonly_prop("cron", TsType::named("string")),
            TsTypeMember::method("ack", vec![], TsType::named("void")),
            TsTypeMember::method(
                "waitUntil",
                vec![TsParam {
                    name: "promise".to_string(),
                    ty: Some(TsType::named_with_args(
                        "Promise",
                        vec![TsType::named("unknown")],
                    )),
                    optional: false,
                }],
                TsType::named("void"),
            ),
        ]);
        assert_eq!(
            print_type(&ty),
            "{ readonly cron: string; ack(): void; waitUntil(promise: Promise<unknown>): void }"
        );
    }

    /// #1323's own real gap: `TsObjectEntry::Method` gained a
    /// `return_type` field (mirroring `TsClassMethod`'s own existing one) —
    /// `workers_entry.rs`'s own `export default { fetch, scheduled?,
    /// queue? }` entries all carry an explicit return-type annotation,
    /// unlike `workers.rs`'s own `compose`-returned wrapper methods
    /// (`#1321`), none of which do.
    #[test]
    fn prints_a_multiline_object_methods_own_return_type_annotation() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ExportDefault(TsExpr::multiline_object_entries(vec![
                TsObjectEntry::Method {
                    name: "fetch".to_string(),
                    is_async: true,
                    generics: Vec::new(),
                    params: vec![],
                    return_type: Some(TsType::named_with_args(
                        "Promise",
                        vec![TsType::named("Response")],
                    )),
                    doc: None,
                    inline: false,
                    body: vec![],
                },
            ])),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "export default {\n  async fetch(): Promise<Response> {\n  },\n};\n"
        );
    }

    /// #1323's own real correction: `Await` no longer auto-parenthesizes
    /// under `As` — `workers_entry.rs`'s own real `await request.json() as
    /// JsonValue` has no parens (`as` binds looser than `await`, so none
    /// are grammatically needed; P7.8's original "always parenthesize"
    /// reasoning conflated one file's own real text with a grammar
    /// requirement).
    #[test]
    fn an_await_used_as_an_as_operand_no_longer_needs_parens() {
        let mut program = TsProgram::new();
        program.push(TsStmt::const_stmt(
            TsBindingName::Ident("args".to_string()),
            None,
            TsExpr::As {
                expr: Box::new(TsExpr::Await(Box::new(TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("request".to_string())),
                        property: "json".to_string(),
                    }),
                    args: vec![],
                }))),
                ty: TsType::named("JsonValue"),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "const args = await request.json() as JsonValue;\n"
        );
    }

    /// #1353's own real gap: `bynk_ts::TsStmtKind::Throw`, added for
    /// `emit_contract_guarded_body`'s own real `throw __e;` sites. Review of
    /// #1354, finding 1: every real call site today reaches this only
    /// through an `InlineBlock` (`render_stmt(out, stmt, 0)`), so the
    /// top-level, non-inline `render_stmt` arm's own `indent(depth)` at a
    /// nonzero depth was untested — pinned directly here, mirroring
    /// `Return`'s own shape exactly.
    #[test]
    fn prints_a_throw_statement_at_a_nested_depth() {
        let stmt = TsStmt::throw_stmt(TsExpr::Ident("__e".to_string()), None);
        assert_eq!(print_stmt(&stmt, 2), "    throw __e;\n");
    }

    /// #1353's own real gap, byte-for-byte: the whole contract-guard shape
    /// `emit_contract_guarded_body` builds (`if (!(pred)) { const __e = ...;
    /// __e.name = ...; throw __e; }`) — an `If` over an `InlineBlock` of a
    /// `Const`/`Assign`/`Throw` trio — asserted as one exact string. Review
    /// of #1354, finding 1: no existing test (fixture, `contract_behaviour.
    /// rs`, `source_map.rs`) asserts this shape's own bytes; they only
    /// `contains`-check a runtime message or a generated line number, none
    /// would catch a missing space, a reordered statement, or a lost
    /// indent.
    #[test]
    fn prints_a_contract_guard_if_stmt() {
        let cond = TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(TsExpr::Paren(Box::new(TsExpr::Ident("n > 0".to_string())))),
        };
        let new_error = TsExpr::New {
            callee: Box::new(TsExpr::Ident("Error".to_string())),
            args: vec![TsExpr::template_lit(
                vec![
                    "contract violated: precondition \\`positive\\` of scale (n=${n})".to_string(),
                ],
                Vec::new(),
            )],
        };
        let const_e = TsStmt::const_stmt(
            TsBindingName::Ident("__e".to_string()),
            None,
            new_error,
            None,
        );
        let assign_name = TsStmt::assign(
            TsExpr::Member {
                object: Box::new(TsExpr::Ident("__e".to_string())),
                property: "name".to_string(),
            },
            TsExpr::Lit(TsLit::Str("BynkContractError".to_string())),
            None,
        );
        let throw_e = TsStmt::throw_stmt(TsExpr::Ident("__e".to_string()), None);
        let then_branch = TsStmt::inline_block(vec![const_e, assign_name, throw_e], None);
        let if_stmt = TsStmt::if_stmt(cond, then_branch, None);
        assert_eq!(
            print_stmt(&if_stmt, 1),
            "  if (!(n > 0)) { const __e = new Error(`contract violated: precondition \\`positive\\` of scale (n=${n})`); __e.name = \"BynkContractError\"; throw __e; }\n"
        );
    }

    /// #1323's own real gap: `if`'s own `else` branch, printed on a fresh
    /// line at the `if`'s own indent, then following the same block-vs-
    /// inline rule the `if` itself does — `workers_entry.rs`'s own real
    /// queue-consumer ack/retry dispatch.
    #[test]
    fn prints_an_if_else_with_an_inline_then_and_an_inline_block_else() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_else_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Member {
                    object: Box::new(TsExpr::Ident("result".to_string())),
                    property: "tag".to_string(),
                }),
                right: Box::new(TsExpr::Lit(TsLit::Str("Ack".to_string()))),
            },
            TsStmt::expr_stmt(
                TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("msg".to_string())),
                        property: "ack".to_string(),
                    }),
                    args: vec![],
                },
                None,
            ),
            TsStmt::inline_block(
                vec![TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Member {
                            object: Box::new(TsExpr::Ident("msg".to_string())),
                            property: "retry".to_string(),
                        }),
                        args: vec![],
                    },
                    None,
                )],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "if (result.tag === \"Ack\") msg.ack();\nelse { msg.retry(); }\n"
        );
    }

    /// #1323's own real gap: a `TryCatch`'s own `catch` body can be an
    /// `InlineBlock` too — the braces stay on their own lines (this shape's
    /// usual convention), but the content packs onto one compact line.
    /// `workers_entry.rs`'s own real queue-consumer catch clause.
    #[test]
    fn a_try_catchs_own_catch_block_can_be_an_inline_block() {
        let mut program = TsProgram::new();
        program.push(TsStmt::try_catch(
            TsStmt::block(
                vec![TsStmt::expr_stmt(TsExpr::Ident("work".to_string()), None)],
                None,
            ),
            Some("e"),
            TsStmt::inline_block(
                vec![
                    TsStmt::expr_stmt(
                        TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(TsExpr::Ident("console".to_string())),
                                property: "error".to_string(),
                            }),
                            args: vec![TsExpr::Ident("e".to_string())],
                        },
                        None,
                    ),
                    TsStmt::expr_stmt(
                        TsExpr::Call {
                            callee: Box::new(TsExpr::Member {
                                object: Box::new(TsExpr::Ident("msg".to_string())),
                                property: "retry".to_string(),
                            }),
                            args: vec![],
                        },
                        None,
                    ),
                ],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "try {\n  work;\n} catch (e) {\n  console.error(e); msg.retry();\n}\n"
        );
    }

    /// #1323's own real gap: `TsBinaryOp::GreaterThan` — the request-body
    /// ceiling guard's own `Number(__contentLength) > <cap>`, the one real
    /// site needing a relational (not equality) comparison.
    #[test]
    fn prints_a_greater_than_comparison() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::GreaterThan,
                left: Box::new(TsExpr::Call {
                    callee: Box::new(TsExpr::Ident("Number".to_string())),
                    args: vec![TsExpr::Ident("__contentLength".to_string())],
                }),
                right: Box::new(TsExpr::Lit(TsLit::Num("100".to_string()))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "Number(__contentLength) > 100;\n");
    }

    /// #1323's own real gap: `TsTypeMember::Index` — `workers_entry.rs`'s
    /// own `args as { [k: string]: JsonValue }`, textually distinct from
    /// the semantically-equivalent `Record<string, JsonValue>`.
    #[test]
    fn print_type_renders_an_index_signature() {
        let ty = TsType::Object(vec![TsTypeMember::index(
            "k",
            TsType::named("string"),
            TsType::named("JsonValue"),
        )]);
        assert_eq!(print_type(&ty), "{ [k: string]: JsonValue }");
    }

    /// #1323's own real gap, found implementing `workers_entry.rs`:
    /// `export { a, b } from "spec";` — a re-export, distinct from both
    /// `Import` and `Export`. Not classified alongside imports for the
    /// "no blank line between adjacent imports" exception.
    #[test]
    fn prints_a_re_export_with_the_ordinary_blank_line_rule() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: "handlers".to_string(),
                from: "./handlers.js".to_string(),
            },
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::ReExport {
                names: vec!["Orders".to_string()],
                from: "./handlers.js".to_string(),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "import * as handlers from \"./handlers.js\";\n\nexport { Orders } from \"./handlers.js\";\n"
        );
    }

    /// #1323's own real gap, found implementing `workers_entry.rs`: a bare
    /// blank line usable at any nesting depth, distinct from `print()`'s
    /// own top-level-only policy.
    #[test]
    fn a_blank_statement_prints_one_empty_line_inside_a_nested_block() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::block(
                vec![
                    TsStmt::expr_stmt(TsExpr::Ident("a".to_string()), None),
                    TsStmt::blank(None),
                    TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None),
                ],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (cond) {\n  a;\n\n  b;\n}\n");
    }

    /// #1323's own real gap, found implementing `workers_entry.rs`:
    /// `CorsPolicy.credentials`/`SecurityPolicy.nosniff` are real booleans.
    #[test]
    fn prints_a_boolean_literal() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::object(vec![
                ("credentials".to_string(), TsExpr::Lit(TsLit::Bool(true))),
                ("nosniff".to_string(), TsExpr::Lit(TsLit::Bool(false))),
            ]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "{ credentials: true, nosniff: false };\n");
    }

    /// #1323's own real gap: an explicit `Paren` always prints its own
    /// literal parens, even when the wrapped expression's own precedence
    /// would not otherwise need any — `workers_entry.rs`'s CORS-preflight
    /// guard wraps its path-match condition in `(...)` unconditionally,
    /// even for a single equality check with nothing lower-precedence than
    /// the outer `&&` inside it (which the ordinary precedence-derived
    /// rules correctly do NOT parenthesize on their own).
    #[test]
    fn an_explicit_paren_always_prints_regardless_of_the_inner_expressions_own_precedence() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::And,
                left: Box::new(TsExpr::Ident("method".to_string())),
                right: Box::new(TsExpr::Paren(Box::new(TsExpr::Binary {
                    op: TsBinaryOp::StrictEq,
                    left: Box::new(TsExpr::Ident("path".to_string())),
                    right: Box::new(TsExpr::Lit(TsLit::Str("/foo".to_string()))),
                }))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "method && (path === \"/foo\");\n");
    }

    /// #1323's own real gap: a 3-term `||` chain of the *same* operator
    /// prints flat, matching `emit_call_handler_dispatch`'s own real
    /// `typeof args !== "object" || args === null || Array.isArray(args)`
    /// — the pre-#1323 "always parenthesize equal precedence" rule would
    /// have wrongly added parens around the first two terms.
    #[test]
    fn a_three_term_or_chain_of_the_same_operator_prints_flat() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Or,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::Or,
                    left: Box::new(TsExpr::Binary {
                        op: TsBinaryOp::StrictNotEq,
                        left: Box::new(TsExpr::Unary {
                            op: TsUnaryOp::Typeof,
                            expr: Box::new(TsExpr::Ident("args".to_string())),
                        }),
                        right: Box::new(TsExpr::Lit(TsLit::Str("object".to_string()))),
                    }),
                    right: Box::new(TsExpr::Binary {
                        op: TsBinaryOp::StrictEq,
                        left: Box::new(TsExpr::Ident("args".to_string())),
                        right: Box::new(TsExpr::Lit(TsLit::Null)),
                    }),
                }),
                right: Box::new(TsExpr::Call {
                    callee: Box::new(TsExpr::Member {
                        object: Box::new(TsExpr::Ident("Array".to_string())),
                        property: "isArray".to_string(),
                    }),
                    args: vec![TsExpr::Ident("args".to_string())],
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "typeof args !== \"object\" || args === null || Array.isArray(args);\n"
        );
    }

    /// Arc C, step (11) (#1388): `TsBinaryOp::Add`'s own real, dominant real
    /// site — a message template's own literal/placeholder segments,
    /// left-folded via `.join(" + ")` — needs the identical flat-chain
    /// treatment `||`/`&&` already established, joining the same
    /// associativity exemption in `render_binary_operand`, not a
    /// bespoke third rule.
    #[test]
    fn a_three_term_add_chain_of_the_same_operator_prints_flat() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::Add,
                    left: Box::new(TsExpr::Lit(TsLit::Str("a".to_string()))),
                    right: Box::new(TsExpr::Lit(TsLit::Str("b".to_string()))),
                }),
                right: Box::new(TsExpr::Lit(TsLit::Str("c".to_string()))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "\"a\" + \"b\" + \"c\";\n");
    }

    /// Review of #1389, finding 1: unlike `||`/`&&` (semantically
    /// associative — flattening either side is safe), `+` is only
    /// grammatically left-associative — `1 + (2 + "3")` and `(1 + 2) + "3"`
    /// disagree once a number joins the chain. A right-nested `Add` must
    /// keep its parens, matching every other non-`Or`/`And` same-operator
    /// chain's own treatment (`a_right_nested_strict_eq_chain_keeps_its_
    /// parens`/`a_right_nested_greater_than_chain_keeps_its_parens`).
    #[test]
    fn a_right_nested_add_chain_keeps_its_parens() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Lit(TsLit::Num("1".to_string()))),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::Add,
                    left: Box::new(TsExpr::Lit(TsLit::Num("2".to_string()))),
                    right: Box::new(TsExpr::Lit(TsLit::Str("3".to_string()))),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "1 + (2 + \"3\");\n");
    }

    /// `Add`'s own new `binary_precedence` entry (higher than
    /// `GreaterThan`, matching real JS/TS) — a nested `Add` under a
    /// `GreaterThan` needs no parens (`+` binds tighter), the reverse
    /// direction does.
    #[test]
    fn add_binds_tighter_than_greater_than_on_both_sides() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::GreaterThan,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::Add,
                    left: Box::new(TsExpr::Ident("a".to_string())),
                    right: Box::new(TsExpr::Ident("b".to_string())),
                }),
                right: Box::new(TsExpr::Ident("c".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a + b > c;\n");
    }

    /// The reverse nesting: a `GreaterThan` operand under an outer `Add`
    /// needs its parens (`+` binds tighter, so `Add`'s own precedence check
    /// against a lower-precedence `GreaterThan` operand must still fire).
    #[test]
    fn greater_than_nested_under_add_keeps_its_parens() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::GreaterThan,
                    left: Box::new(TsExpr::Ident("b".to_string())),
                    right: Box::new(TsExpr::Ident("c".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a + (b > c);\n");
    }

    /// Review of #1324, finding 1: the same-operator flattening #1323 added
    /// for `||`/`&&` was wrongly applied to every operator, including
    /// non-associative ones — a right-nested `StrictEq` inside `StrictEq`
    /// must keep its parens (`a === b === c` parses as `(a === b) === c`,
    /// not the tree's real `a === (b === c)`).
    #[test]
    fn a_right_nested_strict_eq_chain_keeps_its_parens() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::StrictEq,
                    left: Box::new(TsExpr::Ident("b".to_string())),
                    right: Box::new(TsExpr::Ident("c".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a === (b === c);\n");
    }

    /// Same finding, for the new `GreaterThan` operator: `a > b > c` parses
    /// as `(a > b) > c` (a boolean compared against `c`), not the tree's
    /// real `a > (b > c)` — a right-nested chain must keep its parens.
    #[test]
    fn a_right_nested_greater_than_chain_keeps_its_parens() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::GreaterThan,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::GreaterThan,
                    left: Box::new(TsExpr::Ident("b".to_string())),
                    right: Box::new(TsExpr::Ident("c".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a > (b > c);\n");
    }

    /// Review of #1324, finding 2: a `Blank` used as an `if`'s brace-free
    /// body used to render as a bare newline, silently letting the very
    /// next statement in the enclosing block become the `if`'s own body —
    /// now it prints an honest empty statement (`;`) that can't swallow
    /// anything after it.
    #[test]
    fn a_blank_if_body_prints_an_empty_statement_not_a_swallowing_newline() {
        // Nested inside a `Block`, not two top-level program statements —
        // `print`'s own blank-line-between-statements policy only applies
        // between top-level statements (see its own doc comment), so a
        // top-level pair would mask the swallowing bug this test exists to
        // catch: the real hazard is two statements sharing one physical
        // line with nothing separating them, exactly what `render_stmt`'s
        // `Block` arm produces with no blank-line insertion of its own.
        let mut program = TsProgram::new();
        program.push(TsStmt::block(
            vec![
                TsStmt::if_stmt(TsExpr::Ident("cond".to_string()), TsStmt::blank(None), None),
                TsStmt::expr_stmt(
                    TsExpr::Call {
                        callee: Box::new(TsExpr::Ident("nextStatement".to_string())),
                        args: vec![],
                    },
                    None,
                ),
            ],
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "{\n  if (cond) ;\n  nextStatement();\n}\n");
    }

    /// A left-nested same-operator chain also keeps its parens for a
    /// non-associative operator (unlike the `||`/`&&` case above) —
    /// `(a === b) === c` reads the same as `a === b === c` would parse, so
    /// this is really about `render_binary_operand` not silently dropping
    /// parens it should keep, not about a left/right asymmetry.
    #[test]
    fn a_left_nested_strict_eq_chain_keeps_its_parens() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::StrictEq,
                    left: Box::new(TsExpr::Ident("a".to_string())),
                    right: Box::new(TsExpr::Ident("b".to_string())),
                }),
                right: Box::new(TsExpr::Ident("c".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "(a === b) === c;\n");
    }

    // -- #1325: emit_test_main's own new shapes ------------------------------

    #[test]
    fn prints_a_template_literal_with_substitutions() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::template_lit(
                vec![
                    String::new(),
                    " passed, ".to_string(),
                    " failed.".to_string(),
                ],
                vec![
                    TsExpr::Ident("passed".to_string()),
                    TsExpr::Ident("failed".to_string()),
                ],
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "`${passed} passed, ${failed} failed.`;\n");
    }

    /// A template literal's own static parts print with no escaping applied
    /// by the printer — see `TsExpr::TemplateLit`'s own doc for why: a
    /// generic escaper would double the literal backslash of an
    /// already-pre-formed JS unicode escape (`✓`), corrupting it. This
    /// pins the real, grounded shape directly: the six ASCII characters
    /// `✓` pass through unchanged, not doubled into `\\u2713`.
    #[test]
    fn a_template_literal_part_carrying_a_preformed_unicode_escape_is_not_reescaped() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::template_lit(
                vec!["  \\u2713 ".to_string(), String::new()],
                vec![TsExpr::Ident("r".to_string())],
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "`  \\u2713 ${r}`;\n");
    }

    #[test]
    fn a_raw_literal_prints_exactly_as_given_with_no_escaping() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Lit(TsLit::Raw("\"integration \\u00b7 \"".to_string())),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "\"integration \\u00b7 \";\n");
    }

    #[test]
    fn prints_a_multiline_array_literal() {
        let mut program = TsProgram::new();
        program.push(TsStmt::const_stmt(
            TsBindingName::Ident("modules".to_string()),
            None,
            TsExpr::multiline_array(vec![
                TsExpr::object(vec![(
                    "name".to_string(),
                    TsExpr::Lit(TsLit::Str("a".to_string())),
                )]),
                TsExpr::object(vec![(
                    "name".to_string(),
                    TsExpr::Lit(TsLit::Str("b".to_string())),
                )]),
            ]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "const modules = [\n  { name: \"a\" },\n  { name: \"b\" },\n];\n"
        );
    }

    /// Same reachability boundary as `TsExpr::Object`'s own `multiline`
    /// field (see its doc) — a `multiline: true` array nested inside another
    /// expression falls back to single-line via the depth-unaware
    /// `render_expr` recursion.
    #[test]
    fn a_nested_multiline_array_falls_back_to_single_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::array(vec![TsExpr::multiline_array(vec![TsExpr::Lit(
                TsLit::Num("1".to_string()),
            )])]),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "[[1]];\n");
    }

    #[test]
    fn prints_a_declare_const_ambient_binding() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::DeclareConst {
                name: "process".to_string(),
                ty: TsType::Object(vec![TsTypeMember::prop("env", TsType::named("unknown"))]),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "declare const process: { env: unknown };\n");
    }

    #[test]
    fn prints_an_async_top_level_function() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Function {
                name: "main".to_string(),
                generics: Vec::new(),
                params: vec![],
                return_type: None,
                body: vec![TsStmt::return_stmt(None, None)],
                is_async: true,
                inline: false,
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "async function main() {\n  return;\n}\n");
    }

    /// #1369 (Arc C, slice 20): `TsDecl::Function.inline` — the zero-factory
    /// shape (`function name(): Ret { return <expr>; }` all on one line)
    /// `emit_agent` needs, mirroring `TsObjectEntry::Method.inline`'s own
    /// single-line-vs-multi-line precedent (#1337) at a different node kind.
    #[test]
    fn prints_an_inline_top_level_function() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Function {
                name: "zero".to_string(),
                generics: Vec::new(),
                params: vec![],
                return_type: Some(TsType::named("Foo")),
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::object(vec![(
                        "n".to_string(),
                        TsExpr::Lit(TsLit::Num("0".to_string())),
                    )])),
                    None,
                )],
                is_async: false,
                inline: true,
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "function zero(): Foo { return { n: 0 }; }\n");
    }

    #[test]
    fn prints_a_postfix_increment_statement() {
        let mut program = TsProgram::new();
        program.push(TsStmt::increment(TsExpr::Ident("passed".to_string()), None));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "passed++;\n");
    }

    /// An `Increment` inside an `InlineBlock` renders correctly through
    /// `render_inline_stmt`'s fallback — `emit_test_main`'s own real
    /// `{ passed++; console.log(...); }` shape.
    #[test]
    fn a_postfix_increment_renders_correctly_inside_an_inline_block() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::inline_block(
                vec![
                    TsStmt::increment(TsExpr::Ident("passed".to_string()), None),
                    TsStmt::expr_stmt(TsExpr::Ident("next".to_string()), None),
                ],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (cond) { passed++; next; }\n");
    }

    /// `same_line_else` with a `Block` then-branch — `} else {` on one
    /// physical line, distinct from the fresh-line default pinned elsewhere
    /// in this module.
    #[test]
    fn same_line_else_puts_the_else_keyword_after_the_closing_brace() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_else_same_line_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::block(
                vec![TsStmt::expr_stmt(TsExpr::Ident("a".to_string()), None)],
                None,
            ),
            TsStmt::block(
                vec![TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None)],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (cond) {\n  a;\n} else {\n  b;\n}\n");
    }

    /// `same_line_else` with `InlineBlock` branches on both sides —
    /// `emit_test_main`'s own real `if (r.pass) { ... } else { ... }` shape,
    /// entirely on one generated line.
    #[test]
    fn same_line_else_with_inline_block_branches_stays_on_one_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_else_same_line_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::inline_block(
                vec![TsStmt::expr_stmt(TsExpr::Ident("a".to_string()), None)],
                None,
            ),
            TsStmt::inline_block(
                vec![TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None)],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (cond) { a; } else { b; }\n");
    }

    /// `same_line_else` with a brace-free `then_branch` falls back to the
    /// ordinary fresh-line rendering — nothing real needs `<inline-stmt>
    /// else {`, and the printer doesn't claim to support it (see
    /// `TsStmtKind::If`'s own doc).
    #[test]
    fn same_line_else_with_a_brace_free_then_branch_falls_back_to_fresh_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::if_else_same_line_stmt(
            TsExpr::Ident("cond".to_string()),
            TsStmt::continue_stmt(None),
            TsStmt::block(
                vec![TsStmt::expr_stmt(TsExpr::Ident("b".to_string()), None)],
                None,
            ),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "if (cond) continue;\nelse {\n  b;\n}\n");
    }

    /// `TsDecl::ReExportAll` prints `export * from "spec";` — no braces, no
    /// name list, matching `emit_commons_barrel`'s own real per-file line.
    #[test]
    fn re_export_all_prints_a_wildcard_re_export() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ReExportAll {
                from: "./thing/make.js".to_string(),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "export * from \"./thing/make.js\";\n");
    }

    /// Review of #1329's own grounding: `emit_commons_barrel`'s real barrel
    /// module is one header `Comment` immediately followed by one `export
    /// *` line per constituent source file, every one of those lines
    /// adjacent with no blank line anywhere — the exact byte shape pinned
    /// here, matching `251_multi_file_commons_test`'s own real
    /// `expected/thing.ts`.
    #[test]
    fn a_header_comment_and_consecutive_re_export_alls_have_no_blank_lines() {
        let mut program = TsProgram::new();
        program.push(TsStmt::comment(
            "Generated by bynkc — do not edit by hand.",
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::ReExportAll {
                from: "./thing/make.js".to_string(),
            },
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::ReExportAll {
                from: "./thing/widget.js".to_string(),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "// Generated by bynkc — do not edit by hand.\n\
             export * from \"./thing/make.js\";\n\
             export * from \"./thing/widget.js\";\n"
        );
    }

    /// Review of #1330: the grouping rule added for #1329 is scoped to
    /// `ReExportAll`-adjacent-to-`ReExportAll` (and a `Comment` immediately
    /// before one) — a `ReExportAll` next to anything else still gets the
    /// ordinary blank line. `emit_commons_barrel` is `ReExportAll`'s only
    /// producer today, so this is the invariant keeping the rule scoped if
    /// a future slice ever gives it a second one.
    #[test]
    fn a_re_export_all_next_to_a_non_re_export_all_gets_the_ordinary_blank_line() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: "handlers".to_string(),
                from: "./handlers.js".to_string(),
            },
            None,
        ));
        program.push(TsStmt::decl(
            TsDecl::ReExportAll {
                from: "./thing/make.js".to_string(),
            },
            None,
        ));
        program.push(TsStmt::comment("trailing note", None));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "import * as handlers from \"./handlers.js\";\n\n\
             export * from \"./thing/make.js\";\n\n\
             // trailing note\n"
        );
    }

    /// #1333: `print_stmt` prints a `TsStmtKind::DocComment` as a real JSDoc
    /// block, matching `emit_doc_block`'s own real multi-line shape
    /// (`137_agent_instantiation_workers/expected/workers/demo-counter/
    /// handlers.ts`'s own real header comment).
    #[test]
    fn print_stmt_renders_a_multi_line_doc_comment() {
        let stmt = TsStmt::doc_comment(
            "A minimal stateful agent in the bundle target: instantiation lowers through the\ngenerated factory, the method call is a direct call, and state persists per key\nacross calls within a session.",
            None,
        );
        assert_eq!(
            print_stmt(&stmt, 0),
            "/**\n \
             * A minimal stateful agent in the bundle target: instantiation lowers through the\n \
             * generated factory, the method call is a direct call, and state persists per key\n \
             * across calls within a session.\n \
             */\n"
        );
    }

    /// #1333: a literal `*/` inside doc text escapes to `*\/`, matching
    /// `emit_doc_block`'s own pre-conversion behaviour exactly (issue
    /// #720 — an unescaped `*/` would otherwise close the comment early
    /// and let trailing text land as executable top-level TypeScript).
    #[test]
    fn print_stmt_escapes_a_literal_comment_terminator_inside_doc_text() {
        let stmt = TsStmt::doc_comment("docs */ ; (globalThis as any).PWNED = true; /*", None);
        let printed = print_stmt(&stmt, 0);
        assert!(!printed.contains("*/ ;"), "unescaped terminator: {printed}");
        assert_eq!(
            printed,
            "/**\n * docs *\\/ ; (globalThis as any).PWNED = true; /*\n */\n"
        );
    }

    /// #1333: a blank line inside doc text prints as a bare ` *`, no
    /// trailing space — distinct from a non-blank line's own ` * <line>`.
    #[test]
    fn print_stmt_renders_a_blank_doc_line_as_a_bare_star() {
        let stmt = TsStmt::doc_comment("first paragraph\n\nsecond paragraph", None);
        assert_eq!(
            print_stmt(&stmt, 0),
            "/**\n * first paragraph\n *\n * second paragraph\n */\n"
        );
    }

    /// #1333: `print_stmt`'s own `depth` parameter indents every line of
    /// the JSDoc block, matching `render_stmt`'s own 2-space-per-level
    /// convention — `emit_doc_block`'s real callers pass `INDENT_STEP`
    /// (2 raw spaces = depth 1) for a nested doc comment.
    #[test]
    fn print_stmt_indents_a_doc_comment_at_depth() {
        let stmt = TsStmt::doc_comment("a method", None);
        assert_eq!(print_stmt(&stmt, 1), "  /**\n   * a method\n   */\n");
    }

    /// #1337: `TsStmtKind::Raw` prints its own text exactly as given — no
    /// leading indent (unlike every other statement kind, whose own
    /// `render_stmt` arm prefixes `indent(depth)`), no added semicolon or
    /// braces. `emit_method`'s own opaque `lower.rs`-sourced body is
    /// already fully, absolutely indented by the time it's captured into
    /// one `Raw` node, so the printer must contribute nothing further —
    /// the same reasoning `Verbatim` already established for pre-rendered
    /// content, `Raw`'s own doc explains why it's a distinct kind.
    #[test]
    fn print_stmt_renders_raw_text_verbatim_with_no_added_indent_or_punctuation() {
        let stmt = TsStmt::raw("    return x + 1;\n", None);
        // depth is passed but deliberately has no effect on Raw's own output.
        assert_eq!(print_stmt(&stmt, 3), "    return x + 1;\n");
    }

    /// #1337: multi-line `Raw` text (the real shape — a whole function
    /// body, not one line) passes through with every embedded line intact,
    /// confirming the printer doesn't split, re-indent, or otherwise
    /// interpret it.
    #[test]
    fn print_stmt_renders_multi_line_raw_text_unchanged() {
        let stmt = TsStmt::raw(
            "    const r = Uuid.of(crypto.randomUUID());\n    return r.value;\n",
            None,
        );
        assert_eq!(
            print_stmt(&stmt, 0),
            "    const r = Uuid.of(crypto.randomUUID());\n    return r.value;\n"
        );
    }

    /// #1337: `TsObjectEntry::Method` with a `Raw` body — `emit_method`'s
    /// own real shape (`{method}{generics}({params}): {ret} { <raw body>
    /// },`), pinned at the actual depth its own object literal renders at
    /// (a top-level `export const {...}`, depth 0 → entries at depth 1,
    /// two-space indent, matching `emit_method`'s own pre-conversion
    /// hand-written `"  {method}..."` line).
    #[test]
    fn multiline_object_renders_a_method_entry_with_a_raw_body() {
        let mut out = String::new();
        render_multiline_object(
            &mut out,
            &[TsObjectEntry::Method {
                name: "of".to_string(),
                is_async: false,
                generics: Vec::new(),
                params: vec![TsParam {
                    name: "value".to_string(),
                    ty: Some(TsType::named("string")),
                    optional: false,
                }],
                return_type: Some(TsType::named("Uuid")),
                doc: None,
                inline: false,
                body: vec![TsStmt::raw("    return value as Uuid;\n", None)],
            }],
            0,
        );
        assert_eq!(
            out,
            "{\n  of(value: string): Uuid {\n    return value as Uuid;\n  },\n}"
        );
    }

    /// #1337: `TsObjectEntry::Method`'s own `generics` field prints
    /// `<A, U>` between the method name and its parameter list —
    /// `Box.map`'s own real shape (`402_generic_instance_method`, a
    /// single-file-form fixture the accepted proposal's own project-form
    /// search missed).
    #[test]
    fn multiline_object_renders_a_generic_method_entry() {
        let mut out = String::new();
        render_multiline_object(
            &mut out,
            &[TsObjectEntry::Method {
                name: "map".to_string(),
                is_async: false,
                generics: vec!["A".to_string(), "U".to_string()],
                params: vec![
                    TsParam {
                        name: "self".to_string(),
                        ty: Some(TsType::named("Box<A>")),
                        optional: false,
                    },
                    TsParam {
                        name: "f".to_string(),
                        ty: Some(TsType::named("(a0: A) => U")),
                        optional: false,
                    },
                ],
                return_type: Some(TsType::named("Box<U>")),
                doc: None,
                inline: false,
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::Ident("{ value: f(self.value) }".to_string())),
                    None,
                )],
            }],
            0,
        );
        assert_eq!(
            out,
            "{\n  map<A, U>(self: Box<A>, f: (a0: A) => U): Box<U> {\n    return { value: f(self.value) };\n  },\n}"
        );
    }

    /// #1337: a method with no generics prints no `<>` at all — the
    /// ordinary, dominant case (every prior slice's own real method
    /// entries), confirming `generics: Vec::new()` is a true no-op, not
    /// an empty `<>`.
    #[test]
    fn multiline_object_renders_a_non_generic_method_entry_with_no_angle_brackets() {
        let mut out = String::new();
        render_multiline_object(
            &mut out,
            &[TsObjectEntry::Method {
                name: "get".to_string(),
                is_async: false,
                generics: Vec::new(),
                params: vec![],
                return_type: Some(TsType::named("void")),
                doc: None,
                inline: false,
                body: vec![],
            }],
            0,
        );
        assert_eq!(out, "{\n  get(): void {\n  },\n}");
    }

    /// #1337: `TsObjectEntry::Method`'s own `doc` field prints a JSDoc
    /// block immediately before the method entry, same indent, no blank
    /// line between — `Timestamp.diff`'s own real shape
    /// (`65_money_uses_time`), reusing `render_doc_comment` (the same
    /// renderer `TsStmtKind::DocComment` already uses) rather than a
    /// second copy.
    #[test]
    fn multiline_object_renders_a_method_entry_with_a_preceding_doc_comment() {
        let mut out = String::new();
        render_multiline_object(
            &mut out,
            &[TsObjectEntry::Method {
                name: "diff".to_string(),
                is_async: false,
                generics: Vec::new(),
                params: vec![TsParam {
                    name: "self".to_string(),
                    ty: Some(TsType::named("Timestamp")),
                    optional: false,
                }],
                return_type: Some(TsType::named("Span")),
                doc: Some("Compute the duration between two timestamps.".to_string()),
                inline: false,
                body: vec![TsStmt::raw("    return 0;\n", None)],
            }],
            0,
        );
        assert_eq!(
            out,
            "{\n  /**\n   * Compute the duration between two timestamps.\n   */\n  diff(self: Timestamp): Span {\n    return 0;\n  },\n}"
        );
    }

    /// #1337: `TsObjectEntry::Method`'s own `inline: true` renders the
    /// whole entry — signature and one-statement body alike — on ONE
    /// physical line, `emit_forwarded_methods`'s own real shape
    /// (`255_context_uses_commons_static_method`'s own real `equals`
    /// entry: `equals(self: Cents, other: Cents): boolean { return
    /// __CommonsCents.equals(self, other) as unknown as boolean; },`),
    /// distinct from every other real `Method` entry in this tree
    /// (always multi-line, `inline: false`).
    #[test]
    fn multiline_object_renders_an_inline_method_entry_on_one_line() {
        let mut out = String::new();
        render_multiline_object(
            &mut out,
            &[TsObjectEntry::Method {
                name: "equals".to_string(),
                is_async: false,
                generics: Vec::new(),
                params: vec![
                    TsParam {
                        name: "self".to_string(),
                        ty: Some(TsType::named("Cents")),
                        optional: false,
                    },
                    TsParam {
                        name: "other".to_string(),
                        ty: Some(TsType::named("Cents")),
                        optional: false,
                    },
                ],
                return_type: Some(TsType::named("boolean")),
                doc: None,
                inline: true,
                body: vec![TsStmt::return_stmt(
                    Some(TsExpr::As {
                        expr: Box::new(TsExpr::As {
                            expr: Box::new(TsExpr::Call {
                                callee: Box::new(TsExpr::Member {
                                    object: Box::new(TsExpr::Ident("__CommonsCents".to_string())),
                                    property: "equals".to_string(),
                                }),
                                args: vec![
                                    TsExpr::Ident("self".to_string()),
                                    TsExpr::Ident("other".to_string()),
                                ],
                            }),
                            ty: TsType::named("unknown"),
                        }),
                        ty: TsType::named("boolean"),
                    }),
                    None,
                )],
            }],
            0,
        );
        assert_eq!(
            out,
            "{\n  equals(self: Cents, other: Cents): boolean { return __CommonsCents.equals(self, other) as unknown as boolean; },\n}"
        );
    }

    /// Review of #1338, finding 4: `print_object_entry` — the exact public
    /// API `emit_refined_type`/`emit_record_type`/`emit_sum_type`'s own
    /// real call sites depend on — had no direct test; every #1337 test
    /// above drives `render_multiline_object` instead. Pins the contract
    /// those call sites rely on: `print_object_entry(&entry, 0)` produces
    /// the same text as the entry-slice portion of what
    /// `render_multiline_object`'s own depth-0 output produces for the
    /// same entry (its own depth convention — the entry lands one level
    /// deeper than the object, i.e. `depth + 1` — matches exactly, not
    /// coincidentally, since both paths route through the same
    /// `render_multiline_object_entry`).
    #[test]
    fn print_object_entry_matches_the_multiline_objects_own_entry_slice() {
        // A real `TsStmt::return_stmt` body, not `Raw` — `Raw`'s own baked-in
        // indent only matches depth 0 (finding 3's own new guard), so an
        // ordinary real-node body is what this depth-convention contract
        // needs to prove at a non-zero depth.
        let entry = TsObjectEntry::Method {
            name: "of".to_string(),
            is_async: false,
            generics: Vec::new(),
            params: vec![TsParam {
                name: "value".to_string(),
                ty: Some(TsType::named("string")),
                optional: false,
            }],
            return_type: Some(TsType::named("Uuid")),
            doc: None,
            inline: false,
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::As {
                    expr: Box::new(TsExpr::Ident("value".to_string())),
                    ty: TsType::named("Uuid"),
                }),
                None,
            )],
        };

        let mut whole_object = String::new();
        render_multiline_object(&mut whole_object, std::slice::from_ref(&entry), 0);
        // `render_multiline_object`'s own entry line is followed by `\n`
        // then the closing `}` (no blank line between them for a single
        // entry) — `print_object_entry`'s own output keeps that same
        // trailing `\n` (it has no closing brace of its own to attach to),
        // so the real equivalence is entry-slice-plus-newline, not a bare
        // slice.
        let entry_slice_with_trailing_newline = whole_object
            .strip_prefix("{\n")
            .and_then(|s| s.strip_suffix('}'))
            .expect("render_multiline_object's own single-entry output has a { }-wrapper");

        assert_eq!(
            print_object_entry(&entry, 0),
            entry_slice_with_trailing_newline
        );
    }

    /// #1339's own real gap: `emit_refined_type`'s own branded-type alias,
    /// `{base} & { readonly __brand: "..." }`, has no representation among
    /// `Named`/`Array`/`Object`/`Fn`/`Union` — mirrors `Union`'s own single-
    /// line, ` & `-joined shape exactly.
    #[test]
    fn print_type_renders_an_intersection() {
        let ty = TsType::intersection(vec![
            TsType::named("string"),
            TsType::Object(vec![TsTypeMember::readonly_prop(
                "__brand",
                TsType::named("\"Order\""),
            )]),
        ]);
        assert_eq!(print_type(&ty), "string & { readonly __brand: \"Order\" }");
    }

    /// #1339's own real gap: `emit_sum_type`'s own multi-line discriminated
    /// union — a leading `|` on every line except the first (which gets
    /// equivalent spacing instead, matching the pre-conversion `writeln!`
    /// code's own `let pipe = if i == 0 { " " } else { "|" };` exactly), no
    /// trailing newline or `;` of its own (the caller, `TsDecl::TypeAlias`'s
    /// own render arm, owns both).
    #[test]
    fn print_type_renders_a_multiline_union() {
        let ty = TsType::multiline_union(vec![
            TsType::Object(vec![TsTypeMember::readonly_prop(
                "tag",
                TsType::named("\"a\""),
            )]),
            TsType::Object(vec![
                TsTypeMember::readonly_prop("tag", TsType::named("\"b\"")),
                TsTypeMember::readonly_prop("value", TsType::named("number")),
            ]),
        ]);
        assert_eq!(
            print_type(&ty),
            "    { readonly tag: \"a\" }\n  | { readonly tag: \"b\"; readonly value: number }"
        );
    }

    /// #1339: `TsDecl::TypeAlias`'s own multiline-union special case — the
    /// `=` is followed directly by `\n` (no trailing space, matching the
    /// pre-conversion `writeln!(out, "export type {name}{params} =")`
    /// line's own exact bytes), each variant on its own line, the closing
    /// `;` appended directly to the last variant's own line. Also pins
    /// `type_params`' own bare-generics rendering on the alias header.
    #[test]
    fn prints_a_generic_sum_types_own_multiline_type_alias() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::TypeAlias {
                name: "Opt".to_string(),
                type_params: vec!["T".to_string()],
                ty: TsType::multiline_union(vec![
                    TsType::Object(vec![TsTypeMember::readonly_prop(
                        "tag",
                        TsType::named("\"none\""),
                    )]),
                    TsType::Object(vec![
                        TsTypeMember::readonly_prop("tag", TsType::named("\"some\"")),
                        TsTypeMember::readonly_prop("value", TsType::named("T")),
                    ]),
                ]),
            })),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "export type Opt<T> =\n    \
             { readonly tag: \"none\" }\n  \
             | { readonly tag: \"some\"; readonly value: T };\n"
        );
    }

    /// #1339's own real gap: `emit_record_type`'s own `export interface
    /// {name}{params} { readonly {field}: {ty}; ... }` — bare generic names
    /// on the interface header, `readonly` on every real member here.
    #[test]
    fn prints_a_generic_interface_with_readonly_members() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Interface {
                name: "Box".to_string(),
                type_params: vec!["T".to_string()],
                members: vec![TsTypeMember::readonly_prop("value", TsType::named("T"))],
            })),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "export interface Box<T> {\n  readonly value: T;\n}\n"
        );
    }

    /// #1357's own real gap: `TsTypeMember::Method` had no `generics`/`doc`
    /// fields — `emit_capability`'s own interface methods are genuinely
    /// generic (no monomorphisation) and doc-commented per op. `doc` renders
    /// at `TsDecl::Interface`'s own render arm (not `render_type_member`
    /// itself, which has no `depth` to give `render_doc_comment`), mirroring
    /// `TsObjectEntry::Method.doc`'s own identical split (#1337).
    #[test]
    fn prints_a_generic_documented_interface_method() {
        let mut program = TsProgram::new();
        program.push(TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Interface {
                name: "Clock".to_string(),
                type_params: Vec::new(),
                members: vec![TsTypeMember::Method {
                    name: "now".to_string(),
                    generics: vec!["T".to_string()],
                    params: vec![TsParam {
                        name: "unit".to_string(),
                        ty: Some(TsType::named("T")),
                        optional: false,
                    }],
                    ret: TsType::named("number"),
                    doc: Some("Returns the current time.".to_string()),
                }],
            })),
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "export interface Clock {\n  /**\n   * Returns the current time.\n   */\n  now<T>(unit: T): number;\n}\n"
        );
    }

    /// #1359's own real need: `print_class_method` — a class-method sibling
    /// of `print_object_entry`'s own fragment-printing entry point,
    /// `emit_provider`'s own hand-written class wrapper prints each real
    /// method through this directly. `depth` means the *class's* own
    /// depth, so at `depth == 0` the method itself lands at `indent(1)`,
    /// its body at `indent(2)`, matching `TsDecl::Class`'s own equivalent
    /// per-method indent exactly — but with no automatic blank-line
    /// insertion of its own (unlike `TsDecl::Class`'s own render arm),
    /// since `emit_provider`'s own real spacing has none between methods.
    #[test]
    fn prints_a_single_class_method_fragment() {
        let method = TsClassMethod {
            name: "double".to_string(),
            private: false,
            is_async: true,
            params: vec![TsParam {
                name: "n".to_string(),
                ty: Some(TsType::named("number")),
                optional: false,
            }],
            return_type: Some(TsType::named("number")),
            doc: None,
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::Binary {
                    op: TsBinaryOp::NullishCoalescing,
                    left: Box::new(TsExpr::Ident("n".to_string())),
                    right: Box::new(TsExpr::Lit(TsLit::Num("0".to_string()))),
                }),
                None,
            )],
        };
        assert_eq!(
            print_class_method(&method, 0),
            "  async double(n: number): number {\n    return n ?? 0;\n  }\n"
        );
    }

    /// Arc C, slice 21 (#1371): `TsClassMethod.private` — pins both the
    /// keyword itself and its render-*before*-`async` ordering directly,
    /// the same precedent `TsClassField.private` already has (its own
    /// direct test, not left to transitive `bynkc` fixture coverage alone)
    /// — review of #1372 caught this one still missing for the sibling
    /// field.
    #[test]
    fn prints_a_private_class_method_fragment() {
        let method = TsClassMethod {
            name: "loadState".to_string(),
            private: true,
            is_async: true,
            params: Vec::new(),
            return_type: Some(TsType::named_with_args(
                "Promise",
                vec![TsType::named("OrderState")],
            )),
            doc: None,
            body: vec![TsStmt::return_stmt(
                Some(TsExpr::Ident("state".to_string())),
                None,
            )],
        };
        assert_eq!(
            print_class_method(&method, 0),
            "  private async loadState(): Promise<OrderState> {\n    return state;\n  }\n"
        );
    }

    /// Arc C, slice 23 (#1375): `TsClassMethod.doc` — the grounding pass's
    /// own second predicted gap (#1366), the same need
    /// `TsObjectEntry::Method.doc` (#1337) and `TsTypeMember::Method.doc`
    /// (#1357) already solved for their own node kinds, closed here for the
    /// third and last real method-shaped node in this crate.
    #[test]
    fn prints_a_documented_class_method_fragment() {
        let method = TsClassMethod {
            name: "spend".to_string(),
            private: false,
            is_async: false,
            params: Vec::new(),
            return_type: Some(TsType::named("void")),
            doc: Some("Debits the account.".to_string()),
            body: vec![],
        };
        assert_eq!(
            print_class_method(&method, 0),
            "  /**\n   * Debits the account.\n   */\n  spend(): void {\n  }\n"
        );
    }

    /// #1339's own real gap: `TsExpr::Arrow` had no `generics`/`return_type`
    /// field — `emit_sum_type`'s own generic payload-constructor arrows
    /// (`<T>(name: T): Sum<T> => (...)`) need both. The object-literal body
    /// is wrapped in an explicit `Paren` — `Arrow`'s own renderer does not
    /// auto-parenthesise an object body the way real JS/TS syntax requires
    /// to disambiguate it from a block.
    #[test]
    fn prints_a_generic_arrow_with_a_parenthesised_object_body() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Arrow {
                params: vec![TsParam {
                    name: "value".to_string(),
                    ty: Some(TsType::named("T")),
                    optional: false,
                }],
                is_async: false,
                generics: vec!["T".to_string()],
                return_type: Some(TsType::named_with_args("Sum", vec![TsType::named("T")])),
                body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Paren(Box::new(
                    TsExpr::object_entries(vec![
                        TsObjectEntry::Prop(
                            "tag".to_string(),
                            TsExpr::Lit(TsLit::Str("some".to_string())),
                        ),
                        TsObjectEntry::Shorthand("value".to_string()),
                    ]),
                ))))),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(
            printed.text,
            "<T>(value: T): Sum<T> => ({ tag: \"some\", value });\n"
        );
    }

    /// Arc C, slice 33 (#1401): `TsBinaryOp::LessThan` — pins the operator
    /// text itself.
    #[test]
    fn prints_a_less_than_comparison() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::LessThan,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a < b;\n");
    }

    /// `LessThan` shares `GreaterThan`'s own precedence tier — a nested
    /// `Add` under it needs no parens (`+` binds tighter), mirroring
    /// `add_binds_tighter_than_greater_than_on_both_sides` for the other
    /// relational operator.
    #[test]
    fn add_binds_tighter_than_less_than() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::LessThan,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::Add,
                    left: Box::new(TsExpr::Ident("a".to_string())),
                    right: Box::new(TsExpr::Ident("b".to_string())),
                }),
                right: Box::new(TsExpr::Ident("c".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a + b < c;\n");
    }

    /// Arc C, slice 34 (`tests_emit.rs` slice D, #1403): `TsBinaryOp::
    /// InstanceOf` — pins the keyword operator text itself, sharing
    /// `LessThan`/`GreaterThan`'s own precedence tier.
    #[test]
    fn prints_an_instanceof_check() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::InstanceOf,
                left: Box::new(TsExpr::Ident("e".to_string())),
                right: Box::new(TsExpr::Ident("ExpectationError".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "e instanceof ExpectationError;\n");
    }

    /// Review of #1404, finding 2: `prints_an_instanceof_check` alone pins
    /// the operator text but not the precedence tier the same diff added to
    /// `binary_precedence` — it passes identically whether `InstanceOf` sits
    /// at tier 5, tier 1, or is missing from that arm entirely. `Add` (tier
    /// 6) binds tighter, mirroring `add_binds_tighter_than_less_than`
    /// for the other relational operator sharing this tier.
    #[test]
    fn add_binds_tighter_than_instanceof() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::InstanceOf,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::Add,
                    left: Box::new(TsExpr::Ident("a".to_string())),
                    right: Box::new(TsExpr::Ident("b".to_string())),
                }),
                right: Box::new(TsExpr::Ident("C".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a + b instanceof C;\n");
    }

    /// Review of #1404, finding 2 (the more valuable half): a catch clause
    /// naturally grows into `e instanceof A || e instanceof B` as a second
    /// error type is added. `InstanceOf` (tier 5) binds tighter than `Or`
    /// (tier 2), so this must print flat — a wrong tier would silently
    /// over-parenthesize it into `(e instanceof A) || (e instanceof B)`.
    #[test]
    fn instanceof_binds_tighter_than_or_on_both_sides() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Or,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::InstanceOf,
                    left: Box::new(TsExpr::Ident("e".to_string())),
                    right: Box::new(TsExpr::Ident("A".to_string())),
                }),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::InstanceOf,
                    left: Box::new(TsExpr::Ident("e".to_string())),
                    right: Box::new(TsExpr::Ident("B".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "e instanceof A || e instanceof B;\n");
    }

    /// Arc E slice 5 (`serialisation.rs`, #1443): `TsBinaryOp::In` — pins
    /// the keyword operator text itself, sharing `LessThan`/`GreaterThan`/
    /// `InstanceOf`'s own precedence tier.
    #[test]
    fn prints_an_in_check() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::In,
                left: Box::new(TsExpr::Lit(TsLit::Str("name".to_string()))),
                right: Box::new(TsExpr::Ident("obj".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "\"name\" in obj;\n");
    }

    /// Mirrors `add_binds_tighter_than_instanceof`/`add_binds_tighter_than_
    /// less_than` for the newest member of this shared precedence tier:
    /// `Add` (tier 6) binds tighter than `In` (tier 5).
    #[test]
    fn add_binds_tighter_than_in() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::In,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::Add,
                    left: Box::new(TsExpr::Ident("a".to_string())),
                    right: Box::new(TsExpr::Ident("b".to_string())),
                }),
                right: Box::new(TsExpr::Ident("obj".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a + b in obj;\n");
    }

    /// Mirrors `instanceof_binds_tighter_than_or_on_both_sides`: `In` (tier
    /// 5) binds tighter than `Or` (tier 2), so a real
    /// `"a" in obj || "b" in obj` reads flat with no parens — a wrong tier
    /// would silently over-parenthesize it.
    #[test]
    fn in_binds_tighter_than_or_on_both_sides() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::Or,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::In,
                    left: Box::new(TsExpr::Lit(TsLit::Str("a".to_string()))),
                    right: Box::new(TsExpr::Ident("obj".to_string())),
                }),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::In,
                    left: Box::new(TsExpr::Lit(TsLit::Str("b".to_string()))),
                    right: Box::new(TsExpr::Ident("obj".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "\"a\" in obj || \"b\" in obj;\n");
    }

    /// Review of #1471, finding 1: `TsBinaryOp::GreaterThanEq`/`LessThanEq`
    /// landed with no direct `bynk-ts` test — only pinned indirectly through
    /// `bynk-emit`'s own fixture goldens. This test and the four below close
    /// that locality gap, mirroring the coverage every other operator on
    /// this precedence tier already has (`prints_an_in_check`/`add_binds_
    /// tighter_than_in`/`in_binds_tighter_than_or_on_both_sides`).
    #[test]
    fn prints_a_greater_than_or_equal_comparison() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::GreaterThanEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a >= b;\n");
    }

    #[test]
    fn prints_a_less_than_or_equal_comparison() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::LessThanEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Ident("b".to_string())),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a <= b;\n");
    }

    /// Pins `GreaterThanEq`/`LessThanEq`'s own tier-5 placement — the real
    /// `pred_condition_and_message` `InRange` shape (#1471) nests both under
    /// `And` (tier 3) and must print flat, with no parens on either side. A
    /// regression that demoted either operator to `And`'s own tier or below
    /// would silently wrap this in parens instead.
    #[test]
    fn greater_than_eq_and_less_than_eq_nest_flat_under_and() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::And,
                left: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::GreaterThanEq,
                    left: Box::new(TsExpr::Ident("value".to_string())),
                    right: Box::new(TsExpr::Lit(TsLit::Num("0".to_string()))),
                }),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::LessThanEq,
                    left: Box::new(TsExpr::Ident("value".to_string())),
                    right: Box::new(TsExpr::Lit(TsLit::Num("100".to_string()))),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "value >= 0 && value <= 100;\n");
    }

    /// Same tier, *different* operator: `render_binary_operand`'s own doc
    /// says equal precedence still parenthesises unless the two sides are
    /// the exact same associative operator — `GreaterThan`/`GreaterThanEq`
    /// are equal-tier but distinct, so nesting one under the other must
    /// still keep its parens (the `_ => true` fallback of the equal-
    /// precedence match, newly reachable now this tier has more than one
    /// non-keyword operator).
    #[test]
    fn a_greater_than_eq_operand_of_a_different_relational_op_still_parenthesises() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::GreaterThan,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::GreaterThanEq,
                    left: Box::new(TsExpr::Ident("b".to_string())),
                    right: Box::new(TsExpr::Ident("c".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a > (b >= c);\n");
    }

    /// `GreaterThanEq`/`LessThanEq` nested in themselves are NOT associative
    /// (`a >= b >= c` does not parse as `a >= (b >= c)`), unlike `||`/`&&` —
    /// so, unlike `in_binds_tighter_than_or_on_both_sides`'s own flat `In`
    /// chain, a same-operator nesting here must still parenthesise.
    #[test]
    fn a_right_nested_greater_than_eq_chain_keeps_its_parens() {
        let mut program = TsProgram::new();
        program.push(TsStmt::expr_stmt(
            TsExpr::Binary {
                op: TsBinaryOp::GreaterThanEq,
                left: Box::new(TsExpr::Ident("a".to_string())),
                right: Box::new(TsExpr::Binary {
                    op: TsBinaryOp::GreaterThanEq,
                    left: Box::new(TsExpr::Ident("b".to_string())),
                    right: Box::new(TsExpr::Ident("c".to_string())),
                }),
            },
            None,
        ));
        let printed = print(&program, "x.bynk", "", "x.ts");
        assert_eq!(printed.text, "a >= (b >= c);\n");
    }
}
