//! #848 — `textDocument/documentLink`'s intra-doc-link half, end to end.
//!
//! `document_link` itself was, before this, covered only by a
//! capability-advertisement test (`advertises_document_links` in `src/lib.rs`)
//! — no end-to-end fixture test existed at all, for either the pre-existing
//! `uses`/`consumes` links or the new intra-doc ones. This closes that gap for
//! the intra-doc half.
//!
//! `Backend::document_link` is transport (state locks, live-buffer read); the
//! part under test here is the pure resolution its body wires together —
//! `symbols::own_declaration_name` + `symbols::doc_link_spans` +
//! `index_queries::resolve_doc_link` — run directly against a real
//! `bynk_ide::diagnose_project` analysis of `examples/rate-limiter`, the way
//! `hover_references.rs` establishes for hover. That fixture's doc comments
//! (see `src/ratelimit.bynk`/`src/window.bynk`) exercise every decided case in
//! one project: a bare local name, a local dotted field (code-span form), a
//! cross-unit `uses` hit, an unresolved first-party name, and an explicit URL.
//!
//! Excluded from the published crate (`Cargo.toml`) — reads `../examples`,
//! outside this crate's package, same as `hover_references.rs`.

use std::path::Path;

use bynk_check::index::SiteRef;
use bynk_ide::ProjectDiagnostics;
use bynk_lsp::{index_queries, symbols};

/// `examples/rate-limiter`'s source root — absolute on every platform, so it
/// converts to a `file://` URL (a bare `/project` is not absolute on Windows).
fn rate_limiter_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root")
        .join("examples")
        .join("rate-limiter")
        .join("src")
}

/// The analysed `examples/rate-limiter` project.
fn rate_limiter() -> ProjectDiagnostics {
    let root = rate_limiter_root();
    // Content-ownership track (#1086) slice 4: bynk-testkit's complete
    // sources map instead of relying on bynk-emit's disk fallback.
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    bynk_ide::diagnose_project(&root, &sources)
}

fn file_text(r: &ProjectDiagnostics, name: &str) -> String {
    r.files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with(name))
        .unwrap_or_else(|| panic!("{name} analysed"))
        .text
        .clone()
}

/// Every intra-doc link `document_link` would emit for `file`'s own doc
/// comments — `(candidate name, resolved definition)`, resolved exactly the
/// way the handler resolves them (`own_declaration_name` for the owning
/// unit, `doc_link_spans` for the candidates, `resolve_doc_link` for each).
fn doc_links_in(r: &ProjectDiagnostics, file: &str) -> Vec<(String, Option<SiteRef>)> {
    let text = file_text(r, file);
    let owner_unit = symbols::doc_link_unit(&text).expect("a unit file has a doc-link unit");
    let rel = r
        .files
        .iter()
        .find(|f| f.source_path.to_string_lossy().ends_with(file))
        .map(|f| f.source_path.clone())
        .expect("file analysed");
    let no_locals = Vec::new();
    symbols::doc_link_spans_attached(&text)
        .into_iter()
        .map(|link| {
            let ctx = link.attached.map(|attached| index_queries::DocLinkContext {
                path: &rel,
                text: &text,
                attached,
                locals: r.locals.get(&rel).unwrap_or(&no_locals),
            });
            let def = index_queries::resolve_doc_link_in(
                &r.index,
                &r.doc_scope,
                &owner_unit,
                &link.name,
                ctx.as_ref(),
            )
            .map(|d| d.def);
            (link.name, def)
        })
        .collect()
}

#[test]
fn bare_local_name_resolves_to_its_own_file() {
    let r = rate_limiter();
    let links = doc_links_in(&r, "ratelimit.bynk");
    let (_, def) = links
        .iter()
        .find(|(n, _)| n == "Limiter")
        .expect("`[Limiter]` is a candidate in ratelimit.bynk");
    let def = def.as_ref().expect("`Limiter` resolves");
    assert!(
        def.path.to_string_lossy().ends_with("ratelimit.bynk"),
        "{def:?}"
    );
}

#[test]
fn bare_local_service_name_resolves() {
    let r = rate_limiter();
    let links = doc_links_in(&r, "ratelimit.bynk");
    let (_, def) = links
        .iter()
        .find(|(n, _)| n == "api")
        .expect("`[api]` is a candidate in ratelimit.bynk");
    let def = def.as_ref().expect("`api` resolves");
    assert!(
        def.path.to_string_lossy().ends_with("ratelimit.bynk"),
        "{def:?}"
    );
}

