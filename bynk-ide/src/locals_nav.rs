//! v0.31 (ADR 0064): locals navigation — resolve the local binding under the
//! cursor and all its sites (its definition plus every use that resolves to
//! it), for `references`, go-to-`definition`, and `documentHighlight`.
//!
//! Slice 1 records bindings with scope ranges (not use sites); the use sites
//! are recovered here by lexing the file and keeping the identifier tokens of
//! the binding's name within its scope that resolve back to it (so a shadowing
//! inner binding's uses — and every binding's *def* token — are excluded).
//! Pure over the analysed snapshot, like `index_queries`.

use crate::symbols::is_dot_preceded;
use bynk_check::emitted_names::emitted_local_name;
use bynk_check::locals::{LocalBinding, LocalKind, binding_at_def, locals_at};
use bynk_syntax::lexer::{self, TokenKind};
use bynk_syntax::span::Span;

/// Whether two spans cover the same byte range. Lexer tokens carry
/// `FileId::UNKNOWN` while the checker's `def_span`s carry the analysed file's
/// id, so whole-`Span` equality never holds between them; compare offsets, as
/// the rest of bynk-ide does for lexer spans.
fn same_range(a: Span, b: Span) -> bool {
    a.start == b.start && a.end == b.end
}

/// The identifier-token name covering `offset`, if any. Interpolation holes are
/// expanded (issue #473), so a cursor inside `"… \(name) …"` resolves to the
/// hole's `name` identifier rather than the opaque `InterpStr` token.
///
/// Finding #63: a field-access member name (`items` in `p.items`) is lexically
/// indistinguishable from a bare identifier — excluded via `is_dot_preceded`
/// (the same guard `symbols.rs` uses for state/store-op token matching, #596)
/// so the cursor sitting on a field name never resolves to a same-named local.
fn ident_at(text: &str, offset: usize) -> Option<(&str, Span)> {
    let toks = lexer::tokenize_expanding_holes(text).ok()?;
    toks.into_iter()
        .find(|t| {
            t.kind == TokenKind::Ident
                && t.span.start <= offset
                && offset <= t.span.end
                && !is_dot_preceded(text, t.span.start)
        })
        .map(|t| (&text[t.span.start..t.span.end], t.span))
}

/// The binding the cursor refers to — whether it sits on the definition name
/// or on a use — within `locals` (a file's bindings).
fn target_at<'a>(
    locals: &'a [LocalBinding],
    text: &str,
    offset: usize,
) -> Option<&'a LocalBinding> {
    let (name, _) = ident_at(text, offset)?;
    binding_at_def(locals, offset)
        .filter(|b| b.name == name)
        .or_else(|| {
            locals_at(locals, offset)
                .into_iter()
                .find(|b| b.name == name)
        })
}

/// All sites of the local under the cursor — its definition first, then every
/// use that resolves to it (shadowing-safe). `None` when the cursor is not on
/// a local.
pub fn local_sites_at(locals: &[LocalBinding], text: &str, offset: usize) -> Option<Vec<Span>> {
    let target = target_at(locals, text, offset)?;
    // Hole-aware (issue #473): use sites inside `\(…)` holes count too.
    let toks = lexer::tokenize_expanding_holes(text).ok()?;
    let mut sites = vec![target.def_span];
    for t in &toks {
        if t.kind != TokenKind::Ident || text[t.span.start..t.span.end] != target.name {
            continue;
        }
        if same_range(t.span, target.def_span) {
            continue; // the definition, already added
        }
        // A binding's own def token is not a use of anything.
        if locals.iter().any(|b| same_range(b.def_span, t.span)) {
            continue;
        }
        // Finding #63: a field-access member name (`r.total`) is not a use of
        // a same-named local, even when one is in scope.
        if is_dot_preceded(text, t.span.start) {
            continue;
        }
        if t.span.start < target.scope.start || t.span.end > target.scope.end {
            continue; // outside the binding's scope
        }
        // Does this use resolve to `target` (not a shadowing inner binding)?
        let resolves = locals_at(locals, t.span.start)
            .into_iter()
            .find(|b| b.name == target.name)
            .map(|b| b.def_span);
        if resolves == Some(target.def_span) {
            sites.push(t.span);
        }
    }
    Some(sites)
}

