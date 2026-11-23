use super::MergeTarget;
use super::decl::render_class_method;
use super::expr::render_expr;
use super::multiline::render_multiline_object_entry;
use super::stmt::render_stmt;
use super::types::render_type;
#[cfg(doc)]
use crate::program::{TsArrowBody, TsDecl, TsProgram};
use crate::program::{TsClassMethod, TsExpr, TsObjectEntry, TsStmt, TsStmtKind, TsType};
use crate::source_map::SourceMapBuilder;
use bynk_syntax::span::Span;

/// Print a single [`TsType`] on its own — the real callers this closes R7.2
/// for (`bynk-emit`'s `ts_type_ref*`/`ts_ty` families, P7.9, #1315) each
/// want one type fragment to interpolate into a larger, still-hand-built
/// line (a field's own type annotation, a parameter list, …), not a whole
/// [`TsProgram`]. No source-map/buffer machinery — [`print()`](super::print) owns that
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
    render_expr(&mut out, expr, 0, &mut None);
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
        depth == 0 || matches!(stmt.kind, TsStmtKind::Raw(..)) || !stmt_contains_raw(stmt),
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
        depth == 0 || matches!(stmt.kind, TsStmtKind::Raw(..)) || !stmt_contains_raw(stmt),
        "print_stmt_and_merge: a Raw-bearing statement's own baked-in indent only matches depth 0"
    );
    render_stmt(out, stmt, depth, Some(MergeTarget { map, source_id }));
}

/// [`print_stmt`], also reporting where each statement that carries a
/// [`TsStmt::span`] starts in the printed text (back-half paydown 1A.11,
/// Decision C of #3712). The offset is a byte offset into the returned text,
/// at the start of the statement's own line, so *before* its indentation; an
/// inline `if`/`else` branch (no braces, same line as its `if`) starts after
/// the space that separates it. The statement itself, then every statement
/// nested in it, is reported in the order printed; a statement with no span
/// is not reported.
///
/// It is the mechanism S5 of the statement-emitter design note (#3708) moves
/// source-map recording onto, applied one statement at a time: the adapter in
/// `bynk-emit` adds the indentation it writes before each line and records the
/// adjusted offsets, which is what a nested checkpoint needs.
pub fn print_stmt_with_spans(stmt: &TsStmt, depth: usize) -> (String, Vec<(usize, Span)>) {
    debug_assert!(
        depth == 0 || matches!(stmt.kind, TsStmtKind::Raw(..)) || !stmt_contains_raw(stmt),
        "print_stmt_with_spans: a Raw-bearing statement's own baked-in indent only matches depth 0"
    );
    let mut map = SourceMapBuilder::new();
    let mut out = String::new();
    render_stmt(
        &mut out,
        stmt,
        depth,
        Some(MergeTarget {
            map: &mut map,
            source_id: 0,
        }),
    );
    (out, map.checkpoint_spans())
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
pub(super) fn contains_raw(stmts: &[TsStmt]) -> bool {
    stmts.iter().any(stmt_contains_raw)
}

fn stmt_contains_raw(stmt: &TsStmt) -> bool {
    match &stmt.kind {
        TsStmtKind::Raw(..) => true,
        TsStmtKind::Verbatim { .. }
        | TsStmtKind::Decl(_)
        | TsStmtKind::Const { .. }
        | TsStmtKind::Let { .. }
        | TsStmtKind::LetList(_)
        | TsStmtKind::ExprStmt(_)
        | TsStmtKind::Return(_)
        | TsStmtKind::Throw(_)
        | TsStmtKind::Continue(_)
        | TsStmtKind::Break(_)
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
        TsStmtKind::ForOf { body, .. }
        | TsStmtKind::For { body, .. }
        | TsStmtKind::While { body, .. }
        | TsStmtKind::Labelled { body, .. } => stmt_contains_raw(body),
        TsStmtKind::TryCatch {
            try_block,
            catch_block,
            finally_block,
            ..
        } => {
            stmt_contains_raw(try_block)
                || catch_block.as_deref().is_some_and(stmt_contains_raw)
                || finally_block.as_deref().is_some_and(stmt_contains_raw)
        }
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