#[test]
fn dotted_local_field_resolves_via_the_code_span_form() {
    let r = rate_limiter();
    let links = doc_links_in(&r, "ratelimit.bynk");
    let (_, def) = links
        .iter()
        .find(|(n, _)| n == "RateView.remaining")
        .expect("`[`RateView.remaining`]` is a candidate in ratelimit.bynk");
    let def = def.as_ref().expect("`RateView.remaining` resolves");
    assert!(
        def.path.to_string_lossy().ends_with("ratelimit.bynk"),
        "{def:?}"
    );
}

#[test]
fn bare_name_resolves_cross_unit_via_uses() {
    let r = rate_limiter();
    let links = doc_links_in(&r, "ratelimit.bynk");
    let (_, def) = links
        .iter()
        .find(|(n, _)| n == "decide")
        .expect("`[decide]` is a candidate in ratelimit.bynk");
    let def = def
        .as_ref()
        .expect("`decide` resolves, via `ratelimit uses window`");
    assert!(
        def.path.to_string_lossy().ends_with("window.bynk"),
        "{def:?}"
    );
}

#[test]
fn first_party_capability_op_links_to_the_book() {
    let r = rate_limiter();
    let links = doc_links_in(&r, "ratelimit.bynk");
    let (name, def) = links
        .iter()
        .find(|(n, _)| n == "Clock.now")
        .expect("`[Clock.now]` is a candidate in ratelimit.bynk");
    assert!(def.is_none(), "no project definition: {def:?}");
    let text = file_text(&r, "ratelimit.bynk");
    let (owner, _) = symbols::own_declaration_name(&text).unwrap();
    let url = symbols::doc_link_url(
        &r.index,
        &r.doc_scope,
        &rate_limiter_root(),
        &snapshots(&r),
        &owner,
        name,
    )
    .expect("a first-party name in scope links externally");
    assert!(
        url.as_str()
            .starts_with("https://bynk-lang.org/book/reference/bynk-capabilities/#"),
        "{url}"
    );
}

#[test]
fn explicit_url_is_never_a_candidate() {
    let r = rate_limiter();
    let links = doc_links_in(&r, "window.bynk");
    assert!(
        links.iter().all(|(n, _)| n != "fixed window rate limiting"),
        "an explicit `[text](url)` link must never be scanned as a doc-link candidate: {links:?}"
    );
}

/// The round's project-relative path → analysed text map, keyed exactly as
/// the LSP keys `Analysis::snapshots` (by `source_path`).
fn snapshots(r: &ProjectDiagnostics) -> std::collections::HashMap<std::path::PathBuf, String> {
    r.files
        .iter()
        .map(|f| (f.source_path.clone(), f.text.clone()))
        .collect()
}

/// The text a `#L<line>,<column>` link target places the cursor at — what
/// VS Code's opener lands on when the link is followed.
fn text_at_fragment<'a>(
    snapshots: &'a std::collections::HashMap<std::path::PathBuf, String>,
    def: &SiteRef,
    url: &tower_lsp::lsp_types::Url,
) -> &'a str {
    let fragment = url.fragment().expect("the target carries a position");
    let (line, col) = fragment
        .strip_prefix('L')
        .and_then(|f| f.split_once(','))
        .expect("`L<line>,<column>` form");
    let (line, col): (usize, usize) = (line.parse().unwrap(), col.parse().unwrap());
    let text = &snapshots[&def.path];
    let line_text = text.lines().nth(line - 1).expect("line in range");
    // The fixture's declarations are ASCII, so the column is a byte index.
    &line_text[col - 1..]
}

/// The `document_link`/hover target for `name`'s doc link in `file`.
fn doc_link_target_of(
    r: &ProjectDiagnostics,
    file: &str,
    name: &str,
) -> (SiteRef, tower_lsp::lsp_types::Url) {
    let (_, def) = doc_links_in(r, file)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("`[{name}]` is a candidate in {file}"));
    let def = def.unwrap_or_else(|| panic!("`{name}` resolves"));
    let url =
        symbols::doc_link_target(&rate_limiter_root(), &def, &snapshots(r)).expect("a target");
    (def, url)
}

#[test]
fn same_file_target_lands_on_the_declaration() {
    // The click that previously did nothing: the target was the file alone,
    // and the file was already open.
    let r = rate_limiter();
    let (def, url) = doc_link_target_of(&r, "ratelimit.bynk", "Limiter");
    let snaps = snapshots(&r);
    assert!(
        text_at_fragment(&snaps, &def, &url).starts_with("Limiter"),
        "{url}"
    );
}

#[test]
fn cross_unit_target_lands_on_the_declaration() {
    let r = rate_limiter();
    let (def, url) = doc_link_target_of(&r, "ratelimit.bynk", "decide");
    assert!(url.path().ends_with("window.bynk"), "{url}");
    let snaps = snapshots(&r);
    assert!(
        text_at_fragment(&snaps, &def, &url).starts_with("decide"),
        "{url}"
    );
}