/// The definition site of the local under the cursor, if any.
pub fn local_definition_at(locals: &[LocalBinding], text: &str, offset: usize) -> Option<Span> {
    target_at(locals, text, offset).map(|b| b.def_span)
}

/// v0.122 (editor-currency slice 1): a hover summary for the local binding /
/// parameter under the cursor — `let x: <ty>` / `param n: <ty>`, rendered from
/// the checker's captured `LocalBinding` (its `ty` is already the surface
/// `type_ref` form, matching inlay hints and signature help). `None` when the
/// cursor is not on a local. Reuses the same `target_at` resolution as
/// go-to-definition / references, so hover cannot disagree with them.
pub fn describe_local_at(locals: &[LocalBinding], text: &str, offset: usize) -> Option<String> {
    let b = target_at(locals, text, offset)?;
    let keyword = match b.kind {
        LocalKind::Let => "let",
        LocalKind::Param => "param",
    };
    Some(format!("```bynk\n{keyword} {}: {}\n```", b.name, b.ty))
}

/// Every local-binding occurrence in the file — `(span, is_definition)` — for
/// semantic-token colouring. A token is a definition if it sits on a binding's
/// def span, else a use if it resolves to a local in scope at that point.
pub fn local_token_sites(locals: &[LocalBinding], text: &str) -> Vec<(Span, bool)> {
    // Hole-aware (issue #473): locals used inside `\(…)` holes colour too.
    let Ok(toks) = lexer::tokenize_expanding_holes(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for t in &toks {
        if t.kind != TokenKind::Ident {
            continue;
        }
        let name = &text[t.span.start..t.span.end];
        if locals.iter().any(|b| same_range(b.def_span, t.span)) {
            out.push((t.span, true)); // a binding's def
        } else if !is_dot_preceded(text, t.span.start)
            && locals_at(locals, t.span.start)
                .into_iter()
                .any(|b| b.name == name)
        {
            // Finding #63: a field-access member name is excluded — a def
            // token is never dot-preceded (it's a fresh binding, not an
            // access), so that branch above is unaffected by this guard.
            out.push((t.span, false)); // a use that resolves to a local
        }
    }
    out
}

/// #2476: one variable the editor should show inline while a debugger is
/// paused — where it is declared and the name the *emitted* code gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineValueSite {
    /// The binding-name span, whose line gets the value.
    pub range: Span,
    /// The emitted TypeScript variable the debug adapter looks up.
    pub variable: String,
}

/// The bindings whose value to show at `stopped`: each local declared inside
/// `visible` and still in scope where the debugger is paused, named as the
/// emitter names it ([`bynk_check::emitted_names`]). A binding whose emitted
/// name is not knowable from the source (a re-`let`) is left out rather than
/// guessed, so a value is never shown against the wrong variable.
pub fn inline_value_sites(
    locals: &[LocalBinding],
    visible: Span,
    stopped: Span,
) -> Vec<InlineValueSite> {
    let mut out: Vec<InlineValueSite> = locals
        .iter()
        .filter(|b| visible.start <= b.def_span.start && b.def_span.end <= visible.end)
        .filter(|b| b.scope.start <= stopped.start && stopped.start <= b.scope.end)
        .filter_map(|b| {
            let variable = emitted_local_name(b, locals)?;
            Some(InlineValueSite {
                range: b.def_span,
                variable,
            })
        })
        .collect();
    out.sort_by_key(|s| s.range.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // `fn f(n: Int) -> Int { let x = n  <uses> }` laid out so offsets are easy.
    fn bindings() -> Vec<LocalBinding> {
        // text: see `TEXT`; n: param scope over body, x: let scope after its stmt.
        vec![
            LocalBinding {
                name: "n".into(),
                def_span: Span::new(5, 6),
                kind: LocalKind::Param,
                ty: "Int".into(),
                scope: Span::new(20, 60),
            },
            LocalBinding {
                name: "x".into(),
                def_span: Span::new(26, 27),
                kind: LocalKind::Let,
                ty: "Int".into(),
                scope: Span::new(34, 60),
            },
        ]
    }

    const TEXT: &str = "fn f(n: Int) -> Int { let x = n\n  x + x\n}";
    //                   0         1         2         3
    //                   0123456789012345678901234567890123456789

    #[test]
    fn sites_for_a_use_collect_def_plus_uses() {
        let locals = bindings();
        // Cursor on the first `x` use (offset 36, in `  x + x`).
        let x_use = TEXT.match_indices('x').nth(1).unwrap().0; // first use of x
        let sites = local_sites_at(&locals, TEXT, x_use).expect("on a local");
        assert!(
            sites.contains(&Span::new(26, 27)),
            "includes def: {sites:?}"
        );
        assert!(sites.len() >= 2, "def + at least one use: {sites:?}");
    }

    #[test]
    fn definition_resolves_from_a_use() {
        let locals = bindings();
        let n_use = TEXT.rfind('n').unwrap(); // the `n` in `let x = n`
        assert_eq!(
            local_definition_at(&locals, TEXT, n_use),
            Some(Span::new(5, 6))
        );
    }

    #[test]
    fn not_on_a_local_yields_none() {
        let locals = bindings();
        assert!(local_sites_at(&locals, TEXT, 0).is_none()); // on `fn`
    }

    /// Finding #63: `total` is both a local *and*, unrelated, a record field
    /// name — the `r.total` field access must never be mistaken for a use of
    /// the local, in any of the three query surfaces this module exposes.
    #[test]
    fn field_access_is_not_mistaken_for_a_local_use() {
        const TEXT: &str = "fn f(r: Rec) -> Int {\n  let total = 1\n  r.total\n}";
        let total_let = TEXT.find("total").unwrap();
        let field_total = TEXT.rfind("total").unwrap();
        assert_ne!(field_total, total_let, "def and field access are distinct");
        let locals = vec![LocalBinding {
            name: "total".into(),
            def_span: Span::new(total_let, total_let + "total".len()),
            kind: LocalKind::Let,
            ty: "Int".into(),
            scope: Span::new(total_let, TEXT.len()),
        }];
        let field_span = Span::new(field_total, field_total + "total".len());

        // Cursor on the field access resolves to nothing — not the local.
        assert!(
            local_definition_at(&locals, TEXT, field_total).is_none(),
            "a field-access member name must not resolve to the same-named local"
        );

        // The local's own use-site list must not include the field access.
        let sites = local_sites_at(&locals, TEXT, total_let).expect("on the local's own def");
        assert!(
            !sites.contains(&field_span),
            "field access wrongly counted as a use: {sites:?}"
        );

        // Semantic-token colouring must not mark the field access as a use.
        let token_sites = local_token_sites(&locals, TEXT);
        assert!(
            !token_sites.contains(&(field_span, false)),
            "field access wrongly coloured as a local use: {token_sites:?}"
        );
    }

    #[test]
    fn token_sites_mark_definitions_and_uses() {
        let sites = local_token_sites(&bindings(), TEXT);
        assert!(
            sites.iter().any(|(_, decl)| *decl),
            "has a definition token"
        );
        assert!(sites.iter().any(|(_, decl)| !*decl), "has a use token");
        // The `x` def is a declaration token.
        assert!(
            sites.contains(&(Span::new(26, 27), true)),
            "x def is a declaration: {sites:?}"
        );
    }

    // End-to-end against real checker output — the lexer's token spans must
    // line up with the checker's recorded def spans.
    #[test]
    fn resolves_a_real_local_from_diagnose_project() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bynkc/tests/fixtures/inlay/clean/src");
        let r = crate::testkit::diagnose_project(&root);
        let file = r
            .files
            .iter()
            .find(|f| f.source_path.to_string_lossy().ends_with("util.bynk"))
            .expect("util.bynk analysed");
        let text = &file.text;
        let locals = r
            .locals
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with("util.bynk"))
            .map(|(_, l)| l.clone())
            .expect("util.bynk locals");

        // `let total = …` then `total` — cursor on the use resolves to def + use.
        let use_off = text.rfind("total").expect("total use");
        let sites = local_sites_at(&locals, text, use_off).expect("on a local");
        assert!(sites.len() >= 2, "def + use: {sites:?}");
        // The definition is first and is the `let total` name.
        let def = text.find("total").expect("total def");
        assert_eq!(sites[0].start, def, "def first");
    }

    // v0.122 (slice 1): hover renders a local's `let`/`param` prefix and its
    // inferred type, from the same real checker output.
    #[test]
    fn describe_local_renders_kind_and_type() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../bynkc/tests/fixtures/inlay/clean/src");
        let r = crate::testkit::diagnose_project(&root);
        let file = r
            .files
            .iter()
            .find(|f| f.source_path.to_string_lossy().ends_with("util.bynk"))
            .expect("util.bynk analysed");
        let text = &file.text;
        let locals = r
            .locals
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with("util.bynk"))
            .map(|(_, l)| l.clone())
            .expect("util.bynk locals");

        // A `let` binding — `let total = xs.fold(…)` → `let total: Int`.
        let total = text.find("total").expect("total def");
        assert_eq!(
            describe_local_at(&locals, text, total).as_deref(),
            Some("```bynk\nlet total: Int\n```")
        );
        // A parameter — `fn sum(xs: List[Int])` → `param xs: List[Int]`. Use the
        // def site so we land on the parameter, not a shadowing local.
        let xs_param = text.find("xs: List[Int]").expect("xs param");
        assert_eq!(
            describe_local_at(&locals, text, xs_param).as_deref(),
            Some("```bynk\nparam xs: List[Int]\n```")
        );
        // Not on a local (the `fn` keyword) → nothing.
        assert!(describe_local_at(&locals, text, text.find("fn").unwrap()).is_none());
    }

    // Issue #473: a parameter used *inside* an interpolation hole
    // (`"… \(name) …"`) must resolve the same as anywhere else. Drives the real
    // hover / go-to-definition / references resolution against live checker
    // output — the position→symbol step that was previously blind to holes
    // because the file lexes the string to one opaque `InterpStr` token.

    // `greet`'s param `name` is used only inside a `\(shout(name))` hole.
    const HOLE_SRC: &str = "\
commons demo.text