#[test]
fn hover_markdown_links_carry_the_position() {
    let r = rate_limiter();
    let (_, url) = doc_link_target_of(&r, "ratelimit.bynk", "Limiter");
    let (owner_unit, _) =
        symbols::own_declaration_name(&file_text(&r, "ratelimit.bynk")).expect("own name");
    let rendered = symbols::linkify_doc_links(
        "See [Limiter].",
        &r.index,
        &r.doc_scope,
        &rate_limiter_root(),
        &snapshots(&r),
        &owner_unit,
    );
    assert_eq!(rendered, format!("See [Limiter]({url})."));
}

#[test]
fn target_missing_from_the_snapshots_still_links_to_the_file() {
    let r = rate_limiter();
    let (def, _) = doc_link_target_of(&r, "ratelimit.bynk", "Limiter");
    let url = symbols::doc_link_target(&rate_limiter_root(), &def, &Default::default())
        .expect("a target");
    assert_eq!(url.fragment(), None);
    assert!(url.path().ends_with("ratelimit.bynk"), "{url}");
}

#[test]
fn target_column_counts_utf16_code_units() {
    // `😀` is 4 UTF-8 bytes but 2 UTF-16 code units; VS Code's column counts
    // the latter, so `Name` (byte 5) sits at 1-based column 4, not 6.
    let def = SiteRef {
        path: "unit.bynk".into(),
        span: bynk_syntax::span::Span::new(5, 9),
    };
    let snaps = std::collections::HashMap::from([(def.path.clone(), "😀 Name".to_string())]);
    let url = symbols::doc_link_target(&rate_limiter_root(), &def, &snaps).expect("a target");
    assert_eq!(url.fragment(), Some("L1,4"));
}

/// The source text at `def`'s name span.
fn text_of<'a>(r: &'a ProjectDiagnostics, def: &SiteRef) -> &'a str {
    let f = r.files.iter().find(|f| f.source_path == def.path).unwrap();
    &f.text[def.span.start..def.span.end]
}

#[test]
fn own_field_resolves_from_its_records_doc() {
    // #2454: `[remaining]` in `RateView`'s doc names that record's field.
    let r = rate_limiter();
    let (_, def) = doc_links_in(&r, "ratelimit.bynk")
        .into_iter()
        .find(|(n, _)| n == "remaining")
        .expect("`[remaining]` is a candidate");
    let def = def.expect("`remaining` resolves to RateView's field");
    assert_eq!(text_of(&r, &def), "remaining");
    assert!(def.path.to_string_lossy().ends_with("ratelimit.bynk"));
}

#[test]
fn own_parameter_resolves_from_its_fns_doc() {
    let r = rate_limiter();
    let (_, def) = doc_links_in(&r, "window.bynk")
        .into_iter()
        .find(|(n, _)| n == "now")
        .expect("`[now]` is a candidate");
    let def = def.expect("`now` resolves to decide's parameter");
    assert_eq!(text_of(&r, &def), "now");
    let text = file_text(&r, "window.bynk");
    assert!(
        text[..def.span.start].ends_with("prevCount: Int, "),
        "{def:?}"
    );
}

#[test]
fn unit_qualified_name_resolves() {
    let r = rate_limiter();
    let (_, def) = doc_links_in(&r, "ratelimit.bynk")
        .into_iter()
        .find(|(n, _)| n == "window.decide")
        .expect("`[window.decide]` is a candidate");
    let def = def.expect("`window.decide` resolves cross-unit");
    assert_eq!(text_of(&r, &def), "decide");
}

#[test]
fn own_parameter_resolves_from_a_methods_doc() {
    // #2569: a method's doc attaches at its name, so `[n]` finds the parameter.
    let root = std::env::temp_dir().join(format!("bynk-method-doc-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("a scratch directory");
    std::fs::write(
        root.join("unit.bynk"),
        "context u\n\ntype Box = {\n  value: Int,\n}\n\n---\nAdds [n] to the box.\n---\nfn Box.plus(self, n: Int) -> Int {\n  self.value + n\n}\n",
    )
    .expect("the unit written");
    let sources =
        bynk_testkit::read_project_sources(&bynk_ide::AnalysisRoots::SingleTree(root.clone()));
    let r = bynk_ide::diagnose_project(&root, &sources);
    let (_, def) = doc_links_in(&r, "unit.bynk")
        .into_iter()
        .find(|(n, _)| n == "n")
        .expect("`[n]` is a candidate");
    let def = def.expect("`n` resolves to plus's parameter");
    assert_eq!(text_of(&r, &def), "n");
    assert!(file_text(&r, "unit.bynk")[..def.span.start].ends_with("self, "));
    let _ = std::fs::remove_dir_all(&root);
}