fn shout(s: String) -> String {
  s
}

fn greet(name: String) -> String {
  \"Hi, \\(shout(name))!\"
}
";

    /// `(text, locals)` for `demo/text.bynk` after a real project analysis.
    fn analyse_hole_fixture(test_name: &str) -> (String, Vec<LocalBinding>) {
        analyse_source(&format!("hole-{test_name}"), "demo/text.bynk", HOLE_SRC)
    }

    /// `(text, locals)` for `src` written at `rel` (e.g. `demo/text.bynk`) in a
    /// fresh temp project, after a real `diagnose_project` analysis — so the
    /// locals carry the checker's real file id, not `FileId::UNKNOWN`.
    fn analyse_source(tag: &str, rel: &str, src: &str) -> (String, Vec<LocalBinding>) {
        let root = std::env::temp_dir().join(format!("bynk-locals-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let file = root.join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).expect("create dirs");
        std::fs::write(&file, src).expect("write fixture");
        let root = root.canonicalize().unwrap_or(root);

        let name = std::path::Path::new(rel)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let r = crate::testkit::diagnose_project(&root);
        let text = r
            .files
            .iter()
            .find(|f| f.source_path.to_string_lossy().ends_with(&name))
            .expect("fixture analysed")
            .text
            .clone();
        let locals = r
            .locals
            .iter()
            .find(|(p, _)| p.to_string_lossy().ends_with(&name))
            .map(|(_, l)| l.clone())
            .expect("fixture locals");
        (text, locals)
    }

    /// The byte offset of the `n`th occurrence of `needle` in `text`.
    fn nth_offset(text: &str, needle: &str, n: usize) -> usize {
        text.match_indices(needle).nth(n).expect("occurrence").0
    }

    #[test]
    fn hover_describes_a_param_inside_a_hole() {
        let (text, locals) = analyse_hole_fixture("hover");
        // 2nd `name`: the use inside `\(shout(name))` (1st is the declaration).
        let in_hole = nth_offset(&text, "name", 1) + 1; // mid-identifier
        assert_eq!(
            describe_local_at(&locals, &text, in_hole).as_deref(),
            Some("```bynk\nparam name: String\n```"),
            "hover inside the hole renders the param summary"
        );
    }

    #[test]
    fn definition_of_a_param_resolves_from_inside_a_hole() {
        let (text, locals) = analyse_hole_fixture("def");
        let in_hole = nth_offset(&text, "name", 1) + 1;
        let def = local_definition_at(&locals, &text, in_hole).expect("resolves to a def");
        let decl = nth_offset(&text, "name", 0); // the parameter declaration
        assert_eq!(def.start, decl, "def points at the `name` parameter");
    }

    #[test]
    fn references_include_a_param_use_inside_a_hole() {
        let (text, locals) = analyse_hole_fixture("refs");
        let decl = nth_offset(&text, "name", 0);
        let sites = local_sites_at(&locals, &text, decl).expect("on the param");
        let in_hole = nth_offset(&text, "name", 1);
        assert!(
            sites.iter().any(|s| s.start <= in_hole && in_hole < s.end),
            "references include the in-hole use at {in_hole}; got {sites:?}"
        );
    }

    // A `let` binding and a parameter, each defined once and used once.
    const STEPS_SRC: &str =
        "commons demo.steps\n\nfn total(r: Int) -> Int {\n  let plain = r\n  plain\n}\n";

    /// Definition tokens must survive real checker output: the lexer's spans
    /// carry `FileId::UNKNOWN`, the checker's `def_span`s the file's real id,
    /// so a whole-`Span` comparison dropped every definition token.
    #[test]
    fn token_sites_include_real_definitions_from_diagnose_project() {
        let (text, locals) = analyse_source("tokens", "demo/steps.bynk", STEPS_SRC);
        let sites = local_token_sites(&locals, &text);
        let at = |needle: &str, n: usize| {
            let start = nth_offset(&text, needle, n);
            Span::new(start, start + needle.len())
        };
        let has = |span: Span, decl: bool| {
            sites
                .iter()
                .any(|(s, d)| same_range(*s, span) && *d == decl)
        };

        let r_param = text.find("r: Int").unwrap();
        assert!(
            has(Span::new(r_param, r_param + 1), true),
            "param `r` def is a declaration token: {sites:?}"
        );
        assert!(
            has(at("plain", 0), true),
            "`let plain` def is a declaration token: {sites:?}"
        );
        // Uses are still reported, as uses.
        assert!(has(at("plain", 1), false), "`plain` use: {sites:?}");
        let r_use = text.find("= r").unwrap() + 2;
        assert!(
            has(Span::new(r_use, r_use + 1), false),
            "`r` use: {sites:?}"
        );
        // Every token is reported once — a def is never also a use.
        assert_eq!(sites.len(), 4, "two defs + two uses: {sites:?}");
    }

    /// References/rename over real checker output: an inner `let x` that
    /// shadows an outer one is its own binding's definition, not a use of the
    /// outer `x` — and the outer definition is listed exactly once.
    #[test]
    fn sites_exclude_a_shadowing_definition_from_diagnose_project() {
        const SRC: &str =
            "commons demo.shadow\n\nfn f(r: Int) -> Int {\n  let x = r\n  let x = x + 1\n  x\n}\n";
        let (text, locals) = analyse_source("shadow", "demo/shadow.bynk", SRC);
        let outer = nth_offset(&text, "x", 0);
        let inner = nth_offset(&text, "x", 1);
        let outer_use = nth_offset(&text, "x", 2);

        let sites = local_sites_at(&locals, &text, outer).expect("on the outer x");
        let starts: Vec<usize> = sites.iter().map(|s| s.start).collect();
        assert_eq!(
            starts,
            vec![outer, outer_use],
            "outer def once, then its one use; never the inner def: {sites:?}"
        );
        assert!(!starts.contains(&inner));
    }

    // #2476: the sites come from real checker output, with the name mapping
    // taken from the emitter's own rules.
    const INLINE_SRC: &str = "commons demo.inline\n\nfn calc(class: Int) -> Int {\n  let total = class\n  let total = total + 1\n  total\n}\n\nfn other(z: Int) -> Int {\n  let w = z\n  w\n}\n";

    #[test]
    fn inline_sites_map_to_emitted_names_and_skip_re_lets() {
        let (text, locals) = analyse_source("inline", "demo/inline.bynk", INLINE_SRC);
        let whole = Span::new(0, text.len());
        let stop = nth_offset(&text, "  total\n", 0) + 2; // the final `total`
        let sites = inline_value_sites(&locals, whole, Span::new(stop, stop));
        let seen: Vec<(&str, &str)> = sites
            .iter()
            .map(|s| (&text[s.range.start..s.range.end], s.variable.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![("class", "__id_class"), ("total", "total")],
            "param renamed (reserved word); the re-let is dropped; `other` is out of scope"
        );
        // The first `total` is the one kept, not the re-let.
        assert_eq!(sites[1].range.start, nth_offset(&text, "total", 0));
    }

    #[test]
    fn inline_sites_respect_the_visible_range() {
        let (text, locals) = analyse_source("inline-vis", "demo/inline.bynk", INLINE_SRC);
        let stop = nth_offset(&text, "  total\n", 0) + 2;
        // Only the `let total` line is visible.
        let line = nth_offset(&text, "let total = class", 0);
        let visible = Span::new(line, line + "let total = class".len());
        let sites = inline_value_sites(&locals, visible, Span::new(stop, stop));
        assert_eq!(sites.len(), 1);
        assert_eq!(&text[sites[0].range.start..sites[0].range.end], "total");
    }

    #[test]
    fn inline_sites_follow_the_stopped_function() {
        let (text, locals) = analyse_source("inline-fn", "demo/inline.bynk", INLINE_SRC);
        let whole = Span::new(0, text.len());
        let stop = nth_offset(&text, "  w\n", 0) + 2;
        let sites = inline_value_sites(&locals, whole, Span::new(stop, stop));
        let seen: Vec<(&str, &str)> = sites
            .iter()
            .map(|s| (&text[s.range.start..s.range.end], s.variable.as_str()))
            .collect();
        assert_eq!(seen, vec![("z", "z"), ("w", "w")]);
    }

    #[test]
    fn a_let_after_the_stop_is_not_yet_in_scope() {
        let (text, locals) = analyse_source("inline-pre", "demo/inline.bynk", INLINE_SRC);
        let whole = Span::new(0, text.len());
        // Paused on the first `let total` line: neither `total` is live yet.
        let stop = nth_offset(&text, "let total = class", 0);
        let sites = inline_value_sites(&locals, whole, Span::new(stop, stop));
        let names: Vec<&str> = sites
            .iter()
            .map(|s| &text[s.range.start..s.range.end])
            .collect();
        assert_eq!(names, vec!["class"]);
    }
}
